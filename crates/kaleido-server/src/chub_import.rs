//! chub.ai 角色卡一键导入（产品化候选①，2026-10-07）。
//!
//! `POST /api/v1/partner/chub-import` {url}：
//! 1. 解析 chub.ai / venus.chub.ai 角色链接 → {author}/{char} 路径；
//! 2. 调 chub API（先取节点 JSON，失败退 `?download=true` 直出卡片）；
//! 3. 归一为 chara_card_v2 → 委托既有 st-import 管线
//!    （同鉴权 / 同落库 / 同世界书与正则抽取，前端零改动复用）。

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;
use crate::error_codes::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChubImportBody {
    url: String,
}

pub async fn chub_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChubImportBody>,
) -> Response {
    let path = match chub_path(&body.url) {
        Ok(p) => p,
        Err(e) => {
            return err_with_code(
                StatusCode::BAD_REQUEST,
                "CHUB_BAD_URL",
                e,
                json!({"url": body.url}),
            )
        }
    };

    let client = reqwest::Client::builder()
        .no_proxy()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) quill-import")
        .build()
        .unwrap_or_default();
    let mut card: Option<Value> = None;
    let mut last_err = String::from("未尝试");
    for attempt in [
        format!("https://api.chub.ai/api/characters/{path}"),
        format!("https://api.chub.ai/api/characters/{path}?download=true"),
    ] {
        match client
            .get(&attempt)
            .header(header::ACCEPT, "application/json")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let v: Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        last_err = format!("chub 响应解析失败：{e}");
                        continue;
                    }
                };
                match pick_card(&v) {
                    Some(c) => {
                        card = Some(c);
                        break;
                    }
                    None => last_err = "chub 响应中未找到角色卡字段".into(),
                }
            }
            Ok(resp) => last_err = format!("chub API HTTP {}", resp.status()),
            Err(e) => last_err = format!("chub 请求失败：{e}"),
        }
    }
    let Some(card) = card else {
        return err_with_code(
            StatusCode::BAD_GATEWAY,
            "CHUB_IMPORT_FAILED",
            last_err,
            json!({"url": body.url, "path": path}),
        );
    };
    let wrapped = normalize_card(card);

    // 委托 st-import 管线：headers 原样透传（鉴权 + application/json），
    // body 换成归一后的卡片 JSON。
    let bytes = axum::body::Bytes::from(
        serde_json::to_vec(&wrapped).unwrap_or_default(),
    );
    crate::st_import::import_st(State(state), headers, bytes).await
}

/// 从 chub 站点 URL 提取 `{author}/{char}` 路径。
/// 兼容：chub.ai/characters/…、www.chub.ai/…、venus.chub.ai/…、裸 author/name。
fn chub_path(url: &str) -> Result<String, String> {
    let u = url.trim();
    let u = u.split(['?', '#']).next().unwrap_or(u);
    let u = u.trim_end_matches('/');
    let path = match u.find("://") {
        Some(i) => {
            let rest = &u[i + 3..];
            let slash = rest.find('/').ok_or_else(|| "URL 缺少路径".to_string())?;
            &rest[slash + 1..]
        }
        None => u.trim_start_matches('/'),
    };
    let path = path.strip_prefix("characters/").unwrap_or(path).to_string();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return Err("需要形如 https://chub.ai/characters/{author}/{name} 的链接".into());
    }
    Ok(segs.join("/"))
}

/// 从 chub 响应里挑出角色卡对象（node.data → node → data → 根，取首个含 name 的）。
fn pick_card(v: &Value) -> Option<Value> {
    let candidates = [
        v.get("node").and_then(|n| n.get("data")),
        v.get("node").and_then(|n| n.get("card")),
        v.get("node"),
        v.get("data"),
        Some(v),
    ];
    for c in candidates.into_iter().flatten() {
        if c.get("name").is_some()
            && (c.get("description").is_some() || c.get("tagline").is_some())
        {
            return Some(c.clone());
        }
    }
    None
}

/// 归一为 chara_card_v2：已带 spec 的透传，裸卡对象包壳。
fn normalize_card(card: Value) -> Value {
    if card.get("spec").is_some() && card.get("data").is_some() {
        return card;
    }
    json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "data": card,
    })
}
