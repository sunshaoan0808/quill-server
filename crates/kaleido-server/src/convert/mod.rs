//! 角色蒸馏模块（"小说→剧本包"转换流水线第一步）—— [R5 2026-10-06] 拆分自 convert.rs。
//!
//! 子模块：evidence（向量/证据检索）、characters（角色卡蒸馏）、
//! world（世界/事件/规则蒸馏）、chapters（章节节拍/关系/出口蒸馏）。
//! 对外 API 经本文件 glob 再导出，`use crate::convert::*` 调用方零改动。
#![allow(unused_imports)]

use crate::AppState;
use futures_util::future::join_all;
use kaleido_core::{
    ActorStatePackConfig, EventPackage, NodeExit, PackCharacterRef, RuleSystem, TellerEventCard,
};
use serde_json::{json, Value};

mod chapters;
#[allow(unused_imports)]
pub(crate) use chapters::*;
mod characters;
#[allow(unused_imports)]
pub(crate) use characters::*;
mod evidence;
#[allow(unused_imports)]
pub(crate) use evidence::*;
mod world;
#[allow(unused_imports)]
pub(crate) use world::*;
#[cfg(test)]
mod tests;

/// 角色谱 / 角色卡 LLM 超时（秒）。
pub(crate) const BLOCK_SIZE: usize = 800;
pub(crate) const BLOCK_OVERLAP: usize = 100;

/// 单个角色召回的证据片段数上限。
pub(crate) const LLM_TIMEOUT_SECS: u64 = 240;

pub(crate) const EVIDENCE_TOP_K: usize = 6;
/// 蒸馏角色数量上限（importance 高的优先）。
// [fix 2026-08-15] 8→12：角色谱 prompt 强化后 10-50 章书可抽出 ≥10 角色，
// 原 8 上限会截断配角蒸馏卡（红姐/山楂/冯婷等只有名字无卡）。

pub(crate) const MAX_DISTIL_CHARS: usize = 12;
/// 角色谱阶段每个章节取出摘要的最大字符数。
#[allow(dead_code)] // [P7] roster 分片阈值预留

pub(crate) const ROSTER_CHUNK_CHARS: usize = 1600;
/// 确定性伪向量的维度（纯函数自检用，不依赖 embed 模型）。
#[allow(dead_code)]

/// 世界树（Stage 3）：单次盘点后最多蒸馏的实体数（每个实体一次 LLM 调用）。
pub(crate) const SIG_DIM: usize = 64;

/// 世界树：单个实体召回的原文证据块数。
pub(crate) const MAX_WORLD_ENTITIES: usize = 12;

/// 节拍（Stage 3）：每章允许的硬节拍条数上限。
pub(crate) const WORLD_EVIDENCE_TOP_K: usize = 4;

/// 节拍（Stage 3）：单条硬节拍最大字数。
pub(crate) const MAX_BEATS: usize = 3;

/// 出口（Stage 3）：每个节点生成的候选出口数上限。
pub(crate) const BEAT_MAX_LEN: usize = 40;

/// 素材库蒸馏：事件包数量上限。
pub(crate) const MAX_EXITS: usize = 3;

/// 素材库蒸馏：单包事件卡数量上限。
pub(crate) const MAX_EVENT_PACKAGES: usize = 4;

/// 素材库蒸馏：规则检定条数上限。
pub(crate) const MAX_EVENT_CARDS: usize = 5;

/// 素材库蒸馏：规则检定条数下限（低于此值视为不合格，但解析仍返回已收集部分）。
pub(crate) const MAX_RULE_CHECKS: usize = 8;

/// 手写余弦相似度（两组等长标量向量）。
pub(crate) const MIN_RULE_CHECKS: usize = 4;

pub(crate) fn jstr(v: &Value, snake: &str, camel: &str) -> String {
    v.get(snake)
        .or_else(|| v.get(camel))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 证据召回空时的 pure-text 词频兜底：按角色名/别名在章节正文中定位，
/// 截取含名字的上下文片段拼成证据。用于 embed 向量检索召回为空的情形
/// （如全本高频角色但向量 query 偏移），确保主角不因召回空被跳过。
pub(crate) fn jstr_arr(v: &Value, snake: &str, camel: &str) -> Vec<String> {
    v.get(snake)
        .or_else(|| v.get(camel))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// 判定 LLM 产出的角色卡是否为"空壳"——JSON 结构合法但核心内容字段全空。
/// 上游模型在通道抖动时会返回 `{}` 或全空字段的"空壳 JSON"——解析能成功、
/// 不触发 fffd/空响应重试，却会在 card_to_ref 后产出空壳卡(只有默认 role 与
/// importance)。空壳卡一旦落盘会污染 pack，且其 importance=high 会堵塞
/// chars_done 门，导致角色再也无法被自动重蒸。因此解析通过后必须再验核心字段。
///
/// 注意：这里用"任一核心字段非空即非空壳"的宽松判定，刻意不要求
/// personality/speech_style/motivation 三字段齐全——B5 降级保底的"最小卡"
/// 可能只有 identity+personality（如 `{"name":"主角","personality":"勇敢坚毅"}`），
/// 那是配额耗尽时的保底产物，不应被此处重试逻辑吞掉。薄卡(缺核心字段但非全空)
/// 由 crawler.rs 合并守卫在落 pack 前过滤，与重试逻辑分层。
pub(crate) fn json_has_fffd(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => s.contains('\u{FFFD}'),
        serde_json::Value::Array(items) => items.iter().any(json_has_fffd),
        serde_json::Value::Object(map) => map.values().any(json_has_fffd),
        _ => false,
    }
}

/// [fix 2026-09-28 审计·转换导出与爬虫-6] DIAG 落盘统一出口。
///
/// 审计问题：多处调试原始 LLM 输出（fffd-raw-dump.json / roster-parse-fail.json /
/// char-diag.json）直接写进程 CWD——CWD 是全局单目录，跨书共享且随启动目录漂移，
/// 同时污染服务进程的工作目录。现在：
/// - 路径：有 pack 目录则写 pack 目录（与 pack 产物同处隔离）；无 pack 上下文则
///   退到 `$KALEIDO_DATA/diag/`（未配置 KALEIDO_DATA 时为 `./data/diag/`，与
///   [`crate::config::ServerConfig::data_root`] 的既定回退一致），一律不再写 CWD；
/// - 开关：默认开启（保持既有排障行为）；设 `KALEIDO_DIAG=0|false|off|no`
///   （大小写不敏感，走 env_alias 兼容旧 MUSEAI_DIAG 别名）可整体关闭。
///
/// 注意：功能性产物 `roster-diag.json` 不走本函数（不受开关约束）——它被
/// crawler.rs 的续跑门（`roster_high_covered`）按 pack 目录读取做 high 覆盖判定。
pub(crate) fn json_is_empty_shell(v: &serde_json::Value) -> bool {
    let core = [
        "identity",
        "personality",
        "speech_style",
        "motivation",
        "opening_lines",
    ];
    let any_content = core.iter().any(|k| {
        v.get(*k)
            .and_then(|x| x.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    });
    !any_content
}

/// DIAG 落盘开关：默认开启；`KALEIDO_DIAG`（或旧别名 `MUSEAI_DIAG`）设为
/// 0/false/off/no 时关闭。
pub(crate) fn diag_dump(pack_dir: Option<&std::path::Path>, file: &str, payload: &serde_json::Value) {
    if !diag_enabled() {
        return;
    }
    let dir = pack_dir
        .map(|d| d.to_path_buf())
        .unwrap_or_else(diag_fallback_dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(dir = %dir.display(), "diag 目录创建失败，跳过落盘: {e}");
        return;
    }
    let path = dir.join(file);
    if let Err(e) =
        std::fs::write(&path, serde_json::to_string_pretty(payload).unwrap_or_default())
    {
        tracing::warn!(path = %path.display(), "diag 落盘失败: {e}");
    }
}

/// 无 pack 上下文时的 DIAG 目录：`$KALEIDO_DATA/diag/`（未配置则 `./data/diag/`）。
pub(crate) fn diag_enabled() -> bool {
    match crate::config::ServerConfig::env_alias("KALEIDO_DIAG") {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        None => true,
    }
}

/// 蒸馏单个角色认知卡，对 LLM 空响应 / JSON 解析失败 / 含损坏字(U+FFFD)做最多 3 次重试（间隔 500ms）。
///
/// `attempt` 触发一次 LLM 调用（返回携带原文的 `Result<String, String>`）。
/// 返回 `Some(JSON)` 表示取得合法认知卡；重试 3 次仍无合法 JSON 时返回 `None`，
/// 由调用方跳过该角色，不让整批蒸馏因单个角色塌陷。
///
/// 重试上限按角色重要性分级：主角/重要配角(high/medium)给更多轮次，
/// 避免上游 deepseek-flash 的 U+FFFD 坏字把核心角色整张卡吞掉导致主角丢失；
/// 边缘角色(low)保持少轮，节省成本。
pub(crate) fn diag_fallback_dir() -> std::path::PathBuf {
    crate::config::ServerConfig::data_root().join("diag")
}

/// 把任意 JSON 值视为 JSON 数组（对象/字符串也包一层，返回 Vec）。
pub(crate) async fn llm_json(
    state: &AppState,
    system: &str,
    user: &str,
    stage: &str,
) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(LLM_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("{stage}: 构建 LLM client 失败: {e}"))?;
    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Err(format!("{stage}: LLM 未配置"));
    }
    for round in 0..3u32 {
        match crate::llm_stream::chat_completion_dispatch(
            &llm.base_url,
            &llm.api_key,
            &llm.model,
            &state.provider_kind,
            system,
            user,
            0.1, 16384, LLM_TIMEOUT_SECS,
            &client,
        )
        .await
        {
            Ok(raw) => {
                if let Some(v) = crate::llm_stream::extract_json_value(&raw) {
                    // 上游 opencode zen 的 deepseek-v4-flash-free 会 ~25% 概率随机把单个
                    // 中文字采样成 U+FFFD（整字转 2-3 个替换符，位置随机）。含损坏字的结构化
                    // 内容是脏数据，丢弃重试以规避。
                    if json_has_fffd(&v) {
                        tracing::warn!(stage, round, "{stage}: LLM 输出含 U+FFFD 损坏字，重试");
                    } else {
                        return Ok(v);
                    }
                } else {
                    tracing::warn!(stage, round, "{stage}: JSON 解析失败，重试");
                }
            }
            Err(e) => {
                tracing::warn!(stage, round, "{stage}: LLM 调用失败: {e}，重试");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(format!("{stage}: 重试 3 次后仍失败"))
}

/// 世界树条目规范化：缺失字段补默认值，供落库前使用。
///
/// - `title`/`text` 缺失或为空 → 返回 None（无标题/无证据归纳的条目不落库）。
/// - `tags` 缺省空数组；`permanent` 缺省 true；`importance` 缺省 low；
/// - `type` 缺省 setting；`confidence` 缺省 0.5（并夹到 0.0~1.0）。
pub(crate) fn as_json_array(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}