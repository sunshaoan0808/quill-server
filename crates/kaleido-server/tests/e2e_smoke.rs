//! R2 收官：e2e 冒烟 —— 起真实服务器二进制 + 本地 mock LLM，走完整 HTTP 链路：
//! /health → /api/v1/public/info → login → /api/mobile/chat/start → /api/mobile/stream。
//! 零外部依赖（mock LLM 为测试内 TCP 线程；DataRoot 为临时目录）。

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    base: String,
    stderr_log: std::path::PathBuf,
    _mock_port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// OpenAI 兼容 SSE mock（/v1/chat/completions）：两段 delta + usage + [DONE]。
fn spawn_mock_llm() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            std::thread::spawn(move || {
                // 读全请求（头 + content-length body），避免对端写失败
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 16384];
                loop {
                    let Ok(n) = s.read(&mut chunk) else { return };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                        let clen: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if buf.len() >= head_end + clen {
                            break;
                        }
                    }
                }
                let body = "data: {\"choices\":[{\"delta\":{\"content\":\"你好，\"}}]}\n\n\
                            data: {\"choices\":[{\"delta\":{\"content\":\"旅行者。\"}}]}\n\n\
                            data: {\"choices\":[{\"delta\":{}},\"usage\":{\"total_tokens\":8}]}\n\n\
                            data: [DONE]\n\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            });
        }
    });
    port
}

fn spawn_server(llm_base: &str) -> Server {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("port probe");
        l.local_addr().unwrap().port()
    };
    let data = std::env::temp_dir().join(format!(
        "kaleido-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&data).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_kaleido-server"))
        .env("KALEIDO_DATA", &data)
        .env("KALEIDO_ADMIN_PASSWORD", "e2e-pass-123")
        .env("KALEIDO_HOST", "127.0.0.1")
        .env("KALEIDO_PORT", port.to_string())
        .env("LLM_BASE_URL", llm_base)
        .env("LLM_API_KEY", "e2e-key")
        .env("LLM_MODEL", "e2e-model")
        .stdout(Stdio::null())
        .stderr({
            let err_log = data.with_extension("server-stderr.log");
            std::fs::File::create(&err_log).expect("create stderr log")
        })
        .spawn()
        .expect("spawn server binary");
    Server {
        child,
        base: format!("http://127.0.0.1:{port}"),
        stderr_log: data.with_extension("server-stderr.log"),
        _mock_port: 0,
    }
}

async fn wait_healthy(base: &str) -> Option<reqwest::Client> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(format!("{base}/health")).send().await {
            if resp.status().is_success() {
                return Some(client);
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_smoke_boot_login_chat_turn() {
    let mock_port = spawn_mock_llm();
    let server = spawn_server(&format!("http://127.0.0.1:{mock_port}/v1"));
    let Some(client) = wait_healthy(&server.base).await else {
        panic!(
            "服务器 90s 未就绪；stderr 日志: {}",
            server.stderr_log.display()
        );
    };

    // 1) 公共信息
    let info: serde_json::Value = client
        .get(format!("{}/api/v1/public/info", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(info.get("name").is_some(), "public/info 缺 name: {info}");

    // 2) 坏口令必须 4xx（不得 500）
    let bad = client
        .post(format!("{}/api/v1/auth/login", server.base))
        .json(&serde_json::json!({"username": "admin", "password": "wrong-pass"}))
        .send()
        .await
        .unwrap();
    assert!(
        bad.status().is_client_error(),
        "错误口令应 4xx，实际 {}",
        bad.status()
    );

    // 3) 正确登录
    #[derive(serde::Deserialize)]
    struct LoginResp {
        token: String,
    }
    let login: LoginResp = client
        .post(format!("{}/api/v1/auth/login", server.base))
        .json(&serde_json::json!({"username": "admin", "password": "e2e-pass-123"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!login.token.is_empty());
    let bearer = format!("Bearer {}", login.token);

    // 4) 发起 partner chat（真实 LLM 指向本地 mock）
    let start: serde_json::Value = client
        .post(format!("{}/api/mobile/chat/start", server.base))
        .header("Authorization", &bearer)
        .json(&serde_json::json!({"message": "打个招呼", "model": "e2e-model"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = start["runId"]
        .as_str()
        .or_else(|| start["id"].as_str())
        .expect("chat/start 缺 runId")
        .to_string();

    // 5) 读 SSE 流到终态，收集 delta
    let resp = client
        .get(format!("{}/api/mobile/stream?runId={}", server.base, run_id))
        .header("Authorization", &bearer)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .expect("stream 请求失败");
    assert!(resp.status().is_success(), "stream HTTP {}", resp.status());
    let body = resp.text().await.unwrap();
    let mut deltas = String::new();
    let mut done = false;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            done = true;
            break;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if v["eventType"] == "delta" {
                if let Some(d) = v["delta"].as_str() {
                    deltas.push_str(d);
                }
            }
            if v["eventType"] == "error" {
                panic!("流内错误事件: {data}");
            }
            if v["eventType"] == "done" {
                done = true;
            }
        }
    }
    assert!(deltas.contains("你好"), "未收到预期 delta: {deltas:?}; body={body}");
    assert!(done, "SSE 流未收到终止事件");
}
