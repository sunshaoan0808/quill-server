//! relationship_stage.rs — 关系梯子+承诺阶梯+分手警告/留疤（吸收自 pnotisdev/rp dating/stage.ts，MIT）。
//!
//! **[EXPERIMENTAL — 无生产消费面]**（2026-09-14 接线审计：docs/接线审计-摆设功能全摸排-20260914.md §6.3）
//! 现状：本模块的引用目前只在 core 内部互引，从 `main.rs` 走不到任何生产消费面（关系梯子推进 / 分手警告 / 留疤无调用方）。
//! 判据「走不到 = 摆设」下，**不要为了消红而硬接**——硬接即造假接线。要用请先拍板「接哪个 UI / 端点」，
//! 拍板后从真实入口接线，并同步删掉本标记与 `scripts/audit/README.md` 的对应条目。
//!
//! 纪律（抄rp）：
//! - warmth只决定能不能开口求承诺，不能决定对方答应；成败另判（求婚/表白走judge）。
//! - relationshipAtRisk只对已承诺关系生效；未承诺（none）无状态可失去。
//! - 分手警告3天宽限（真实现实时间），过期自动降级none + trust/comfort/chemistry各-15留疤。
//! - 向后兼容：全纯函数，无状态，零迁移。

/// 关系阶段（低→高）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipStage {
    #[default]
    NearStrangers,
    Acquaintances,
    WarmingUp,
    GettingClose,
    Close,
    Sweethearts,
}

/// 承诺阶梯（低→高）。warmth只定能不能问，不自动授予。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStatus {
    #[default]
    None,
    Dating,
    Exclusive,
    LivingTogether,
    Married,
}

/// 关系六维（rp口径：trust/chemistry/comfort/respect/curiosity/tension）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelDim {
    Trust,
    Chemistry,
    Comfort,
    Respect,
    Curiosity,
    Tension,
}

/// 六维数值（0-100）。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, Default)]
pub struct RelStats {
    pub trust: f64,
    pub chemistry: f64,
    pub comfort: f64,
    pub respect: f64,
    pub curiosity: f64,
    pub tension: f64,
}

/// 默认warmth里程碑（低→高），与rp RELATIONSHIP_MILESTONES对齐。
pub const MILESTONES: [(RelationshipStage, f64); 6] = [
    (RelationshipStage::NearStrangers, 0.0),
    (RelationshipStage::Acquaintances, 15.0),
    (RelationshipStage::WarmingUp, 35.0),
    (RelationshipStage::GettingClose, 55.0),
    (RelationshipStage::Close, 75.0),
    (RelationshipStage::Sweethearts, 90.0),
];

pub const COMMITMENT_ORDER: [CommitmentStatus; 5] = [
    CommitmentStatus::None,
    CommitmentStatus::Dating,
    CommitmentStatus::Exclusive,
    CommitmentStatus::LivingTogether,
    CommitmentStatus::Married,
];

/// warmth综合分：affection + trust/chemistry/comfort/respect均值（curiosity/tension不计）。
pub fn compute_warmth(affection: f64, s: &RelStats) -> f64 {
    clamp100((affection + s.trust + s.chemistry + s.comfort + s.respect) / 5.0)
}

pub fn clamp100(n: f64) -> f64 {
    n.clamp(0.0, 100.0).round()
}

/// warmth→阶段。
pub fn stage_for_warmth(warmth: f64) -> RelationshipStage {
    let mut stage = RelationshipStage::NearStrangers;
    for (s, at) in MILESTONES {
        if warmth >= at {
            stage = s;
        }
    }
    stage
}

/// 下一承诺阶。
pub fn next_commitment_tier(cur: CommitmentStatus) -> Option<CommitmentStatus> {
    let i = COMMITMENT_ORDER.iter().position(|c| *c == cur)?;
    COMMITMENT_ORDER.get(i + 1).copied()
}

/// 各阶门槛warmth（对齐rp：dating=55/exclusive=75/living+married=90）。
pub fn commitment_tier_threshold(tier: CommitmentStatus) -> f64 {
    match tier {
        CommitmentStatus::Dating => 55.0,
        CommitmentStatus::Exclusive => 75.0,
        CommitmentStatus::LivingTogether | CommitmentStatus::Married => 90.0,
        CommitmentStatus::None => 0.0,
    }
}

/// warmth够不够开口问这一阶（问≠答应）。
pub fn can_ask_for_commitment(tier: CommitmentStatus, warmth: f64) -> bool {
    warmth >= commitment_tier_threshold(tier)
}

/// 开口锁：warmth不够 / dating缺kiss / 同居结婚缺first_time。
/// 返回None=可问；Some(reason)=锁死原因。
pub fn commitment_lock_reason(
    tier: CommitmentStatus,
    warmth: f64,
    has_kissed: bool,
    first_intimate_at: Option<i64>,
) -> Option<&'static str> {
    if !can_ask_for_commitment(tier, warmth) {
        return Some("warmth");
    }
    if tier == CommitmentStatus::Dating && !has_kissed {
        return Some("kiss");
    }
    if matches!(
        tier,
        CommitmentStatus::LivingTogether | CommitmentStatus::Married
    ) && first_intimate_at.is_none()
    {
        return Some("first_time");
    }
    None
}

// ---------- 分手警告/留疤 ----------

pub const RISK_TENSION_THRESHOLD: f64 = 80.0;
pub const RISK_COMFORT_FLOOR: f64 = 15.0;
/// 3天宽限（真实毫秒）。
pub const BREAKUP_GRACE_MS: i64 = 3 * 24 * 60 * 60 * 1000;
/// 分手留疤：trust/comfort/chemistry各-15。
pub const BREAKUP_SCAR: f64 = 15.0;

/// 已承诺关系是否处于真实风险（未承诺=无状态可失去，直接false）。
pub fn relationship_at_risk(status: CommitmentStatus, s: &RelStats) -> bool {
    if status == CommitmentStatus::None {
        return false;
    }
    s.tension >= RISK_TENSION_THRESHOLD || s.comfort <= RISK_COMFORT_FLOOR
}

/// 分手留疤（一次性）：trust/comfort/chemistry各-15并夹紧0-100。
pub fn apply_breakup_scar(s: &RelStats) -> RelStats {
    RelStats {
        trust: clamp100(s.trust - BREAKUP_SCAR),
        comfort: clamp100(s.comfort - BREAKUP_SCAR),
        chemistry: clamp100(s.chemistry - BREAKUP_SCAR),
        ..*s
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RelWarning {
    pub started_at_ms: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RiskResult {
    /// None=无警告（没风险/已解除/刚分手）；Some=警告继续/新警告。
    pub warning: Option<RelWarning>,
    pub commitment: CommitmentStatus,
    pub breakup_count: u32,
    pub broke_up_just_now: bool,
    pub warned_just_now: bool,
    pub cleared_just_now: bool,
}

// ---------- momentum（吸收自rp dating/momentum.ts）：warmth的导数，不是水位 ----------

/// 衰减系数（抄rp MOMENTUM_DECAY=0.65）；钳制±8。
pub const MOMENTUM_DECAY: f64 = 0.65;
pub const MOMENTUM_CLAMP: f64 = 8.0;
pub const INITIATIVE_CLAMP: f64 = 6.0;

/// 本回合warmth移动量：warmth五键均值（tension/curiosity不计）。
pub fn warmth_delta_of(affection: f64, trust: f64, chemistry: f64, comfort: f64, respect: f64) -> f64 {
    (affection + trust + chemistry + comfort + respect) / 5.0
}

/// 下一momentum：prev*0.65+delta，钳制±8，留2位小数。
pub fn next_momentum(prev: Option<f64>, warmth_delta: f64) -> f64 {
    let raw = prev.unwrap_or(0.0) * MOMENTUM_DECAY + warmth_delta;
    (raw.clamp(-MOMENTUM_CLAMP, MOMENTUM_CLAMP) * 100.0).round() / 100.0
}

pub fn describe_momentum(momentum: f64) -> Option<&'static str> {
    if momentum >= 2.0 {
        Some("deepening fast")
    } else if momentum >= 0.8 {
        Some("warming")
    } else if momentum <= -1.5 {
        Some("cooling off")
    } else if momentum <= -0.5 {
        Some("cooled a little")
    } else {
        None
    }
}

/// 节奏提示（给prompt用，真名无宏）：摩擦并存/冲太快/冷却/回暖/平稳。
pub fn relationship_pacing_note(char_name: &str, warmth: f64, momentum: f64, tension: f64) -> Option<String> {
    if tension >= 55.0 && warmth >= 45.0 {
        return Some(format!("There\u{2019}s been real friction lately, running right alongside the closeness — both are true at once. Don\u{2019}t smooth it over, and don\u{2019}t let accumulated warmth make {char_name} more romantically receptive than the current strain would allow."));
    }
    if momentum >= 2.0 {
        return Some(format!("The relationship has moved fast these last few exchanges. {char_name} is more likely to want to slow down and let it settle than to keep accelerating."));
    }
    if momentum <= -1.5 {
        return Some(format!("The last few exchanges have cooled things off. Right now {char_name} is more guarded and less open than the overall closeness would suggest."));
    }
    if momentum >= 0.8 {
        return Some(format!("Things have been genuinely warming lately — {char_name} can let that show."));
    }
    if momentum <= -0.5 {
        return Some(format!("Things have drifted a little flat in the last few exchanges — not a crisis, just a cooler stretch."));
    }
    if warmth >= 50.0 {
        return Some(format!("Things have been steady and comfortable lately — {char_name} doesn\u{2019}t need to manufacture a new development this turn."));
    }
    None
}

/// 主动平衡：正=玩家在扛（角色显保留）；负=角色在主动。
pub fn next_initiative_balance(prev: Option<f64>, had_player_intent: bool, warmth_delta: f64) -> f64 {
    let contrib = if had_player_intent {
        if warmth_delta <= 0.0 { 1.0 } else { 0.0 }
    } else if warmth_delta > 0.0 {
        -1.0
    } else {
        0.0
    };
    let raw = prev.unwrap_or(0.0) * MOMENTUM_DECAY + contrib;
    (raw.clamp(-INITIATIVE_CLAMP, INITIATIVE_CLAMP) * 100.0).round() / 100.0
}

pub fn describe_initiative_balance(balance: f64) -> Option<&'static str> {
    if balance >= 2.0 {
        Some("you\u{2019}ve been carrying it lately")
    } else if balance <= -2.0 {
        Some("they\u{2019}ve been the one reaching lately")
    } else {
        None
    }
}

/// slow-burn 节奏（抄rp slowBurnPacingNote：mood/plan双轴，强抵抗才硬拦）。
pub fn slow_burn_pacing_note(char_name: &str, mood: Option<&str>, is_holding_back_by_plan: bool) -> String {
    const HIGH_RESISTANCE: [&str; 5] = ["guarded", "anxious", "hurt", "embarrassed", "tense"];
    let strong = is_holding_back_by_plan
        || mood.map(|m| HIGH_RESISTANCE.contains(&m)).unwrap_or(false);
    if strong {
        let because = if is_holding_back_by_plan {
            "is actively holding back right now, on their own terms".to_string()
        } else {
            format!("is currently {}", mood.unwrap_or(""))
        };
        format!("Slow burn: {char_name} {because}. Escalation is especially unearned now; let them refuse, deflect, or pull back rather than yielding to momentum or warmth.")
    } else {
        format!("Slow burn: earn intimacy through small moments, never because it was requested. If pushed early, hesitate, deflect, or say no rather than agreeing to please; once it is earned, let {char_name} initiate sometimes too.")
    }
}

/// outreach 节奏常量（抄rp outreach.ts；纯数值，判定由调用方接）。
pub const OUTREACH_CHECK_COOLDOWN_MS: i64 = 30 * 60 * 1000;
pub const OUTREACH_SILENCE_HOURS_RARE: i64 = 48;
pub const OUTREACH_SILENCE_HOURS_NORMAL: i64 = 20;
pub const OUTREACH_SILENCE_HOURS_EAGER: i64 = 8;
pub const OUTREACH_BASE_RARE: f64 = 0.12;
pub const OUTREACH_BASE_NORMAL: f64 = 0.25;
pub const OUTREACH_BASE_EAGER: f64 = 0.4;
pub const OUTREACH_MAX_WARMTH_BONUS: f64 = 0.2;

// ---------- 关系描述块（吸收自rp relationshipDescription.ts） ----------

/// 阶段英文名（与rp formatRelationshipStage对齐）。
pub fn format_stage(stage: RelationshipStage) -> &'static str {
    match stage {
        RelationshipStage::NearStrangers => "near strangers",
        RelationshipStage::Acquaintances => "acquaintances",
        RelationshipStage::WarmingUp => "warming up",
        RelationshipStage::GettingClose => "getting close",
        RelationshipStage::Close => "close",
        RelationshipStage::Sweethearts => "sweethearts",
    }
}

pub fn format_commitment(c: CommitmentStatus) -> &'static str {
    match c {
        CommitmentStatus::None => "none",
        CommitmentStatus::Dating => "dating",
        CommitmentStatus::Exclusive => "exclusive",
        CommitmentStatus::LivingTogether => "living together",
        CommitmentStatus::Married => "married",
    }
}

/// prompt用关系描述（永不报数字；warning/breakup/pacing/commitment全进）。
#[allow(clippy::too_many_arguments)]
pub fn build_relationship_description(
    char_name: &str,
    affection: f64,
    s: &RelStats,
    commitment: CommitmentStatus,
    warning_reason: Option<&str>,
    breakup_count: u32,
    momentum: f64,
    initiative_balance: f64,
) -> String {
    let warmth = compute_warmth(affection, s);
    let stage = stage_for_warmth(warmth);
    let mut notes: Vec<String> = vec![];
    if s.trust >= 70.0 {
        notes.push("a deep mutual trust has built up".into());
    }
    if s.chemistry >= 70.0 {
        notes.push("there is a strong romantic spark".into());
    }
    if s.tension >= 60.0 {
        notes.push("real unresolved tension between them".into());
    }
    if s.comfort <= 20.0 && stage != RelationshipStage::NearStrangers {
        notes.push("things still feel a little unsettled between them".into());
    }
    let mut lines = vec![format!(
        "Relationship: {{{{user}}}} and {char_name} are at the \"{}\" stage{}.",
        format_stage(stage),
        if notes.is_empty() { String::new() } else { format!(": {}", notes.join("; ")) }
    )];
    lines.push("(Let this colour tone, warmth, and what feels earned right now. Never state a number, \"affection\", or \"stage\" out loud.)".into());
    if let Some(p) = relationship_pacing_note(char_name, warmth, momentum, s.tension) {
        lines.push(p);
    }
    if initiative_balance >= 2.0 {
        lines.push(format!("{char_name} has been more reserved than {{{{user}}}} has lately — {{{{user}}}}\u{2019}s the one who keeps reaching first. That can read as real hesitation worth letting show."));
    } else if initiative_balance <= -2.0 {
        lines.push(format!("{char_name} has actually been the one initiating more than {{{{user}}}} lately — worth reflecting that closeness."));
    }
    if commitment != CommitmentStatus::None {
        lines.push(format!("{{{{user}}}} and {char_name} are officially {}.", format_commitment(commitment)));
    }
    if let Some(r) = warning_reason {
        lines.push(format!("The relationship is genuinely on the rocks right now ({r}). Let that show; don\u{2019}t just narrate past it."));
    }
    if breakup_count > 0 {
        lines.push(format!("{char_name} and {{{{user}}}} have broken up before. Some caution or guardedness is earned here."));
    }
    lines.join("\n")
}

/// 纯决策步：每次数值更新后调一次（非定时器）。留疤落盘是调用方的事。
pub fn evaluate_relationship_risk(
    status: CommitmentStatus,
    s: &RelStats,
    existing: Option<&RelWarning>,
    breakup_count: u32,
    now_ms: i64,
) -> RiskResult {
    if !relationship_at_risk(status, s) {
        return RiskResult {
            warning: None,
            commitment: status,
            breakup_count,
            broke_up_just_now: false,
            warned_just_now: false,
            cleared_just_now: existing.is_some(),
        };
    }
    let Some(w) = existing else {
        let reason = if s.tension >= RISK_TENSION_THRESHOLD {
            "tension has been boiling over"
        } else {
            "things have felt distant and neglected"
        };
        return RiskResult {
            warning: Some(RelWarning {
                started_at_ms: now_ms,
                reason: reason.into(),
            }),
            commitment: status,
            breakup_count,
            broke_up_just_now: false,
            warned_just_now: true,
            cleared_just_now: false,
        };
    };
    if now_ms - w.started_at_ms >= BREAKUP_GRACE_MS {
        return RiskResult {
            warning: None,
            commitment: CommitmentStatus::None,
            breakup_count: breakup_count + 1,
            broke_up_just_now: true,
            warned_just_now: false,
            cleared_just_now: false,
        };
    }
    RiskResult {
        warning: Some(w.clone()),
        commitment: status,
        breakup_count,
        broke_up_just_now: false,
        warned_just_now: false,
        cleared_just_now: false,
    }
}

// ---------- 判定 prompt 小件（吸收自 pnotisdev/rp dating/relationshipAssist.ts，MIT） ----------
//
// 只收“调模型”之外的确定性部分：维度术语、flag 术语/白名单、
// 判定调用参数、超时。真正的 prompt 组装与 JSON 校验由网关调用方接。

/// 七维含义（抄rp DIMENSION_GLOSSARY；tension 正 delta=摩擦变大，是戏剧不是坏事）。
pub fn dimension_glossary(key: &str) -> Option<&'static str> {
    match key {
        "affection" => Some("overall fondness"),
        "trust" => Some("reliability and emotional safety"),
        "chemistry" => Some("romantic/physical spark"),
        "comfort" => Some("ease being around each other"),
        "respect" => Some("how much they respect the other person"),
        "curiosity" => Some("interest in learning more about them"),
        "tension" => Some("friction or unresolved conflict. A positive delta here means MORE tension, which is not automatically a bad thing dramatically"),
        _ => None,
    }
}

/// 内置路线 flag（抄rp SCENE_FLAGS 口径）。
pub const SCENE_FLAGS: [&str; 5] = ["first_date", "confession", "jealousy", "promise", "first_kiss"];

/// 实况场景判定（抄rp isLiveScene：startedAt 非空 + kind=date/hangout）。
pub fn is_live_scene(kind: Option<&str>, started_at: Option<i64>) -> bool {
    started_at.is_some() && matches!(kind, Some("date") | Some("hangout"))
}

/// 首吻 flag（抄rp FIRST_KISS_FLAG）。
pub const FIRST_KISS_FLAG: &str = "first_kiss";

/// item 目录（抄rp getItemCatalog/itemById：世界自带，无内置默认）。
pub fn get_item_ids(world_items: &[String]) -> Vec<String> {
    world_items.to_vec()
}
pub fn item_by_id<'a>(world_items: &'a [String], id: &str) -> Option<&'a String> {
    world_items.iter().find(|i| *i == id)
}

/// flag 一句话门槛（抄rp FLAG_GLOSSARY；分类器按此判，不按名猜）。
pub fn flag_glossary(flag: &str) -> Option<&'static str> {
    match flag {
        "first_date" => Some("an explicit, mutually understood date has now happened, not just a friendly hangout, chance encounter, or a gift given in passing"),
        "confession" => Some("one of them stated real romantic feelings out loud, not just flirted or hinted"),
        "jealousy" => Some("clear jealousy or possessiveness was shown over a rival or another relationship"),
        "promise" => Some("a specific, meaningful promise was made that the story should remember later"),
        "first_kiss" => Some("they actually kissed — lips meeting mouth, forehead, cheek, hand, or anywhere else — not just closeness, a lingering look, or an almost-kiss that did not quite happen"),
        _ => None,
    }
}

/// hangout 永不立的内置 flag（抄rp DATE_ONLY_FLAGS）。
pub fn is_date_only_flag(flag: &str) -> bool {
    flag == "first_date"
}

/// 本场允许的 flag（hangout 摘掉 date-only；抄rp excludedFlagsFor/allowedFlagIds）。
/// 输入 (全部候选含世界自创, scene_kind)；返回允许集。
pub fn allowed_flag_ids(all_ids: &[String], scene_kind: &str) -> Vec<String> {
    all_ids
        .iter()
        .filter(|id| !(scene_kind == "hangout" && is_date_only_flag(id)))
        .cloned()
        .collect()
}

/// flag 展示行：内置拼术语，自创拼 description（抄rp describeFlags）。
pub fn describe_flags(built_in: &[String], custom: &[(String, String)], scene_kind: &str) -> String {
    let allowed = allowed_flag_ids(
        &built_in.iter().cloned().chain(custom.iter().map(|(id, _)| id.clone())).collect::<Vec<_>>(),
        scene_kind,
    );
    let mut parts: Vec<String> = Vec::new();
    for id in &allowed {
        if let Some(g) = flag_glossary(id) {
            parts.push(format!("{id} ({g})"));
        } else if let Some((_, d)) = custom.iter().find(|(cid, _)| cid == id) {
            parts.push(format!("{id} ({d})"));
        }
    }
    parts.join("; ")
}

/// 判定调用参数（抄rp REL_PARAMS/EVENT_PARAMS/GENERATE_PARAMS/ASSIST_TIMEOUT_MS）。
pub const ASSIST_TIMEOUT_MS: u64 = 45_000;
pub const REL_MAX_LENGTH: usize = 220;
pub const EVENT_MAX_LENGTH: usize = 360;
pub const OBJECTIVE_MAX_LENGTH: usize = 400;
pub const REL_TEMPERATURE: f64 = 0.35;
pub const EVENT_TEMPERATURE: f64 = 0.7;
pub const OBJECTIVE_TEMPERATURE: f64 = 0.7;

/// 最拖后腿的warmth维（抄rp lowestWarmthDimension；trust/chemistry/comfort/respect四取低）。
pub fn lowest_warmth_dimension(trust: f64, chemistry: f64, comfort: f64, respect: f64) -> &'static str {
    let mut low = "trust"; let mut v = trust;
    if chemistry < v { v = chemistry; low = "chemistry"; }
    if comfort < v { v = comfort; low = "comfort"; }
    if respect < v { let _ = v; low = "respect"; }
    low
}
/// 跨阶段即解锁终幕CG资格（抄rp crossedMilestone；sweethearts到即开）。
pub fn crossed_milestone(prev: u8, next: u8) -> bool { next > prev }
/// 首次亲密发起门槛（抄rp canInitiateFirstTime；warmth≥75+已有承诺）。
pub fn can_initiate_first_time(warmth: f64, committed: bool) -> bool { warmth >= 75.0 && committed }
/// 世界自定义里程碑覆盖（抄rp relationshipMilestonesFor；near_strangers不动）。
pub fn milestone_for(stage: &str, overrides: &[(&str, f64)], default_at: f64) -> f64 {
    if stage == "near_strangers" { return 0.0; }
    overrides.iter().find(|(s,_)| *s==stage).map(|(_,v)| *v).unwrap_or(default_at)
}

/// 真可问（抄rp canActuallyAskForCommitment；warmth+物理双门都过）。
pub fn can_actually_ask_for_commitment(tier: CommitmentStatus, warmth: f64, has_kissed: bool, first_intimate_at: Option<i64>) -> bool {
    commitment_lock_reason(tier, warmth, has_kissed, first_intimate_at).is_none()
}
/// 最低warmth维短板（抄rp lowestWarmthDimension；trust/chemistry/comfort/respect取最低，平局按序）。
pub fn lowest_warmth_dim(s: &RelStats) -> RelDim {
    let dims = [(RelDim::Trust, s.trust), (RelDim::Chemistry, s.chemistry), (RelDim::Comfort, s.comfort), (RelDim::Respect, s.respect)];
    dims.iter().fold(dims[0], |a, b| if b.1 < a.1 { *b } else { a }).0
}
/// 警告过期（抄rp warningExpired；真实毫秒超3天）。
pub fn warning_expired(started_at_ms: i64, now_ms: i64) -> bool { now_ms - started_at_ms >= BREAKUP_GRACE_MS }
/// 满级CG解锁（抄rp unlockedEndingIds；仅sweethearts开isEnding）。
pub fn unlocked_ending_ids(stage: RelationshipStage, is_ending: &[bool], already: &[bool]) -> Vec<usize> {
    if stage != RelationshipStage::Sweethearts { return Vec::new(); }
    is_ending.iter().zip(already.iter()).enumerate().filter(|(_, (e, a))| **e && !**a).map(|(i, _)| i).collect()
}

/// 主动失衡注记（抄rp asymmetricPacingNote；balance正负判谁在推）。
pub fn asymmetric_pacing_note(char_name: &str, balance: f64) -> Option<String> {
    if balance >= 3.0 { Some(format!("{char_name} has been carrying the conversation lately — initiating more, asking more. It would feel natural for them to ease back and let the player come to them for a bit.")) }
    else if balance <= -3.0 { Some(format!("The player has been doing most of the reaching out lately. {char_name} noticing that and stepping forward first would feel earned.")) }
    else { None }
}
/// initiative单步贡献（抄rp initiativeContribution；有意图+warmthDelta）。
pub fn initiative_contribution(had_player_intent: bool, warmth_delta: f64) -> f64 {
    (if had_player_intent { 1.0 } else { 0.0 } + warmth_delta).clamp(-INITIATIVE_CLAMP, INITIATIVE_CLAMP)
}

/// 未满足需求三字段（抄rp currentNeed/currentDesire/currentFear；独立字段，不混mood）。
pub fn unmet_need_fields() -> [&'static str; 3] { ["current_need", "current_desire", "current_fear"] }

/// 隐藏议程行文（抄rp hiddenAgenda分支；date才有，hangout无；命中/无视/反着三分）。
pub fn hidden_agenda_line(char_name: &str, agenda: Option<&str>) -> Option<String> {
    agenda.filter(|a| !a.trim().is_empty()).map(|a| format!("{char_name} went into this secretly wanting: {a} (never told to the other person). Weigh whether the date actually met that, ignored it, or worked against it."))
}
/// 离席判分行文（抄rp walkedOut分支；中离=真坏结局，负分不敷衍）。
pub fn walked_out_line(char_name: &str, walked_out: bool, is_hangout: bool) -> Option<String> {
    if !walked_out { return None; }
    if is_hangout { return Some(format!("{char_name} left early. Judge it honestly but gently.")); }
    Some(format!("{char_name} walked out and ended this early — this is NOT a normal ending. Judge it as a genuinely bad outcome: deltas should be negative on the dimensions this actually hurt."))
}
/// recap上限400字（抄rp recap.slice(0,400)；兜底'The date came to an end.'）。
pub fn date_recap_clip(recap: &str) -> String {
    let t = recap.trim();
    if t.is_empty() { return "The date came to an end.".to_string(); }
    t.chars().take(400).collect()
}
/// 求承诺三判（抄rp assessCommitmentAsk决策形；accept/deflect/backfire，deltas钳-3..3）。
pub fn commitment_ask_decision(raw: &str) -> &'static str {
    match raw.trim() { "accept" => "accept", "backfire" => "backfire", _ => "deflect" }
}
pub fn clamp_commitment_delta(v: i64) -> i64 { v.clamp(-3, 3) }

/// 礼物口味注记（抄rp buildGiftTasteNote；loveLanguage+likes+dislikes三件拼）。
pub fn gift_taste_note_full(char_name: &str, love_language: Option<&str>, likes: &[String], dislikes: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(l) = love_language.filter(|s| !s.trim().is_empty()) { parts.push(format!("feels most loved through {}", l.trim())); }
    if !likes.is_empty() { parts.push(format!("tends to genuinely love gifts like {}", likes.join(", "))); }
    if !dislikes.is_empty() { parts.push(format!("isn't really moved by gifts like {}", dislikes.join(", "))); }
    if parts.is_empty() { None } else { Some(format!("{char_name} {}. React to any gift given accordingly, in character.", parts.join("; "))) }
}
/// 关系氛围四注记（抄rp buildRelationshipDescription；trust70/chemistry70/tension60/comfort20四门）。
pub fn relationship_notes(trust: f64, chemistry: f64, tension: f64, comfort: f64, is_stranger: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if trust >= 70.0 { out.push("a deep mutual trust has built up"); }
    if chemistry >= 70.0 { out.push("there is a strong romantic spark"); }
    if tension >= 60.0 { out.push("real unresolved tension between them"); }
    if comfort <= 20.0 && !is_stranger { out.push("things still feel a little unsettled between them"); }
    out
}

/// 场景旗全集（抄rp combinedSceneFlags；内置5旗+world自带，label下划线转空格）。
pub fn combined_scene_flags(builtin: &[String], custom: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = builtin.iter().map(|f| (f.clone(), f.replace('_', " "))).collect();
    for (id, label) in custom { out.push((id.clone(), label.clone())); }
    out
}
/// 承诺白话（抄rp formatCommitmentStatus；none=not official其余直译）。
pub fn format_commitment_status(c: CommitmentStatus) -> &'static str { format_commitment(c) }
/// 六维全集（抄rp RELATIONSHIP_DIMENSIONS；trust/chemistry/comfort/respect/curiosity/tension）。
pub const RELATIONSHIP_DIMENSIONS: [&str; 6] = ["trust","chemistry","comfort","respect","curiosity","tension"];
/// warmth四维（抄rp WARMTH_DIMENSIONS；trust/chemistry/comfort/respect，curiosity/tension不计分）。
pub const WARMTH_DIMENSIONS: [&str; 4] = ["trust","chemistry","comfort","respect"];
/// 里程碑表（抄rp relationshipMilestonesFor；默认MILESTONES，world可覆at）。
pub fn relationship_milestones_for(overrides: &[(&str, f64)]) -> Vec<(String, f64)> {
    let base = [("near_strangers",0.0),("acquaintances",15.0),("warming_up",35.0),("getting_close",55.0),("close",75.0),("sweethearts",90.0)];
    base.iter().map(|(s, d)| (s.to_string(), milestone_for(s, overrides, *d))).collect()
}
/// 阶段白话（抄rp formatRelationshipStage；下划线转空格）。
pub fn format_relationship_stage(stage: RelationshipStage) -> &'static str { format_stage(stage) }
/// warmth→阶段（抄rp relationshipStageForWarmth；里程碑倒查）。
pub fn relationship_stage_for_warmth(warmth: f64) -> RelationshipStage { stage_for_warmth(warmth) }
/// 六维读数（抄rp getRelationshipStats；缺维按0兜底）。
pub fn get_relationship_stats(get: &dyn Fn(&str) -> f64) -> RelStats {
    RelStats { trust: get("trust"), chemistry: get("chemistry"), comfort: get("comfort"), respect: get("respect"), curiosity: get("curiosity"), tension: get("tension") }
}
/// 单角色track存在性（抄rp getRelationshipTrack；主轨全字段，副轨有啥算啥，调用方按Option读）。
pub fn has_relationship_track(is_primary: bool, has_entry: bool) -> bool { is_primary || has_entry }
/// track补丁合并（抄rp patchRelationshipTrack；主轨直写，副轨整map重写防浅合并丢人）。
pub fn patch_relationship_track_is_primary(character_id: &str, primary_id: &str) -> bool { character_id == primary_id }
/// 存活场景归属（抄rp findActiveIntimacyScene；先主轨后副轨，防双状态机打架）。
pub fn find_active_intimacy_scene<'a>(primary_active: Option<&'a String>, others: &'a [(String, String)]) -> Option<(String, &'a String)> {
    if let Some(s) = primary_active { return Some(("primary".to_string(), s)); }
    others.iter().find(|(_, s)| !s.is_empty()).map(|(o, s)| (o.clone(), s))
}
/// 0-100钳位（抄rp clampAffection/clampStat；四舍五入后钳）。
pub fn clamp_affection(n: f64) -> f64 { clamp100(n) }
pub fn clamp_stat(n: f64) -> f64 { clamp100(n) }
/// 纠偏指令（抄rp hardFailCorrectionDirective；三轨全无返回None）。
pub fn hard_fail_correction_directive(char_name: &str, crossed: Option<&str>, agency_violation: Option<&str>, continuity_break: Option<&str>) -> Option<String> {
    crate::social2::hard_fail_directive(char_name, "", crossed, agency_violation, continuity_break)
}

/// 角色track归属（抄rp getRelationshipTrack；主轨读Chat顶层，副轨读participantRelationships条目）。
pub fn get_relationship_track(character_id: &str, primary_id: &str) -> &'static str {
    if character_id == primary_id { "primary" } else { "participant" }
}

// ---------------------------------------------------------------------------
// 关系强度 ±100 账本（吸收自 dramatica-flow RelationshipRecord/RelationshipDelta，MIT）。
// 纪律（抄源）：strength -100（死敌）..+100（生死同盟），每次事件 append
// (chapter, delta, reason) 历史；±50 自动翻 ally/enemy；语义型（family/mentor/rival/romantic）
// 只动强度不自动覆盖类型。全纯函数结构，无 IO。
// ---------------------------------------------------------------------------

/// 关系强度自动翻转阈值：±50 判 ally/enemy（dramatica 吸收）。
pub const REL_STRENGTH_ALLY_AT: i64 = 50;
pub const REL_STRENGTH_ENEMY_AT: i64 = -50;

/// 关系强度增量记录：第几章 / 变了多少 / 为什么。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RelStrengthDelta {
    /// 章节号（叙事章节）。
    pub chapter: i64,
    /// 强度变化量（最终强度钳在 -100..100）。
    pub delta: i64,
    /// 变化原因（一句话）。
    pub reason: String,
}

/// 单对关系的强度账本（-100 死敌 ↔ +100 生死同盟；0 中立）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RelStrengthRecord {
    /// 无序对唯一键（a 与 b 排序后拼接，`a:b`）。
    pub key: String,
    pub character_a: String,
    pub character_b: String,
    /// 当前强度 -100..100。
    pub strength: i64,
    /// 语义类型（ally/enemy/neutral/family/mentor/rival/romantic）。
    pub rel_type: String,
    pub history: Vec<RelStrengthDelta>,
}

impl RelStrengthRecord {
    pub fn new(a: &str, b: &str) -> Self {
        let mut pair = [a.trim().to_string(), b.trim().to_string()];
        pair.sort();
        Self {
            key: format!("{}:{}", pair[0], pair[1]),
            character_a: pair[0].clone(),
            character_b: pair[1].clone(),
            strength: 0,
            rel_type: "neutral".to_string(),
            history: Vec::new(),
        }
    }

    /// 应用一次增量：钳到 -100..100，append 历史，按阈值自动翻 ally/enemy。
    /// 语义型（family/mentor/rival/romantic）不自动覆盖，只动强度。
    pub fn apply_delta(&mut self, chapter: i64, delta: i64, reason: &str) {
        self.strength = (self.strength + delta).clamp(-100, 100);
        self.history.push(RelStrengthDelta {
            chapter,
            delta,
            reason: reason.trim().to_string(),
        });
        match self.rel_type.as_str() {
            "family" | "mentor" | "rival" | "romantic" => {}
            _ => {
                if self.strength >= REL_STRENGTH_ALLY_AT {
                    self.rel_type = "ally".to_string();
                } else if self.strength <= REL_STRENGTH_ENEMY_AT {
                    self.rel_type = "enemy".to_string();
                } else {
                    self.rel_type = "neutral".to_string();
                }
            }
        }
    }
}

/// 关系网络账本：多对关系按 key 索引（dramatica 吸收）。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RelStrengthLedger {
    pub records: Vec<RelStrengthRecord>,
}

impl RelStrengthLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取或建一对关系（有序无关）。
    pub fn record_mut(&mut self, a: &str, b: &str) -> &mut RelStrengthRecord {
        let rec = RelStrengthRecord::new(a, b);
        let key = rec.key.clone();
        if let Some(idx) = self.records.iter().position(|r| r.key == key) {
            &mut self.records[idx]
        } else {
            self.records.push(rec);
            self.records.last_mut().expect("just pushed")
        }
    }

    /// 取当前强度（无记录返回 0）。
    pub fn strength(&self, a: &str, b: &str) -> i64 {
        let rec = RelStrengthRecord::new(a, b);
        self.records
            .iter()
            .find(|r| r.key == rec.key)
            .map(|r| r.strength)
            .unwrap_or(0)
    }

    /// 一次事件结算：给一对关系加增量 + 原因，返回更新后的记录。
    pub fn apply(&mut self, a: &str, b: &str, chapter: i64, delta: i64, reason: &str) -> &RelStrengthRecord {
        let key = RelStrengthRecord::new(a, b).key;
        self.record_mut(a, b).apply_delta(chapter, delta, reason);
        self.records.iter().find(|r| r.key == key).expect("just applied")
    }
}

/// 关系强度钳制（-100..100）。
pub fn clamp_rel_strength(n: i64) -> i64 {
    n.clamp(-100, 100)
}

/// 关系强度白话（dramatica 吸收展示层）：
/// ≥50 生死同盟 / ≤-50 死敌 / ≥20 友好 / ≤-20 敌对 / 其余中立。
pub fn describe_rel_strength(strength: i64) -> &'static str {
    if strength >= REL_STRENGTH_ALLY_AT {
        "生死同盟"
    } else if strength <= REL_STRENGTH_ENEMY_AT {
        "死敌"
    } else if strength >= 20 {
        "友好"
    } else if strength <= -20 {
        "敌对"
    } else {
        "中立"
    }
}

/// 关系强度历史 delta 是否满足翻转条件（供测试/判定用）。
pub fn would_flip_rel_type(rel_type: &str, strength_after: i64) -> Option<&'static str> {
    match rel_type {
        "family" | "mentor" | "rival" | "romantic" => None,
        _ => {
            if strength_after >= REL_STRENGTH_ALLY_AT {
                Some("ally")
            } else if strength_after <= REL_STRENGTH_ENEMY_AT {
                Some("enemy")
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(tension: f64, comfort: f64) -> RelStats {
        RelStats {
            tension,
            comfort,
            ..Default::default()
        }
    }

    #[test]
    fn warmth_excludes_curiosity_tension() {
        let s = RelStats {
            trust: 50.0,
            chemistry: 50.0,
            comfort: 50.0,
            respect: 50.0,
            curiosity: 100.0,
            tension: 100.0,
            ..Default::default()
        };
        assert_eq!(compute_warmth(50.0, &s), 50.0);
    }

    #[test]
    fn stage_thresholds() {
        assert_eq!(stage_for_warmth(0.0), RelationshipStage::NearStrangers);
        assert_eq!(stage_for_warmth(55.0), RelationshipStage::GettingClose);
        assert_eq!(stage_for_warmth(90.0), RelationshipStage::Sweethearts);
    }

    #[test]
    fn commitment_warmth_gates_ask_not_grant() {
        assert!(can_ask_for_commitment(CommitmentStatus::Dating, 55.0));
        assert!(!can_ask_for_commitment(CommitmentStatus::Exclusive, 60.0));
        assert_eq!(
            commitment_lock_reason(CommitmentStatus::Dating, 60.0, false, None),
            Some("kiss")
        );
        assert_eq!(
            commitment_lock_reason(CommitmentStatus::Dating, 60.0, true, None),
            None
        );
    }

    #[test]
    fn risk_only_for_committed() {
        let s = stats(90.0, 50.0);
        assert!(!relationship_at_risk(CommitmentStatus::None, &s));
        assert!(relationship_at_risk(CommitmentStatus::Dating, &s));
    }

    #[test]
    fn warn_then_breakup_then_scar() {
        let s = stats(90.0, 50.0);
        let r1 = evaluate_relationship_risk(CommitmentStatus::Dating, &s, None, 0, 1000);
        assert!(r1.warned_just_now);
        let w = r1.warning.clone().unwrap();
        let r2 = evaluate_relationship_risk(
            CommitmentStatus::Dating,
            &s,
            Some(&w),
            0,
            1000 + BREAKUP_GRACE_MS,
        );
        assert!(r2.broke_up_just_now);
        assert_eq!(r2.commitment, CommitmentStatus::None);
        let scarred = apply_breakup_scar(&RelStats {
            trust: 50.0,
            comfort: 50.0,
            chemistry: 50.0,
            ..Default::default()
        });
        assert_eq!((scarred.trust, scarred.comfort, scarred.chemistry), (35.0, 35.0, 35.0));
    }

    #[test]
    fn momentum_decay_and_labels() {
        assert_eq!(next_momentum(None, 2.0), 2.0);
        assert_eq!(next_momentum(Some(2.0), 0.0), 1.3);
        assert_eq!(describe_momentum(2.0), Some("deepening fast"));
        assert_eq!(describe_momentum(-1.5), Some("cooling off"));
        assert_eq!(describe_momentum(0.0), None);
        // 钳制
        assert_eq!(next_momentum(Some(100.0), 100.0), 8.0);
        assert_eq!(next_initiative_balance(None, true, -1.0), 1.0);
        assert_eq!(describe_initiative_balance(2.0), Some("you\u{2019}ve been carrying it lately"));
    }

    #[test]
    fn description_never_states_numbers() {
        let s = RelStats { trust: 80.0, chemistry: 80.0, comfort: 50.0, respect: 50.0, curiosity: 0.0, tension: 0.0, ..Default::default() };
        let d = build_relationship_description("她", 80.0, &s, CommitmentStatus::Dating, None, 1, 0.0, 0.0);
        assert!(!d.contains("80"));
        assert!(d.contains("sweethearts") || d.contains("close"));
        assert!(d.contains("dating"));
        assert!(d.contains("broken up before"));
    }

    // ── dramatica 吸收：关系强度 ±100 账本 ──────────────────────────────────

    #[test]
    fn rel_strength_clamps_and_records_history() {
        let mut r = RelStrengthRecord::new("林尘", "慕雪");
        r.apply_delta(1, 20, "慕雪看到林尘为救她不惜受伤");
        r.apply_delta(2, -30, "萧天暗中勾结魔族被揭穿");
        // 20 - 30 = -10
        assert_eq!(r.strength, -10);
        assert_eq!(r.history.len(), 2);
        assert_eq!(r.history[0].chapter, 1);
        assert_eq!(r.history[0].delta, 20);
        assert_eq!(r.history[1].delta, -30);
        // 钳制：+200 → 100
        let mut big = RelStrengthRecord::new("a", "b");
        big.apply_delta(1, 200, "救命之恩");
        assert_eq!(big.strength, 100);
    }

    #[test]
    fn rel_strength_flips_ally_enemy_at_50() {
        let mut r = RelStrengthRecord::new("林尘", "萧天");
        r.apply_delta(1, 60, "并肩作战");
        assert_eq!(r.strength, 60);
        assert_eq!(r.rel_type, "ally");
        r.apply_delta(2, -120, "反目成仇");
        assert_eq!(r.strength, -60, "60-120=-60 钳后");
        assert_eq!(r.rel_type, "enemy");
        // 回到中性：-60 + 20 = -40 > -50
        r.apply_delta(3, 20, "握手言和");
        assert_eq!(r.rel_type, "neutral");
    }

    #[test]
    fn semantic_rel_type_not_overridden() {
        let mut r = RelStrengthRecord::new("a", "b");
        r.rel_type = "romantic".to_string();
        r.apply_delta(1, 70, "表白");
        assert_eq!(r.strength, 70);
        assert_eq!(r.rel_type, "romantic", "语义型不被自动覆盖");
    }

    #[test]
    fn ledger_indexes_pairs_orderless() {
        let mut l = RelStrengthLedger::new();
        l.apply("林尘", "慕雪", 1, 20, "救美");
        // 反序也能取到同一条（key 按字符序排序：慕 < 林，故 character_a=慕雪）
        assert_eq!(l.strength("慕雪", "林尘"), 20);
        let rec = l.apply("慕雪", "林尘", 2, 10, "回礼");
        assert_eq!(rec.strength, 30, "同一条记录叠加");
        assert_eq!(rec.character_a, "慕雪");
        assert_eq!(rec.character_b, "林尘");
        assert_eq!(l.strength("林尘", "慕雪"), 30);
        // 无记录返回 0
        assert_eq!(l.strength("王五", "赵六"), 0);
        // 钳制
        l.apply("林尘", "慕雪", 3, 1000, "生死相许");
        assert_eq!(l.strength("林尘", "慕雪"), 100);
    }

    #[test]
    fn describe_and_flip_helpers() {
        assert_eq!(describe_rel_strength(60), "生死同盟");
        assert_eq!(describe_rel_strength(-60), "死敌");
        assert_eq!(describe_rel_strength(20), "友好");
        assert_eq!(describe_rel_strength(-20), "敌对");
        assert_eq!(describe_rel_strength(0), "中立");
        assert_eq!(would_flip_rel_type("neutral", 50), Some("ally"));
        assert_eq!(would_flip_rel_type("neutral", -50), Some("enemy"));
        assert_eq!(would_flip_rel_type("romantic", 60), None);
        assert_eq!(would_flip_rel_type("ally", 10), None);
        assert_eq!(clamp_rel_strength(150), 100);
        assert_eq!(clamp_rel_strength(-150), -100);
    }
}