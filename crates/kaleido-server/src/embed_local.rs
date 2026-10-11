//! In-process embedding via `fastembed` (BGE-small-zh-v1.5, 512-d).
//!
//! Prefer local ONNX under `$KALEIDO_EMBED_CACHE` / python fastembed cache so we do not
//! depend on HuggingFace at runtime. Falls back to remote `EMBEDDING_BASE_URL` via
//! `llm_stream::get_embedding` when local init fails.
//!
//! Env:
//! - `KALEIDO_EMBED_INLINE` — default on; `0`/`false` disables local
//! - `KALEIDO_EMBED_CACHE` — model dir (default `$KALEIDO_DATA/embed-cache` or `./data/embed-cache`)
//! - `KALEIDO_EMBED_MODEL_DIR` — explicit snapshot dir with model_optimized.onnx + tokenizer*

use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

static ENGINE: OnceLock<Mutex<Option<LocalEmbedEngine>>> = OnceLock::new();

/// 内联引擎加载的 embedding 模型标识。索引文件与查询侧都以它为准，
/// `st_vector_index::rank_hits` 据此拒绝跨模型向量互比。
pub const MODEL_NAME: &str = "BAAI/bge-small-zh-v1.5";

pub struct LocalEmbedEngine {
    model: TextEmbedding,
    pub model_name: String,
    pub dim: usize,
}

fn cache_dir() -> PathBuf {
    if let Ok(p) = std::env::var("KALEIDO_EMBED_CACHE") {
        let pb = PathBuf::from(p);
        let _ = std::fs::create_dir_all(&pb);
        return pb;
    }
    if let Ok(data) = std::env::var("KALEIDO_DATA") {
        let pb = PathBuf::from(data).join("embed-cache");
        let _ = std::fs::create_dir_all(&pb);
        return pb;
    }
    let pb = PathBuf::from("data/embed-cache");
    let _ = std::fs::create_dir_all(&pb);
    pb
}

pub fn inline_enabled() -> bool {
    match std::env::var("KALEIDO_EMBED_INLINE") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        }
        Err(_) => true,
    }
}

fn engine_slot() -> &'static Mutex<Option<LocalEmbedEngine>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

/// Candidate dirs that already hold Qdrant/python fastembed BGE-zh files.
fn model_snapshot_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("KALEIDO_EMBED_MODEL_DIR") {
        out.push(PathBuf::from(p));
    }
    let cache = cache_dir();
    // Prefer explicit snapshot under our cache
    out.push(cache.join("bge-small-zh-v1.5"));
    out.push(cache.join("models--Qdrant--bge-small-zh-v1.5/snapshots/46fbe35fd4374a00fee7de77dfddaeb6dd6a2c59"));
    // python /tmp fastembed cache (shared host)
    out.push(PathBuf::from(
        "/tmp/fastembed_cache/models--Qdrant--bge-small-zh-v1.5/snapshots/46fbe35fd4374a00fee7de77dfddaeb6dd6a2c59",
    ));
    out.push(PathBuf::from(
        "/tmp/fastembed_cache/models--Qdrant--bge-small-zh-v1.5",
    ));
    out
}

fn resolve_file(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    for n in names {
        let p = dir.join(n);
        if p.is_file() {
            return Some(p);
        }
        // also one-level nested onnx/
        let p2 = dir.join("onnx").join(n);
        if p2.is_file() {
            return Some(p2);
        }
    }
    // walk one level of snapshots/*
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                for n in names {
                    let c = p.join(n);
                    if c.is_file() {
                        return Some(c);
                    }
                }
            }
        }
    }
    None
}

fn load_from_dir(dir: &Path) -> Result<TextEmbedding, String> {
    let onnx = resolve_file(
        dir,
        &[
            "model_optimized.onnx",
            "model.onnx",
            "onnx/model_optimized.onnx",
            "onnx/model.onnx",
        ],
    )
    .ok_or_else(|| format!("no onnx under {}", dir.display()))?;
    let tokenizer = resolve_file(dir, &["tokenizer.json"])
        .ok_or_else(|| format!("no tokenizer.json under {}", dir.display()))?;
    let config = resolve_file(dir, &["config.json"])
        .ok_or_else(|| format!("no config.json under {}", dir.display()))?;
    let special = resolve_file(dir, &["special_tokens_map.json"]).unwrap_or_else(|| config.clone());
    let tok_cfg =
        resolve_file(dir, &["tokenizer_config.json"]).unwrap_or_else(|| config.clone());

    let onnx_bytes = std::fs::read(&onnx).map_err(|e| format!("read {}: {e}", onnx.display()))?;
    let tokenizer_files = TokenizerFiles {
        tokenizer_file: std::fs::read(&tokenizer)
            .map_err(|e| format!("read {}: {e}", tokenizer.display()))?,
        config_file: std::fs::read(&config)
            .map_err(|e| format!("read {}: {e}", config.display()))?,
        special_tokens_map_file: std::fs::read(&special)
            .map_err(|e| format!("read {}: {e}", special.display()))?,
        tokenizer_config_file: std::fs::read(&tok_cfg)
            .map_err(|e| format!("read {}: {e}", tok_cfg.display()))?,
    };

    let mut ud = UserDefinedEmbeddingModel::new(onnx_bytes, tokenizer_files);
    // BGE uses CLS pooling
    ud.pooling = Some(Pooling::Cls);

    let opts = InitOptionsUserDefined::new().with_max_length(512);
    TextEmbedding::try_new_from_user_defined(ud, opts)
        .map_err(|e| format!("user-defined init from {}: {e}", dir.display()))
}

/// Lazy-init local model. Returns error string on failure (caller may fall back to HTTP).
pub fn ensure_local() -> Result<(), String> {
    if !inline_enabled() {
        return Err("inline embed disabled (KALEIDO_EMBED_INLINE=0)".into());
    }
    let mut slot = engine_slot().lock();
    if slot.is_some() {
        return Ok(());
    }
    let t0 = Instant::now();
    let mut last_err = String::from("no model dir found");
    let mut model: Option<TextEmbedding> = None;
    let mut used = PathBuf::new();
    for cand in model_snapshot_candidates() {
        if !cand.exists() {
            continue;
        }
        tracing::info!(dir=%cand.display(), "embed_local: trying snapshot…");
        match load_from_dir(&cand) {
            Ok(m) => {
                model = Some(m);
                used = cand;
                break;
            }
            Err(e) => {
                tracing::warn!(dir=%cand.display(), error=%e, "embed_local: candidate failed");
                last_err = e;
            }
        }
    }
    let mut model = model.ok_or(last_err)?;

    let probe = model
        .embed(vec!["ping".to_string()], None)
        .map_err(|e| format!("fastembed probe: {e}"))?;
    let dim = probe.first().map(|v| v.len()).unwrap_or(0);
    if dim == 0 {
        return Err("fastembed probe returned empty vector".into());
    }
    tracing::info!(
        dim,
        path = %used.display(),
        elapsed_ms = t0.elapsed().as_millis() as u64,
        "embed_local: ready"
    );
    *slot = Some(LocalEmbedEngine {
        model,
        model_name: MODEL_NAME.into(),
        dim,
    });
    Ok(())
}

pub fn status() -> serde_json::Value {
    let enabled = inline_enabled();
    let slot = engine_slot().lock();
    match slot.as_ref() {
        Some(e) => serde_json::json!({
            "enabled": enabled,
            "ready": true,
            "backend": "fastembed",
            "model": e.model_name,
            "dim": e.dim,
            "cache": cache_dir().display().to_string(),
        }),
        None => serde_json::json!({
            "enabled": enabled,
            "ready": false,
            "backend": if enabled { "fastembed" } else { "remote" },
            "model": MODEL_NAME,
            "cache": cache_dir().display().to_string(),
            "modelFilesReady": has_local_model(),
            "autodl": autodl_enabled(),
        }),
    }
}

/// Embed one string with the in-process model.
pub fn embed_one(input: &str) -> Result<Vec<f32>, String> {
    ensure_local()?;
    let mut slot = engine_slot().lock();
    let eng = slot
        .as_mut()
        .ok_or_else(|| "embed_local not initialized".to_string())?;
    let text = if input.trim().is_empty() {
        " ".to_string()
    } else {
        let t = input.trim();
        if t.chars().count() > 2000 {
            t.chars().take(2000).collect()
        } else {
            t.to_string()
        }
    };
    let out = eng
        .model
        .embed(vec![text], None)
        .map_err(|e| format!("fastembed embed: {e}"))?;
    out.into_iter()
        .next()
        .ok_or_else(|| "fastembed returned no vectors".into())
}

/// Embed many texts (batch).
pub fn embed_many(inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    ensure_local()?;
    let mut slot = engine_slot().lock();
    let eng = slot
        .as_mut()
        .ok_or_else(|| "embed_local not initialized".to_string())?;
    if inputs.is_empty() {
        return Ok(vec![]);
    }
    let batch: Vec<String> = inputs
        .iter()
        .map(|s| {
            let t = s.trim();
            if t.is_empty() {
                " ".into()
            } else if t.chars().count() > 2000 {
                t.chars().take(2000).collect()
            } else {
                t.to_string()
            }
        })
        .collect();
    eng.model
        .embed(batch, None)
        .map_err(|e| format!("fastembed embed batch: {e}"))
}

// ─── 模型自举（首启下载，文本协议根治同批 2026-10-04）────────────────────────
// 此前 embed 主路径只探测不下载：全新部署缺模型文件时本地向量静默降级。
// 现由启动 warm 路径与 /api/v1/embed/status 触发一次性下载（不阻塞请求路径）。

/// HF 仓库（Qdrant 官方 bge-small-zh-v1.5 镜像）。
const HF_REPO: &str = "Qdrant/bge-small-zh-v1.5";
/// 引擎必需文件（相对仓库根）；onnx 权重在 onnx/ 子目录。
const MODEL_FILES: [&str; 5] = [
    "onnx/model_optimized.onnx",
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

/// HF 端点；国内部署可设 `KALEIDO_HF_ENDPOINT=https://hf-mirror.com`。
fn hf_endpoint() -> String {
    std::env::var("KALEIDO_HF_ENDPOINT")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://huggingface.co".into())
}

/// 首启自动下载开关（`KALEIDO_EMBED_AUTODL`，默认开）。
pub fn autodl_enabled() -> bool {
    match std::env::var("KALEIDO_EMBED_AUTODL") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        }
        Err(_) => true,
    }
}

/// 本地候选目录里是否已有可用模型文件（onnx + tokenizer.json 齐备）。
pub fn has_local_model() -> bool {
    for cand in model_snapshot_candidates() {
        if !cand.exists() {
            continue;
        }
        let onnx = resolve_file(
            &cand,
            &["model_optimized.onnx", "model.onnx", "onnx/model_optimized.onnx", "onnx/model.onnx"],
        )
        .is_some();
        let tok = resolve_file(&cand, &["tokenizer.json"]).is_some();
        if onnx && tok {
            return true;
        }
    }
    false
}

/// 下载单文件到 dest；非空文件已存在则跳过（按文件粒度续传）；.part 防半截。
/// 返回写入字节数（跳过时 0）。
async fn download_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
) -> Result<u64, String> {
    if dest.is_file() {
        let sz = dest.metadata().map(|m| m.len()).unwrap_or(0);
        if sz > 0 {
            return Ok(0);
        }
    }
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let part = dest.with_extension("part");
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut f = std::fs::File::create(&part).map_err(|e| format!("create {}: {e}", part.display()))?;
    use std::io::Write as _;
    let mut n: u64 = 0;
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(|e| format!("read {url}: {e}"))?;
        f.write_all(&chunk).map_err(|e| format!("write {}: {e}", part.display()))?;
        n += chunk.len() as u64;
    }
    drop(f);
    std::fs::rename(&part, dest).map_err(|e| format!("rename {}: {e}", part.display()))?;
    Ok(n)
}

/// 下载模型全量文件到 `cache_dir()/bge-small-zh-v1.5/`，返回该目录。
/// 仅应在启动 warm 路径 / 显式 warm 端点调用（90MB 级权重，勿进请求热路径）。
pub async fn bootstrap_model() -> Result<PathBuf, String> {
    let dir = cache_dir().join("bge-small-zh-v1.5");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let client = reqwest::Client::new();
    let base = hf_endpoint();
    for rel in MODEL_FILES {
        let url = format!("{base}/{HF_REPO}/resolve/main/{rel}");
        let dest = dir.join(rel);
        match download_file(&client, &url, &dest).await {
            Ok(0) => tracing::info!(file = rel, "embed_local: bootstrap file already present"),
            Ok(n) => tracing::info!(file = rel, bytes = n, "embed_local: bootstrap downloaded"),
            Err(e) => return Err(e),
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autodl_flag_parsing() {
        // 未设置 → 默认开
        std::env::remove_var("KALEIDO_EMBED_AUTODL");
        assert!(autodl_enabled());
        // 显式关
        for off in ["0", "false", "off", "no", "FALSE"] {
            std::env::set_var("KALEIDO_EMBED_AUTODL", off);
            assert!(!autodl_enabled(), "off value: {off}");
        }
        // 其它任意值 → 开
        std::env::set_var("KALEIDO_EMBED_AUTODL", "1");
        assert!(autodl_enabled());
        std::env::remove_var("KALEIDO_EMBED_AUTODL");
    }

    #[test]
    fn hf_endpoint_default_and_override() {
        std::env::remove_var("KALEIDO_HF_ENDPOINT");
        assert_eq!(hf_endpoint(), "https://huggingface.co");
        std::env::set_var("KALEIDO_HF_ENDPOINT", "https://hf-mirror.com/");
        assert_eq!(hf_endpoint(), "https://hf-mirror.com");
        std::env::remove_var("KALEIDO_HF_ENDPOINT");
    }
}
