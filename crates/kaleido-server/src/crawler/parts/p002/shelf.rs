//! crawler/parts/p002/shelf —— 书架存储：目录/slug/封面/scan_shelf + list/content/cover 路由 + 请求体。（R6-3① 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn shelf_dir() -> FsPathBuf {
    FsPathBuf::from("novel_workspace")
}

pub(crate) fn shelf_covers_dir() -> FsPathBuf {
    shelf_dir().join("covers")
}


pub(crate) fn extract_cover_url(html: &str) -> Option<String> {
    // Try <meta property="og:image" content="...">
    if let Some(start) = html.find("property=\"og:image\"") {
        let after = &html[start..];
        if let Some(cs) = after.find("content=\"") {
            let vs = cs + 9;
            let remaining = &after[vs..];
            if let Some(end) = remaining.find('"') {
                let url = &remaining[..end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
    }
    // Try <meta content="..." property="og:image">
    if let Some(start) = html.find("content=\"") {
        let after = &html[start..];
        if let Some(_prop) = after.find("og:image") {
            let cs = 9;
            let vs = start + cs;
            let remaining = &html[vs..];
            if let Some(end) = remaining.find('"') {
                let url = &remaining[..end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
    }
    None
}

pub(crate) fn shelf_slug(title: &str) -> String {
    let slug: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == ' ')
        .collect::<String>()
        .trim()
        .replace(' ', "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .trim_matches('_')
        .to_lowercase();
    // Truncate to 60 chars to keep filesystem happy
    slug.chars().take(60).collect()
}

/// 回读 md 首行 `# 标题`（与 scan_shelf 的标题提取逻辑保持一致，校验时两边必须同源）。
pub(crate) fn shelf_md_title(content: &str) -> String {
    content
        .lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l.trim_start_matches("# ").trim().to_string())
        .unwrap_or_default()
}

/// 把 slug 解析回 shelf 目录里的 .md 文件（novel_content / novel_to_pack /
/// novel_distil / exec_shelf_distil_world / novel_export 共用）。
/// 修复：此前各处用 `shelf_slug(stem).contains(slug_tag)` 取 read_dir 首个命中，
/// 顺序敏感且 contains 太松，slug 前缀重叠（如「茶馆」vs「老茶馆」）时会读错书。
/// 解析顺序：
/// 1) 优先 stem 归一化后与 slug_tag **精确相等** 的候选；
/// 2) 精确候选未通过校验、或退到 contains 候选时，回读文件首行 `# 标题` 与 title
///    比对，只接受标题一致的文件（title 为空时无法校验，保持旧行为不阻塞）；
/// 3) 无任何通过校验的候选 → None（调用方按 file not found 处理，宁缺勿错）。
pub(crate) fn find_shelf_md_file(
    dir: &std::path::Path,
    slug_tag: &str,
    title: &str,
) -> Option<std::path::PathBuf> {
    let mut exact: Vec<std::path::PathBuf> = Vec::new();
    let mut fuzzy: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let stem_normalized = shelf_slug(stem);
            if stem_normalized == slug_tag {
                exact.push(p);
            } else if stem_normalized.contains(slug_tag) {
                fuzzy.push(p);
            }
        }
    }
    let title_ok = |p: &std::path::PathBuf| -> bool {
        if title.trim().is_empty() {
            return true; // 空标题退化场景：无标题可校验，不阻塞
        }
        std::fs::read_to_string(p)
            .ok()
            .map(|c| shelf_md_title(&c) == title)
            .unwrap_or(false)
    };
    if let Some(p) = exact.iter().find(|p| title_ok(p)) {
        return Some(p.clone());
    }
    if let Some(p) = fuzzy.iter().find(|p| title_ok(p)) {
        return Some(p.clone());
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NovelshelfEntry {
    pub slug: String,
    pub title: String,
    pub chapter_count: usize,
    pub has_cover: bool,
    pub file_size: u64,
}
pub fn scan_shelf() -> Vec<NovelshelfEntry> {
    let dir = shelf_dir();
    if !dir.exists() { return vec![]; }
    let mut entries = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") { continue; }
            let meta = match std::fs::metadata(&path) { Ok(m) => m, _ => continue };
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let title = shelf_md_title(&content);
            let slug = shelf_slug(&title);
            let cdir = shelf_covers_dir();
            let has_cover = cdir.join(format!("{slug}.jpg")).exists()
                || cdir.join(format!("{slug}.webp")).exists();
            let cc = content.lines().filter(|l| l.starts_with("- 第")).count();
            entries.push(NovelshelfEntry { slug, title, chapter_count: cc, has_cover, file_size: meta.len() });
        }
    }
    entries.sort_by(|a,b| b.file_size.cmp(&a.file_size));
    // Dedupe by slug, keep largest file_size (already sorted desc).
    let mut seen = std::collections::HashSet::new();
    entries.retain(|e| seen.insert(e.slug.clone()));
    entries
}

pub async fn novels_list(
    State(_state): State<AppState>,
    _headers: HeaderMap,
) -> Response {
    Json(json!({"ok":true,"novels":scan_shelf()})).into_response()
}

pub async fn novel_content(
    _state: State<AppState>,
    _headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    let shelf = scan_shelf();
    let found = shelf.into_iter().find(|e| e.slug == slug);
    match found {
        Some(entry) => {
            let dir = shelf_dir();
            let slug_tag = shelf_slug(&entry.title);
            // 精确匹配优先 + 回读首行 `# 标题` 校验，见 find_shelf_md_file
            let file_path = find_shelf_md_file(&dir, &slug_tag, &entry.title);
            match file_path {
                Some(path) => match std::fs::read_to_string(&path) {
                    Ok(content) => Json(json!({"ok":true,"title":entry.title,"slug":slug,"content":content,"chapter_count":entry.chapter_count})).into_response(),
                    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok":false,"error":e.to_string()}))).into_response()
                },
                None => (StatusCode::NOT_FOUND, Json(json!({"ok":false,"error":"file not found"}))).into_response()
            }
        }
        None => (StatusCode::NOT_FOUND, Json(json!({"ok":false,"error":"novel not found"}))).into_response()
    }
}

pub async fn novel_cover(Path(slug): Path<String>) -> Response {
    // shelf_slug keeps only [a-z0-9_] — strips `.`, `/`, `\`, NUL and any traversal shape.
    let slug = shelf_slug(&slug);
    let cdir = shelf_covers_dir();
    for ext in &["jpg","jpeg","png","webp"] {
        let path = cdir.join(format!("{slug}.{ext}"));
        if path.exists() {
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    let mime = match *ext { "jpg"|"jpeg" => "image/jpeg", "png" => "image/png", "webp" => "image/webp", _ => "application/octet-stream" };
                    return (StatusCode::OK, [("Content-Type",mime),("Cache-Control","public, max-age=86400")], bytes).into_response();
                }
                Err(_) => continue,
            }
        }
    }
    (StatusCode::NOT_FOUND, "cover not found").into_response()
}

// ─── Shelf import / promote to Story Tavern ───────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfImportBody {
    /// Raw novel text (.txt / .md body).
    pub text: String,
    /// Optional base64-encoded raw bytes. When present, `text` is ignored and the
    /// bytes are sniffed (BOM → UTF-8/GB18030/Big5/Shift_JIS/EUC-KR strict decode +
    /// mojibake check) to avoid GBK/Big5 source txt being mis-decoded as UTF-8 (乱码).
    #[serde(default)]
    pub data: Option<String>,
    /// Display title; defaults to first heading or "未命名".
    #[serde(default)]
    pub title: Option<String>,
    /// When true, also build a Story Tavern pack from the text (default true).
    #[serde(default)]
    pub to_pack: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfToPackBody {
    /// Optional pack id override.
    #[serde(default)]
    pub pack_id: Option<String>,
    /// Force rebuild even if a pack with same source already exists.
    #[serde(default)]
    pub force: Option<bool>,
}
