#![allow(unused_imports)]
use crate::{
    st_compass::{Compass, CompassStore},
    st_skimming::SkimIssue,
    text_hash, CoreError, CoreResult, DataRoot,
};
use crate::world_state::{EntityKind as WsEntityKind, WorldEntity, WorldEvent, WorldState};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};
use uuid::Uuid;
use super::*;

/// [morphling ROMA P0 2026-08-19] 回合进度阶段。
/// 参考 ROMA EventLoopController 的 READY→EXECUTING→COMPLETED/FAILED 状态机——
/// 但这里仅为「中断处快照」用途（幂等观察），不做 DAG 恢复，保持零回归风险。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnPhase {
    /// 已 acquire 回合锁，尚未开始 LLM 流式（含 F3 队列判定后）。
    Queued,
    /// LLM 流式正文生成中（主流 -> 质量管道入口）。
    Streaming,
    /// 已进入质量管道（Quality Refine / 技能模板 stage）。
    Quality,
    /// 正文已定稿、assistant 消息已入列、turn 已 +1（回合完成前的写盘阶段）。
    Persisting,
    /// 回合已完成落盘。
    Done,
}

/// [morphling ROMA P0 2026-08-19] 单条回合进度快照（跟随会话持久化）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnProgress {
    pub turn: u32,
    pub phase: TurnPhase,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub at: String,
}

/// 章节剧情摘要条目。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterDiaryEntry {
    pub chapter_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub start_turn: u32,
    #[serde(default)]
    pub end_turn: u32,
    #[serde(default)]
    pub updated_at_turn: u32,
    /// 用户手动编辑过（或已确认）→ 自动提炼不再覆盖。
    #[serde(default)]
    pub manual_edited: bool,
}

/// [morphling C3 2026-08-16] 章节摘要 fallback 提炼触发配置（None = 默认 10 回合 / 20 事件）。
/// 默认模式是「生成时顺带总结」（每回合自动），此配置只控制 LLM 未输出摘要块时的兜底提炼。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterDiaryConfig {
    /// 每 N 回合兜底提炼一次。
    #[serde(default = "default_diary_turn_interval")]
    pub turn_interval: u32,
    /// 本章事件数 ≥ N 时兜底提炼。
    #[serde(default = "default_diary_event_threshold")]
    pub event_threshold: u32,
}

pub(crate) fn default_diary_turn_interval() -> u32 {
    10
}

pub(crate) fn default_diary_event_threshold() -> u32 {
    20
}

impl Default for ChapterDiaryConfig {
    fn default() -> Self {
        Self {
            turn_interval: default_diary_turn_interval(),
            event_threshold: default_diary_event_threshold(),
        }
    }
}

/// U11: 多轮累计成本/耗时记账（job 级单回合明细在 payload.u11，此处为会话级累计账本）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCostLedger {
    /// 已完成并记账的回合数。
    #[serde(default)]
    pub turns: u32,
    /// 累计 LLM 调用次数（主流 + 质量管道 + 非阻塞后处理，估算口径）。
    #[serde(default)]
    pub llm_calls: u32,
    /// 累计回合耗时（毫秒）。
    #[serde(default)]
    pub total_duration_ms: u64,
    /// 累计估算成本（USD）。
    #[serde(default)]
    pub est_cost_usd: f64,
}

/// G10: 最近一次回合的提交诊断摘要（幂等回执 + 诊断信息）。
/// `stop_turn` 完成（取消回合）或回合正常完成时写入 `TavernSession.last_turn_diagnostic`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnDiagnostic {
    /// 该回合的回合号（正常完成 = 已完成回合数；被停止 = 在途回合号 turn+1）。
    #[serde(default)]
    pub turn: u32,
    /// 回合是否被接受并正常完成（true = 已产出对白；false = 被停止/取消）。
    #[serde(default)]
    pub accepted: bool,
    /// 回合耗时（毫秒）。
    #[serde(default)]
    pub duration_ms: u64,
    /// LLM 是否成功（对白生成成功）。
    #[serde(default)]
    pub llm_ok: bool,
    /// 去 AI 味评分（humanizer-zh 5 维 50 分制，0 = 未跑）。
    #[serde(default)]
    pub humanize_total: u8,
    /// 去 AI 味等级（优秀/良好/需修订/空）。
    #[serde(default)]
    pub humanize_grade: String,
    /// 去 AI 味命中数。
    #[serde(default)]
    pub humanize_hits: usize,
    /// 声线漂移分 [0,1]（0 = 未跑/无台词）。
    #[serde(default)]
    pub voice_drift_score: f64,
    /// 声线漂移角色 id（空 = 无漂移）。
    #[serde(default)]
    pub voice_drift_char: String,
    /// 声线漂移原因（中文）。
    #[serde(default)]
    pub voice_drift_reasons: Vec<String>,
    /// 骰子伪造嫌疑（吞噬 loreweaver turn_checks 伪造检）。
    #[serde(default)]
    pub dice_forgery: bool,
    /// 骰子矛盾（吞噬 loreweaver turn_checks 矛盾检）。
    #[serde(default)]
    pub dice_contradiction: bool,
    /// 骰审明细（中文）。
    #[serde(default)]
    pub dice_reasons: Vec<String>,
    /// 幻觉候选名（吞噬 AI-Reader-V2 hallucination_filter 口径：提取名在正文无 grounding）。
    #[serde(default)]
    pub hallu_names: Vec<String>,
    /// 冲突告警（吞噬 AI-Reader-V2 conflict_detector 口径：能力回退/关系反复/死亡复活）。
    #[serde(default)]
    pub conflicts: Vec<String>,
}

pub(crate) fn default_timeline() -> String {
    "main".into()
}

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_live_every_n() -> u32 {
    1
}

// ─── Persona (cross-session L4) ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TavernPersona {
    pub character_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub memory_l4: MemoryL4,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at: String,
}

// ─── Create session request ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub pack_id: String,
    #[serde(default)]
    pub playable: Playable,
    #[serde(default)]
    pub play_mode: PlayMode,
    #[serde(default)]
    pub user_tier: ContentTier,
    #[serde(default)]
    pub global_tier: Option<ContentTier>,
    #[serde(default)]
    pub entry: Option<EntryConfig>,
    #[serde(default)]
    pub player_display_name: Option<String>,
    #[serde(default)]
    pub adult_confirmed: bool,
    #[serde(default)]
    pub title: Option<String>,
    /// F1: owner user_id (set by server handler from session_from).
    #[serde(default)]
    pub owner: Option<String>,
    /// P0: 回合生成写作档位（lite/standard/heavy），默认 lite。
    #[serde(default)]
    pub quality: Quality,
    /// Optional Author Zone project to bind on create (AZ-1+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_project_id: Option<String>,
    /// U13: Optional work id. When present and that work has a saved creation
    /// compass (CompassStore), auto-mount it on the new session at create time
    /// (so new sessions no longer need a re-PUT after setting the compass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub(crate) fn safe_id(id: &str) -> CoreResult<String> {
    let s = id.trim();
    if s.is_empty()
        || s.contains('/')
        || s.contains('\\')
        || s.contains("..")
        || s.chars().any(|c| c.is_control())
    {
        return Err(CoreError::BadRequest("invalid id".into()));
    }
    Ok(s.to_string())
}

pub(crate) fn write_atomic(path: &Path, body: &str) -> CoreResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // 通用兜底：清除 U+FFFD 替换符。上游 LLM 会偶发把单字损坏成 2-3 个 FFFD，
    // 此字符是坏字残骸、无恢复语义，落盘前一律剔除，避免脏字符进入任何持久化文件。
    let body = body.replace('\u{FFFD}', "");
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

// ─── PackStore ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PackStore {
    data: DataRoot,
    lock: std::sync::Arc<Mutex<()>>,
}

impl PackStore {
    pub fn new(data: DataRoot) -> Self {
        let _ = data.ensure_layout();
        Self {
            data,
            lock: std::sync::Arc::new(Mutex::new(())),
        }
    }

    fn packs_root(&self) -> PathBuf {
        self.data.story_packs_dir()
    }

    /// 暴露 pack 目录（正文 bodyPath 相对此目录）——剧情助手全书检索等场景用。
    pub fn pack_dir(&self, id: &str) -> CoreResult<PathBuf> {
        let id = safe_id(id)?;
        Ok(self.packs_root().join(id))
    }

    fn pack_json_path(&self, id: &str) -> CoreResult<PathBuf> {
        Ok(self.pack_dir(id)?.join("pack.json"))
    }

    pub fn list(&self) -> CoreResult<Vec<PackSummary>> {
        let _g = self.lock.lock();
        let root = self.packs_root();
        fs::create_dir_all(&root)?;
        let mut out = Vec::new();
        for ent in fs::read_dir(&root)? {
            let ent = ent?;
            if !ent.file_type()?.is_dir() {
                continue;
            }
            let name = ent.file_name().to_string_lossy().to_string();
            // [fix 2026-08-16] 跳过备份目录（*-bak-*）：备份 pack.json 内 id 未改写，
            // 会与主 pack 同 id 重复返回（窝边草 pack 曾因此列出两份）。
            if name.contains("-bak-") || name.ends_with("-bak") {
                continue;
            }
            match self.load_unlocked(&name) {
                Ok(p) => out.push(PackSummary::from(&p)),
                Err(e) => tracing::warn!(pack=%name, err=%e, "skip unreadable pack"),
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> CoreResult<StoryPack> {
        let _g = self.lock.lock();
        self.load_unlocked(id)
    }

    pub(crate) fn load_unlocked(&self, id: &str) -> CoreResult<StoryPack> {
        let path = self.pack_json_path(id)?;
        if !path.exists() {
            return Err(CoreError::NotFound(format!("pack not found: {id}")));
        }
        let raw = fs::read_to_string(&path)?;
        let mut pack: StoryPack = serde_json::from_str(&raw)?;
        // T 层 (2026-08-19): 旁挂 worldline.json（静态蒸馏时间线）加载进 pack.worldline。
        // lenient：文件缺失/解析失败静默空（旧 pack 或未蒸馏 -> 零影响），不阻塞 pack 加载。
        let wl_path = self.pack_dir(id)?.join("worldline.json");
        if let Ok(wl_raw) = fs::read_to_string(&wl_path) {
            if let Ok(list) = serde_json::from_str::<Vec<Value>>(&wl_raw) {
                pack.worldline = list;
            } else {
                tracing::warn!(pack=%id, "worldline.json 解析失败，时间线为空");
            }
        }
        Ok(pack)
    }

    pub fn save(&self, pack: StoryPack) -> CoreResult<StoryPack> {
        self.save_inner(pack, None)
    }

    /// CAS write: only persists if the on-disk `updated_at` still equals `base_revision`.
    pub fn save_with_revision(
        &self,
        pack: StoryPack,
        base_revision: &str,
    ) -> CoreResult<StoryPack> {
        self.save_inner(pack, Some(base_revision))
    }

    fn save_inner(
        &self,
        mut pack: StoryPack,
        base_revision: Option<&str>,
    ) -> CoreResult<StoryPack> {
        let _g = self.lock.lock();
        let is_new = pack.id.trim().is_empty();
        if is_new {
            pack.id = format!("pack-{}", Uuid::new_v4());
        }
        let id = safe_id(&pack.id)?;
        pack.id = id.clone();
        let now = now_rfc3339();
        if pack.created_at.is_empty() {
            pack.created_at = now.clone();
        }
        pack.updated_at = now;
        if pack.language.is_empty() {
            pack.language = "zh".into();
        }

        // Revision CAS: existing packs must be saved on top of the revision the caller read.
        if !is_new {
            if let Some(base) = base_revision {
                let current = match self.load_unlocked(&id) {
                    Ok(c) => c,
                    Err(CoreError::NotFound(_)) => {
                        return Err(CoreError::Conflict(
                            "pack 已被其他操作删除，请重新加载".into(),
                        ));
                    }
                    Err(e) => return Err(e),
                };
                if current.updated_at != base {
                    return Err(CoreError::Conflict(format!(
                        "pack 已被其他操作更新，请重新加载后再保存 (期望 revision {base}，当前 {})",
                        current.updated_at
                    )));
                }
            }
        }

        let dir = self.pack_dir(&id)?;
        fs::create_dir_all(dir.join("chapters"))?;

        // Write chapter bodies if body_path empty but we have content in goals? — no, bodies separate.
        let path = self.pack_json_path(&id)?;
        write_atomic(&path, &serde_json::to_string_pretty(&pack)?)?;
        Ok(pack)
    }

    pub fn write_chapter_body(&self, pack_id: &str, rel_path: &str, body: &str) -> CoreResult<()> {
        let _g = self.lock.lock();
        let id = safe_id(pack_id)?;
        let rel = rel_path.trim().trim_start_matches('/');
        if rel.is_empty() || rel.contains("..") || Path::new(rel).is_absolute() {
            return Err(CoreError::BadRequest("invalid chapter path".into()));
        }
        let full = self.pack_dir(&id)?.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&full, body)?;
        Ok(())
    }

    pub fn read_chapter_body(&self, pack_id: &str, rel_path: &str) -> CoreResult<String> {
        let _g = self.lock.lock();
        let id = safe_id(pack_id)?;
        let rel = rel_path.trim().trim_start_matches('/');
        if rel.is_empty() || rel.contains("..") {
            return Err(CoreError::BadRequest("invalid chapter path".into()));
        }
        let full = self.pack_dir(&id)?.join(rel);
        if !full.exists() {
            return Err(CoreError::NotFound(format!("chapter body missing: {rel}")));
        }
        Ok(fs::read_to_string(full)?)
    }

    pub fn delete(&self, id: &str) -> CoreResult<()> {
        let _g = self.lock.lock();
        let dir = self.pack_dir(id)?;
        if !dir.exists() {
            return Err(CoreError::NotFound(format!("pack not found: {id}")));
        }
        fs::remove_dir_all(dir)?;
        Ok(())
    }


    /// Export pack directory as zip bytes (pack.json + chapters/*).
    pub fn export_zip(&self, pack_id: &str) -> CoreResult<Vec<u8>> {
        let _g = self.lock.lock();
        let id = safe_id(pack_id)?;
        let dir = self.pack_dir(&id)?;
        if !dir.exists() {
            return Err(CoreError::NotFound(format!("pack not found: {id}")));
        }
        let mut buf: Vec<u8> = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zip = ZipWriter::new(cursor);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            // walk dir
            fn add_dir(
                zip: &mut ZipWriter<Cursor<&mut Vec<u8>>>,
                opts: SimpleFileOptions,
                base: &Path,
                rel: &Path,
            ) -> CoreResult<()> {
                let full = base.join(rel);
                if full.is_dir() {
                    for ent in fs::read_dir(&full)? {
                        let ent = ent?;
                        let name = ent.file_name();
                        let child_rel = rel.join(&name);
                        add_dir(zip, opts, base, &child_rel)?;
                    }
                } else if full.is_file() {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    zip.start_file(rel_str, opts)
                        .map_err(|e| CoreError::BadRequest(format!("zip start: {e}")))?;
                    let data = fs::read(&full)?;
                    zip.write_all(&data)
                        .map_err(|e| CoreError::BadRequest(format!("zip write: {e}")))?;
                }
                Ok(())
            }
            add_dir(&mut zip, opts, &dir, Path::new("pack.json"))?;
            let chapters = dir.join("chapters");
            if chapters.is_dir() {
                for ent in fs::read_dir(&chapters)? {
                    let ent = ent?;
                    if ent.file_type()?.is_file() {
                        let name = ent.file_name();
                        let rel = Path::new("chapters").join(name);
                        add_dir(&mut zip, opts, &dir, &rel)?;
                    }
                }
            }
            zip.finish()
                .map_err(|e| CoreError::BadRequest(format!("zip finish: {e}")))?;
        }
        Ok(buf)
    }

    /// Import pack from zip bytes. Optional id override. Returns saved StoryPack.
    pub fn import_zip(&self, zip_bytes: &[u8], id_override: Option<String>) -> CoreResult<StoryPack> {
        const MAX_PACK_ENTRIES: usize = 2000;
        const MAX_PACK_ENTRY_BYTES: u64 = 32 * 1024 * 1024; // 32 MiB per entry
        const MAX_PACK_TOTAL_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB total expanded
        const MAX_PACK_RATIO: u64 = 200;
        let _g = self.lock.lock();
        let cursor = Cursor::new(zip_bytes);
        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| CoreError::BadRequest(format!("invalid zip: {e}")))?;
        if archive.len() > MAX_PACK_ENTRIES {
            return Err(CoreError::BadRequest(format!(
                "zip entry count {} exceeds limit {MAX_PACK_ENTRIES}",
                archive.len()
            )));
        }
        // Find pack.json
        let mut pack_json: Option<String> = None;
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut declared_total: u64 = 0;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| CoreError::BadRequest(format!("zip entry: {e}")))?;
            let name = file.name().replace('\\', "/").trim_start_matches('/').to_string();
            if name.is_empty() || name.ends_with('/') || name.contains("..") {
                continue;
            }
            // Budget checks against declared sizes before decompressing.
            let (declared, compressed) = (file.size(), file.compressed_size());
            if declared > MAX_PACK_ENTRY_BYTES {
                return Err(CoreError::BadRequest(format!(
                    "zip entry {name} declared {declared} bytes exceeds limit"
                )));
            }
            declared_total = declared_total.saturating_add(declared);
            if declared_total > MAX_PACK_TOTAL_BYTES {
                return Err(CoreError::BadRequest(
                    "zip expanded size exceeds limit".to_string(),
                ));
            }
            if declared > 0 && compressed > 0 && declared / compressed > MAX_PACK_RATIO {
                return Err(CoreError::BadRequest(format!(
                    "zip entry {name} compression ratio exceeds limit"
                )));
            }
            let mut data = Vec::new();
            file.by_ref()
                .take(MAX_PACK_ENTRY_BYTES + 1)
                .read_to_end(&mut data)
                .map_err(|e| CoreError::BadRequest(format!("zip read: {e}")))?;
            if data.len() as u64 > MAX_PACK_ENTRY_BYTES {
                return Err(CoreError::BadRequest(format!(
                    "zip entry {name} expanded beyond limit"
                )));
            }
            if name == "pack.json" || name.ends_with("/pack.json") {
                pack_json = Some(String::from_utf8_lossy(&data).into_owned());
            }
            files.push((name, data));
        }
        let pack_raw = pack_json.ok_or_else(|| CoreError::BadRequest("zip missing pack.json".into()))?;
        let mut pack: StoryPack = serde_json::from_str(&pack_raw)
            .map_err(|e| CoreError::BadRequest(format!("pack.json parse: {e}")))?;
        if let Some(id) = id_override.filter(|s| !s.trim().is_empty()) {
            pack.id = id;
        }
        if pack.id.trim().is_empty() {
            pack.id = format!("pack-{}", Uuid::new_v4());
        }
        let id = safe_id(&pack.id)?;
        pack.id = id.clone();
        let now = now_rfc3339();
        if pack.created_at.is_empty() {
            pack.created_at = now.clone();
        }
        pack.updated_at = now;
        if pack.language.is_empty() {
            pack.language = "zh".into();
        }
        // if id exists, append suffix
        let mut final_id = id.clone();
        let mut dir = self.pack_dir(&final_id)?;
        if dir.exists() {
            final_id = format!("{id}-imp-{}", &Uuid::new_v4().to_string()[..8]);
            pack.id = final_id.clone();
            dir = self.pack_dir(&final_id)?;
        }
        fs::create_dir_all(dir.join("chapters"))?;
        // write files
        for (name, data) in files {
            let rel = if let Some(rest) = name.strip_prefix("pack.json") {
                if rest.is_empty() {
                    "pack.json".to_string()
                } else {
                    // nested pack.json path — only use basename files under chapters
                    continue;
                }
            } else if name.ends_with("/pack.json") {
                "pack.json".to_string()
            } else if let Some(idx) = name.rfind("chapters/") {
                name[idx..].to_string()
            } else if name.starts_with("chapters/") {
                name
            } else {
                continue;
            };
            if rel.contains("..") {
                continue;
            }
            if rel == "pack.json" {
                continue; // write from struct below
            }
            let full = dir.join(&rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            write_atomic(
                &full,
                &String::from_utf8_lossy(&data),
            )?;
        }
        let path = dir.join("pack.json");
        write_atomic(&path, &serde_json::to_string_pretty(&pack)?)?;
        Ok(pack)
    }

    /// Seed demo pack if missing. Idempotent.
    pub fn ensure_demo_pack(&self) -> CoreResult<StoryPack> {
        const DEMO_ID: &str = "demo-rain-alley";
        if self.pack_json_path(DEMO_ID)?.exists() {
            return self.get(DEMO_ID);
        }
        self.install_demo_pack()
    }

    pub fn install_demo_pack(&self) -> CoreResult<StoryPack> {
        let pack = build_demo_pack();
        let ch1 = DEMO_CH01_MD;
        let ch2 = DEMO_CH02_MD;
        let saved = self.save(pack)?;
        self.write_chapter_body(&saved.id, "chapters/ch01.md", ch1)?;
        self.write_chapter_body(&saved.id, "chapters/ch02.md", ch2)?;
        Ok(saved)
    }
}

pub(crate) const DEMO_CH01_MD: &str = r#"# 第一章 · 雨巷来客

雨下了一整夜。巷口那盏坏掉的路灯偶尔闪一下，像有人在黑暗里眨眼。

沈棠撑着油纸伞，停在旧茶馆门前。门楣上的铜铃已经锈住，推门时仍发出一声细响。

店里只有一个人。

林晚坐在靠窗的位置，面前是半杯凉透的茶。她抬眼，声音很轻：

「你比约定晚了十二分钟。」

沈棠把伞靠在门边，雨水顺着伞骨滴到青石板上。

「路上有人跟着我。」

林晚的手指在杯沿顿了一下，随即恢复平静：

「那就说明，故事开始了。」
"#;

pub(crate) const DEMO_CH02_MD: &str = r#"# 第二章 · 铜铃之后

铜铃再响时，进来的不是客人。

风把一封没有署名的信吹到门槛内。信封是旧式的牛皮纸，封口用红蜡，印记像一枚破碎的月亮。

林晚没有立刻去捡。她看着沈棠：

「你来选。拆开它，或者我们假装今夜什么都没发生。」

沈棠知道这不是真的选择题。巷子外面的脚步声已经停了。

雨变小了，但没停。
"#;

pub(crate) fn build_demo_pack() -> StoryPack {
    let now = now_rfc3339();
    StoryPack {
        id: "demo-rain-alley".into(),
        title: "雨巷来客".into(),
        source: PackSource {
            source_type: "demo".into(),
            refs: vec![],
        },
        characters: vec![
            PackCharacterRef {
                id: "cc-shentang".into(),
                name: "沈棠".into(),
                role: "主角视角可选".into(),
                gender: "女".into(),
                appearance: "未知".into(),
                opening_scene: "未知".into(),
                opening_lines: "".into(),
                nsfw_profile: String::new(),
                importance: "high".into(),
                content_tier: Some(ContentTier::Standard),
                example_dialogs: vec![
                    "路上有人跟着我。".into(),
                    "你若怕，现在还可以走。".into(),
                ],
                boundaries: vec!["不无现代网络梗".into()],
                personality: "谨慎、观察力强，嘴硬心软".into(),
                speech_style: "短句，偶尔带一点旧小说腔".into(),
                voice_profile: String::new(),
                motivation: String::new(),
                relationships: vec![],
                evidence_refs: vec![],
                mental_models: vec![],
                decision_heuristics: vec![],
                beliefs: vec![],
                expressions: Default::default(),
            voice: None,
            archive: None,
                avatar: None,
                starting_wardrobe: Default::default(),
            },
            PackCharacterRef {
                id: "cc-linwan".into(),
                name: "林晚".into(),
                role: "关键 NPC".into(),
                gender: "女".into(),
                appearance: "未知".into(),
                opening_scene: "未知".into(),
                opening_lines: "".into(),
                nsfw_profile: String::new(),
                importance: "high".into(),
                content_tier: Some(ContentTier::Standard),
                example_dialogs: vec![
                    "你比约定晚了十二分钟。".into(),
                    "那就说明，故事开始了。".into(),
                ],
                boundaries: vec!["不突然变成无理由反派".into()],
                personality: "冷静、话少、知道得比表面多".into(),
                speech_style: "轻声、句子干净".into(),
                voice_profile: String::new(),
                motivation: String::new(),
                relationships: vec![],
                evidence_refs: vec![],
                mental_models: vec![],
                decision_heuristics: vec![],
                beliefs: vec![],
                expressions: Default::default(),
            voice: None,
            archive: None,
                avatar: None,
                starting_wardrobe: Default::default(),
            },
        ],
        world_book_ids: vec![],
        chapters: vec![
            StoryChapter {
                id: "ch01".into(),
                title: "雨巷来客".into(),
                order: 1,
                goals: vec!["抵达茶馆".into(), "与林晚会面".into()],
                node_ids: vec!["n1".into(), "n2".into()],
                body_path: "chapters/ch01.md".into(),
                image_path: String::new(), // U10
            },
            StoryChapter {
                id: "ch02".into(),
                title: "铜铃之后".into(),
                order: 2,
                goals: vec!["面对无名信".into(), "做出第一选择".into()],
                node_ids: vec!["n3".into()],
                body_path: "chapters/ch02.md".into(),
                image_path: String::new(), // U10
            },
        ],
        nodes: vec![
            StoryNode {
                id: "n1".into(),
                chapter_id: "ch01".into(),
                title: "巷口".into(),
                entry: "雨夜，玩家抵达旧茶馆门前".into(),
                exit: vec![NodeExit {
                    id: "e1".into(),
                    when: "enter_teahouse".into(),
                    next: "n2".into(),
                }],
                locked_beats: vec!["不能跳过会面".into()],
                allowed_divergence: "branch".into(),
                present_characters: vec!["cc-shentang".into()],
                location_id: Some("loc-alley".into()),
                summary: "雨巷、坏路灯、旧茶馆门前".into(),
            },
            StoryNode {
                id: "n2".into(),
                chapter_id: "ch01".into(),
                title: "茶馆初见".into(),
                entry: "推门见林晚，对上约定".into(),
                exit: vec![NodeExit {
                    id: "e2".into(),
                    when: "talk_done_or_threat_mentioned".into(),
                    next: "n3".into(),
                }],
                locked_beats: vec!["林晚已知有人跟踪的事实可被揭示".into()],
                allowed_divergence: "branch".into(),
                present_characters: vec!["cc-shentang".into(), "cc-linwan".into()],
                location_id: Some("loc-teahouse".into()),
                summary: "林晚候着，提起迟到与跟踪".into(),
            },
            StoryNode {
                id: "n3".into(),
                chapter_id: "ch02".into(),
                title: "无名信".into(),
                entry: "铜铃再响，红蜡信到门槛".into(),
                exit: vec![
                    NodeExit {
                        id: "e3a".into(),
                        when: "open_letter".into(),
                        // L-5: intentional open ending — final node of the demo pack; the
                        // player stays at the teahouse to keep exploring/dialogue freely
                        // rather than hitting a hard terminal. Both exits loop back to n3.
                        next: "n3".into(),
                    },
                    NodeExit {
                        id: "e3b".into(),
                        when: "ignore_letter".into(),
                        next: "n3".into(),
                    },
                ],
                locked_beats: vec!["信的存在不可抹除".into()],
                allowed_divergence: "branch".into(),
                present_characters: vec!["cc-shentang".into(), "cc-linwan".into()],
                location_id: Some("loc-teahouse".into()),
                summary: "红蜡无名信，门外脚步停住".into(),
            },
        ],
        lore_entries: vec![],
        event_packages: vec![],
        actor_state_config: ActorStatePackConfig::default(),
        default_mode: PlayMode::Mainline,
        max_tier: ContentTier::Standard,
        language: "zh".into(),
        created_at: now.clone(),
        updated_at: now,
        stage_director: StageDirectorConfig::default(),
        worldline: vec![], // T 层测试 fixture：demo pack 无静态时间线
    }
}

// ─── SessionStore ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TavernSessionStore {
    pub(crate) data: DataRoot,
    pub(crate) lock: std::sync::Arc<Mutex<()>>,
}


/// Clip by Unicode scalar count (CJK-safe).
/// P1/C1: entry/开场白占位检测——「本章开始」等占位文本（或过短无场景信息的）不配作为
/// 开场锚点，应回退 node.summary（蒸馏自原著正文，含真实场景），避免 LLM 靠标题脑补
/// （度蜜月 pack 实测：entry="本章开始" 时开幕直接脑补「酒店蜜月套房醒来」）。
/// 判定：命中占位词列表，或 trim 后 <4 字符（无信息量），或含「待更新/占位/TODO/placeholder」。
pub(crate) fn is_placeholder_entry(e: &str) -> bool {
    let t = e.trim();
    if t.chars().count() < 4 {
        return true;
    }
    let lower = t.to_lowercase();
    const PLACEHOLDERS: &[&str] = &[
        "本章开始", "本段开始", "待更新", "占位", "待补充", "待完善", "此处插入",
        "placeholder", "todo", "tbd", "lorem", "xxx", "此处为",
    ];
    PLACEHOLDERS.iter().any(|p| lower.contains(p))
}

pub fn clip_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub(crate) fn chapter_title_of(pack: &StoryPack, chapter_id: &str) -> String {
    pack.chapters
        .iter()
        .find(|c| c.id == chapter_id)
        .map(|c| c.title.clone())
        .unwrap_or_else(|| chapter_id.to_string())
}

pub(crate) fn chapter_order_of(pack: &StoryPack, chapter_id: &str) -> u32 {
    pack.chapters
        .iter()
        .find(|c| c.id == chapter_id)
        .map(|c| c.order)
        .unwrap_or(9999)
}

/// Whole-novel summary from lore + ordered node summaries (no LLM).
pub fn build_pack_novel_summary(pack: &StoryPack) -> String {
    let mut parts: Vec<String> = Vec::new();
    // Prefer lore 简介
    for lore in &pack.lore_entries {
        let title = lore
            .get("title")
            .or_else(|| lore.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let body = lore
            .get("content")
            .or_else(|| lore.get("text"))
            .or_else(|| lore.get("body"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if title.contains("简介") || title.contains("概要") || title.to_ascii_lowercase().contains("blurb") {
            let b = body.trim();
            if !b.is_empty() {
                parts.push(clip_chars(b, 360));
                break;
            }
        }
    }
    if parts.is_empty() {
        for lore in &pack.lore_entries {
            let body = lore
                .get("content")
                .or_else(|| lore.get("text"))
                .or_else(|| lore.get("body"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if body.chars().count() > 40 {
                parts.push(clip_chars(body, 360));
                break;
            }
        }
    }

    let mut nodes = pack.nodes.clone();
    nodes.sort_by_key(|n| chapter_order_of(pack, &n.chapter_id));
    let mut beats: Vec<String> = Vec::new();
    let step = ((nodes.len().max(1) + 5) / 6).max(1);
    for (i, n) in nodes.iter().enumerate() {
        if i == 0 || i + 1 == nodes.len() || i % step == 0 {
            let snip = n.summary.trim();
            if snip.is_empty() {
                continue;
            }
            beats.push(format!("· {}：{}", n.title, clip_chars(snip, 72)));
        }
    }
    if !beats.is_empty() {
        parts.push(format!("关键节点：\n{}", beats.join("\n")));
    }
    if parts.is_empty() {
        parts.push(format!(
            "《{}》共 {} 章、{} 个剧情节点。",
            pack.title,
            pack.chapters.len(),
            pack.nodes.len()
        ));
    }
    parts.join("\n\n")
}

/// Pick important nodes across the novel for side-branch entry points.
pub fn select_side_branch_nodes(pack: &StoryPack, limit: usize) -> Vec<SideBranchNode> {
    let limit = limit.clamp(1, 16);
    let mut nodes = pack.nodes.clone();
    nodes.sort_by_key(|n| chapter_order_of(pack, &n.chapter_id));
    if nodes.is_empty() {
        return Vec::new();
    }
    let n = nodes.len();
    let mut idxs: Vec<usize> = Vec::new();
    // always first + last
    idxs.push(0);
    if n > 1 {
        idxs.push(n - 1);
    }
    // evenly spaced middles
    let want_mid = limit.saturating_sub(idxs.len()).max(0);
    if want_mid > 0 && n > 2 {
        for k in 1..=want_mid {
            let i = (k * (n - 1)) / (want_mid + 1);
            if i > 0 && i + 1 < n {
                idxs.push(i);
            }
        }
    }
    // prefer nodes with richer summary if still short
    if idxs.len() < limit {
        let mut ranked: Vec<(usize, usize)> = nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.summary.chars().count(), i))
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, i) in ranked {
            if idxs.len() >= limit {
                break;
            }
            if !idxs.contains(&i) {
                idxs.push(i);
            }
        }
    }
    idxs.sort_unstable();
    idxs.dedup();
    idxs.truncate(limit);

    let reasons = [
        "开篇切入",
        "中段转折",
        "关系升温",
        "冲突爆发",
        "情感高潮",
        "收束前夜",
        "结局余韵",
        "关键节点",
    ];
    idxs.into_iter()
        .enumerate()
        .map(|(rank, i)| {
            let node = &nodes[i];
            let reason = if i == 0 {
                "开篇切入".into()
            } else if i + 1 == n {
                "结局余韵".into()
            } else {
                reasons
                    .get((rank + i) % reasons.len())
                    .copied()
                    .unwrap_or("关键节点")
                    .to_string()
            };
            SideBranchNode {
                id: node.id.clone(),
                chapter_id: node.chapter_id.clone(),
                chapter_title: chapter_title_of(pack, &node.chapter_id),
                title: node.title.clone(),
                summary: clip_chars(node.summary.trim(), 220),
                entry: node.entry.clone(),
                present_characters: node.present_characters.clone(),
                reason,
                order: chapter_order_of(pack, &node.chapter_id),
            }
        })
        .collect()
}

pub fn build_side_branch_catalog(pack: &StoryPack, resume_node_id: Option<String>) -> SideBranchCatalog {
    SideBranchCatalog {
        pack_id: pack.id.clone(),
        pack_title: pack.title.clone(),
        novel_summary: build_pack_novel_summary(pack),
        nodes: select_side_branch_nodes(pack, 8),
        resume_node_id,
    }
}

pub(crate) fn present_names(pack: &StoryPack, ids: &[String]) -> String {
    let names: Vec<&str> = ids
        .iter()
        .filter_map(|id| {
            pack.characters
                .iter()
                .find(|c| c.id == *id)
                .map(|c| c.name.as_str())
        })
        .filter(|n| *n != "旁白" && *n != "读者")
        .collect();
    if names.is_empty() {
        "（待定）".into()
    } else {
        names.join("、")
    }
}

/// Mainline / first-open opening monologue + option chips.
pub fn build_mainline_opening(pack: &StoryPack, session: &TavernSession) -> (String, Vec<String>) {
    let node = session
        .node_id
        .as_ref()
        .and_then(|nid| pack.nodes.iter().find(|n| n.id == *nid));
    let ch_title = session
        .chapter_cursor
        .as_ref()
        .map(|cid| chapter_title_of(pack, cid))
        .unwrap_or_else(|| "未知章节".into());
    let node_title = node.map(|n| n.title.as_str()).unwrap_or("起始");
    let entry = node
        .map(|n| {
            let e = n.entry.trim();
            // P1/C1: entry 占位检测——「本章开始」等占位（或 <4 字无信息量）不配作为开场锚点，
            // 回退 node.summary 前 280 字（node.summary 蒸馏自原著该章正文，含真实场景）。
            // 否则 LLM 只能靠标题脑补场景（度蜜月 pack 实测：开幕直接脑补「酒店蜜月套房醒来」）。
            if is_placeholder_entry(e) {
                clip_chars(n.summary.trim(), 280)
            } else if !e.is_empty() {
                e.to_string()
            } else {
                clip_chars(n.summary.trim(), 280)
            }
        })
        .unwrap_or_default();
    let present = present_names(pack, &session.present_character_ids);
    let play = match session.playable {
        Playable::P1 => "P1 旁观",
        Playable::P2 => "P2 多角色",
        Playable::P3 => "P3 进入世界",
        Playable::P4 => "P4 自由演绎",
    };
    let mut entry_line = String::new();
    if let Some(role) = session.entry.entry_role {
        let role_cn = match role {
            EntryRole::Supporting => "配角",
            EntryRole::Protagonist => "主角",
            EntryRole::Isekai => "穿越者",
            EntryRole::Extra => "路人",
        };
        entry_line = format!("进入身份：{}。", role_cn);
    }
    // 演出层 L3a（P14）：蒸馏出的个性化 first_mes——焦点角色的开场场景画布 + 完整开场白。
    // 非空时置于开场正文顶部；空则回退通用模板（不破坏旧 pack）。
    let focus_char = session
        .focus_character_id
        .as_ref()
        .and_then(|fid| pack.characters.iter().find(|c| c.id == *fid))
        .or_else(|| pack.characters.first());
    let opening_scene = focus_char
        .map(|c| c.opening_scene.trim())
        .filter(|s| !s.is_empty());
    let opening_lines = focus_char
        .map(|c| c.opening_lines.trim())
        .filter(|s| !s.is_empty());
    let personalized_opening: Option<String> = match (opening_scene, opening_lines) {
        (Some(scene), Some(lines)) => Some(format!("{scene}\n\n{lines}")),
        (Some(scene), None) => Some(scene.to_string()),
        (None, Some(lines)) => Some(lines.to_string()),
        (None, None) => None,
    };
    let mut body = format!(
        "【开场】\n你踏入《{title}》的世界。\n当前：{ch} · 节点「{nt}」· {play}。\n在场：{present}。\n{entry_line}\n{entry}\n\n夜色与故事同时拉开——你可以选择下一步，或直接输入想做的事。",
        title = pack.title,
        ch = ch_title,
        nt = node_title,
        play = play,
        present = present,
        entry_line = entry_line,
        entry = if entry.is_empty() { "世界在你眼前缓缓展开。".into() } else { entry },
    );
    // 个性化 first_mes 置顶（在通用 brainfuck 之前），保留下方「本段原著场景即你此刻所在」
    // 时空硬锚以继续约束玩家行动范围（个性化开场不替代该锚点）。
    if let Some(personalized) = personalized_opening {
        body = format!("{personalized}\n\n{body}");
    }
    // §13.4② (2026-08-18): 当下状态硬锚——上面的 entry（原著场景）即此刻时空，
    // 玩家行动必须发生在此场景内；禁止跳跃到其他时间地点（机场/酒店/数日后）。
    // 修复实证：度蜜月开幕「环顾四周/打招呼」被 LLM 脑补到三亚机场（选项-场景解耦）。
    // 若玩家明确要求推进时间/地点，由正文末尾 [时间推进: ...] 标注走系统处理。
    body.push_str(
        "\n（本段原著场景即你此刻所在：所有行动（环顾/招呼/探索）都发生在此场景内，\n不得跳跃到其他时间地点；玩家明确要求离开时，在正文末尾用 [时间推进: ...] 标注，\n由系统统一处理时间地点切换。）",
    );
    let options = vec![
        "环顾四周，感受这个世界".into(),
        "与在场的人打招呼".into(),
        "按原著节奏往下走".into(),
    ];
    (body, options)
}

/// Side-branch opening after entering a key node.
pub fn build_side_opening(pack: &StoryPack, session: &TavernSession, node_id: &str) -> (String, Vec<String>) {
    let node = pack.nodes.iter().find(|n| n.id == node_id);
    let title = node.map(|n| n.title.as_str()).unwrap_or(node_id);
    let ch = node
        .map(|n| chapter_title_of(pack, &n.chapter_id))
        .unwrap_or_default();
    let sum = node
        .map(|n| clip_chars(n.summary.trim(), 320))
        .unwrap_or_default();
    let entry = node
        .map(|n| n.entry.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "本章开始".into());
    let present_ids = node
        .map(|n| n.present_characters.clone())
        .unwrap_or_else(|| session.present_character_ids.clone());
    let present = present_names(pack, &present_ids);
    let resume = session
        .resume_node_id
        .as_deref()
        .unwrap_or(session.node_id.as_deref().unwrap_or("?"));
    let novel = clip_chars(&build_pack_novel_summary(pack), 280);
    let body = format!(
        "【支线开场 · {title}】\n整本脉络（节选）：\n{novel}\n\n你从主线锚点 {resume} 岔入支线「{title}」（{ch}）。\n入口：{entry}\n{sum}\n在场：{present}。\n\n这条线可以大胆偏离主线——结束支线后可回到锚点。",
        title = title,
        novel = novel,
        resume = resume,
        ch = ch,
        entry = entry,
        sum = if sum.is_empty() { String::new() } else { format!("节点摘要：{}\n", sum) },
        present = present,
    );
    let options = vec![
        "顺着这条支线深入".into(),
        "先观察在场人物".into(),
        "准备回到主线".into(),
    ];
    (body, options)
}
