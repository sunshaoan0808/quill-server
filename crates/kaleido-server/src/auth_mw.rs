//! Auth middleware + bearer/session helpers + login DTOs (P0-1)
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method},
    middleware::Next,
    response::Response,
};
use kaleido_core::SessionRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error_map::map_core_err;
use crate::error_codes::*;
use crate::tickets::{consume_sse_ticket, peek_sse_ticket};
use std::net::{IpAddr, SocketAddr};

use crate::state::AppState;

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    pub(crate) username: String,
    pub(crate) password: String,
}

#[derive(Serialize)]
pub(crate) struct LoginResponse {
    pub(crate) token: String,
    pub(crate) user_id: String,
    pub(crate) username: String,
    pub(crate) workspace_id: String,
    pub(crate) expires_at: String,
}

pub(crate) fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(t) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
        {
            return Some(t.trim().to_string());
        }
    }
    // mobile compat: X-Mobile-Token / X-Kaleido-Token
    for key in ["x-mobile-token", "x-kaleido-token"] {
        if let Some(v) = headers.get(key).and_then(|v| v.to_str().ok()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

// ── 登录限流键（P0 审计修复 服务路由与聊天管线-4）────────────────────────────
// 旧逻辑盲信 X-Forwarded-For：直连部署下攻击者每次请求轮换该头即可获得全新
// 限流桶，KALEIDO_LOGIN_MAX_ATTEMPTS 形同虚设。现在仅当直连对端（ConnectInfo
// 的真实 IP）落在 KALEIDO_TRUSTED_PROXIES（逗号分隔的 IP 或 CIDR，如
// "172.18.0.0/16,::1"；docker 部署经 Caddy 反代时配置其网段）内时才采信 XFF；
// 未配置（默认，直连部署）一律用真实对端 IP。
//
// XFF 按标准语义解析：条目自左向右为 "client, proxy1, proxy2"，从右往左跳过
// 受信代理添加的条目，第一个非受信 IP 即原始客户端（防客户端自带伪造 XFF
// 穿过受信代理）。任一条目非法或全部为受信代理 → 回落对端 IP（fail-closed）。

/// 受信代理列表，进程内只解析一次。
fn trusted_proxies() -> &'static Vec<(IpAddr, u8)> {
    use std::sync::OnceLock;
    static PROXIES: OnceLock<Vec<(IpAddr, u8)>> = OnceLock::new();
    PROXIES.get_or_init(|| {
        std::env::var("KALEIDO_TRUSTED_PROXIES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(parse_ip_cidr)
            .collect()
    })
}

/// "1.2.3.4" → (v4, 32)；"10.0.0.0/8" → (v4, 8)；IPv6 同理。非法条目忽略。
fn parse_ip_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (ip_s, prefix) = match s.split_once('/') {
        Some((ip, p)) => (ip, Some(p)),
        None => (s, None),
    };
    let ip: IpAddr = ip_s.trim().parse().ok()?;
    let max_bits: u8 = if ip.is_ipv4() { 32 } else { 128 };
    let bits = match prefix {
        Some(p) => {
            let p: u8 = p.trim().parse().ok()?;
            if p > max_bits {
                return None;
            }
            p
        }
        None => max_bits,
    };
    Some((ip, bits))
}

fn ip_is_trusted(ip: &IpAddr, list: &[(IpAddr, u8)]) -> bool {
    list.iter().any(|(net, bits)| ip_in_cidr(ip, net, *bits))
}

/// bits 前缀内的字节相等比较；a/b 总是同族同长（v4×4 / v6×16 字节）。
fn prefix_eq(a: &[u8], b: &[u8], bits: u8) -> bool {
    let full = (bits / 8) as usize;
    if full > a.len() {
        return false;
    }
    if &a[..full] != &b[..full] {
        return false;
    }
    let rem = bits % 8;
    if rem == 0 || full == a.len() {
        return true;
    }
    let mask = 0xFFu8 << (8 - rem);
    (a[full] & mask) == (b[full] & mask)
}

fn ip_in_cidr(ip: &IpAddr, net: &IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            prefix_eq(&u32::from(*a).to_be_bytes(), &u32::from(*b).to_be_bytes(), bits)
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => prefix_eq(&a.octets(), &b.octets(), bits),
        // v4-mapped v6（::ffff:a.b.c.d）与 v4 互比，覆盖双栈监听场景。
        (IpAddr::V6(a), IpAddr::V4(b)) => match a.to_ipv4_mapped() {
            Some(a4) => prefix_eq(&u32::from(a4).to_be_bytes(), &u32::from(*b).to_be_bytes(), bits),
            None => false,
        },
        (IpAddr::V4(a), IpAddr::V6(b)) => match b.to_ipv4_mapped() {
            Some(b4) => prefix_eq(&u32::from(*a).to_be_bytes(), &u32::from(b4).to_be_bytes(), bits),
            None => false,
        },
    }
}

/// 从右往左跳过受信代理条目，返回第一个非受信 IP；任一条目非法或全部受信
/// 返回 None（调用方回落对端 IP）。
fn client_ip_from_xff(xff: &str, trusted: &[(IpAddr, u8)]) -> Option<IpAddr> {
    for part in xff.split(',').rev() {
        let ip: IpAddr = part.trim().parse().ok()?;
        if !ip_is_trusted(&ip, trusted) {
            return Some(ip);
        }
    }
    None
}

pub(crate) fn client_key(headers: &HeaderMap, connect: Option<&SocketAddr>) -> String {
    let peer = connect.map(|a| a.ip());
    let trusted = trusted_proxies();
    let peer_trusted = peer
        .as_ref()
        .map(|ip| ip_is_trusted(ip, trusted))
        .unwrap_or(false);
    if peer_trusted {
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(ip) = client_ip_from_xff(xff, trusted) {
                return format!("ip:{}", ip);
            }
        }
    }
    match peer {
        Some(ip) => format!("ip:{}", ip),
        None => "ip:unknown".into(),
    }
}

/// P0 审计修复（服务路由与聊天管线-1 / 转换导出与爬虫-1）：允许 `?ticket=`
/// 认证的端点白名单。仅两类端点因客户端无法携带 header 而需要 ticket：
/// - SSE 流（EventSource 不支持自定义 header）：全部 */stream 路由
///   （/api/v1/story/stream、/api/mobile/stream、/api/v1/jobs/{id}/stream、
///   /api/v1/story-tavern/sessions/{id}/stream、/api/v1/background/stream、
///   /api/v1/book-travel/stream）
/// - GET /api/v1/appearance/wallpaper（CSS background-image 无法带 header）
/// 新增 ticket 端点必须显式登记到这里；其余路径一律要求 Bearer。
fn is_ticket_endpoint(path: &str) -> bool {
    path.ends_with("/stream") || path == "/api/v1/appearance/wallpaper"
}

pub(crate) async fn auth_middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    // public
    // Public: health/login + GET bookshelf list/content/cover only (writes need auth)
    let novels_get_public = method == Method::GET
        && (path == "/api/v1/crawler/novels"
            || path.starts_with("/api/v1/crawler/novels/"));
    // 分享链接公开只读（吸收 DEEIX share）：GET + 精确前缀，token 校验在 handler 内。
    let share_get_public = method == Method::GET && path.starts_with("/api/v1/story-tavern/share/");
    if matches!(
        path.as_str(),
        "/health" | "/api/v1/auth/login" | "/api/v1/public/info"
    ) || novels_get_public
        || share_get_public
        || path.starts_with("/web")
        || path == "/"
        || path == "/index.html"
        || path.starts_with("/assets/")
    {
        return next.run(req).await;
    }

    // OPTIONS for CORS preflight
    if method == Method::OPTIONS {
        return next.run(req).await;
    }

    // SSE 认证：仅接受一次性 ticket（M-3）。?token=/access_token= 回退已移除
    // （URL 会进访问日志/历史/Referer，见 docs/SECURITY_NOTES.md）。
    //
    // P0 审计修复（服务路由与聊天管线-1 / 转换导出与爬虫-1）：?ticket= 放行面
    // 收窄。旧逻辑对任何带 ?ticket= 的请求直接放行且完全不校验，而 moa_api /
    // book_export / import_scan / background::stream / book_travel::stream 等
    // handler 自身不做 session 校验 → 未认证读写旁路。现在：
    //   1. 仅已知的 ticket 消费端点放行（SSE 的 */stream 路由 + GET
    //      /api/v1/appearance/wallpaper——CSS background-image 无法带 header，
    //      见 appearance.rs M-3 注释）；
    //   2. 放行前先 peek 校验票据（存在/未使用/未过期）——完全依赖 middleware
    //      的端点也被强制认证；单次消费仍由 handler（session_from_any /
    //      appearance::session_from_headers_or_ticket）完成，语义不变；
    //   3. 其余路径带 ?ticket= 一律落入下方 Bearer 校验（无 Bearer → 401，
    //      fail-closed）。
    let token = extract_bearer(req.headers());
    if token.is_none() {
        if let Some(q) = req.uri().query() {
            let ticket = q.split('&').find_map(|part| {
                let mut kv = part.splitn(2, '=');
                match (kv.next(), kv.next()) {
                    (Some("ticket"), Some(v)) if !v.is_empty() => Some(v),
                    _ => None,
                }
            });
            if let Some(t) = ticket {
                if !is_ticket_endpoint(&path) {
                    // 非 ticket 消费端点：不接受 ?ticket=，落入下方 Bearer 校验。
                } else if peek_sse_ticket(t) {
                    // 有效一次性票据：放行给 handler 消费并解析 session。
                    return next.run(req).await;
                } else {
                    return unauthorized("invalid or expired SSE ticket");
                }
            }
        }
    }

    let Some(token) = token else {
        return unauthorized("missing bearer");
    };
    match state.auth.resolve_session(&token) {
        Ok(_) => next.run(req).await,
        Err(_) => unauthorized("invalid or expired session"),
    }
}


// ── M-3: short-lived one-time SSE tickets ─────────────────────────────────
// EventSource cannot set custom headers; the one-time ticket exists so the
// long-lived bearer never travels in a URL (leaked to logs/history/Referer).
// Clients POST a
// one-time, 5-minute-expiring ticket with their Authorization header, then use
// `?ticket=` for the SSE subscription. The ticket is single-use and bound to
// the issuing user.
pub(crate) fn session_from(state: &AppState, headers: &HeaderMap) -> Result<SessionRecord, Response> {
    session_from_any(state, headers, None)
}

/// P0-2 审计修复：admin 专用 session 解析——非 admin 一律 403。
/// ai_admin / sessions_stats / prune 等管理 handler 用它，防止越权访问管理面。
pub(crate) fn admin_session_from(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<SessionRecord, Response> {
    let session = session_from(state, headers)?;
    if !session.is_admin {
        return Err(forbidden("ADMIN_REQUIRED", "admin role required"));
    }
    Ok(session)
}

/// SSE/EventSource 场景认证：header Bearer 或一次性 ticket（M-3）。
/// 历史 `?token=`/`?access_token=` query 回退已移除——URL 凭据会泄漏到
/// 访问日志、浏览器历史与 Referer；EventSource 一律走 ticket。
pub(crate) fn session_from_any(
    state: &AppState,
    headers: &HeaderMap,
    query: Option<&HashMap<String, String>>,
) -> Result<SessionRecord, Response> {
    if let Some(ticket) = query.and_then(|q| q.get("ticket")) {
        if let Some(token) = consume_sse_ticket(ticket) {
            return state.auth.resolve_session(&token).map_err(map_core_err);
        }
        return Err(unauthorized("invalid or expired SSE ticket"));
    }
    let token =
        extract_bearer(headers).ok_or_else(|| unauthorized("missing bearer"))?;
    state.auth.resolve_session(&token).map_err(map_core_err)
}
