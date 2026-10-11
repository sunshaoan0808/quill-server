//! retry.rs — 分级退避重试（backport 自 museai-server PR #108，源吸收 dramatica-flow core/llm with_retry，MIT）。
//!
//! 纪律（抄 dramatica-flow）：
//! - 错误按「限流(429/排队) / 网络(超时/断线) / 其他」分三档退避：
//!   限流指数退避 3s*2^(n-1) 上限 60s + 随机抖动；断线 2s*2^(n-1) 上限 30s + 抖动；
//!   其他固定 delay*attempt 快速重试。
//! - 限流/断线用更高硬上限（_RETRY_HARD_CAP=7），普通错误用调用方给的 max_attempts。
//! - 纯同步工具：内部 sleep 用 std::thread::sleep，仅适合非异步上下文（review_tavern 等
//!   tokio 调用点自行 await 外部 sleep 版本）。

use std::time::Duration;

/// 限流(429/排队)与临时网络错误的最大重试次数（高于普通错误）。
pub const RETRY_HARD_CAP: u32 = 7;

/// 判断错误类型：
/// - "rate_limit"：服务端限流/排队（429），放慢指数退避多等
/// - "network"：连接/超时/断线重置，稍等重连
/// - ""：其他（JSON/格式等），走快速重试
pub fn retry_kind(msg: &str) -> &'static str {
    let m = msg.to_lowercase();
    if m.contains("429")
        || m.contains("rate limit")
        || m.contains("too many requests")
        || m.contains("rate_limit_reached")
    {
        return "rate_limit";
    }
    if m.contains("timeout")
        || m.contains("timed out")
        || m.contains("read timed")
        || m.contains("connection")
        || m.contains("refused")
        || m.contains("econnreset")
        || m.contains("econnrefused")
        || m.contains("server disconnected")
        || m.contains("remote disconnected")
        || m.contains("bad gateway")
        || m.contains("gateway error")
        || m.contains("internal server error")
    {
        return "network";
    }
    ""
}

/// 计算第 attempt 次重试前的退避时长（attempt 从 1 起）。
/// - rate_limit：min(60, 3*2^(n-1)) + [0,2)s 抖动
/// - network：min(30, 2*2^(n-1)) + [0,1)s 抖动
/// - 其他：delay * attempt（无抖动）
pub fn backoff_for(kind: &str, attempt: u32, delay_seconds: f64) -> Duration {
    let base = match kind {
        "rate_limit" => (3.0 * 2f64.powi(attempt as i32 - 1)).min(60.0),
        "network" => (2.0 * 2f64.powi(attempt as i32 - 1)).min(30.0),
        _ => delay_seconds * attempt as f64,
    };
    let jitter = match kind {
        "rate_limit" => rand::random::<f64>() * 2.0,
        "network" => rand::random::<f64>(),
        _ => 0.0,
    };
    Duration::from_secs_f64(base + jitter)
}

/// 同步重试包装：对 `attempt` 计数 + 退避 sleep 由调用方驱动时，
/// 本函数提供「是否还能再试」判定（普通错误 max_attempts，限流/断线 RETRY_HARD_CAP）。
pub fn can_retry(kind: &str, attempt: u32, max_attempts: u32) -> bool {
    let effective = if kind.is_empty() { max_attempts } else { RETRY_HARD_CAP };
    attempt < effective
}

/// 同步重试：fn 返回 Result<T, E>，E 需实现 AsRef<str>（String / &str 均可）。
/// 普通错误重试 max_attempts 次；限流/断线最多 RETRY_HARD_CAP 次，退避见 backoff_for。
/// 全部失败返回最后一次错误。
pub fn retry_sync<T, E, F>(mut fn_call: F, max_attempts: u32, delay_seconds: f64) -> Result<T, E>
where
    E: AsRef<str> + Clone,
    F: FnMut() -> Result<T, E>,
{
    #[allow(unused_assignments)] // 初值恒被覆盖：break 前必先写入 Some(e)
    let mut last_err: Option<E> = None;
    let mut attempt: u32 = 1;
    loop {
        match fn_call() {
            Ok(v) => return Ok(v),
            Err(e) => {
                let kind = retry_kind(e.as_ref());
                last_err = Some(e);
                if !can_retry(kind, attempt, max_attempts) {
                    break;
                }
                std::thread::sleep(backoff_for(kind, attempt, delay_seconds));
                attempt += 1;
            }
        }
    }
    Err(last_err.expect("loop always sets last_err"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_classification() {
        assert_eq!(retry_kind("HTTP 429 rate limit"), "rate_limit");
        assert_eq!(retry_kind("too many requests: retry later"), "rate_limit");
        assert_eq!(retry_kind("rate_limit_reached"), "rate_limit");
        assert_eq!(retry_kind("connection refused"), "network");
        assert_eq!(retry_kind("timed out after 30s"), "network");
        assert_eq!(retry_kind("bad gateway"), "network");
        assert_eq!(retry_kind("json parse error"), "");
        assert_eq!(retry_kind(""), "");
    }

    #[test]
    fn hard_cap_for_rate_limit() {
        // 普通错误 3 次封顶；限流/断线 7 次封顶
        assert!(can_retry("", 3, 3) == false);
        assert!(can_retry("", 2, 3));
        assert!(can_retry("rate_limit", 7, 3) == false);
        assert!(can_retry("rate_limit", 6, 3));
        assert!(can_retry("network", 7, 3) == false);
        assert!(can_retry("network", 6, 3));
    }

    #[test]
    fn backoff_increases_with_attempt() {
        let a1 = backoff_for("rate_limit", 1, 2.0);
        let a2 = backoff_for("rate_limit", 2, 2.0);
        let a3 = backoff_for("rate_limit", 3, 2.0);
        assert!(a2 >= a1, "rate_limit 应递增: {a1:?} vs {a2:?}");
        assert!(a3 >= a2, "rate_limit 应递增: {a2:?} vs {a3:?}");
        let n1 = backoff_for("network", 1, 2.0);
        let n2 = backoff_for("network", 2, 2.0);
        assert!(n2 >= n1, "network 应递增: {n1:?} vs {n2:?}");
        // 上限 60s / 30s
        let big = backoff_for("rate_limit", 10, 2.0);
        assert!(big.as_secs_f64() <= 62.0, "rate_limit 上限约 60s+2 抖动: {big:?}");
        let big_n = backoff_for("network", 10, 2.0);
        assert!(big_n.as_secs_f64() <= 31.0, "network 上限约 30s+1 抖动: {big_n:?}");
    }

    #[test]
    fn retry_sync_succeeds_after_failures() {
        let mut n = 0;
        let out = retry_sync(
            || {
                n += 1;
                if n < 3 {
                    Err("connection reset by peer".to_string())
                } else {
                    Ok(n)
                }
            },
            5,
            0.001,
        );
        assert_eq!(out, Ok(3));
    }

    #[test]
    fn retry_sync_gives_up_after_max() {
        let mut n = 0;
        let out: Result<i32, String> = retry_sync(
            || {
                n += 1;
                Err(format!("boom {n}"))
            },
            2,
            0.001,
        );
        assert!(out.is_err());
        assert_eq!(n, 2);
    }

    #[test]
    fn retry_sync_hard_cap_wins_on_rate_limit() {
        let mut n = 0;
        let out: Result<i32, String> = retry_sync(
            || {
                n += 1;
                Err("HTTP 429 too many requests".to_string())
            },
            2, // 普通上限 2，但限流走 HARD_CAP=7
            0.001,
        );
        assert!(out.is_err());
        assert_eq!(n, RETRY_HARD_CAP as i32);
    }
}
