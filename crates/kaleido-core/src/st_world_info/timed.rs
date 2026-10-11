//! st_world_info/timed —— check_world_info_timed 主扫描（含 timed 效果/recursion/预算）。
#![allow(unused_imports)]

use super::*;

pub fn check_world_info_timed(
    entries: &[WiEntry],
    chat_newest_first: &[String],
    max_context_tokens: i32,
    settings: &WiSettings,
    timed_in: Option<TimedWorldInfo>,
    dry_run: bool,
    scan_ctx: Option<WiScanContext>,
) -> WiScanResult {
    let mut rng = rand::thread_rng();
    let ctx = scan_ctx.unwrap_or_default();
    let max_context_tokens = if ctx.max_context_tokens > 0 {
        ctx.max_context_tokens
    } else {
        max_context_tokens
    };
    let mut budget = ((settings.budget_pct / 100.0) * max_context_tokens as f64).round() as i32;
    if budget < 1 {
        budget = 1;
    }
    if settings.budget_cap > 0 && budget > settings.budget_cap {
        budget = settings.budget_cap;
    }

    // Sort like ST: by order desc (higher insertion_order first)
    let mut sorted: Vec<&WiEntry> = entries.iter().filter(|e| !e.disable).collect();
    sorted.sort_by(|a, b| b.order.cmp(&a.order).then_with(|| a.uid.cmp(&b.uid)));

    #[derive(Clone, Copy, PartialEq)]
    enum ScanState {
        None,
        Initial,
        Recursion,
        MinActivations,
    }

    let mut scan_state = ScanState::Initial;
    let mut all_activated: HashMap<String, (WiEntry, String)> = HashMap::new(); // key world.uid
    let mut failed_prob: HashSet<String> = HashSet::new();
    let mut recurse_buf: Vec<String> = Vec::new();
    let mut all_activated_text = String::new();
    let mut overflowed = false;
    let mut loop_count = 0;
    let mut depth_skew = 0i32;
    let mut skipped_vectorized = 0i32;
    let mut vector_activated = 0i32;
    let mut skipped_filter = 0i32;
    let mut skipped_trigger = 0i32;
    let vector_hit_map: std::collections::HashMap<String, f64> = {
        let settings = ctx
            .vector_settings
            .clone()
            .unwrap_or_default();
        if settings.enabled {
            crate::hits_to_map(&ctx.vector_hits)
        } else {
            std::collections::HashMap::new()
        }
    };

    // --- Timed effects (ST WorldInfoTimedEffects) ---
    // chat_len uses full message count; scan buffer is newest-first subset.
    let chat_len = chat_newest_first.len() as i32;
    let mut timed = timed_in.unwrap_or_default();
    let mut sticky_active: HashSet<String> = HashSet::new();
    let mut cooldown_active: HashSet<String> = HashSet::new();
    let mut delay_active: HashSet<String> = HashSet::new();

    if !dry_run {
        // Process sticky map
        let keys: Vec<String> = timed.sticky.keys().cloned().collect();
        for key in keys {
            let Some(eff) = timed.sticky.get(&key).cloned() else { continue };
            if chat_len <= eff.start && !eff.protected {
                timed.sticky.remove(&key);
                continue;
            }
            let entry_exists = entries.iter().any(|e| TimedWorldInfo::entry_key(&e.world, &e.uid) == key);
            if !entry_exists {
                if chat_len >= eff.end {
                    timed.sticky.remove(&key);
                }
                continue;
            }
            if chat_len >= eff.end {
                // sticky ended → optional cooldown
                timed.sticky.remove(&key);
                if let Some(entry) = entries.iter().find(|e| TimedWorldInfo::entry_key(&e.world, &e.uid) == key) {
                    if let Some(cd) = entry.cooldown {
                        if cd > 0 {
                            let ce = WiTimedEffect {
                                key: key.clone(),
                                start: chat_len,
                                end: chat_len + cd,
                                protected: true,
                            };
                            timed.cooldown.insert(key.clone(), ce);
                            cooldown_active.insert(key.clone());
                        }
                    }
                }
                continue;
            }
            sticky_active.insert(key);
        }
        // Process cooldown map
        let keys: Vec<String> = timed.cooldown.keys().cloned().collect();
        for key in keys {
            let Some(eff) = timed.cooldown.get(&key).cloned() else { continue };
            if chat_len <= eff.start && !eff.protected {
                timed.cooldown.remove(&key);
                continue;
            }
            if chat_len >= eff.end {
                timed.cooldown.remove(&key);
                continue;
            }
            cooldown_active.insert(key);
        }
    }
    // Delay: suppress until chat has enough messages
    for e in entries {
        if let Some(d) = e.delay {
            if d > 0 && chat_len < d {
                delay_active.insert(TimedWorldInfo::entry_key(&e.world, &e.uid));
            }
        }
    }


    while scan_state != ScanState::None {
        if settings.max_recursion_steps > 0 && loop_count >= settings.max_recursion_steps {
            break;
        }
        loop_count += 1;
        let mut activated_now: Vec<(WiEntry, String)> = Vec::new();
        // [R8 2026-10-07] 每趟缓存：depth→小写缓冲（避免每 key 全缓冲 lowercase）+
        // regex 键编译缓存（避免每 key 现场编译）。
        let mut lower_cache: std::collections::HashMap<i32, std::rc::Rc<String>> =
            std::collections::HashMap::new();
        let mut regex_cache: std::collections::HashMap<String, Option<regex::Regex>> =
            std::collections::HashMap::new();

        for entry in &sorted {
            let key = format!("{}.{}", entry.world, entry.uid);
            if failed_prob.contains(&key) || all_activated.contains_key(&key) {
                continue;
            }
            if scan_state != ScanState::Recursion && entry.delay_until_recursion {
                continue;
            }
            if scan_state == ScanState::Recursion && settings.recursive && entry.exclude_recursion
            {
                continue;
            }

            let depth = entry
                .scan_depth
                .unwrap_or(settings.depth + depth_skew)
                .max(0);
            let text = buffer_text(chat_newest_first, depth, &recurse_buf);
            let text_lower: std::rc::Rc<String> = match lower_cache.get(&depth) {
                Some(v) => v.clone(),
                None => {
                    let v = std::rc::Rc::new(text.to_lowercase());
                    lower_cache.insert(depth, v.clone());
                    v
                }
            };

            let ekey = TimedWorldInfo::entry_key(&entry.world, &entry.uid);
            if delay_active.contains(&ekey) {
                continue;
            }
            if cooldown_active.contains(&ekey) && !sticky_active.contains(&ekey) {
                continue;
            }
            if sticky_active.contains(&ekey) {
                {
                let mut e = (*entry).clone();
                e.content = substitute_params(&entry.content, &ctx);
                activated_now.push((e, "sticky".into()));
            }
                continue;
            }
            // vectorized entries: activate via precomputed embed hits (W5); else skip keyword path (ST)
            if entry.vectorized {
                let vkey = format!("{}.{}", entry.world, entry.uid);
                if let Some(score) = vector_hit_map.get(&vkey) {
                    let mut e = (*entry).clone();
                    e.content = substitute_params(&entry.content, &ctx);
                    activated_now.push((e, format!("vector:{score:.4}")));
                    vector_activated += 1;
                } else {
                    skipped_vectorized += 1;
                }
                continue;
            }
            // generation type triggers
            if !entry.triggers.is_empty() {
                let trig = ctx.trigger.trim();
                if trig.is_empty() || !entry.triggers.iter().any(|t| t == trig) {
                    skipped_trigger += 1;
                    continue;
                }
            }
            // characterFilter (names + tags)
            if let Some(ref cf) = entry.character_filter {
                let mut filtered = false;
                if !cf.names.is_empty() {
                    let name = ctx.character_name.trim();
                    let included = !name.is_empty()
                        && cf.names.iter().any(|n| n.eq_ignore_ascii_case(name) || name.contains(n.as_str()));
                    filtered = if cf.is_exclude { included } else { !included };
                }
                if !filtered && !cf.tags.is_empty() {
                    let included = cf.tags.iter().any(|t| {
                        ctx.character_tags.iter().any(|ct| ct.eq_ignore_ascii_case(t))
                    });
                    filtered = if cf.is_exclude { included } else { !included };
                }
                if filtered {
                    skipped_filter += 1;
                    continue;
                }
            }
            // ST decorators (after timed gates, before constant/keys)
            if entry.decorators.iter().any(|d| d == "@@dont_activate") {
                continue;
            }
            if entry.decorators.iter().any(|d| d == "@@activate") {
                {
                let mut e = (*entry).clone();
                e.content = substitute_params(&entry.content, &ctx);
                activated_now.push((e, "decorator:@@activate".into()));
            }
                continue;
            }
            if entry.constant {
                {
                let mut e = (*entry).clone();
                e.content = substitute_params(&entry.content, &ctx);
                activated_now.push((e, "constant".into()));
            }
                continue;
            }
            if entry.keys.is_empty() {
                continue;
            }
            // macro-expand keys against scan context
            let mut entry_sub = (*entry).clone();
            entry_sub.keys = entry
                .keys
                .iter()
                .map(|k| substitute_params(k, &ctx))
                .collect();
            entry_sub.keysecondary = entry
                .keysecondary
                .iter()
                .map(|k| substitute_params(k, &ctx))
                .collect();
            let Some(pk) = primary_match_pre(&entry_sub, &text, &text_lower, settings, &mut regex_cache)
            else {
                continue;
            };
            if !secondary_match_pre(&entry_sub, &text, &text_lower, settings, &mut regex_cache) {
                continue;
            }
            entry_sub.content = substitute_params(&entry.content, &ctx);
            activated_now.push((entry_sub, format!("key:{pk}")));
        }

        // Inclusion groups
        filter_inclusion_groups(&mut activated_now, &all_activated, chat_newest_first, settings, depth_skew, &recurse_buf, &mut rng);

        // Probability + budget
        let mut new_content = String::new();
        let tok_mode = settings.token_estimate_mode.as_str();
        let base_tokens = estimate_tokens_mode(&all_activated_text, tok_mode);
        let mut ignore_left = activated_now.iter().filter(|(e, _)| e.ignore_budget).count();

        for (entry, reason) in activated_now {
            let key = format!("{}.{}", entry.world, entry.uid);
            if overflowed && !entry.ignore_budget {
                if ignore_left > 0 {
                    continue;
                }
                break;
            }
            if entry.ignore_budget {
                ignore_left = ignore_left.saturating_sub(1);
            }

            // probability
            if entry.use_probability && entry.probability < 100.0 {
                let roll: f64 = rng.gen::<f64>() * 100.0;
                if roll > entry.probability {
                    failed_prob.insert(key);
                    continue;
                }
            }

            new_content.push_str(&entry.content);
            new_content.push('\n');
            if !entry.ignore_budget
                && (base_tokens + estimate_tokens_mode(&new_content, tok_mode)) >= budget
            {
                overflowed = true;
                // still skip adding this one (ST continues ignoreBudget only)
                continue;
            }
            all_activated.insert(key, (entry, reason));
        }

        // recursion / min activations
        let successful: Vec<&WiEntry> = all_activated
            .values()
            .map(|(e, _)| e)
            .collect();
        // Only newly added this loop for recursion — approximate: use recurse from last new_content
        let mut next = ScanState::None;
        if settings.recursive && !overflowed && !new_content.trim().is_empty() {
            // re-scan with recurse buffer of prevent_recursion=false contents
            let rec_text: String = all_activated
                .values()
                .filter(|(e, _)| !e.prevent_recursion)
                .filter(|(e, _)| new_content.contains(&e.content)) // rough: newly relevant
                .map(|(e, _)| e.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !rec_text.is_empty() {
                recurse_buf.push(rec_text.clone());
                all_activated_text = format!("{rec_text}\n{all_activated_text}");
                next = ScanState::Recursion;
            }
        }

        if next == ScanState::None
            && !overflowed
            && settings.min_activations > 0
            && (successful.len() as i32) < settings.min_activations
        {
            let over = (settings.min_activations_depth_max > 0
                && (settings.depth + depth_skew) > settings.min_activations_depth_max)
                || ((settings.depth + depth_skew) as usize) > chat_newest_first.len();
            if !over {
                depth_skew += 1;
                next = ScanState::MinActivations;
            }
        }

        // Avoid infinite recursion: if recursion produced nothing new, stop
        if next == ScanState::Recursion && loop_count > 1 {
            // if no new keys this round beyond previous, ST still may loop — cap soft
            if new_content.trim().is_empty() {
                next = ScanState::None;
            }
        }
        if scan_state == ScanState::Initial && next == ScanState::None {
            // done
        }
        scan_state = next;
        if loop_count > 32 {
            break;
        }
    }

    // Build prompt sections — sort by order desc
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut an_top = Vec::new();
    let mut an_bot = Vec::new();
    let mut em_top = Vec::new();
    let mut em_bot = Vec::new();
    let mut outlet_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut depth_map: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
    let mut activated_meta = Vec::new();
    let mut automation_ids_collect: Vec<String> = Vec::new();
    let mut ordered: Vec<_> = all_activated.into_values().collect();
    ordered.sort_by(|a, b| b.0.order.cmp(&a.0.order));

    // Set timed effects for newly activated (sticky/cooldown start)
    if !dry_run {
        for (entry, _reason) in &ordered {
            let key = TimedWorldInfo::entry_key(&entry.world, &entry.uid);
            if let Some(st) = entry.sticky {
                if st > 0 && !timed.sticky.contains_key(&key) {
                    timed.sticky.insert(
                        key.clone(),
                        WiTimedEffect {
                            key: key.clone(),
                            start: chat_len,
                            end: chat_len + st,
                            protected: false,
                        },
                    );
                }
            }
            if let Some(cd) = entry.cooldown {
                if cd > 0 && !timed.cooldown.contains_key(&key) {
                    // ST sets cooldown on activation too (parallel sticky)
                    // Only if not already from sticky-end
                    if !cooldown_active.contains(&key) {
                        timed.cooldown.insert(
                            key.clone(),
                            WiTimedEffect {
                                key: key.clone(),
                                start: chat_len,
                                end: chat_len + cd,
                                protected: false,
                            },
                        );
                    }
                }
            }
        }
    }

    for (entry, reason) in ordered {
        let content = entry.content.trim();
        if content.is_empty() {
            continue;
        }
        let auto_id = entry.automation_id.trim().to_string();
        activated_meta.push(ActivatedEntry {
            uid: entry.uid.clone(),
            world: entry.world.clone(),
            comment: entry.comment.clone(),
            order: entry.order,
            position: entry.position as i32,
            content: content.to_string(),
            reason,
            automation_id: auto_id.clone(),
        });
        if !auto_id.is_empty() {
            if !automation_ids_collect.iter().any(|x| x == &auto_id) {
                automation_ids_collect.push(auto_id);
            }
        }
        match entry.position {
            WiPosition::Before => {
                before.push(content.to_string());
            }
            WiPosition::EmTop => {
                em_top.push(content.to_string());
            }
            WiPosition::AnTop => {
                an_top.push(content.to_string());
            }
            WiPosition::AnBottom => {
                an_bot.push(content.to_string());
            }
            WiPosition::AtDepth => {
                depth_map
                    .entry(entry.depth.max(0))
                    .or_default()
                    .push((entry.role, content.to_string()));
            }
            WiPosition::EmBottom => {
                em_bot.push(content.to_string());
            }
            WiPosition::Outlet => {
                let name = if entry.outlet_name.trim().is_empty() {
                    "default".to_string()
                } else {
                    entry.outlet_name.trim().to_string()
                };
                outlet_map.entry(name).or_default().push(content.to_string());
            }
            WiPosition::After => {
                after.push(content.to_string());
            }
        }
    }
    let mut depth_entries: Vec<WiDepthEntry> = depth_map
        .into_iter()
        .map(|(depth, parts)| {
            let role = match parts.first().map(|p| p.0).unwrap_or(0) {
                1 => "user",
                2 => "assistant",
                _ => "system",
            };
            WiDepthEntry {
                depth,
                role: role.into(),
                content: parts.into_iter().map(|(_, c)| c).collect::<Vec<_>>().join("\n"),
            }
        })
        .collect();
    depth_entries.sort_by_key(|d| d.depth);
    let mut outlet_entries: Vec<WiOutletEntry> = outlet_map
        .into_iter()
        .map(|(name, parts)| WiOutletEntry {
            name,
            content: parts.join("\n"),
        })
        .collect();
    outlet_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let world_info_before = before.join("\n");
    let world_info_after = after.join("\n");
    let an_before = an_top.join("\n");
    let an_after = an_bot.join("\n");
    let em_before = em_top.join("\n");
    let em_after = em_bot.join("\n");

    // Multi-slot chat injections (ST setExtensionPrompt IN_CHAT / examples)
    let mut chat_injections = Vec::new();
    let mut example_messages: Vec<WiExampleMessage> = Vec::new();
    let user_n = ctx.user_name.clone();
    let char_n = if ctx.char_name.is_empty() { ctx.character_name.clone() } else { ctx.char_name.clone() };
    if !em_before.trim().is_empty() {
        let pairs = parse_mes_examples(em_before.trim(), &user_n, &char_n);
        if pairs.len() >= 1 && pairs.iter().any(|(r, _)| r == "user" || r == "assistant") {
            for (role, content) in pairs {
                example_messages.push(WiExampleMessage {
                    role: role.clone(),
                    content: content.clone(),
                    anchor: "before".into(),
                });
                chat_injections.push(WiChatInjection {
                    role,
                    content,
                    depth: 0,
                    kind: "em_example_before".into(),
                });
            }
        } else {
            chat_injections.push(WiChatInjection {
                role: "system".into(),
                content: em_before.clone(),
                depth: 0,
                kind: "em_before".into(),
            });
        }
    }
    if !em_after.trim().is_empty() {
        let pairs = parse_mes_examples(em_after.trim(), &user_n, &char_n);
        if pairs.len() >= 1 && pairs.iter().any(|(r, _)| r == "user" || r == "assistant") {
            for (role, content) in pairs {
                example_messages.push(WiExampleMessage {
                    role: role.clone(),
                    content: content.clone(),
                    anchor: "after".into(),
                });
                chat_injections.push(WiChatInjection {
                    role,
                    content,
                    depth: 0,
                    kind: "em_example_after".into(),
                });
            }
        } else {
            chat_injections.push(WiChatInjection {
                role: "system".into(),
                content: em_after.clone(),
                depth: 0,
                kind: "em_after".into(),
            });
        }
    }
    for d in &depth_entries {
        if d.content.trim().is_empty() {
            continue;
        }
        let role = match d.role.as_str() {
            "user" => "user",
            "assistant" => "assistant",
            _ => "system",
        };
        chat_injections.push(WiChatInjection {
            role: role.into(),
            content: d.content.clone(),
            depth: d.depth.max(0),
            kind: "depth".into(),
        });
    }
    for o in &outlet_entries {
        if o.content.trim().is_empty() {
            continue;
        }
        chat_injections.push(WiChatInjection {
            role: "system".into(),
            content: format!("[outlet:{}]\n{}", o.name, o.content),
            depth: 0,
            kind: format!("outlet:{}", o.name),
        });
    }

    // Fix depth_entries role from entry.role if we stored system only — enhance depth_map to keep role
    let prompt_slots = WiPromptSlots {
        world_info_before: world_info_before.clone(),
        world_info_after: world_info_after.clone(),
        an_before: an_before.clone(),
        an_after: an_after.clone(),
        em_before: em_before.clone(),
        em_after: em_after.clone(),
        depth_entries: depth_entries.clone(),
        outlet_entries: outlet_entries.clone(),
        chat_injections: chat_injections.clone(),
    };

    WiScanResult {
        world_info_before,
        world_info_after,
        an_before,
        an_after,
        depth_entries,
        em_before,
        em_after,
        outlet_entries,
        activated: activated_meta,
        budget_tokens: budget,
        token_estimate_mode: {
            let m = settings.token_estimate_mode.trim();
            if m.is_empty() {
                "heuristic".into()
            } else {
                crate::TokenEstimateMode::parse(m).as_str().to_string()
            }
        },
        overflowed,
        timed_world_info: if dry_run { None } else { Some(timed) },
        prompt_slots,
        skipped_vectorized,
        vector_activated,
        skipped_filter,
        skipped_trigger,
        automation_ids: automation_ids_collect,
        example_messages,
    }
}

pub(crate) fn filter_inclusion_groups(
    activated_now: &mut Vec<(WiEntry, String)>,
    all_activated: &HashMap<String, (WiEntry, String)>,
    chat: &[String],
    settings: &WiSettings,
    depth_skew: i32,
    recurse: &[String],
    rng: &mut impl Rng,
) {
    // group name -> indices
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (e, _)) in activated_now.iter().enumerate() {
        if e.group.trim().is_empty() {
            continue;
        }
        for g in e.group.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            groups.entry(g.to_string()).or_default().push(i);
        }
    }
    if groups.is_empty() {
        return;
    }
    let mut remove: HashSet<usize> = HashSet::new();
    for (gname, idxs) in &groups {
        // already activated in previous loop?
        if all_activated
            .values()
            .any(|(e, _)| e.group.split(',').any(|x| x.trim() == gname.as_str()))
        {
            for &i in idxs {
                remove.insert(i);
            }
            continue;
        }
        if idxs.len() <= 1 {
            continue;
        }
        // prio override
        let mut prio: Option<usize> = None;
        let mut best_order = i32::MIN;
        for &i in idxs {
            if activated_now[i].0.group_override && activated_now[i].0.order >= best_order {
                best_order = activated_now[i].0.order;
                prio = Some(i);
            }
        }
        if let Some(w) = prio {
            for &i in idxs {
                if i != w {
                    remove.insert(i);
                }
            }
            continue;
        }
        // group scoring
        let use_score = settings.use_group_scoring
            || idxs
                .iter()
                .any(|&i| activated_now[i].0.use_group_scoring.unwrap_or(false));
        if use_score {
            let depth = settings.depth + depth_skew;
            let text = buffer_text(chat, depth, recurse);
            let text_lower = text.to_lowercase();
            let mut regex_cache: std::collections::HashMap<String, Option<regex::Regex>> =
                std::collections::HashMap::new();
            let scores: Vec<i32> = idxs
                .iter()
                .map(|&i| {
                    score_entry_pre(
                        &activated_now[i].0,
                        &text,
                        &text_lower,
                        settings,
                        &mut regex_cache,
                    )
                })
                .collect();
            let max = scores.iter().copied().max().unwrap_or(0);
            for (k, &i) in idxs.iter().enumerate() {
                let scored = activated_now[i]
                    .0
                    .use_group_scoring
                    .unwrap_or(settings.use_group_scoring);
                if scored && scores[k] < max {
                    remove.insert(i);
                }
            }
            // recompute remaining for weight roll
        }
        let remain: Vec<usize> = idxs.iter().copied().filter(|i| !remove.contains(i)).collect();
        if remain.len() <= 1 {
            continue;
        }
        // weighted random
        let total: f64 = remain
            .iter()
            .map(|&i| activated_now[i].0.group_weight.max(0.0))
            .sum();
        let roll = rng.gen::<f64>() * total.max(1.0);
        let mut acc = 0.0;
        let mut winner = remain[0];
        for &i in &remain {
            acc += activated_now[i].0.group_weight.max(0.0);
            if roll <= acc {
                winner = i;
                break;
            }
        }
        for &i in &remain {
            if i != winner {
                remove.insert(i);
            }
        }
    }
    if remove.is_empty() {
        return;
    }
    let i = 0;
    let mut idx = 0;
    activated_now.retain(|_| {
        let drop = remove.contains(&idx);
        idx += 1;
        !drop
    });
    let _ = i;
}

/// Format WI scan into the Kaleido system-prompt section (before + after around card).
pub fn format_wi_for_system(scan: &WiScanResult) -> String {
    let mut parts = Vec::new();
    if !scan.world_info_before.trim().is_empty() {
        parts.push(scan.world_info_before.trim().to_string());
    }
    if !scan.em_before.trim().is_empty() {
        parts.push(format!("[EM↑]\n{}", scan.em_before.trim()));
    }
    if !scan.an_before.trim().is_empty() {
        parts.push(scan.an_before.trim().to_string());
    }
    if !scan.world_info_after.trim().is_empty() {
        parts.push(scan.world_info_after.trim().to_string());
    }
    if !scan.an_after.trim().is_empty() {
        parts.push(scan.an_after.trim().to_string());
    }
    if !scan.em_after.trim().is_empty() {
        parts.push(format!("[EM↓]\n{}", scan.em_after.trim()));
    }
    for d in &scan.depth_entries {
        if !d.content.trim().is_empty() {
            parts.push(format!("[depth {}]\n{}", d.depth, d.content.trim()));
        }
    }
    for o in &scan.outlet_entries {
        if !o.content.trim().is_empty() {
            parts.push(format!("[outlet:{}]\n{}", o.name, o.content.trim()));
        }
    }
    parts.join("\n\n")
}
