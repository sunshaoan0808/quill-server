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

// ─── PersonaStore ────────────────────────────────────────────────────────────

// ─── Saves / checkpoints (ST-7) ──────────────────────────────────────────────

/// Agent 自建面板（吸收自 Liyuan panels.ts）：舞台美术/元信息层，绝不承载剧情正文。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TavernPanel {
    pub name: String,
    /// markdown | svg | html
    pub kind: String,
    pub content: String,
    #[serde(default)]
    pub updated_at: String,
}

impl TavernPanel {
    pub fn new(name: String, kind: String, content: String) -> Self {
        Self {
            name,
            kind,
            content,
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// P0-1: 回合级检查点（吸收自梨园 story_command /rewind /reroll）。
/// 只存可变字段快照 + messages 长度，不持有 messages 引用/整 session，供回退恢复。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnCheckpoint {
    pub turn: u32,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub chapter_cursor: Option<String>,
    #[serde(default)]
    pub messages_len: usize,
    #[serde(default)]
    pub memory_l1: MemoryL1,
    #[serde(default)]
    pub memory_l2: MemoryL2,
    #[serde(default)]
    pub present_character_ids: Vec<String>,
    #[serde(default)]
    pub focus_character_id: Option<String>,
    #[serde(default)]
    pub director_plan: Option<DirectorPlan>,
    #[serde(default)]
    pub last_event: Option<EventLogEntry>,
    #[serde(default)]
    pub panels: Vec<TavernPanel>,
    #[serde(default)]
    pub created_at: String,
}

impl TavernSession {
    /// P0-1: 快照当前完成态（turn 已 +1、assistant 已入列、memory/director/panels 已更新）。
    /// 在回合 job 落盘前调用；cap 30，超限丢弃最旧。
    pub fn push_checkpoint(&mut self) {
        let cp = TurnCheckpoint {
            turn: self.turn,
            node_id: self.node_id.clone(),
            chapter_cursor: self.chapter_cursor.clone(),
            messages_len: self.messages.len(),
            memory_l1: self.memory_l1.clone(),
            // P0-4: checkpoint 剥离 L2 embedding——30 份快照 × 512 维冗余达 MB 级。
            // 恢复后空向量由 RAG 的 `!e.embedding.is_empty()` 保护自动跳过语义匹配，
            // 仅文本/关键词匹配仍生效（语义 RAG 由当前会话 memory_l2 提供）。
            memory_l2: MemoryL2 {
                events: self
                    .memory_l2
                    .events
                    .iter()
                    .map(|e| MemoryL2Event {
                        id: e.id.clone(),
                        turn: e.turn,
                        kind: e.kind.clone(),
                        summary: e.summary.clone(),
                        actors: e.actors.clone(),
                        node_id: e.node_id.clone(),
                        embedding: Vec::new(),
                    })
                    .collect(),
                updated_at_turn: self.memory_l2.updated_at_turn,
            },
            present_character_ids: self.present_character_ids.clone(),
            focus_character_id: self.focus_character_id.clone(),
            director_plan: self.director_plan.clone(),
            last_event: self.last_event.clone(),
            panels: self.panels.clone(),
            created_at: now_rfc3339(),
        };
        self.checkpoints.push(cp);
        const MAX_CHECKPOINTS: usize = 30;
        if self.checkpoints.len() > MAX_CHECKPOINTS {
            let drop = self.checkpoints.len() - MAX_CHECKPOINTS;
            self.checkpoints.drain(..drop);
        }
    }

    /// P0-1: 回退 steps 个回合：恢复 checkpoint 快照字段并按 messages_len 截断消息。
    /// 列表尾部恒为「当前已完成回合」，故 steps 从尾前一个开始取（回退 1 = 倒数第二个）。
    /// 返回实际回退步数（0 = 无可回退）。
    pub fn restore_checkpoint(&mut self, steps: usize) -> CoreResult<usize> {
        if steps == 0 || self.checkpoints.is_empty() {
            return Ok(0);
        }
        let len = self.checkpoints.len();
        // The tail checkpoint is the snapshot taken before the current turn.
        // For a single checkpoint (the first completed turn), reroll must use
        // that tail snapshot rather than treating it as already consumed.
        let mut idx = if len == 1 && steps == 1 {
            0
        } else {
            len.saturating_sub(steps + 1).min(len - 1)
        };
        // 折叠水位守卫：禁跨水位回退（loreweaver undo lag 窗口同款）
        if self.checkpoints[idx].turn <= self.fold_through_turn
            && !(len == 1 && steps == 1 && self.checkpoints[idx].turn == 0)
        {
            // 找水位之后最近的 checkpoint
            if let Some(j) = self.checkpoints.iter().rposition(|c| c.turn > self.fold_through_turn) {
                idx = j;
            } else {
                return Ok(0);
            }
        }
        let cp = self.checkpoints[idx].clone();
        self.turn = cp.turn;
        self.node_id = cp.node_id;
        self.chapter_cursor = cp.chapter_cursor;
        self.memory_l1 = cp.memory_l1;
        self.memory_l2 = cp.memory_l2;
        self.present_character_ids = cp.present_character_ids;
        self.focus_character_id = cp.focus_character_id;
        self.director_plan = cp.director_plan;
        self.last_event = cp.last_event;
        self.panels = cp.panels;
        self.messages.truncate(cp.messages_len);
        self.checkpoints.truncate(idx + 1);
        // A singleton pre-turn checkpoint represents one actual reroll step,
        // even though it is index 0 of the truncated list.
        Ok(if len == 1 && steps == 1 { 1 } else { len - 1 - idx })
    }

    /// [morphling ROMA P0 2026-08-19] 记录当前回合执行阶段进度（幂等快照，跟随会话落盘）。
    /// 用于崩溃/中断时判断「上回合死在哪一步」，不改故事流。run_id 可为空串。
    pub fn set_turn_progress(&mut self, turn: u32, phase: TurnPhase, run_id: &str, note: &str) {
        self.turn_progress = Some(TurnProgress {
            turn,
            phase,
            run_id: run_id.to_string(),
            note: note.to_string(),
            at: now_rfc3339(),
        });
    }
}

/// MCP 外设工具执行结果摘要（吸收自 Liyuan mcp.ts, 默认仅本机 stdio server）。
/// 只存截断摘要防注入, 完整结果不落库。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultBrief {
    /// 形如 `gemini-search:web_search`
    pub tool: String,
    pub ok: bool,
    pub summary: String,
}

/// skill 工具按需加载结果（吸收自 denova skill.NewMiddleware：模型声明需要完整 SKILL.md 后，下轮构建 system prompt 时注入全文）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLoadInfo {
    /// 请求时命中的写作档位（lite|standard|heavy）。
    pub tier: String,
    /// 完整 SKILL.md + 分档 stage 模板的注入文本。
    pub markdown: String,
}

/// 世界线视图（吸收自 Liyuan worldline.ts 的 WorldlineView）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldlineLine {
    pub id: String,
    /// 分叉源存档 id（主线无）。
    pub fork_from_save_id: Option<String>,
    pub saves: Vec<TavernSaveMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldlineView {
    pub lines: Vec<WorldlineLine>,
    /// 当前所在世界线 id。
    pub current_worldline_id: String,
    /// 当前分支上最近的存档 id（无则 None）。
    pub current_save_id: Option<String>,
}

/// 由会话 + 存档列表构建世界线视图（纯函数）。
/// 主线 = fork_from 为空的存档；分叉线 = 从某存档 fork 出的存档序列。
pub fn build_worldline(session: &TavernSession, saves: &[TavernSaveMeta]) -> WorldlineView {
    let mut lines: Vec<WorldlineLine> = Vec::new();

    // 主线：worldline_id == "main"（兼容旧存档：无 worldline_id 时按 fork_from 为空处理）
    let mut main: Vec<TavernSaveMeta> = saves
        .iter()
        .filter(|s| s.worldline_id == "main" || (s.worldline_id.is_empty() && s.fork_from_save_id.is_none()))
        .cloned()
        .collect();
    main.sort_by(|a, b| a.turn.cmp(&b.turn));
    lines.push(WorldlineLine {
        id: "main".into(),
        fork_from_save_id: None,
        saves: main,
    });

    // 分叉线：按 fork_from_save_id 分组
    let mut fork_map: std::collections::BTreeMap<String, Vec<TavernSaveMeta>> =
        std::collections::BTreeMap::new();
    for s in saves.iter().filter(|s| s.worldline_id != "main") {
        let key = s.worldline_id.clone();
        fork_map.entry(key).or_default().push(s.clone());
    }
    for (wid, mut svs) in fork_map {
        svs.sort_by(|a, b| a.turn.cmp(&b.turn));
        let fork_from = svs.iter().find_map(|s| s.fork_from_save_id.clone());
        lines.push(WorldlineLine {
            id: wid,
            fork_from_save_id: fork_from,
            saves: svs,
        });
    }

    let current_worldline_id = session
        .current_worldline_id
        .clone()
        .unwrap_or_else(|| "main".into());
    let current_save_id = saves
        .iter()
        .filter(|s| s.worldline_id == current_worldline_id)
        .max_by(|a, b| a.created_at.cmp(&b.created_at))
        .map(|s| s.save_id.clone());

    WorldlineView {
        lines,
        current_worldline_id,
        current_save_id,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TavernSaveMeta {
    pub save_id: String,
    pub session_id: String,
    pub label: String,
    pub turn: u32,
    pub node_id: Option<String>,
    pub chapter_cursor: Option<String>,
    pub play_mode: PlayMode,
    pub created_at: String,
    /// 分叉源存档 id（主线存档无）。
    #[serde(default)]
    pub fork_from_save_id: Option<String>,
    /// 所属世界线 id（"main" 或分叉线 id）。
    #[serde(default)]
    pub worldline_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TavernSave {
    pub save_id: String,
    pub session_id: String,
    pub label: String,
    pub turn: u32,
    pub created_at: String,
    /// 分叉源存档 id（主线存档无）。
    #[serde(default)]
    pub fork_from_save_id: Option<String>,
    /// 所属世界线 id。
    #[serde(default)]
    pub worldline_id: String,
    pub snapshot: TavernSession,
}

impl TavernSessionStore {
    fn saves_root(&self) -> PathBuf {
        self.data.tavern_saves_dir()
    }

    fn saves_dir_for(&self, session_id: &str) -> CoreResult<PathBuf> {
        let id = safe_id(session_id)?;
        Ok(self.saves_root().join(id))
    }

    fn save_path(&self, session_id: &str, save_id: &str) -> CoreResult<PathBuf> {
        let sid = safe_id(session_id)?;
        let sav = safe_id(save_id)?;
        if !sav.starts_with("save-") {
            return Err(CoreError::BadRequest("save id must start with save-".into()));
        }
        Ok(self.saves_root().join(sid).join(format!("{sav}.json")))
    }

    pub fn list_saves(&self, session_id: &str) -> CoreResult<Vec<TavernSaveMeta>> {
        let _g = self.lock.lock();
        let dir = self.saves_dir_for(session_id)?;
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for ent in fs::read_dir(&dir)? {
            let ent = ent?;
            let name = ent.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            match fs::read_to_string(ent.path()) {
                Ok(raw) => {
                    if let Ok(s) = serde_json::from_str::<TavernSave>(&raw) {
                        out.push(TavernSaveMeta {
                            save_id: s.save_id,
                            session_id: s.session_id,
                            label: s.label,
                            turn: s.turn,
                            node_id: s.snapshot.node_id.clone(),
                            chapter_cursor: s.snapshot.chapter_cursor.clone(),
                            play_mode: s.snapshot.play_mode,
                            created_at: s.created_at,
                            fork_from_save_id: s.fork_from_save_id,
                            worldline_id: s.worldline_id,
                        });
                    }
                }
                Err(_) => continue,
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    }

    /// 世界线视图（吸收自 Liyuan worldline.ts）：按分叉组织存档线。
    /// 注意：不能持锁调 list_saves（其内部会再次 lock，重入死锁）。
    pub fn worldline(&self, session_id: &str) -> CoreResult<WorldlineView> {
        let sess = {
            let _g = self.lock.lock();
            let path_sess = self.path_for(session_id)?;
            if !path_sess.exists() {
                return Err(CoreError::NotFound(format!("session not found: {session_id}")));
            }
            serde_json::from_str::<TavernSession>(&fs::read_to_string(&path_sess)?)?
        };
        let saves = self.list_saves(session_id)?;
        Ok(build_worldline(&sess, &saves))
    }

    pub fn create_save(&self, session_id: &str, label: Option<String>) -> CoreResult<TavernSave> {
        let _g = self.lock.lock();
        let path_sess = self.path_for(session_id)?;
        if !path_sess.exists() {
            return Err(CoreError::NotFound(format!("session not found: {session_id}")));
        }
        let mut sess: TavernSession = serde_json::from_str(&fs::read_to_string(&path_sess)?)?;
        let save_id = format!("save-{}", Uuid::new_v4());
        let now = now_rfc3339();
        let label = label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("第{}回合", sess.turn));

        // 世界线分叉检测（吸收自 Liyuan worldline.ts）：
        // 回档后走出不同路（turn 前进）再存档 → 产生新世界线；同回合立即存 → 不算分叉。
        let restored_marker = sess.last_restored_save_id.clone();
        let (fork_from_save_id, worldline_id) =
            if let Some(restored_id) = restored_marker {
            let restored_turn = fs::read_to_string(self.save_path(session_id, &restored_id)?)
                .ok()
                .and_then(|raw| serde_json::from_str::<TavernSave>(&raw).ok())
                .map(|s| s.turn)
                .unwrap_or(u32::MAX);
            if restored_turn < sess.turn {
                let new_line = format!("wl-{}", Uuid::new_v4());
                sess.last_restored_save_id = None; // 一次回档 → 一次分叉
                (Some(restored_id.clone()), new_line)
            } else {
                (None, sess.current_worldline_id.clone().unwrap_or_else(|| "main".into()))
            }
        } else {
            (None, sess.current_worldline_id.clone().unwrap_or_else(|| "main".into()))
        };

        let mut snapshot = sess;
        snapshot.active_run_id = None;
        let save = TavernSave {
            save_id: save_id.clone(),
            session_id: session_id.to_string(),
            label,
            turn: snapshot.turn,
            created_at: now,
            fork_from_save_id,
            worldline_id: worldline_id.clone(),
            snapshot,
        };
        let path = self.save_path(session_id, &save_id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, &serde_json::to_string_pretty(&save)?)?;

        // 同步回写会话：当前世界线推进（仅分叉时），并清除已消费的回档标记
        if save.fork_from_save_id.is_some() {
            if let Ok(mut live) = serde_json::from_str::<TavernSession>(&fs::read_to_string(&path_sess)?) {
                live.current_worldline_id = Some(worldline_id);
                live.last_restored_save_id = None;
                let _ = write_atomic(&path_sess, &serde_json::to_string_pretty(&live)?);
            }
        }
        Ok(save)
    }

    pub fn get_save(&self, session_id: &str, save_id: &str) -> CoreResult<TavernSave> {
        let _g = self.lock.lock();
        let path = self.save_path(session_id, save_id)?;
        if !path.exists() {
            return Err(CoreError::NotFound(format!("save not found: {save_id}")));
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn delete_save(&self, session_id: &str, save_id: &str) -> CoreResult<()> {
        let _g = self.lock.lock();
        let path = self.save_path(session_id, save_id)?;
        if !path.exists() {
            return Err(CoreError::NotFound(format!("save not found: {save_id}")));
        }
        fs::remove_file(path)?;
        Ok(())
    }

    /// Restore a save into the live session (same sessionId). Keeps sessionId; clears active run.
    /// Fork a save into a NEW session (cross-session branch, Front Porch forkFromMessage parity).
    /// New session_id, same pack, snapshot restored, parent linked via fork_from_save_id lineage.
    /// Journal/Growth copySessionTo semantics: receipts >= cursor stay behind — here cursor = snapshot turn's message len.
    pub fn fork_save_to_session(&self, session_id: &str, save_id: &str, label: Option<String>) -> CoreResult<TavernSession> {
        let _g = self.lock.lock();
        let path_save = self.save_path(session_id, save_id)?;
        if !path_save.exists() {
            return Err(CoreError::NotFound(format!("save not found: {save_id}")));
        }
        let save: TavernSave = serde_json::from_str(&fs::read_to_string(&path_save)?)?;
        let mut snap = save.snapshot.clone();
        let new_id = format!("tavern-session-{}", Uuid::new_v4());
        snap.session_id = new_id.clone();
        snap.active_run_id = None;
        snap.checkpoints = vec![];
        snap.turn_progress = None;
        // lineage: new worldline forked from this save
        snap.last_restored_save_id = None;
        snap.current_worldline_id = Some(format!("wl-{}", Uuid::new_v4()));
        let now = now_rfc3339();
        snap.created_at = now.clone();
        snap.updated_at = now;
        let flabel = label.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| format!("分叉自「{}」", save.label));
        snap.messages.push(TavernMessage {
            id: format!("msg-{}", Uuid::new_v4()),
            role: "assistant".into(),
            content: format!("〔分叉〕新分支「{flabel}」（源存档「{}」第{}回合）。旧会话不受影响。", save.label, save.turn),
            created_at: now_rfc3339(),
            options: vec![],
            swipes: vec![],
            swipe_index: 0,
            engine_tag: None,
            program: None,
            reasoning: None,
            tokens: 0,
        });
        let _ = safe_id(&new_id)?;
        fs::create_dir_all(self.dir())?;
        let path = self.path_for(&new_id)?;
        write_atomic(&path, &serde_json::to_string_pretty(&snap)?)?;
        // record fork lineage as a save in the NEW session pointing back
        let fork_save = TavernSave {
            save_id: format!("save-{}", Uuid::new_v4()),
            session_id: new_id.clone(),
            label: format!("分叉点（源 {}/{})", session_id, save_id),
            turn: snap.turn,
            created_at: now_rfc3339(),
            fork_from_save_id: Some(save_id.to_string()),
            worldline_id: snap.current_worldline_id.clone().unwrap_or_else(|| "main".into()),
            snapshot: snap.clone(),
        };
        let fpath = self.save_path(&new_id, &fork_save.save_id)?;
        if let Some(parent) = fpath.parent() { let _ = fs::create_dir_all(parent); }
        let _ = write_atomic(&fpath, &serde_json::to_string_pretty(&fork_save)?);
        Ok(snap)
    }

    pub fn restore_save(&self, session_id: &str, save_id: &str) -> CoreResult<TavernSession> {
        let _g = self.lock.lock();
        let path_save = self.save_path(session_id, save_id)?;
        if !path_save.exists() {
            return Err(CoreError::NotFound(format!("save not found: {save_id}")));
        }
        let save: TavernSave = serde_json::from_str(&fs::read_to_string(&path_save)?)?;
        let path_sess = self.path_for(session_id)?;
        if !path_sess.exists() {
            return Err(CoreError::NotFound(format!("session not found: {session_id}")));
        }
        let live: TavernSession = serde_json::from_str(&fs::read_to_string(&path_sess)?)?;
        if live.active_run_id.is_some() {
            return Err(CoreError::BadRequest(
                "turn in progress; stop or wait before restore".into(),
            ));
        }
        let mut snap = save.snapshot;
        snap.session_id = live.session_id.clone();
        snap.active_run_id = None;
        // 世界线（吸收自 Liyuan worldline.ts）：记录回档来源 + 切回该存档所在世界线
        snap.last_restored_save_id = Some(save_id.to_string());
        snap.current_worldline_id = Some(save.worldline_id.clone());
        let note = format!(
            "〔回档〕已恢复存档「{}」（第{}回合 · 节点 {}）。",
            save.label,
            save.turn,
            snap.node_id.as_deref().unwrap_or("?")
        );
        snap.messages.push(TavernMessage {
            id: format!("msg-{}", Uuid::new_v4()),
            role: "assistant".into(),
            content: note,
            created_at: now_rfc3339(),
            options: vec![],
            engine_tag: None,
            program: None,
            reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
        });
        // inline save under same lock
        if snap.session_id.trim().is_empty() {
            snap.session_id = format!("tavern-session-{}", Uuid::new_v4());
        }
        let _ = safe_id(&snap.session_id)?;
        let now = now_rfc3339();
        if snap.created_at.is_empty() {
            snap.created_at = now.clone();
        }
        snap.updated_at = now;
        fs::create_dir_all(self.dir())?;
        let path = self.path_for(&snap.session_id)?;
        write_atomic(&path, &serde_json::to_string_pretty(&snap)?)?;
        Ok(snap)
    }
}

#[derive(Clone)]
pub struct TavernPersonaStore {
    data: DataRoot,
    lock: std::sync::Arc<Mutex<()>>,
}

impl TavernPersonaStore {
    pub fn new(data: DataRoot) -> Self {
        let _ = data.ensure_layout();
        Self {
            data,
            lock: std::sync::Arc::new(Mutex::new(())),
        }
    }

    fn path_for(&self, character_id: &str) -> CoreResult<PathBuf> {
        let id = safe_id(character_id)?;
        Ok(self.data.tavern_persona_dir().join(format!("{id}.json")))
    }

    pub fn get(&self, character_id: &str) -> CoreResult<TavernPersona> {
        let _g = self.lock.lock();
        let path = self.path_for(character_id)?;
        if !path.exists() {
            return Ok(TavernPersona {
                character_id: character_id.to_string(),
                ..Default::default()
            });
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, mut persona: TavernPersona) -> CoreResult<TavernPersona> {
        let _g = self.lock.lock();
        if persona.character_id.trim().is_empty() {
            return Err(CoreError::BadRequest("characterId required".into()));
        }
        let id = safe_id(&persona.character_id)?;
        persona.character_id = id;
        persona.updated_at = now_rfc3339();
        fs::create_dir_all(self.data.tavern_persona_dir())?;
        let path = self.path_for(&persona.character_id)?;
        write_atomic(&path, &serde_json::to_string_pretty(&persona)?)?;
        Ok(persona)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────


/// Rotate focus among present characters. Returns new focus id.
pub fn rotate_focus_character(session: &mut TavernSession) -> Option<String> {
    if !session.speaker_rotation {
        return session.focus_character_id.clone();
    }
    let candidates = session.present_character_ids.clone();
    if candidates.is_empty() {
        return session.focus_character_id.clone();
    }
    let cur = session.focus_character_id.clone();
    let idx = cur
        .as_ref()
        .and_then(|c| candidates.iter().position(|x| x == c))
        .unwrap_or(0);
    let next = candidates[(idx + 1) % candidates.len()].clone();
    session.focus_character_id = Some(next.clone());
    Some(next)
}

pub fn ensure_focus_character(session: &mut TavernSession) {
    if session.focus_character_id.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
        // if focus not in present, reset
        if let Some(f) = &session.focus_character_id {
            if !session.present_character_ids.is_empty() && !session.present_character_ids.contains(f) {
                session.focus_character_id = session.present_character_ids.first().cloned();
            }
        }
        return;
    }
    session.focus_character_id = session
        .present_character_ids
        .first()
        .cloned()
        .or(session.player.control_character_id.clone());
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn clean_cast_name_keeps_xiang_surname() {
        // [fix 2026-08-16] 男主「向明初」以「向」为姓，曾被 non_name_starts 黑名单误杀，
        // 导致蒸馏成功的男主卡在书架/角色列表不可见。
        assert!(is_clean_cast_name("向明初"));
        assert!(is_clean_cast_name("向华强"));
        // 2 字介词短语仍应被拒（「向」+方位/人称）
        assert!(!is_clean_cast_name("向他"));
        assert!(!is_clean_cast_name("向前"));
        assert!(!is_clean_cast_name("向外"));
    }

    #[test]
    fn clean_cast_name_keeps_regular_names() {
        assert!(is_clean_cast_name("庄眉"));
        assert!(is_clean_cast_name("山楂"));
        assert!(is_clean_cast_name("莫旺财"));
        assert!(is_clean_cast_name("宿舍阿姨"));
    }

    mod tempfile_shim {
        use super::*;
        pub struct Tmp {
            path: PathBuf,
        }
        impl Tmp {
            pub fn new() -> Self {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir().join(format!("kaleido-tavern-test-{nanos}"));
                fs::create_dir_all(&path).unwrap();
                Self { path }
            }
            pub fn path(&self) -> &Path {
                &self.path
            }
        }
        impl Drop for Tmp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn tier_min3() {
        assert_eq!(
            ContentTier::min3(ContentTier::Open, ContentTier::Standard, ContentTier::Open),
            ContentTier::Standard
        );
        assert_eq!(
            ContentTier::min3(ContentTier::Safe, ContentTier::Open, ContentTier::Open),
            ContentTier::Safe
        );
    }

    #[test]
    fn b1_regression_turn_save_after_acquire_persists_turn_and_messages() {
        // B1 (P0, 2026-08-26): tavern turn succeeded 但 session.turns=0。
        // 生产序列（story_tavern start_turn → worker 尾部落盘）：
        //   acquire_turn（锁写 activeRunId）→ save_with_revision(用户消息) →
        //   worker: get() → push(asst)+turn+=1 → save()【无 base_revision 的普通保存】
        // 实测该序列在 04:45 生产环境把回合弄丢（turn 回 0 / messages 回 1），
        // 本测试固化该最小复现：普通 save 绝不允许回滚已持久化的 turn/messages。
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let packs = PackStore::new(data.clone());
        let sessions = TavernSessionStore::new(data.clone());

        let pack = packs.ensure_demo_pack().unwrap();
        let mut sess = sessions
            .create_from_pack(
                &packs,
                CreateSessionRequest {
                    pack_id: pack.id.clone(),
                    playable: Playable::P3,
                    play_mode: PlayMode::Mainline,
                    quality: Quality::Lite,
                    user_tier: ContentTier::Open,
                    global_tier: Some(ContentTier::Open),
                    author_project_id: None,
                    work_id: None,
                    entry: None,
                    player_display_name: Some("b1".into()),
                    adult_confirmed: true,
                    title: None,
                    owner: None,
                },
            )
            .unwrap();
        // create_from_pack 不 seed 开场（生产由 GET auto-seed / ensure-opening 负责）；
        // 本测试自 seed 一条开场 assistant 消息以对齐生产基线（messages=1）。
        sess.messages.push(TavernMessage {
            id: "msg-b1-opening".into(),
            role: "assistant".into(),
            content: "【开场】雨夜，玩家抵达旧茶馆门前。".into(),
            created_at: "2026-08-26T04:40:00Z".into(),
            options: vec![],
            swipes: vec![],
            swipe_index: 0,
            engine_tag: None,
            program: None,
            reasoning: None,
            tokens: 0,
        });
        sess.opening_seeded = true;
        sessions.save(sess.clone()).unwrap();

        // 1) handler: 原子取锁（activeRunId=run）
        let run_id = "b1-run-1";
        sess = sessions.acquire_turn(&sess.session_id, run_id).unwrap();
        assert_eq!(sess.active_run_id.as_deref(), Some(run_id));

        // 2) handler: CAS 写入用户消息（save_with_revision, base=acquire 返回的 updated_at）
        sess.messages.push(TavernMessage {
            id: "msg-b1-user".into(),
            role: "user".into(),
            content: "请直接回复两个字：好的".into(),
            created_at: "2026-08-26T04:45:18Z".into(),
            options: vec![],
            swipes: vec![],
            swipe_index: 0,
            engine_tag: None,
            program: None,
            reasoning: None,
            tokens: 0,
        });
        sess.active_run_id = Some(run_id.to_string());
        let base_rev = sess.updated_at.clone();
        sessions
            .save_with_revision(sess.clone(), &base_rev)
            .expect("user-msg CAS save must succeed");

        // 3) worker 尾部: get() → push assistant + turn+=1 + 清锁 → 普通 save()
        let mut disk = sessions.get(&sess.session_id).unwrap();
        disk.messages.push(TavernMessage {
            id: "msg-b1-asst".into(),
            role: "assistant".into(),
            content: "好的".into(),
            created_at: "2026-08-26T04:45:51Z".into(),
            options: vec![],
            swipes: vec![],
            swipe_index: 0,
            engine_tag: None,
            program: None,
            reasoning: None,
            tokens: 0,
        });
        disk.turn += 1;
        disk.active_run_id = None;
        sessions.save(disk.clone()).expect("worker final save must succeed");

        // 4) 断言落盘终态 —— B1 症状即此处失败（turn=0 / messages 缺失）
        let after = sessions.get(&sess.session_id).unwrap();
        assert_eq!(after.turn, 1, "turn must persist after successful turn");
        assert_eq!(after.messages.len(), 4, "opening+user+assistant(+1) must persist");
        assert!(after.messages.iter().any(|m| m.role == "user" && m.content == "请直接回复两个字：好的"));
        assert_eq!(
            after.messages.last().map(|m| (m.role.as_str(), m.content.as_str())),
            Some(("assistant", "好的"))
        );
        assert_eq!(after.active_run_id, None);
    }

    #[test]
    fn demo_pack_playable_and_session() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let packs = PackStore::new(data.clone());
        let sessions = TavernSessionStore::new(data.clone());

        let pack = packs.ensure_demo_pack().unwrap();
        assert_eq!(pack.id, "demo-rain-alley");
        assert!(pack.is_playable());
        assert_eq!(pack.chapters.len(), 2);
        assert!(packs
            .read_chapter_body(&pack.id, "chapters/ch01.md")
            .unwrap()
            .contains("沈棠"));

        let sess = sessions
            .create_from_pack(
                &packs,
                CreateSessionRequest {
                    pack_id: pack.id.clone(),
                    playable: Playable::P3,
                    play_mode: PlayMode::Mainline,
                    quality: Quality::Lite,
                    user_tier: ContentTier::Open,
                    global_tier: Some(ContentTier::Open),
                    author_project_id: None,
                    work_id: None,
                    entry: Some(EntryConfig {
                        entry_role: Some(EntryRole::Supporting),
                        vessel_character_id: Some("cc-linwan".into()),
                        meta_knowledge: MetaKnowledge::Reader,
                        rewrite_intensity: RewriteIntensity::Canon,
                        isekai: None,
                        extra_profile: None,
                    }),
                    player_display_name: Some("测试者".into()),
                    adult_confirmed: true,
                    title: None,
                    owner: None,
                },
            )
            .unwrap();
        assert!(sess.session_id.starts_with("tavern-session-"));
        assert_eq!(sess.content_tier, ContentTier::Standard); // pack max standard
        assert_eq!(sess.node_id.as_deref(), Some("n1"));

        let loaded = sessions.get(&sess.session_id).unwrap();
        assert_eq!(loaded.pack_id, "demo-rain-alley");

        packs.delete(&pack.id).unwrap();
        let n = sessions.mark_pack_missing("demo-rain-alley").unwrap();
        assert!(n >= 1);
        let after = sessions.get(&sess.session_id).unwrap();
        assert!(after.pack_missing);
    }

    #[test]
    fn u13_create_session_auto_mounts_work_compass() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let packs = PackStore::new(data.clone());
        let sessions = TavernSessionStore::new(data.clone());
        let compass_store = CompassStore::new(data.clone());

        let pack = packs.ensure_demo_pack().unwrap();
        let work = "work-u13-demo";
        // 1) 目标 work 已存在创作罗盘（等价于前端先设置罗盘再开新会话）
        compass_store
            .save(
                work,
                &Compass::new("承诺：雨巷的秘密", "近期目标：找到沈棠的信"),
            )
            .unwrap();
        // 2) 开新 session 时带 work_id → 自动挂载罗盘
        let sess = sessions
            .create_from_pack(
                &packs,
                CreateSessionRequest {
                    pack_id: pack.id.clone(),
                    playable: Playable::P3,
                    play_mode: PlayMode::Mainline,
                    quality: Quality::Lite,
                    user_tier: ContentTier::Open,
                    global_tier: Some(ContentTier::Open),
                    author_project_id: None,
                    work_id: Some(work.to_string()),
                    entry: Some(EntryConfig {
                        entry_role: Some(EntryRole::Supporting),
                        vessel_character_id: Some("cc-linwan".into()),
                        meta_knowledge: MetaKnowledge::Reader,
                        rewrite_intensity: RewriteIntensity::Canon,
                        isekai: None,
                        extra_profile: None,
                    }),
                    player_display_name: Some("测试者".into()),
                    adult_confirmed: true,
                    title: None,
                    owner: None,
                },
            )
            .unwrap();
        assert_eq!(
            sess.actor_states.compass().author_intent,
            "承诺：雨巷的秘密"
        );
        assert_eq!(
            sess.actor_states.compass().current_focus,
            "近期目标：找到沈棠的信"
        );
        // 落盘后 reload 仍挂载（持久化正确）
        let loaded = sessions.get(&sess.session_id).unwrap();
        assert!(!loaded.actor_states.compass().is_empty());
        // 3) 不带 work_id → 无手写罗盘 → 自动派生首章目标（自动罗盘行为）
        let sess2 = sessions
            .create_from_pack(
                &packs,
                CreateSessionRequest {
                    pack_id: pack.id.clone(),
                    playable: Playable::P3,
                    play_mode: PlayMode::Mainline,
                    quality: Quality::Lite,
                    user_tier: ContentTier::Open,
                    global_tier: Some(ContentTier::Open),
                    author_project_id: None,
                    work_id: None,
                    entry: None,
                    player_display_name: Some("无罗盘".into()),
                    adult_confirmed: true,
                    title: None,
                    owner: None,
                },
            )
            .unwrap();
        // 自动罗盘：current_focus 派生自首章 goals（若 pack 首章无 goals 则仍空）
        let cf = sess2.actor_states.compass().current_focus.clone();
        assert!(sess2.actor_states.compass().author_intent.is_empty(), "author_intent 保持空（不编造承诺）");
        let _ = cf;
    }

    #[test]
    fn persona_roundtrip() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let store = TavernPersonaStore::new(data);
        let p = store
            .save(TavernPersona {
                character_id: "cc-linwan".into(),
                display_name: "林晚".into(),
                memory_l4: MemoryL4 {
                    affinity: json!({"cc-shentang": 55}),
                    secrets_known: vec!["有人跟踪".into()],
                    promises: vec![],
                    relationships: json!({}),
                },
                notes: "跨会话好感".into(),
                updated_at: String::new(),
            })
            .unwrap();
        let g = store.get("cc-linwan").unwrap();
        assert_eq!(g.display_name, "林晚");
        assert_eq!(p.character_id, g.character_id);
    }

    pub(crate) fn minimal_pack() -> StoryPack {
        StoryPack {
            id: String::new(),
            title: "CAS 测试".into(),
            source: PackSource::default(),
            characters: vec![],
            world_book_ids: vec![],
            chapters: vec![],
            nodes: vec![],
            lore_entries: vec![],
            default_mode: PlayMode::Mainline,
            max_tier: ContentTier::Standard,
            language: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            stage_director: StageDirectorConfig::default(),
            event_packages: vec![],
            actor_state_config: ActorStatePackConfig::default(),
            worldline: vec![],
        }
    }

    pub(crate) fn minimal_session() -> TavernSession {
        TavernSession {
            session_id: String::new(),
            pack_id: "pack-cas-test".into(),
            pack_missing: false,
            owner: None,
            quality: Quality::Lite,
            playable: Playable::P1,
            play_mode: PlayMode::Mainline,
            content_tier: ContentTier::Standard,
            user_tier_request: ContentTier::Standard,
            entry: EntryConfig::default(),
            chapter_cursor: None,
            node_id: None,
            resume_node_id: None,
            opening_seeded: false,
            side_branch_node_id: None,
            side_branch_label: None,
            current_worldline_id: None,
            last_restored_save_id: None,
            panels: vec![],
            mcp_tool_results: vec![],
            skill_load: None,
            timeline_id: "main".into(),
            turn: 0,
            present_character_ids: vec![],
            focus_character_id: None,
            speaker_rotation: false,
            player: PlayerState::default(),
            memory_l1: MemoryL1::default(),
            memory_l2: MemoryL2::default(),
            memory_l3: MemoryL3::default(),
            memory_l4: MemoryL4::default(),
            guard_events: vec![],
            messages: vec![],
            active_run_id: None,
            adult_confirmed: true,
            title: "CAS 测试".into(),
            created_at: String::new(),
            updated_at: String::new(),
            author_project_id: None,
            author_live_path: None,
            author_live_enabled: true,
            author_live_every_n: 1,
            author_live_write_turns: false,
            actor_states: ActorStateSystem::default(),
            director_plan: None,
            director_pending: false,
            director_task: None,
            last_event: None,
            last_check_results: vec![],
        check_history: vec![],
            checkpoints: vec![],
            fold_through_turn: 0,
            // U11: 测试构造显式初始化新字段。
            epoch: 0,
            epoch_last_turn: None,
            epoch_last_chars: None,
            turn_cost_ledger: TurnCostLedger::default(),
            last_turn_diagnostic: None,
            scribe_whispers: Default::default(),
            skim_issues: Vec::new(),
            skim_sample: String::new(),
            chapter_diaries: Vec::new(),
            turn_progress: None,
            diary_config: None,
            pockets: Default::default(),
            pockets_enabled: true,
            needs: Default::default(),
            growth: Default::default(),
            world_climate: Default::default(),
            chaos: Default::default(),
            milestones: Default::default(),
            objectives: Default::default(),
            ambitions: Default::default(),
            dream: Default::default(),
            episodes: Default::default(),
            journal: Default::default(),
            relationships: Default::default(),
            pending_swipes: Default::default(),
            promises: Default::default(),
            preferences: Default::default(),
            presence: Default::default(),
            timed_world_info: Default::default(),
            event_extract: true,
            world: WorldState::default(),
            game_clock: Default::default(),
        }
    }

    #[test]
    fn pack_save_with_revision_ok() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let packs = PackStore::new(data);

        let saved = packs.save(minimal_pack()).unwrap();
        assert!(!saved.updated_at.is_empty());
        assert!(!saved.id.is_empty());

        let again = packs
            .save_with_revision(saved.clone(), &saved.updated_at)
            .unwrap();
        assert_eq!(again.id, saved.id);
        assert!(!again.updated_at.is_empty());
    }

    #[test]
    fn pack_save_with_revision_conflict() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let packs = PackStore::new(data);

        let saved = packs.save(minimal_pack()).unwrap();
        let wrong = format!("{}bogus", saved.updated_at);
        let err = packs.save_with_revision(saved.clone(), &wrong).unwrap_err();
        assert!(
            matches!(err, CoreError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );

        // Disk content must be untouched by the failed write.
        let on_disk = packs.load_unlocked(&saved.id).unwrap();
        assert_eq!(on_disk.updated_at, saved.updated_at);
        assert_eq!(on_disk.title, saved.title);
    }

    #[test]
    fn pack_save_without_revision_compat() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let packs = PackStore::new(data);

        let first = packs.save(minimal_pack()).unwrap();
        let reloaded = packs.load_unlocked(&first.id).unwrap();
        let second = packs.save(reloaded).unwrap();
        assert!(!first.updated_at.is_empty());
        assert!(!second.updated_at.is_empty());
        assert_eq!(second.id, first.id);
    }

    #[test]
    fn pack_save_with_revision_missing_file() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let packs = PackStore::new(data);

        let mut p = minimal_pack();
        p.id = "pack-cas-missing".into();
        let err = packs
            .save_with_revision(p, "2026-08-01T00:00:00Z")
            .unwrap_err();
        assert!(
            matches!(err, CoreError::Conflict(_)),
            "expected Conflict for missing file, got {err:?}"
        );
    }

    #[test]
    fn session_save_with_revision_ok_and_conflict() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let sessions = TavernSessionStore::new(data);

        let saved = sessions.save(minimal_session()).unwrap();
        assert!(!saved.updated_at.is_empty());

        let ok = sessions
            .save_with_revision(saved.clone(), &saved.updated_at)
            .unwrap();
        assert_eq!(ok.session_id, saved.session_id);

        let wrong = format!("{}bogus", saved.updated_at);
        let err = sessions.save_with_revision(saved, &wrong).unwrap_err();
        assert!(matches!(err, CoreError::Conflict(_)));
    }

    #[test]
    fn checkpoint_push_restore_rewinds_messages_and_fields() {
        let mut s = minimal_session();
        fn msg(role: &str, content: &str) -> TavernMessage {
            TavernMessage {
                id: format!("msg-{}", role),
                role: role.into(),
                content: content.into(),
                created_at: String::new(),
                options: vec![],
                engine_tag: None,
                program: None,
                reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
            }
        }

        // Turn 1 completion
        s.messages.push(msg("user", "你好"));
        s.messages.push(msg("assistant", "嗨"));
        s.turn = 1;
        s.memory_l1.scene_summary = "scene-1".into();
        s.push_checkpoint();
        assert_eq!(s.checkpoints.len(), 1);

        // Turn 2 completion (memory + node mutated)
        s.messages.push(msg("user", "继续"));
        s.messages.push(msg("assistant", "好的"));
        s.turn = 2;
        s.node_id = Some("n2".into());
        s.memory_l1.scene_summary = "scene-2".into();
        s.push_checkpoint();
        assert_eq!(s.checkpoints.len(), 2);

        // Rewind 1 → back to end of turn 1
        let rewound = s.restore_checkpoint(1).unwrap();
        assert_eq!(rewound, 1);
        assert_eq!(s.turn, 1);
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.messages.last().unwrap().content, "嗨");
        assert_eq!(s.node_id.as_deref(), None);
        assert_eq!(s.memory_l1.scene_summary, "scene-1");
        assert_eq!(s.checkpoints.len(), 1);

        // Rewind beyond available clamps to oldest checkpoint (only 1 left → no-op)
        let rewound = s.restore_checkpoint(5).unwrap();
        assert_eq!(rewound, 0);
        assert_eq!(s.turn, 1);

        // steps=0 / empty are no-ops
        assert_eq!(s.restore_checkpoint(0).unwrap(), 0);
        s.checkpoints.clear();
        assert_eq!(s.restore_checkpoint(1).unwrap(), 0);
    }
}

