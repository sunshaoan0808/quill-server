//! intimacy2.rs — 亲密场景状态机：arousal仪表+kink门控+touch门控+边界/人称守卫+事后余晖（吸收自 pnotisdev/rp dating/*.ts，MIT）。
//!
//! **[EXPERIMENTAL — 无生产消费面]**（2026-09-14 接线审计：docs/接线审计-摆设功能全摸排-20260914.md §6.3）
//! 现状：本模块的引用目前只在 core 内部互引，从 `main.rs` 走不到任何生产消费面（arousal 引擎 / touch·kink 门控 / 事后余晖无调用方）。
//! 判据「走不到 = 摆设」下，**不要为了消红而硬接**——硬接即造假接线。要用请先拍板「接哪个 UI / 端点」，
//! 拍板后从真实入口接线，并同步删掉本标记与 `scripts/audit/README.md` 的对应条目。
//!
//! 纪律（抄rp）：
//! - arousal只由引擎算（目录权重/触摸/漂移/衰减），模型无权打分。
//! - 硬限制=过滤行动集（不渲染/不可达），不是提示词劝说。
//! - 守卫只标记不改写（false negative优先）。
//! - 全纯函数，零迁移。

/// 身体部位闭合词表（与rp BODY_REGIONS对齐）。
pub const BODY_REGIONS: [&str; 16] = [
    "hair", "face", "lips", "neck", "ears", "shoulders", "back",
    "chest", "waist", "hips", "thighs", "inner_thigh", "hands", "feet",
    "genitals", "other",
];

pub fn is_body_region(s: &str) -> bool {
    BODY_REGIONS.contains(&s)
}

/// 唤起档位下限：baseline=0/warming=20/engaged=45/edge=78/over=100。
pub fn arousal_band(value: f64) -> &'static str {
    if value >= 100.0 {
        "over"
    } else if value >= 78.0 {
        "edge"
    } else if value >= 45.0 {
        "engaged"
    } else if value >= 20.0 {
        "warming"
    } else {
        "baseline"
    }
}

/// 同部位连摸递减：1/(1+0.4*(n-1))。
pub fn habituation_factor(touch_count: usize) -> f64 {
    1.0 / (1.0 + 0.4 * (touch_count.saturating_sub(1) as f64))
}

/// 默认部位敏感度（未给人物画像时的回退）。
pub fn default_sensitivity(region: &str) -> f64 {
    match region {
        "genitals" => 3.0,
        "inner_thigh" => 2.5,
        "lips" | "neck" | "ears" | "chest" => 2.0,
        "hips" | "thighs" => 1.5,
        "waist" | "back" | "shoulders" | "hands" => 1.0,
        _ => 0.5,
    }
}

/// 单回合仪表步进（抄rp advanceArousal核心算式）。
/// engaged=false→只吃衰减；pullback（delta<0）→扣一半权重。
pub fn advance_arousal(
    value: f64,
    activity_weight: f64,
    intensity_delta: i32,
    touch_gain: f64,
    passive: f64,
    responsiveness: f64,
    engaged: bool,
) -> f64 {
    let mult = match intensity_delta {
        -1 => -0.5,
        0 => 0.5,
        1 => 1.0,
        _ => 1.75,
    };
    let gains = if engaged {
        let drift = if intensity_delta < 0 { 0.0 } else { passive };
        (activity_weight * mult + touch_gain + drift) * responsiveness
    } else {
        0.0
    };
    (value + gains - 1.5).clamp(0.0, 100.0).round()
}

/// kink价：-2硬限制/-1不喜/0中性/1喜欢/2热衷。
pub fn kink_valence(valence: Option<i32>, hard_limited: bool) -> i32 {
    if hard_limited {
        return -2;
    }
    valence.unwrap_or(0)
}

/// 内容综合价：有 dislike 取最差，有喜爱取最高（dislike 压倒 enthusiasm）。
pub fn combined_valence(valences: &[i32]) -> i32 {
    if valences.is_empty() {
        return 0;
    }
    let worst = *valences.iter().min().unwrap();
    if worst < 0 {
        worst
    } else {
        *valences.iter().max().unwrap()
    }
}

/// 价→唤起乘区：2→1.35 / 1→1.15 / -1→0.7 / -2→0.4（理论值，硬限制到不了这）。
pub fn kink_responsiveness(valence: i32) -> f64 {
    match valence {
        2 => 1.35,
        1 => 1.15,
        -1 => 0.7,
        -2 => 0.4,
        _ => 1.0,
    }
}

/// 部位可用：offLimits绝对禁；gated按warmth阈值。
pub fn is_region_available(off_limits: &[String], gated: &[(String, f64)], region: &str, warmth: f64) -> bool {
    if off_limits.iter().any(|r| r == region) {
        return false;
    }
    match gated.iter().find(|(r, _)| r == region) {
        None => true,
        Some((_, gate)) => warmth >= *gate,
    }
}

/// 边界踩线：boundary显著词（≥4字母去停用词）≤2个全中，>2个中75%（抄rp boundaryGuard.ts原表）。
pub fn boundary_phrase_crossed(boundary: &str, reply: &str) -> bool {
    let stop = [
        "the", "a", "an", "and", "or", "of", "to", "in", "on", "with", "without", "being", "is", "are",
        "wont", "won't", "never", "not", "no", "anything", "something", "their", "they", "them", "her",
        "his", "she", "he", "about", "that", "this", "from", "any", "own", "will", "wouldnt", "wouldn't",
        "doesnt", "doesn't", "dont", "don't", "cant", "can't", "just", "only", "ever", "always",
    ];
    let words: Vec<String> = boundary
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| w.len() >= 4 && !stop.contains(w))
        .map(|w| w.to_string())
        .collect();
    if words.is_empty() {
        return false;
    }
    let lower = reply.to_lowercase();
    let hits = words.iter().filter(|w| lower.contains(w.as_str())).count();
    if words.len() <= 2 {
        hits == words.len()
    } else {
        hits * 4 >= words.len() * 3
    }
}

/// 人称越界：玩家名作明确主语+无歧义动词（抄rp agencyGuard.ts 主语锚定）。
/// 只认“名 (+0~1个ly副词) + 内心/失控动词”，绝不只因同句共现就报；
/// 所有格（Kai's）不算主语。分句级扫描，宁漏勿错。
pub fn persona_agency_violation(persona_name: &str, reply: &str) -> Option<String> {
    fn toks(s: &str) -> Vec<String> {
        s.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
            .filter(|t| !t.is_empty())
            .map(|t| t.to_lowercase())
            .collect()
    }
    const INTERNAL: [&str; 13] = [
        "felt", "feels", "realized", "realizes", "thought", "thinks", "wondered", "wonders",
        "knew", "understood", "decided", "remembered", "noticed",
    ];
    const INVOLUNTARY: [&str; 8] = [
        "shivered", "gasped", "flinched", "tensed", "trembled", "shuddered", "ached", "melted",
    ];
    const CLIMAX_NOUN: [&str; 3] = ["climax", "orgasm", "release"];
    let name = persona_name.trim();
    if name.is_empty() || reply.trim().is_empty() {
        return None;
    }
    let name_t = toks(name);
    if name_t.is_empty() {
        return None;
    }
    let is_verb = |w: &str| INTERNAL.contains(&w) || INVOLUNTARY.contains(&w);
    for sent in reply.split(|c| c == '.' || c == '!' || c == '?' || c == '。' || c == '！' || c == '？') {
        let t = sent.trim();
        if t.is_empty() {
            continue;
        }
        let w = toks(t);
        let mut i = 0;
        while i + name_t.len() <= w.len() {
            if w[i..i + name_t.len()] != name_t[..] {
                i += 1;
                continue;
            }
            let mut k = i + name_t.len();
            // 高潮三式：name (+0~2词) reached his/her/their [own] climax
            //        | name's own climax/orgasm/release | name (+0~2词) orgasmed
            let mut j = k;
            let mut steps = 0;
            while j < w.len() && steps < 2 && w[j] != "reached" && w[j] != "orgasmed" {
                j += 1;
                steps += 1;
            }
            if j < w.len() && w[j] == "reached"
                && j + 2 < w.len() + 1
                && ["his", "her", "their"].contains(&w.get(j + 1).map(|s| s.as_str()).unwrap_or(""))
            {
                let after = j + 2;
                let noun_idx = if w.get(after).map(|s| s.as_str()) == Some("own") { after + 1 } else { after };
                if w.get(noun_idx).map(|s| CLIMAX_NOUN.contains(&s.as_str())).unwrap_or(false) {
                    return Some(t.to_string());
                }
            }
            if j < w.len() && w[j] == "orgasmed" {
                return Some(t.to_string());
            }
            // 内心/失控：name (+0~1个ly副词) + 动词
            if k < w.len() && w[k].ends_with("ly") && w[k].len() > 2 {
                k += 1;
            }
            if k < w.len() && is_verb(&w[k]) {
                return Some(t.to_string());
            }
            // 所有格：name's own climax/orgasm/release
            if name_t.len() == 1 && k < w.len() {
                let poss = w[k] == format!("{}'s", name_t[0]) || w[k] == format!("{}’s", name_t[0]);
                if poss
                    && w.get(k + 1).map(|s| s.as_str()) == Some("own")
                    && w.get(k + 2).map(|s| CLIMAX_NOUN.contains(&s.as_str())).unwrap_or(false)
                {
                    return Some(t.to_string());
                }
            }
            i += 1;
        }
    }
    None
}

/// 亲密档位（抄rp intimacyGuidance.ts；world 档优先于全局档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntimacyLevel {
    Default,
    FadeToBlack,
    Suggestive,
    Explicit,
}

/// 世界档（Some=用世界的；None=继承全局）。
pub fn resolve_intimacy_level(world_level: Option<IntimacyLevel>, global: IntimacyLevel) -> IntimacyLevel {
    world_level.unwrap_or(global)
}

pub fn intimacy_guidance(level: IntimacyLevel) -> &'static str {
    match level {
        IntimacyLevel::FadeToBlack => "When a scene turns intimate, imply and suggest rather than describe explicitly — trail off, cut to after, or shift the scene once things move past kissing or undressing. Don't narrate explicit sexual content.",
        IntimacyLevel::Suggestive => "Intimate scenes can include sensual, romantic detail — physical closeness, described touch, charged tension, arousal — without being graphically explicit. Evocative, not clinical or crude.",
        IntimacyLevel::Explicit => "When a scene the story has actually built toward turns sexual, write it fully and explicitly — physical detail, dialogue, sensation — in the character's own established voice, not a generic one. Don't fade to black or cut away once the scene has genuinely earned it.",
        IntimacyLevel::Default => "",
    }
}

/// 低 warmth 下玩家自由文本试图身体升级（抄rp EARLY_PHYSICAL_ESCALATION核心词）。
/// warmth>=15 或无升级词→空串；否则返回预警行。
pub fn early_escalation_guidance(warmth: f64, latest_user_text: &str, char_name: &str) -> String {
    if warmth >= 15.0 {
        return String::new();
    }
    let low = latest_user_text.to_lowercase();
    let cues = [
        "kiss", "kisses", "kissed", "kissing", "make out", "make-out", "touch", "touches",
        "touched", "touching", "caress", "grope", "undress", "strip", "hold her", "hold him",
        "hold them", "hold you", "holding her", "holding him", "holding you", "pull her",
        "pull him", "pull you", "pulling her", "pulling him", "pulling you", "亲", "吻", "抱",
        "摸", "脱",
    ];
    if !cues.iter().any(|c| low.contains(c)) {
        return String::new();
    }
    let who = if char_name.trim().is_empty() { "she".to_string() } else { char_name.trim().to_string() };
    format!("Warmth is still very low. This physical escalation is not assumed welcome or earned; let {who} pull back, deflect, set distance, or keep things lighter if that is the honest read. Never accept it just to be agreeable.")
}

/// 事后余晖窗口4回合（角色自己回复数计）。
pub const AFTERGLOW_TURNS: i64 = 4;

pub fn is_afterglow_active(started_at_turn: i64, char_replies: i64) -> bool {
    let since = char_replies - started_at_turn;
    since >= 0 && since < AFTERGLOW_TURNS
}

/// 余晖是否已到期该结算（started 后满 4 个角色回复）。
pub fn is_afterglow_complete(started_at_turn: i64, char_replies: i64) -> bool {
    char_replies - started_at_turn >= AFTERGLOW_TURNS
}

/// 余晖裁决封闭词表（抄rp AFTERCARE_VERDICTS）。
pub const AFTERCARE_VERDICTS: [&str; 3] = ["tender", "awkward", "cold"];

/// 裁决→七维 delta（抄rp VERDICT_DELTAS；cold 唯一动 tension）。
/// 返回 (affection, trust, chemistry, comfort, respect, curiosity, tension)。
pub fn aftercare_deltas(verdict: &str) -> (f64, f64, f64, f64, f64, f64, f64) {
    match verdict {
        "tender" => (2.0, 3.0, 1.0, 3.0, 0.0, 0.0, 0.0),
        "awkward" => (0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0),
        "cold" => (-1.0, -4.0, 0.0, -4.0, 0.0, 0.0, 3.0),
        _ => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    }
}

/// 角色回复数（role==char 才算；余晖窗口的单位）。
pub fn count_char_replies(roles: &[&str]) -> i64 {
    roles.iter().filter(|r| **r == "char").count() as i64
}

/// 人物触摸画像：某部位响应值（画像缺省→默认表）。
pub fn sensitivity_for(profile: &[(String, f64)], region: &str) -> f64 {
    profile
        .iter()
        .find(|(r, _)| r == region)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| default_sensitivity(region))
}

/// 发现阈值 2.0：摸到高响应新部位才记入“已发现”。
pub const DISCOVERY_SENSITIVITY: f64 = 2.0;

/// 新发现部位（去重、排除已知、响应≥2.0；抄rp newlyDiscoveredRegions）。
pub fn newly_discovered_regions(
    profile: &[(String, f64)],
    touched: &[String],
    known: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in touched {
        if known.iter().any(|k| k == r) || out.iter().any(|o| o == r) {
            continue;
        }
        if sensitivity_for(profile, r) >= DISCOVERY_SENSITIVITY {
            out.push(r.clone());
        }
    }
    out
}

/// 未开放部位（offLimits + 未达 warmth 门槛的 gated；抄rp unavailableRegions）。
pub fn unavailable_regions(
    off_limits: &[String],
    gated: &[(String, f64)],
    warmth: f64,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in off_limits {
        out.push(r.clone());
    }
    for (r, gate) in gated {
        if warmth < *gate && !out.iter().any(|o| o == r) {
            out.push(r.clone());
        }
    }
    out
}

/// 内置 kink 词表（抄rp BUILT_IN_KINKS；世界可扩展，开放字符串）。
pub const BUILT_IN_KINKS: [&str; 11] = [
    "bondage", "sensory_play", "toys", "edging", "praise", "dirty_talk",
    "roleplay", "exhibitionism", "oral", "rough", "gentle",
];

/// 画像拍平给 stage 条件读：valence 全抄 + hardLimits 记 -2（抄rp kinkValenceMap）。
/// 输入 (valence表, hard表)；返回 None=画像全空（调用方可跳过条件）。
pub fn kink_valence_map(
    valence: &[(String, i32)],
    hard_limits: &[String],
) -> Option<Vec<(String, i32)>> {
    if valence.is_empty() && hard_limits.is_empty() {
        return None;
    }
    let mut map: Vec<(String, i32)> = valence.to_vec();
    for h in hard_limits {
        if let Some(e) = map.iter_mut().find(|(k, _)| k == h) {
            e.1 = -2;
        } else {
            map.push((h.clone(), -2));
        }
    }
    Some(map)
}

/// 唤起档位白话（抄rp AROUSAL_BAND_PHRASE；prompt与UI共用一套，防漂移）。
pub fn arousal_band_phrase(band: &str) -> &'static str {
    match band { "warming"=>"warming up", "engaged"=>"well into it", "edge"=>"close to the edge", "over"=>"over the edge", _=>"not yet worked up" }
}
/// 场景终结哨兵（抄rp RESOLVE_STAGE=resolve；终结走judge观察，不自动跳）。
pub const RESOLVE_STAGE: &str = "resolve";
/// 分支默认超时3回合（抄rp CHOICE_DEFAULT_AFTER_TURNS；防死等）。
pub const CHOICE_DEFAULT_AFTER_TURNS: i64 = 3;
/// stage自身门槛：turns与arousal双达标才放行（抄rp stageFloorsMet）。
pub fn stage_floors_met(min_turns: i64, turns_in_stage: i64, min_arousal: f64, arousal: f64) -> bool {
    turns_in_stage >= min_turns && arousal >= min_arousal
}

/// arousal→phase唯一判定（抄rp phaseForArousal；≥78 peak else building）。
pub fn phase_for_arousal(value: f64) -> &'static str { if value >= 78.0 { "peak" } else { "building" } }
/// 场景过期/存活（抄rp isIntimacySceneStale/Active；updatedAtTurn>count=回档分叉）。
pub fn is_intimacy_scene_stale(updated_at_turn: i64, char_replies: i64) -> bool { updated_at_turn > char_replies }
pub fn is_intimacy_scene_active(has_scene: bool, updated_at_turn: i64, char_replies: i64) -> bool {
    has_scene && !is_intimacy_scene_stale(updated_at_turn, char_replies)
}
/// 内容立场（抄rp stanceOfScene；≤-1 reluctant，≥2 eager）。
pub fn stance_of_scene(activity_valence: i32) -> Option<&'static str> {
    if activity_valence <= -1 { Some("reluctant") } else if activity_valence >= 2 { Some("eager") } else { None }
}
/// 本stage呆了几回合（抄rp turnsInStage；stageSince??started??updated）。
pub fn turns_in_stage(char_replies: i64, stage_since: Option<i64>, started: Option<i64>, updated: i64) -> i64 {
    char_replies - stage_since.or(started).unwrap_or(updated)
}
/// 终场快照（抄rp sceneResolveSnapshot；turns/最低arousal/走过stage数）。
pub fn scene_resolve_snapshot(char_replies: i64, started: Option<i64>, updated: i64, arousal_floor: f64, visited: &[String], stage_id: &str) -> (i64, f64, usize) {
    let turns = (char_replies - started.unwrap_or(updated)).max(0);
    let mut uniq = visited.to_vec(); uniq.push(stage_id.to_string()); uniq.sort(); uniq.dedup();
    (turns, arousal_floor, uniq.len())
}
/// 节奏三档（抄rp intimacyPaceFor；reserved: plan/≥2边界/焦虑系mood）。
pub fn intimacy_pace_for(mood: Option<&str>, holding_back: bool, boundary_count: usize) -> &'static str {
    const RESERVED: [&str; 5] = ["anxious","guarded","embarrassed","tense","exhausted"];
    const EAGER: [&str; 4] = ["playful","excited","confident","affectionate"];
    if holding_back || boundary_count >= 2 || mood.map(|m| RESERVED.contains(&m)).unwrap_or(false) { return "reserved"; }
    if mood.map(|m| EAGER.contains(&m)).unwrap_or(false) { return "eager"; }
    "neutral"
}
/// 同意张力（抄rp isConsentTension；comfort<45且chemistry-comfort≥20）。
pub const CONSENT_TENSION_GAP: f64 = 20.0;
pub const CONSENT_TENSION_COMFORT_FLOOR: f64 = 45.0;
pub fn is_consent_tension(comfort: Option<f64>, chemistry: Option<f64>) -> bool {
    match (comfort, chemistry) { (Some(c), Some(k)) => c < CONSENT_TENSION_COMFORT_FLOOR && k - c >= CONSENT_TENSION_GAP, _ => false }
}
/// 全量responsiveness（抄rp responsivenessFor；pace*chemistry*tension*kink）。
pub fn responsiveness_full(pace: &str, comfort: Option<f64>, chemistry: Option<f64>, valence: i32) -> f64 {
    let pace_f = match pace { "reserved"=>0.7, "eager"=>1.3, _=>1.0 };
    let chem_f = match chemistry { None=>1.0, Some(k)=> 0.8 + k/250.0 };
    let tens_f = if is_consent_tension(comfort, chemistry) { 0.75 } else { 1.0 };
    pace_f * chem_f * tens_f * kink_responsiveness(valence)
}
/// explicit套话表+检测（抄rp EXPLICIT_ANTI_PATTERNS；仅peak查）。
pub const EXPLICIT_ANTI_PATTERNS: [&str; 15] = ["waves of pleasure","lost in the sensation","lost in the feeling","their bodies moved as one","their bodies became one","she felt so full","ecstasy","rapture","bliss","ministrations","he entered her","she took him in","he filled her","buried himself","moaned in pleasure"];
pub fn detect_explicit_anti_pattern(reply: &str, phase: &str) -> Option<&'static str> {
    if phase != "peak" || reply.trim().is_empty() { return None; }
    let lower = reply.to_lowercase();
    EXPLICIT_ANTI_PATTERNS.iter().find(|p| lower.contains(**p)).copied()
}
/// 场景形状log（抄rp SCENE_SHAPE_LOG_CAP=3；记最近3场目录序列）。
pub const SCENE_SHAPE_LOG_CAP: usize = 3;
pub fn append_scene_shape_log(log: &[Vec<String>], shape: Vec<String>) -> Vec<Vec<String>> {
    let mut next = log.to_vec(); next.push(shape);
    if next.len() > SCENE_SHAPE_LOG_CAP { next[next.len()-SCENE_SHAPE_LOG_CAP..].to_vec() } else { next }
}
/// 连三场同曲线→求变（抄rp repeatedEscalationShapeGuidance）。
pub fn repeated_escalation_shape_guidance(char_name: &str, log: &[Vec<String>]) -> Option<String> {
    if log.len() < 2 { return None; }
    let (a,b) = (&log[log.len()-2], &log[log.len()-1]);
    if a.len() < 2 || a.len() != b.len() || !a.iter().zip(b.iter()).all(|(x,y)| x==y) { return None; }
    Some(format!("The last two intimate scenes with {char_name} both moved through the exact same sequence ({}), start to finish. Let this one actually diverge somewhere rather than tracing the identical curve a third time running.", a.join(" → ")))
}
/// 事前期待/同意张力行文（抄rp anticipation/consent guidance；55双高才推）。
pub fn intimacy_anticipation_guidance(char_name: &str, user_name: &str, chemistry: f64, comfort: f64) -> Option<String> {
    if chemistry < 55.0 || comfort < 55.0 { return None; }
    Some(format!("Nothing physical has started yet, but the chemistry and ease between {char_name} and {user_name} are both genuinely high right now. Let the anticipation build honestly rather than forcing the escalation early."))
}
pub fn intimacy_consent_tension_guidance(char_name: &str, comfort: f64, chemistry: f64) -> Option<String> {
    if !is_consent_tension(Some(comfort), Some(chemistry)) { return None; }
    Some(format!("Right now {char_name}'s comfort is trailing well behind the physical chemistry in this scene. That's worth letting show: a beat of hesitation or an unprompted check-in is the right call here."))
}

/// 事后一句话（抄rp aftercareReason；tender贴/cold走/awkward尬）。
pub fn aftercare_reason(verdict: &str) -> &'static str {
    match verdict {
        "tender" => "Stayed close and warm in the hours after",
        "cold" => "Pulled away in the hours after",
        _ => "The hours after landed awkwardly",
    }
}
/// 事后toast（抄rp aftercareToast；awkward不打扰返回None）。
pub fn aftercare_toast(char_name: &str, verdict: &str) -> Option<String> {
    match verdict {
        "tender" => Some(format!("{char_name} felt looked after in the hours that followed.")),
        "cold" => Some(format!("{char_name} was left alone with it afterwards.")),
        _ => None,
    }
}
/// 事后遗留需求（抄rp aftercareNeed；只cold留reassurance）。
pub fn aftercare_need(verdict: &str) -> Option<&'static str> {
    if verdict == "cold" { Some("reassurance") } else { None }
}
/// 裁决合法校验（抄rp isAftercareVerdict；三词封闭表）。
pub fn is_aftercare_verdict(v: &str) -> bool {
    matches!(v, "tender" | "awkward" | "cold")
}
/// 余晖第几回合（抄rp afterglowTurnsSince；倒带返回None）。
pub fn afterglow_turns_since(started_at_turn: Option<i64>, char_replies: i64) -> Option<i64> {
    match started_at_turn { None => None, Some(s) => { let d = char_replies - s; if d < 0 { None } else { Some(d) } } }
}
/// 事后节奏（抄rp aftercarePaceContext；scene优先：<4回合/唤起<60=rushed，≥8回合或跨阶段=earned；无scene看动量≥2）。
pub fn aftercare_pace(momentum_at_start: Option<f64>, scene_turns: Option<i64>, arousal_at_resolve: Option<f64>, stages_moved: Option<i64>) -> Option<&'static str> {
    if let (Some(t), Some(a)) = (scene_turns, arousal_at_resolve) {
        if t < 4 { return Some("rushed"); }
        if a < 60.0 { return Some("rushed"); }
        if t >= 8 || stages_moved.unwrap_or(0) > 1 { return Some("earned"); }
    } else if scene_turns.is_some() {
        return Some("earned");
    }
    match momentum_at_start { None => { if scene_turns.is_some() { Some("earned") } else { None } }, Some(m) => Some(if m >= 2.0 { "rushed" } else { "earned" }) }
}

/// 空唤起态（抄rp emptyArousalState；value0+空暴露+bandSince=当前回合）。
pub fn empty_arousal_value() -> f64 { 0.0 }
/// 部位名白话（抄rp regionLabel；下划线转空格）。
pub fn region_label(region: &str) -> String { region.replace('_', " ") }
/// 钝化部位（抄rp habituatedRegion；同部位摸≥4次取最多者）。
pub fn habituated_region(exposure: &[(String, i64)]) -> Option<String> {
    exposure.iter().filter(|(_, c)| *c >= 4).max_by_key(|(_, c)| *c).map(|(r, _)| r.clone())
}
/// 唤起步进常量（抄rp REGION_GAIN_STEP2/PASSIVE2/DECAY1.5/STALL-6/DRIFT-10/HESITATION-4/-12）。
pub const REGION_GAIN_STEP: f64 = 2.0;
pub const AROUSAL_PASSIVE_GAIN: f64 = 2.0;
pub const AROUSAL_DECAY_PER_TURN: f64 = 1.5;
pub const STALL_PENALTY: f64 = -6.0;
pub const DRIFT_PENALTY: f64 = -10.0;
pub const HESITATION_PENALTY: f64 = -4.0;
pub const HESITATION_TENSION_PENALTY: f64 = -12.0;
/// 强度乘子（抄rp DELTA_MULTIPLIER；-1=-0.5/0=0.5/1=1/2=1.75）。
pub fn intensity_multiplier(delta: i64) -> f64 {
    match delta { -1 => -0.5, 0 => 0.5, 1 => 1.0, 2 => 1.75, _ => 0.0 }
}
/// 节奏响应（抄rp PACE_RESPONSIVENESS；reserved0.7/neutral1/eager1.3）。
pub fn pace_responsiveness(pace: &str) -> f64 {
    match pace { "reserved" => 0.7, "eager" => 1.3, _ => 1.0 }
}

/// 老存档唤起播种（抄rp arousalOf；有表用表，无表peak=78否则0，bandSince沿phaseSince??updated）。
pub fn arousal_seed_for_phase(has_arousal: bool, value: f64, phase_is_peak: bool) -> f64 {
    if has_arousal { value } else if phase_is_peak { 78.0 } else { 0.0 }
}
/// 全场最低唤起（抄rp sceneArousalFloorValue；单人=owner值，多人取最落后者，防被快带快）。
pub fn scene_arousal_floor(owner_value: f64, others: &[f64]) -> f64 {
    if others.is_empty() { owner_value } else { others.iter().fold(owner_value, |a, b| a.min(*b)) }
}
/// 当前档位（抄rp sceneArousalBand；默认看owner）。
pub fn scene_arousal_band(value: f64) -> &'static str { arousal_band(value) }
/// 中途切活动压顶（抄rp startOrShift；新欢要自己再赚上去，封顶edge-1=77）。
pub fn recapped_arousal(prior: f64) -> f64 { prior.min(77.0) }
/// 观察合法性（抄rp parseIntimacyObservation；无engagement=非观察；intensity只认-1/0/1/2）。
pub fn is_valid_intimacy_observation(has_engagement: bool) -> bool { has_engagement }
pub fn normalize_intensity_delta(raw: i32) -> i32 {
    match raw { -1|0|1|2 => raw, _ => 0 }
}
/// 终结旗落袋（抄rp resolvedSceneFlags；去重并入sceneFlags）。
pub fn merge_scene_flags(mut flags: Vec<String>, add: &[String]) -> Vec<String> {
    for s in add { if !flags.iter().any(|x| x==s) { flags.push(s.clone()); } }
    flags
}

/// 档位下限表（抄rp BAND_FLOORS；baseline0/warming20/engaged45/edge78/over100）。
pub const BAND_FLOORS: [(&str, f64); 5] = [("baseline",0.0),("warming",20.0),("engaged",45.0),("edge",78.0),("over",100.0)];
pub fn band_floor(band: &str) -> f64 { BAND_FLOORS.iter().find(|(b,_)| *b==band).map(|(_,v)| *v).unwrap_or(0.0) }
/// 别名：arousalBandFor（与arousal_band同义，防上游改名断线）。
pub fn arousal_band_for(value: f64) -> &'static str { arousal_band(value) }
/// 空唤起态三件套（抄rp emptyArousalState；value0+空暴露+bandSince=当前回合）。
pub fn empty_arousal_state(char_replies: i64) -> (f64, i64) { (empty_arousal_value(), char_replies) }
/// 默认敏感度别名（抄rp DEFAULT_REGION_SENSITIVITY；与default_sensitivity同表）。
pub fn default_region_sensitivity(region: &str) -> f64 { default_sensitivity(region) }
/// 单回合responsiveness别名（抄rp responsivenessFor；pace*chemistry*tension*kink）。
pub fn responsiveness_for(pace: &str, comfort: Option<f64>, chemistry: Option<f64>, valence: i32) -> f64 { responsiveness_full(pace, comfort, chemistry, valence) }
/// 联合边界检查（抄rp detectAnyBoundaryCrossing；角色边界优先，人设limit兜底）。
pub fn detect_any_boundary_crossing(char_bounds: &[String], persona_limits: &[String], reply: &str) -> Option<String> {
    for b in char_bounds.iter().chain(persona_limits.iter()) { if boundary_phrase_crossed(b, reply) { return Some(b.clone()); } }
    None
}
/// 发现列表合并（抄rp withDiscoveredRegions；无新增返回原样）。
pub fn with_discovered_regions(known: Vec<String>, found: &[String]) -> Vec<String> {
    let mut out = known.clone();
    for f in found { if !out.iter().any(|x| x==f) { out.push(f.clone()); } }
    out
}

/// 观察解析（抄rp parseIntimacyObservation；无engagement=非观察；intensity只认-1/0/1/2其余归0）。
pub fn parse_intimacy_observation(has_engagement: bool, raw_delta: i32) -> Option<i32> {
    if !has_engagement { return None; }
    Some(normalize_intensity_delta(raw_delta))
}
/// stage定位（抄rp stageOf；已知id直用，否则按phase找同类，再否则entry，永不stranded）。
pub fn stage_of<'a>(known: Option<&'a str>, by_phase: Option<&'a str>, entry: &'a str) -> &'a str {
    known.or(by_phase).unwrap_or(entry)
}
/// 唤起读取（抄rp arousalOf；owner看主表，非owner看个人表，老存档按phase播种）。
pub fn arousal_of(has_table: bool, value: f64, phase_is_peak: bool) -> f64 { arousal_seed_for_phase(has_table, value, phase_is_peak) }
/// 衣物账本（抄rp clothingOf；owner看主场，非owner看个人账）。
pub fn clothing_of<'a>(owner_ledger: Option<&'a str>, personal_ledger: Option<&'a str>, is_owner: bool) -> Option<&'a str> {
    if is_owner { owner_ledger } else { personal_ledger }
}
/// 开场/中转（抄rp startOrShiftIntimacyScene；点按即consent检查点，building重开，唤起压顶77，history续记）。
pub fn start_or_shift_arousal(prior: Option<f64>, char_replies: i64) -> (f64, i64) {
    match prior { Some(v) => (recapped_arousal(v), char_replies), None => empty_arousal_state(char_replies) }
}
/// 推进机（抄rp advanceIntimacyScene；pendingChoice未决不推进，auto边按条件走，resolve只认judge亲眼见）。
pub fn advance_intimacy_scene(has_pending_choice: bool, auto_edge: Option<&str>, resolve_seen: bool) -> Option<String> {
    if has_pending_choice { return None; }
    if resolve_seen { return Some("resolve".to_string()); }
    auto_edge.map(|s| s.to_string())
}
/// 分支作答（抄rp resolveIntimacyChoice；对id不对edgeTo，老存档edgeTo兜底）。
pub fn resolve_intimacy_choice<'a>(options: &'a [(String, String)], answer: &str) -> Option<&'a (String, String)> {
    crate::catalog2::pending_choice_option(options, answer)
}
/// 场景行文（抄rp intimacySceneGuidance；只讲已发生，不剧透未解锁）。
pub fn intimacy_scene_guidance(activity_label: &str, phase: &str, arousal_band: &str) -> String {
    format!("{activity_label} ({phase}, {arousal_band})")
}
/// explicit套话旧文（抄rp detectExplicitAntiPatternUsed；仅peak查，building不扰）。
pub fn detect_explicit_anti_pattern_used(reply: &str, phase: &str) -> Option<&'static str> { detect_explicit_anti_pattern(reply, phase) }
/// explicit行文（抄rp explicitSceneGuidance；peak才给浓写法，其余收）。
pub fn explicit_scene_guidance(phase: &str) -> Option<&'static str> {
    if phase == "peak" { Some("write it as it happens, beat by beat") } else { None }
}
/// 事后行文（抄rp explicitAftercareGuidance；aftercare不是stage，余晖另走）。
pub fn explicit_aftercare_guidance() -> &'static str { "stay close a beat; let them come down at their own pace" }
/// 事后节奏上下文（抄rp aftercarePaceContext；scene优先判rushed/earned，无scene看动量）。
pub fn aftercare_pace_context(momentum_at_start: Option<f64>, scene_turns: Option<i64>, arousal_at_resolve: Option<f64>, stages_moved: Option<i64>) -> Option<&'static str> {
    aftercare_pace(momentum_at_start, scene_turns, arousal_at_resolve, stages_moved)
}

/// 场景唤起地板（抄rp sceneArousalFloorValue；取最不兴奋者，防快者拖慢者走；单人=本人）。
pub fn scene_arousal_floor_value(owner_value: f64, others: &[f64]) -> f64 {
    others.iter().fold(owner_value, |m, v| m.min(*v))
}
/// 开场/中转全量（抄rp startOrShiftIntimacyScene；点按即consent检查点，building重开，history续记）。
pub fn start_or_shift_intimacy_scene(prior_category_history: Option<&[String]>, category: &str, char_replies: i64) -> (Vec<String>, f64, i64) {
    let mut hist = prior_category_history.map(|h| h.to_vec()).unwrap_or_default();
    hist.push(category.to_string());
    let (a, t) = start_or_shift_arousal(None, char_replies);
    (hist, a, t)
}
/// 终结旗预演（抄rp resolvedSceneFlags；resolving答案回null后唯一 durable 痕迹，调用方并入Chat.sceneFlags）。
pub fn resolved_scene_flags(current: &[String], sets_flag: Option<&str>) -> Vec<String> {
    let mut out = current.to_vec();
    if let Some(f) = sets_flag { if !out.contains(&f.to_string()) { out.push(f.to_string()); } }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_and_habituation() {
        assert_eq!(arousal_band(0.0), "baseline");
        assert_eq!(arousal_band(78.0), "edge");
        assert!((habituation_factor(1) - 1.0).abs() < 1e-9);
        assert!(habituation_factor(4) < 0.5);
    }

    #[test]
    fn kink_combined() {
        assert_eq!(combined_valence(&[2, 1]), 2);
        assert_eq!(combined_valence(&[2, -1]), -1);
        assert_eq!(kink_responsiveness(-2), 0.4);
        assert!(!is_region_available(&["lips".into()], &[], "lips", 99.0));
        assert!(!is_region_available(&[], &[("lips".into(), 80.0)], "lips", 10.0));
    }

    #[test]
    fn guards_catch() {
        assert!(boundary_phrase_crossed("never touch my hair", "he touched my hair roughly"));
        assert!(persona_agency_violation("小明", "小明 felt a sudden fear").is_some());
        assert!(is_afterglow_active(10, 13));
        assert!(!is_afterglow_active(10, 14));
    }
}
