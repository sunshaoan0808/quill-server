//! llm_stream 测试（R6-3② 自 llm_stream.rs 迁入，零改动）。
#![allow(unused_imports)]
use super::super::*;

#[cfg(test)]
mod tests {
    use super::super::extract_json_value;

    #[test]
    fn extract_json_value_plain_object() {
        let v = extract_json_value(r#"{"a":1}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn extract_json_value_object_with_trailing_text() {
        // 模型在合法 JSON 后追加文字（常见于 deepseek/grok 免费档）
        let t = r#"{"settings":[{"key":"世界观","value":"现代都市"}],"direction":"测试"}
我们根据系统提示，需要输出一个JSON，包含settings, direction 等字段。我们应遵循格式。"#;
        let v = extract_json_value(t).unwrap();
        assert_eq!(v["direction"], "测试");
        assert_eq!(v["settings"][0]["key"], "世界观");
    }

    #[test]
    fn extract_json_value_array_with_trailing_text() {
        let t = r#"[{"severity":"major","issue":"情节矛盾"}]
以上是审稿结果。"#;
        let v = extract_json_value(t).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["severity"], "major");
    }

    #[test]
    fn extract_json_value_code_fence() {
        let t = "```json\n{\"ok\":true}\n```";
        let v = extract_json_value(t).unwrap();
        assert_eq!(v["ok"], true);
    }

    // -------------------------------------------------------------------
    // F3: stream_chat_turn_dispatch 单测 —— 本地 TCP 假上游，覆盖三家协议
    // 解析 + Stopped/Status 错误路径（不依赖外网）。
    // -------------------------------------------------------------------

    use super::super::{stream_chat_turn_dispatch, TurnStreamEvent, TurnStreamError};

    /// 起一个一次性 TCP 服务：读掉请求头后按给定响应体回包并关闭。
    async fn spawn_mock_upstream(response: &'static [u8]) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await; // 读请求头（内容不校验）
            let _ = sock.write_all(response).await;
            let _ = sock.shutdown().await;
        });
        port
    }

    fn no_stop() -> bool {
        false
    }

    #[tokio::test]
    async fn turn_openai_parses_content_thinking_usage() {
        let resp = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hmm\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}],\"usage\":{\"total_tokens\":42}}\n\n\
data: [DONE]\n\n";
        let port = spawn_mock_upstream(resp).await;
        let mut events = Vec::new();
        let out = stream_chat_turn_dispatch(
            &format!("http://127.0.0.1:{port}/v1"), "sk-test", "test-model", "",
            "sys", "usr", 0.75, 1024, 30,
            |ev| {
                events.push(ev);
                true
            },
            no_stop,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "hello");
        assert_eq!(out.thinking, "hmm");
        assert_eq!(out.total_tokens, Some(42));
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], TurnStreamEvent::Thinking(t) if t == "hmm"));
        assert!(matches!(&events[1], TurnStreamEvent::Delta(d) if d == "he"));
        assert!(matches!(&events[2], TurnStreamEvent::Delta(d) if d == "llo"));
    }

    #[tokio::test]
    async fn turn_openai_status_error_carries_body() {
        let resp = b"HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\
{\"error\":{\"code\":\"model_not_found\"}}";
        let port = spawn_mock_upstream(resp).await;
        let err = stream_chat_turn_dispatch(
            &format!("http://127.0.0.1:{port}/v1"), "sk-test", "ghost-model", "",
            "sys", "usr", 0.75, 1024, 30,
            |_| true,
            no_stop,
        )
        .await
        .unwrap_err();
        match err {
            TurnStreamError::Status { status, body } => {
                assert_eq!(status, 404);
                assert!(super::super::is_model_rejection(status, &body));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn turn_stops_on_should_stop() {
        let resp = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
data: {\"choices\":[{\"delta\":{\"content\":\"abc\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"def\"}}]}\n\n\
data: [DONE]\n\n";
        let port = spawn_mock_upstream(resp).await;
        let err = stream_chat_turn_dispatch(
            &format!("http://127.0.0.1:{port}/v1"), "sk-test", "m", "",
            "sys", "usr", 0.75, 1024, 30,
            |_| true,
            || true, // 立即停止
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TurnStreamError::Stopped));
    }

    #[tokio::test]
    async fn turn_anthropic_parses_text_thinking_and_usage() {
        let resp = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":11}}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"reasoning\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"answer\"}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let port = spawn_mock_upstream(resp).await;
        let mut kinds = Vec::new();
        let out = stream_chat_turn_dispatch(
            &format!("http://127.0.0.1:{port}"), "sk-ant", "claude-x", "anthropic",
            "sys", "usr", 0.75, 1024, 30,
            |ev| {
                kinds.push(match ev {
                    TurnStreamEvent::Delta(_) => "d",
                    TurnStreamEvent::Thinking(_) => "t",
                });
                true
            },
            no_stop,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "answer");
        assert_eq!(out.thinking, "reasoning");
        assert_eq!(out.total_tokens, Some(18)); // input 11 + output 7
        assert_eq!(kinds, vec!["t", "d"]);
    }

    #[tokio::test]
    async fn turn_google_parses_parts_thought_and_usage() {
        let resp = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\
{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"think\"},{\"thought\":true,\"text\":\"th\"}]}}],\"usageMetadata\":{\"totalTokenCount\":33}}\n\
{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ing\"}]}}]}\n";
        let port = spawn_mock_upstream(resp).await;
        let mut kinds = Vec::new();
        let out = stream_chat_turn_dispatch(
            &format!("http://127.0.0.1:{port}"), "g-key", "gemini-x", "google",
            "sys", "usr", 0.75, 1024, 30,
            |ev| {
                kinds.push(match ev {
                    TurnStreamEvent::Delta(_) => "d",
                    TurnStreamEvent::Thinking(_) => "t",
                });
                true
            },
            no_stop,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "thinking");
        assert_eq!(out.thinking, "th");
        assert_eq!(out.total_tokens, Some(33));
        assert_eq!(kinds.len(), 3);
    }

    #[test]
    fn is_model_rejection_matches_legacy_strings() {
        assert!(super::super::is_model_rejection(404, "{\"error\":\"model_not_found\"}"));
        assert!(!super::super::is_model_rejection(500, "model_not_found"));
        assert!(!super::super::is_model_rejection(404, "rate limited"));
    }

    // -------------------------------------------------------------------
    // Prompt cache：Anthropic system 形态 —— 大 system 挂 cache_control、
    // 小 system 保持纯字符串（捕获请求体断言，不依赖外网）。
    // -------------------------------------------------------------------

    use std::sync::{Arc, Mutex};

    /// 起一个捕获完整请求（header + Content-Length body）的假上游。
    async fn spawn_mock_upstream_capture(
        response: &'static [u8],
    ) -> (u16, Arc<Mutex<Vec<u8>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 4096];
            // 读到 header 结束，再按 Content-Length 收满 body
            let mut body_len: Option<usize> = None;
            let mut head_end: Option<usize> = None;
            loop {
                let n = match sock.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                buf.extend_from_slice(&tmp[..n]);
                if head_end.is_none() {
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(pos);
                        let head = String::from_utf8_lossy(&buf[..pos]);
                        body_len = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                if k.eq_ignore_ascii_case("content-length") {
                                    v.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .or(Some(0));
                    }
                }
                if let (Some(pos), Some(cl)) = (head_end, body_len) {
                    if buf.len() >= pos + 4 + cl {
                        break;
                    }
                }
            }
            *cap.lock().unwrap() = buf;
            let _ = sock.write_all(response).await;
            let _ = sock.shutdown().await;
        });
        (port, captured)
    }

    fn anthropic_sse_resp() -> &'static [u8] {
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    }

    #[tokio::test]
    async fn turn_anthropic_large_system_gets_cache_control() {
        let (port, captured) = spawn_mock_upstream_capture(anthropic_sse_resp()).await;
        // 4800 字符中文 → 估算 ≈1371 token ≥ 1024
        let system = "长系统提示。".repeat(800);
        let out = stream_chat_turn_dispatch(
            &format!("http://127.0.0.1:{port}"), "sk-ant", "claude-x", "anthropic",
            &system, "usr", 0.75, 1024, 30,
            |_| true,
            no_stop,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "");
        let raw = captured.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&raw);
        let body_str = text.split("\r\n\r\n").nth(1).expect("has body");
        let v: serde_json::Value = serde_json::from_str(body_str).expect("json body");
        assert!(v["system"].is_array(), "large system must be block array: {}", v["system"]);
        assert_eq!(v["system"][0]["type"], "text");
        assert_eq!(v["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(v["system"][0]["text"], system);
    }

    #[tokio::test]
    async fn turn_anthropic_small_system_stays_plain_string() {
        let (port, captured) = spawn_mock_upstream_capture(anthropic_sse_resp()).await;
        let out = stream_chat_turn_dispatch(
            &format!("http://127.0.0.1:{port}"), "sk-ant", "claude-x", "anthropic",
            "sys", "usr", 0.75, 1024, 30,
            |_| true,
            no_stop,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "");
        let raw = captured.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&raw);
        let body_str = text.split("\r\n\r\n").nth(1).expect("has body");
        assert!(!body_str.contains("cache_control"), "small system must not attach cache_control");
        let v: serde_json::Value = serde_json::from_str(body_str).expect("json body");
        assert_eq!(v["system"], "sys");
    }
}
