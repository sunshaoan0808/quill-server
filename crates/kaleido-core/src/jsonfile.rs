//! 统一 JSON 读写：原子写（tmp + rename）+ 写前备份。
//!
//! 背景：仓库里曾有三份语义**不一致**的 `write_atomic`：
//! - `story_tavern.rs`  —— 剔除 U+FFFD 替换符，tmp 后缀 `tmp`
//! - `st_review.rs`      —— 不剔除，tmp 后缀 `tmp`
//! - `st_vector_index.rs`—— 不剔除，tmp 后缀 `json.tmp`，且无 `sync_all`
//!
//! 本模块不强行抹平这些差异（那会改变落盘字节），而是把它们显式建模为
//! [`Sanitize`] / [`TmpExt`] 两个策略参数，由调用点按原语义传参。
//!
//! 命名沿用既有约定：`path.with_extension("json.bak")`（与 `ledger.rs`
//! 反序列化失败时的备份路径一致），即 `pack.json` → `pack.json.bak`。

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// 落盘前的字符清洗策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sanitize {
    /// 原样落盘。
    None,
    /// 剔除 U+FFFD 替换符。
    ///
    /// 上游 LLM 偶发把单字损坏成 2-3 个 FFFD，该字符是坏字残骸、无恢复语义，
    /// 落盘前一律剔除，避免脏字符进入任何持久化文件。
    StripFffd,
}

impl Sanitize {
    fn apply(self, body: &str) -> String {
        match self {
            Sanitize::None => body.to_string(),
            Sanitize::StripFffd => body.replace('\u{FFFD}', ""),
        }
    }
}

/// 临时文件后缀策略。
///
/// 用 `Path::with_extension` 构造，因此**必须**沿用各调用点原有的后缀，
/// 否则崩溃残留物的路径会变（可能有巡检/清理逻辑依赖）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmpExt {
    /// `pack.json` → `pack.tmp`
    Tmp,
    /// `index.json` → `index.json.tmp`
    JsonTmp,
}

impl TmpExt {
    fn path(self, path: &Path) -> std::path::PathBuf {
        match self {
            TmpExt::Tmp => path.with_extension("tmp"),
            TmpExt::JsonTmp => path.with_extension("json.tmp"),
        }
    }
}

/// 临时文件命名策略。
///
/// [`TmpNaming::Fixed`] 是历史行为：tmp 路径是目标的确定性变换（`pack.json` →
/// `pack.tmp`），同进程内**两个并发写同一目标会撞名**（A 写完 tmp、B 覆盖
/// tmp、A rename 走 B 的内容）。旧调用点靠各自的锁（如向量索引的
/// `index_write_lock`）串行化，公共 API 本身不提供保护。
///
/// [`TmpNaming::Unique`] 给 tmp 加进程内唯一后缀（pid + 单调计数器），
/// 让并发写互不干扰：每个写者只认自己的 tmp，rename 仍是最后一步原子发布。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmpNaming {
    /// 固定名（历史语义，崩溃残留物路径稳定）。
    Fixed,
    /// 进程内唯一名（`pack.tmp.<pid>.<n>`），并发写不撞名。
    Unique,
}

/// 进程内临时名计数器（与 pid 组合保证同进程唯一）。
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 构造 tmp 路径。
fn tmp_path(path: &Path, tmp_ext: TmpExt, naming: TmpNaming) -> PathBuf {
    let base = tmp_ext.path(path);
    match naming {
        TmpNaming::Fixed => base,
        TmpNaming::Unique => {
            let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let file_name = base
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "tmp".to_string());
            base.with_file_name(format!("{file_name}.{}.{n}", std::process::id()))
        }
    }
}

/// 落盘后端策略：是否 `sync_all`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// `File::create` + `write_all` + `sync_all`（崩溃后内容不丢）。
    Synced,
    /// `fs::write`（不 fsync，崩溃后可能拿到空/半截文件）。
    Unsynced,
}

/// JSON 文件读写错误。`Display` 始终带路径上下文。
#[derive(Debug)]
pub enum JsonFileError {
    Read { path: String, source: std::io::Error },
    Parse { path: String, source: serde_json::Error },
    Serialize { path: String, source: serde_json::Error },
    WriteTmp { path: String, source: std::io::Error },
    Backup { path: String, source: std::io::Error },
    Rename { path: String, source: std::io::Error },
    MissingBackup { path: String },
}

impl fmt::Display for JsonFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsonFileError::Read { path, source } => write!(f, "读取 JSON 失败 {path}: {source}"),
            JsonFileError::Parse { path, source } => {
                write!(f, "解析 JSON 失败 {path}: {source}")
            }
            JsonFileError::Serialize { path, source } => {
                write!(f, "序列化 JSON 失败 {path}: {source}")
            }
            JsonFileError::WriteTmp { path, source } => {
                write!(f, "写临时文件失败 {path}: {source}")
            }
            JsonFileError::Backup { path, source } => write!(f, "写备份失败 {path}: {source}"),
            JsonFileError::Rename { path, source } => write!(f, "重命名失败 {path}: {source}"),
            JsonFileError::MissingBackup { path } => write!(f, "备份不存在，无法恢复 {path}"),
        }
    }
}

impl std::error::Error for JsonFileError {}

impl From<JsonFileError> for crate::CoreError {
    fn from(e: JsonFileError) -> Self {
        match e {
            JsonFileError::Parse { source, .. } | JsonFileError::Serialize { source, .. } => {
                crate::CoreError::Json(source)
            }
            other => crate::CoreError::BadRequest(other.to_string()),
        }
    }
}

/// 读并反序列化 JSON 文件。
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, JsonFileError> {
    let raw = fs::read_to_string(path).map_err(|source| JsonFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(|source| JsonFileError::Parse {
        path: path.display().to_string(),
        source,
    })
}

/// 原子写：先写 tmp，再 `rename` 覆盖目标（POSIX 下读者只见旧或新完整文件）。
pub fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), JsonFileError> {
    let raw = serde_json::to_string_pretty(value).map_err(|source| JsonFileError::Serialize {
        path: path.display().to_string(),
        source,
    })?;
    write_raw_named(
        path,
        &raw,
        Sanitize::None,
        TmpExt::Tmp,
        Durability::Synced,
        TmpNaming::Unique,
    )
}

/// 原子写 + **写前备份**：先把当前 `path` 复制成 `path.json.bak`，再 rename。
///
/// 崩溃/写失败后 `path` 可能缺失或损坏，此时可用 [`restore_from_backup`]
/// 把旧版本搬回 `path`。
pub fn write_with_backup<T: Serialize>(path: &Path, value: &T) -> Result<(), JsonFileError> {
    let raw = serde_json::to_string_pretty(value).map_err(|source| JsonFileError::Serialize {
        path: path.display().to_string(),
        source,
    })?;
    write_raw_with_backup(path, &raw, Sanitize::None, TmpExt::Tmp, Durability::Synced)
}

/// 从 `path.json.bak` 恢复 `path`。
///
/// 用于「写前备份 + 崩溃后文件损坏/缺失」的人工与自动回滚路径。
/// 备份不存在时报 [`JsonFileError::MissingBackup`]。
pub fn restore_from_backup(path: &Path) -> Result<(), JsonFileError> {
    let backup = path.with_extension("json.bak");
    if !backup.exists() {
        return Err(JsonFileError::MissingBackup {
            path: backup.display().to_string(),
        });
    }
    fs::copy(&backup, path).map_err(|source| JsonFileError::Backup {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

/// 原子写的字符串核心，供已有「先序列化成 String 再落盘」的调用点复用。
///
/// 保留原 `write_atomic(path, body)` 的参数化语义，避免调用点重复实现 IO。
///
/// tmp 命名沿用历史固定名（[`TmpNaming::Fixed`]）；需要并发写同一目标时改用
/// [`write_raw_named`] + [`TmpNaming::Unique`]。
pub fn write_raw(
    path: &Path,
    body: &str,
    sanitize: Sanitize,
    tmp_ext: TmpExt,
    durability: Durability,
) -> Result<(), JsonFileError> {
    write_raw_named(path, body, sanitize, tmp_ext, durability, TmpNaming::Fixed)
}

/// [`write_raw`] + 可选唯一 tmp 名（[`TmpNaming::Unique`]）。
///
/// 语义与 [`write_raw`] 完全一致，唯一区别是 tmp 文件名：并发写同一目标时
/// 每个写者独占自己的 tmp，不会互相截断或覆盖中间产物。
pub fn write_raw_named(
    path: &Path,
    body: &str,
    sanitize: Sanitize,
    tmp_ext: TmpExt,
    durability: Durability,
    naming: TmpNaming,
) -> Result<(), JsonFileError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| JsonFileError::WriteTmp {
                path: parent.display().to_string(),
                source,
            })?;
        }
    }
    let body = sanitize.apply(body);
    let tmp = tmp_path(path, tmp_ext, naming);
    match durability {
        Durability::Synced => {
            let mut f = fs::File::create(&tmp).map_err(|source| JsonFileError::WriteTmp {
                path: tmp.display().to_string(),
                source,
            })?;
            f.write_all(body.as_bytes())
                .map_err(|source| JsonFileError::WriteTmp {
                    path: tmp.display().to_string(),
                    source,
                })?;
            f.sync_all().map_err(|source| JsonFileError::WriteTmp {
                path: tmp.display().to_string(),
                source,
            })?;
        }
        Durability::Unsynced => {
            fs::write(&tmp, body.as_bytes()).map_err(|source| JsonFileError::WriteTmp {
                path: tmp.display().to_string(),
                source,
            })?;
        }
    }
    fs::rename(&tmp, path).map_err(|source| JsonFileError::Rename {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

/// 原子写且**不覆盖**已存在文件：目标已存在则拒绝写入并原样保留。
///
/// 返回 `Ok(true)` = 本次写入成功创建了目标；`Ok(false)` = 目标已存在，
/// 未做任何修改（不是错误）。用于「首次落地独占」的场景（导入新 pack、
/// 分配新 id 等），避免 [`write_atomic`] 的 rename 语义静默吃掉他人数据。
///
/// 实现：先把内容写进同目录唯一 tmp（`create_new`，绝不 clobber 别人的残留），
/// 再用 `hard_link(tmp, path)` 发布。link 是**原子且 exclusive** 的 —— POSIX 下
/// link 到已存在的目标必失败，因此「已存在则拒写」没有 TOCTOU 窗口。成功后删
/// tmp；link 失败则删 tmp 并返回 `Ok(false)`。
///
/// 退化路径：若平台/文件系统不支持硬链接（`ErrorKind` 非 `AlreadyExists` 但
/// 报错，如 Android 部分 sdcardfs 挂载返回 `EPERM`/`EXDEV`），退到
/// 「先 `path.exists()` 检查再 `rename`」。**该路径存在 TOCTOU 竞态**：检查
/// 与 rename 之间另一进程仍可能创建目标，此时会覆盖。仅在链接不可用时使用。
pub fn write_atomic_noclobber(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建父目录失败 {}: {e}", parent.display()))?;
        }
    }

    let tmp = tmp_path(path, TmpExt::Tmp, TmpNaming::Unique);
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| format!("创建临时文件失败 {}: {e}", tmp.display()))?;
        f.write_all(bytes)
            .map_err(|e| format!("写临时文件失败 {}: {e}", tmp.display()))?;
        f.sync_all()
            .map_err(|e| format!("sync 临时文件失败 {}: {e}", tmp.display()))?;
    }

    match fs::hard_link(&tmp, path) {
        Ok(()) => {
            let _ = fs::remove_file(&tmp);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp);
            Ok(false)
        }
        Err(link_err) => {
            // 硬链接不可用 → 退化路径（TOCTOU，见文档注释）。
            let _ = fs::remove_file(&tmp);
            if path.exists() {
                return Ok(false);
            }
            let tmp2 = tmp_path(path, TmpExt::Tmp, TmpNaming::Unique);
            fs::write(&tmp2, bytes)
                .map_err(|e| format!("写临时文件失败 {}: {e}", tmp2.display()))?;
            match fs::rename(&tmp2, path) {
                Ok(()) => Ok(true),
                Err(rename_err) => {
                    let _ = fs::remove_file(&tmp2);
                    if path.exists() {
                        Ok(false)
                    } else {
                        Err(format!(
                            "无替换原子写失败 {}: 硬链接不可用 ({link_err}) 且 rename 失败 ({rename_err})",
                            path.display()
                        ))
                    }
                }
            }
        }
    }
}

/// [`write_raw`] + 写前备份（存在旧文件才备份，首次写盘不产生空 `.bak`）。
pub fn write_raw_with_backup(
    path: &Path,
    body: &str,
    sanitize: Sanitize,
    tmp_ext: TmpExt,
    durability: Durability,
) -> Result<(), JsonFileError> {
    if path.exists() {
        let backup = path.with_extension("json.bak");
        fs::copy(path, &backup).map_err(|source| JsonFileError::Backup {
            path: backup.display().to_string(),
            source,
        })?;
    }
    write_raw(path, body, sanitize, tmp_ext, durability)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Doc {
        name: String,
        n: i64,
    }

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("museai-jsonfile-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 正常写读往返。
    #[test]
    fn roundtrip() {
        let dir = scratch("roundtrip");
        let path = dir.join("d.json");
        let doc = Doc { name: "x".into(), n: 7 };
        write_atomic(&path, &doc).unwrap();
        let back: Doc = read_json(&path).unwrap();
        assert_eq!(back, doc);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 原子写后不留 tmp 残留。
    #[test]
    fn atomic_leaves_no_tmp() {
        let dir = scratch("notmp");
        let path = dir.join("d.json");
        write_atomic(&path, &Doc { name: "a".into(), n: 1 }).unwrap();
        assert!(!path.with_extension("tmp").exists(), "不应残留 d.tmp");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "目录应只剩目标文件");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// TmpExt::JsonTmp 也不留残留，且 tmp 名与原实现一致。
    #[test]
    fn json_tmp_ext_leaves_no_tmp() {
        let dir = scratch("jsontmp");
        let path = dir.join("idx.json");
        write_raw(
            &path,
            "{}",
            Sanitize::None,
            TmpExt::JsonTmp,
            Durability::Unsynced,
        )
        .unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 写前备份生成 .bak，且内容为**旧版本**。
    #[test]
    fn backup_holds_previous_version() {
        let dir = scratch("backup");
        let path = dir.join("d.json");
        let v1 = Doc { name: "v1".into(), n: 1 };
        let v2 = Doc { name: "v2".into(), n: 2 };

        // 首次写盘：目标不存在，不应产生空备份
        write_with_backup(&path, &v1).unwrap();
        assert!(!path.with_extension("json.bak").exists(), "首次写不应建空 .bak");

        // 二次写盘：.bak 应为 v1，path 应为 v2
        write_with_backup(&path, &v2).unwrap();
        let bak: Doc = read_json(&path.with_extension("json.bak")).unwrap();
        assert_eq!(bak, v1, ".bak 应是写前旧版本");
        let cur: Doc = read_json(&path).unwrap();
        assert_eq!(cur, v2);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 备份恢复流程：path 损坏/丢失后能从 .bak 搬回旧版本。
    #[test]
    fn restore_roundtrip() {
        let dir = scratch("restore");
        let path = dir.join("d.json");
        let v1 = Doc { name: "good".into(), n: 1 };
        let v2 = Doc { name: "bad".into(), n: 2 };
        write_with_backup(&path, &v1).unwrap();
        write_with_backup(&path, &v2).unwrap();

        // 模拟崩溃：目标文件被截断
        fs::write(&path, b"{ truncated").unwrap();
        assert!(read_json::<Doc>(&path).is_err());

        restore_from_backup(&path).unwrap();
        let back: Doc = read_json(&path).unwrap();
        assert_eq!(back, v1, "应从 .bak 恢复出旧版本");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 无备份时恢复报带路径上下文的错。
    #[test]
    fn restore_without_backup_errors_with_context() {
        let dir = scratch("nobak");
        let path = dir.join("d.json");
        let err = restore_from_backup(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("d.json.bak"), "错误应带备份路径: {msg}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 非可序列化值报错，且带路径上下文。
    #[test]
    fn non_serializable_reports_path() {
        use std::collections::HashMap;
        let dir = scratch("badser");
        let path = dir.join("d.json");
        // f64::NAN / INF 在 serde_json 中序列化为 null，但自定义不可序列化更直接：
        // 用一个 Serialize impl 直接返回 Err 的类型。
        struct Bad;
        impl Serialize for Bad {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("nope"))
            }
        }
        let err = write_atomic(&path, &Bad).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("序列化 JSON 失败"), "应报序列化错: {msg}");
        assert!(msg.contains("d.json"), "错误应带路径上下文: {msg}");

        // HashMap 往返，覆盖正常集合序列化
        let mut m = HashMap::new();
        m.insert("k".to_string(), vec![1i64, 2, 3]);
        write_atomic(&path, &m).unwrap();
        let back: HashMap<String, Vec<i64>> = read_json(&path).unwrap();
        assert_eq!(back, m);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Sanitize::StripFffd 剔除替换符，None 原样保留 —— 锁定两种策略的差异。
    #[test]
    fn sanitize_strategies_differ() {
        let dir = scratch("sanitize");
        let a = dir.join("a.json");
        let b = dir.join("b.json");
        let raw = "手段\u{FFFD}\u{FFFD}诚意";
        write_raw(&a, raw, Sanitize::StripFffd, TmpExt::Tmp, Durability::Synced).unwrap();
        write_raw(&b, raw, Sanitize::None, TmpExt::Tmp, Durability::Synced).unwrap();
        let sa = fs::read_to_string(&a).unwrap();
        let sb = fs::read_to_string(&b).unwrap();
        assert_eq!(sa, "手段诚意");
        assert!(sb.contains('\u{FFFD}'), "None 策略应原样保留");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// write_raw_with_backup 首次写盘不建空 .bak，二次写盘 .bak 为旧版本。
    #[test]
    fn raw_with_backup() {
        let dir = scratch("rawbak");
        let path = dir.join("d.json");
        write_raw_with_backup(&path, "old", Sanitize::None, TmpExt::Tmp, Durability::Synced)
            .unwrap();
        assert!(!path.with_extension("json.bak").exists());
        write_raw_with_backup(&path, "new", Sanitize::None, TmpExt::Tmp, Durability::Synced)
            .unwrap();
        assert_eq!(
            fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "old"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 读不存在/坏文件时报带路径的错。
    #[test]
    fn read_errors_carry_path() {
        let dir = scratch("readerr");
        let path = dir.join("d.json");
        let err = read_json::<Doc>(&path).unwrap_err();
        assert!(err.to_string().contains("d.json"));

        fs::write(&path, b"{ not json").unwrap();
        let err = read_json::<Doc>(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("解析 JSON 失败") && msg.contains("d.json"), "{msg}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 目标不存在 → 无替换写成功，返回 true，内容落盘。
    #[test]
    fn noclobber_creates_when_absent() {
        let dir = scratch("nc-new");
        let path = dir.join("d.json");
        assert!(write_atomic_noclobber(&path, b"fresh").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "fresh");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "不应残留 tmp");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 目标已存在 → 返回 false 且原内容分毫不动。
    #[test]
    fn noclobber_refuses_existing() {
        let dir = scratch("nc-exists");
        let path = dir.join("d.json");
        fs::write(&path, b"original").unwrap();
        assert!(!write_atomic_noclobber(&path, b"clobber").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original", "原内容必须保留");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "不应残留 tmp");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 并发 N 次无替换写同一新文件：有且仅有 1 个返回 true。
    #[test]
    fn noclobber_concurrent_exactly_one_winner() {
        let dir = scratch("nc-race");
        let path = dir.join("d.json");
        let n = 16;
        let wins: usize = std::thread::scope(|s| {
            let handles: Vec<_> = (0..n)
                .map(|i| {
                    let path = path.clone();
                    s.spawn(move || write_atomic_noclobber(&path, format!("w{i}").as_bytes()))
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap().unwrap() as usize)
                .sum()
        });
        assert_eq!(wins, 1, "并发无替换写应恰有 1 个赢家，实际 {wins}");
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.starts_with('w'), "落盘内容应来自赢家之一: {body}");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "不应残留 tmp");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 唯一 tmp 名下并发写同一目标不同内容：不产生互相截断的中间产物，
    /// 最终文件内容必为某个写者的完整原文（无拼接/半截）。
    #[test]
    fn unique_tmp_concurrent_writes_do_not_clobber() {
        let dir = scratch("uniq-conc");
        let path = dir.join("d.json");
        let n = 16;
        let bodies: Vec<String> = (0..n)
            .map(|i| format!("{{\"payload\":\"{}-{}\"}}", "x".repeat(512), i))
            .collect();
        std::thread::scope(|s| {
            for body in &bodies {
                let path = path.clone();
                let body = body.clone();
                s.spawn(move || {
                    write_raw_named(
                        &path,
                        &body,
                        Sanitize::None,
                        TmpExt::Tmp,
                        Durability::Synced,
                        TmpNaming::Unique,
                    )
                    .unwrap();
                });
            }
        });
        let final_body = fs::read_to_string(&path).unwrap();
        assert!(
            bodies.iter().any(|b| *b == final_body),
            "最终内容应为某个写者的完整原文，未被并发写互相污染"
        );
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "不应残留 tmp");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 固定 tmp 名（历史语义）仍保留：TmpNaming::Fixed 产出的 tmp 名不变。
    #[test]
    fn fixed_naming_preserved() {
        let dir = scratch("uniq-fixed");
        let path = dir.join("d.json");
        assert_eq!(
            tmp_path(&path, TmpExt::Tmp, TmpNaming::Fixed),
            path.with_extension("tmp"),
            "Fixed 必须等于历史固定名"
        );
        assert_eq!(
            tmp_path(&path, TmpExt::JsonTmp, TmpNaming::Fixed),
            path.with_extension("json.tmp")
        );
        let unique = tmp_path(&path, TmpExt::Tmp, TmpNaming::Unique);
        assert_ne!(unique, path.with_extension("tmp"), "Unique 必须偏离固定名");
        let other = tmp_path(&path, TmpExt::Tmp, TmpNaming::Unique);
        assert_ne!(unique, other, "Unique 每次调用都应不同");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 失败路径不留 tmp：无替换写被拒后目录只剩目标文件。
    #[test]
    fn noclobber_failure_leaves_no_tmp() {
        let dir = scratch("nc-notmp");
        let path = dir.join("d.json");
        fs::write(&path, b"keep").unwrap();
        for _ in 0..3 {
            assert!(!write_atomic_noclobber(&path, b"nope").unwrap());
        }
        let entries: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["d.json".to_string()], "不得残留 tmp: {entries:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 无替换写对父目录不存在的情况应自建目录，不残留 tmp。
    #[test]
    fn noclobber_creates_parent_dirs() {
        let dir = scratch("nc-mkdir");
        let path = dir.join("nested/deep/d.json");
        assert!(write_atomic_noclobber(&path, b"{}").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        assert!(!write_atomic_noclobber(&path, b"{}").unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }
}
