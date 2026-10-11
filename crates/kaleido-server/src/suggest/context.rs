//! suggest/context —— 风格统计 / 相关章节召回 / 账本 / call_llm_tracked。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStyleStats {
    pub chapters: usize,
    pub total_chars: usize,
    pub avg_chars: usize,
    pub em_dashes: usize,
    pub triple_lists: usize,
    pub short_endings: usize,
    pub short_ending_rate: f32,
    pub top_phrases: Vec<(String, usize)>,
}

pub(crate) fn project_style_stats(
    state: &AppState,
    ws: &str,
    pid: &str,
) -> ProjectStyleStats {
    use std::collections::HashMap;
    let dir = format!("projects/{pid}/chapters");
    let mut chapters = 0usize;
    let mut total_chars = 0usize;
    let mut em_dashes = 0usize;
    let mut triple_lists = 0usize;
    let mut short_endings = 0usize;
    let mut gram: HashMap<String, usize> = HashMap::new();
    if let Ok(entry) = state.works.list(ws, &dir, 1) {
        for child in entry.children {
            if child.kind == "dir" {
                continue;
            }
            let rel = format!("{dir}/{}", child.name);
            let body = match state.works.read_text(ws, &rel) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let ch: ChapterDoc = match serde_json::from_str(&body.content) {
                Ok(c) => c,
                Err(_) => continue,
            };
            chapters += 1;
            let text = format!("{}\n{}", ch.title, ch.content);
            let n_chars = text.chars().count();
            total_chars += n_chars;
            em_dashes += text.matches("\u{2014}\u{2014}").count();
            // 三段式信号：、.*、.*和
            for line in text.lines() {
                let commas = line.matches('、').count();
                if commas >= 2 && line.contains('和') {
                    triple_lists += 1;
                }
            }
            // 章末短句：末段 ≤20 字
            if let Some(last) = text.lines().filter(|l| !l.trim().is_empty()).last() {
                if last.chars().count() <= 20 {
                    short_endings += 1;
                }
            }
            // 高频 2-gram（中文）
            let chars: Vec<char> = text.chars().collect();
            for w in chars.windows(2) {
                if (w[0] as u32) >= 0x4E00
                    && (w[0] as u32) <= 0x9FFF
                    && (w[1] as u32) >= 0x4E00
                    && (w[1] as u32) <= 0x9FFF
                {
                    let g: String = w.iter().collect();
                    *gram.entry(g).or_insert(0) += 1;
                }
            }
        }
    }
    let mut top: Vec<(String, usize)> = gram.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    top.truncate(5);
    ProjectStyleStats {
        chapters,
        total_chars,
        avg_chars: if chapters > 0 { total_chars / chapters } else { 0 },
        em_dashes,
        triple_lists,
        short_endings,
        short_ending_rate: if chapters > 0 { short_endings as f32 / chapters as f32 } else { 0.0 },
        top_phrases: top,
    }
}

/// 相关章节推荐（吞噬 ainovel-cli `buildRelatedChapters` 四维思路，Kaleido 化）。
/// 同 project 其他章节，按标题/正文关键词交集推荐 top5（去自身）。
/// 返回 (chapter_id, title, reason)。
/// 相关章节双通道（吞噬 nov-manager query_lore/query_plot_memory 分离口径）：
/// - lore 通道：focus 取标题+指令关键词，命中设定向（世界/人物名词多）
/// - plot 通道：focus 取指令全文，命中剧情向（事件/动作词多）
/// 当前实现两通道同算法、不同 focus 配比，返回 (id,title,reason,channel)。
pub(crate) fn related_chapters(
    state: &AppState,
    ws: &str,
    pid: &str,
    cur_id: &str,
    focus: &str,
) -> Vec<(String, String, String)> {
    related_chapters_dual(state, ws, pid, cur_id, focus, focus)
        .into_iter()
        .map(|(a, b, c, _)| (a, b, c))
        .collect()
}

pub(crate) fn related_chapters_dual(
    state: &AppState,
    ws: &str,
    pid: &str,
    cur_id: &str,
    lore_focus: &str,
    plot_focus: &str,
) -> Vec<(String, String, String, &'static str)> {
    related_chapters_dual_inner(state, ws, pid, cur_id, lore_focus, plot_focus)
}

/// focus 分词（≥2 char 中文词 + ASCII 词）。
pub(crate) fn focus_terms_of(focus: &str) -> Vec<String> {
        let mut v = Vec::new();
        let chars: Vec<char> = focus.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_alphanumeric() {
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_alphanumeric() {
                    j += 1;
                }
                let w: String = chars[i..j].iter().collect();
                if w.len() >= 3 {
                    v.push(w);
                }
                i = j;
            } else if (c as u32) >= 0x4E00 && (c as u32) <= 0x9FFF {
                // 中文 2-gram
                if i + 1 < chars.len() {
                    let w: String = chars[i..i + 2].iter().collect();
                    v.push(w);
                }
                i += 1;
            } else {
                i += 1;
            }
        }
        v.sort();
        v.dedup();
        v
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn related_chapters_dual_inner(
    state: &AppState,
    ws: &str,
    pid: &str,
    cur_id: &str,
    lore_focus: &str,
    plot_focus: &str,
) -> Vec<(String, String, String, &'static str)> {
    let dir = format!("projects/{pid}/chapters");
    let entry = match state.works.list(ws, &dir, 1) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    // 双通道分别打分（lore 通道用标题向 focus，plot 通道用指令向 focus）
    let lore_terms = focus_terms_of(lore_focus);
    let plot_terms = focus_terms_of(plot_focus);
    if lore_terms.is_empty() && plot_terms.is_empty() {
        return vec![];
    }
    let mut scored: Vec<(i32, String, String, String, &'static str)> = Vec::new();
    for child in entry.children {
        if child.kind == "dir" {
            continue;
        }
        let rel = format!("{dir}/{}", child.name);
        let body = match state.works.read_text(ws, &rel) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let ch: ChapterDoc = match serde_json::from_str(&body.content) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if ch.id == cur_id {
            continue;
        }
        let hay = format!("{} {}", ch.title, ch.content);
        // lore 通道（标题向）+ plot 通道（指令向）分别打分
        let mut lscore = 0;
        let mut lhit: Vec<String> = Vec::new();
        for term in &lore_terms {
            if hay.contains(term) {
                lscore += 1;
                if lhit.len() < 2 {
                    lhit.push(term.clone());
                }
            }
        }
        let mut pscore = 0;
        let mut phit: Vec<String> = Vec::new();
        for term in &plot_terms {
            if hay.contains(term) {
                pscore += 1;
                if phit.len() < 2 {
                    phit.push(term.clone());
                }
            }
        }
        if lscore > 0 {
            scored.push((lscore, ch.id.clone(), ch.title.clone(), format!("设定{}", lhit.join("/")), "lore"));
        }
        if pscore > 0 {
            scored.push((pscore, ch.id, ch.title, format!("剧情{}", phit.join("/")), "plot"));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    // 去重（同 id 只留高分），lore+plot 各保底 1 席
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<(String, String, String, &'static str)> = Vec::new();
    let mut lore_kept = 0;
    let mut plot_kept = 0;
    for (s, id, title, terms, ch_) in scored {
        if !seen.insert(id.clone()) {
            continue;
        }
        if ch_ == "lore" && lore_kept >= 3 && out.len() >= 3 {
            continue;
        }
        if ch_ == "plot" && plot_kept >= 3 && out.len() >= 3 {
            continue;
        }
        if ch_ == "lore" {
            lore_kept += 1;
        } else {
            plot_kept += 1;
        }
        out.push((id, title, format!("关键词交集{score}（{terms}）", score = s, terms = terms), ch_));
        if out.len() >= 5 {
            break;
        }
    }
    out
}


pub(crate) fn read_guard(state: &AppState, ws: &str, pid: &str, sg: &str) -> Option<ContinuationGuard> {
    state
        .works
        .read_text(ws, &guard_path(pid, sg))
        .ok()
        .and_then(|b| serde_json::from_str::<ContinuationGuard>(&b.content).ok())
}

/// Token 账本（吞噬 DeterminFlow `token_usage.py` 节点级账本口径，Kaleido 化）。
/// 存 `projects/{pid}/ledger.json`：{total:{prompt,completion,calls},byOp:{op:{prompt,completion,calls}},updatedAt}。
pub(crate) fn ledger_path(pid: &str) -> String {
    format!("projects/{pid}/ledger.json")
}

pub(crate) fn read_ledger(state: &AppState, ws: &str, pid: &str) -> Value {
    state
        .works
        .read_text(ws, &ledger_path(pid))
        .ok()
        .and_then(|b| serde_json::from_str::<Value>(&b.content).ok())
        .unwrap_or(json!({"total":{"prompt":0,"completion":0,"calls":0},"byOp":{}}))
}

pub(crate) fn add_ledger(state: &AppState, ws: &str, pid: &str, op: &str, prompt: u64, completion: u64) {
    let mut v = read_ledger(state, ws, pid);
    let bump = |o: &mut Value| {
        o["prompt"] = json!(o.get("prompt").and_then(|x| x.as_u64()).unwrap_or(0) + prompt);
        o["completion"] = json!(o.get("completion").and_then(|x| x.as_u64()).unwrap_or(0) + completion);
        o["calls"] = json!(o.get("calls").and_then(|x| x.as_u64()).unwrap_or(0) + 1);
    };
    bump(&mut v["total"]);
    if v.get("byOp").map(|x| x.is_object()).unwrap_or(false) == false {
        v["byOp"] = json!({});
    }
    let mut entry = v["byOp"].get(op).cloned().unwrap_or(json!({"prompt":0,"completion":0,"calls":0}));
    bump(&mut entry);
    v["byOp"][op] = entry;
    v["updatedAt"] = json!(now());
    let s = serde_json::to_string_pretty(&v).unwrap_or_default();
    let _ = state.works.mkdir(ws, &format!("projects/{pid}"));
    let _ = state.works.write_text(ws, &ledger_path(pid), &s);
}

/// 带计数的 LLM 调用（op = suggest/guard/triage/posthoc），usage 解析容错。
pub(crate) async fn call_llm_tracked(
    state: &AppState,
    ws: &str,
    pid: &str,
    op: &str,
    system: &str,
    user: &str,
) -> Result<(String, u64, u64), String> {
    call_llm_tracked_with(state, ws, pid, op, system, user, "").await
}

pub(crate) async fn call_llm_tracked_with(
    state: &AppState,
    ws: &str,
    pid: &str,
    op: &str,
    system: &str,
    user: &str,
    model_override: &str,
) -> Result<(String, u64, u64), String> {
    let llm = state.app_state.resolve_llm(
        state.llm_base.as_deref(),
        state.llm_key.as_deref(),
        &state.llm_model,
    );
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Err("LLM not configured".into());
    }
    let mut model = if llm.model.is_empty() {
        state.llm_model.clone()
    } else {
        llm.model
    };
    if !model_override.trim().is_empty() {
        model = model_override.trim().to_string();
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .pool_max_idle_per_host(2)
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/chat/completions", llm.base_url.trim_end_matches('/'));
    let body = json!({
        "model": model, "stream": false, "temperature": 0.7,
        "max_tokens": 4096,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });
    let resp = client
        .post(&url)
        .bearer_auth(&llm.api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("LLM read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM HTTP {status}"));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("LLM bad json: {e}"))?;
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM empty content".to_string())?;
    let pt = v.pointer("/usage/prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
    let ct = v.pointer("/usage/completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
    add_ledger(state, ws, pid, op, pt, ct);
    Ok((content, pt, ct))
}
