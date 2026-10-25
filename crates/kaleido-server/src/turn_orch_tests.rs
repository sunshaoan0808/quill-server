//! [R2 2026-10-06] start_turn 编排层集成测试：幂等守卫 / U11 resume / 并发护栏 /
//! panic 看门狗 / 客户端断连。伪 LLM 上游（本地 TCP SSE）+ 临时目录 store。
//! 每个用例独立 AppState（独立 DataRoot），互不共享状态。
use crate::state::AppState;
use crate::story_tavern::{start_turn, TurnQuality, TurnStartRequest};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::Json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static SEQ: AtomicU64 = AtomicU64::new(0);
const ADMIN_PW: &str = "orch-test-pw-2026";

const SSE_STATIC: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}],\"usage\":{\"total_tokens\":10}}\n\n\
data: [DONE]\n\n";

/// 伪 LLM 上游：独立线程 + 同步 socket。
/// "static" = 回完即关；"streaming" = 持续吐 delta（供断连测试）。
fn spawn_mock(mode: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                // 必须读完整个请求（头 + content-length 指定的 body）再响应，
                // 否则客户端还在写 body 时对端关闭 → "error sending request"。
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 16384];
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let head_end = buf
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| p + 4);
                    if let Some(head_end) = head_end {
                        let headers = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                        let clen: usize = headers
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if buf.len() >= head_end + clen {
                            break;
                        }
                    }
                }
                match mode {
                    "static" => {
                        let _ = s.write_all(SSE_STATIC);
                        let _ = s.flush();
                    }
                    _ => {
                        let _ = s
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
                        for i in 0..600 {
                            if s.write_all(
                                format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"第{i}句。\"}}}}]}}\n\n")
                                    .as_bytes(),
                            )
                            .is_err()
                            {
                                break;
                            }
                            let _ = s.flush();
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            });
        }
    });
    port
}

fn orch_state(tag: &str, llm_base: Option<String>) -> AppState {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("kaleido-orch-{tag}-{nanos}-{seq}"));
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("KALEIDO_ADMIN_PASSWORD", ADMIN_PW);
    let data = kaleido_core::DataRoot::new(root.join("d")).unwrap();
    let app_state = kaleido_core::AppStateStore::new(data.clone());
    AppState {
        auth: kaleido_core::AuthStore::load(data.clone()).unwrap(),
        jobs: kaleido_core::JobStore::with_max_concurrent(data.clone(), 2),
        sessions: kaleido_core::AgentSessionStore::new(data.clone()),
        app_state: app_state.clone(),
        partner: kaleido_core::PartnerStore::new(app_state.clone()),
        works: kaleido_core::WorksFs::new(data.clone()),
        packs: kaleido_core::PackStore::new(data.clone()),
        search: kaleido_core::hybrid_search::SearchIndex::new(data.clone()).unwrap(),
        sessions_tavern: kaleido_core::TavernSessionStore::new(data.clone()),
        personas: kaleido_core::TavernPersonaStore::new(data.clone()),
        regex_library: kaleido_core::RegexLibraryStore::new(&data),
        vector_index: kaleido_core::VectorIndexStore::new(&data),
        hub: std::sync::Arc::new(crate::state::StreamHub::new()),
        llm_base,
        llm_key: Some("orch-test-key".into()),
        llm_model: "orch-test-model".into(),
        provider_kind: "OpenAI".into(),
        embedding_base: None,
        image_base_url: None,
        image_api_key: None,
        image_model: "t".into(),
        cf_image_base_url: None,
        cf_image_model: None,
        grok2api_image_base_url: None,
        grok2api_image_key: None,
        grok2api_image_model: None,
        a1111_base_url: None,
        plugin_registry: std::sync::Arc::new(kaleido_core::plugin::PluginRegistry::new()),
        world_state: Default::default(),
        weaver_config: Default::default(),
        graph: kaleido_core::graph_store::GraphStore::open_in_memory().unwrap(),
        foreshadow: kaleido_core::foreshadow_store::ForeshadowStore::open_in_memory().unwrap(),
        analysis: kaleido_core::analysis_store::AnalysisStore::open_in_memory().unwrap(),
        ai_admin: kaleido_core::ai_admin_store::AiAdminStore::open_in_memory().unwrap(),
        scene_cards: kaleido_core::scene_card_store::SceneCardStore::open_in_memory().unwrap(),
        reference_library: crate::reference_library::ReferenceLibraryStore::new(data.root()),
        rpm: crate::ai_admin::RpmLimiter::new(),
        director_tasks: std::sync::Arc::new(kaleido_core::DirectorTaskGroup::new()),
        reviews: kaleido_core::ReviewStore::new(data.clone()),
    }
}

async fn login_token(state: &AppState) -> (String, String, String) {
    let rec = state
        .auth
        .login("admin", ADMIN_PW, "orch-test")
        .expect("admin login");
    (rec.token, rec.user_id, rec.workspace_id)
}

async fn seed_session(state: &AppState, user_id: &str) -> String {
    let pack = state.packs.ensure_demo_pack().expect("demo pack");
    let req: kaleido_core::CreateSessionRequest = serde_json::from_value(serde_json::json!({
        "packId": pack.id,
        "owner": user_id,
    }))
    .expect("session request");
    let sess = state
        .sessions_tavern
        .create_from_pack(&state.packs, req)
        .expect("create session");
    sess.session_id
}

fn bearer_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    h
}

async fn call_start(state: &AppState, token: &str, session_id: &str, message: &str) -> axum::response::Response {
    let headers = bearer_headers(token);
    let body = TurnStartRequest {
        message: message.to_string(),
        model: None,
        quality: Some(TurnQuality::Lite),
    };
    start_turn(
        State(state.clone()),
        headers,
        Path(session_id.to_string()),
        Json(body),
    )
    .await
}

async fn resp_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// 订阅 run 的事件流并持续排水（模拟生产中前端先开 SSE 再收流的时序）。
/// worker 的 hub.send 在无接收者时会被判为客户端断连 → 立即取消收尾，
/// 故每个“让回合正常跑完”的用例必须在 call_start 返回后立刻订阅。
fn spawn_drainer(st: &AppState, run_id: &str) {
    if let Some((mut rx, _replay)) = st.hub.subscribe(run_id) {
        tokio::spawn(async move {
            while rx.recv().await.is_ok() {}
        });
    }
}

async fn wait_until<F: FnMut() -> bool>(mut f: F, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if f() {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ─────────────────────────────────────────────────────────────────
// 用例①：重复提交幂等守卫
// ─────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_submit_is_rejected_after_completion() {
    let st = orch_state("dup", Some(format!("http://127.0.0.1:{}/v1", spawn_mock("static"))));
    let (token, user_id, _ws) = login_token(&st).await;
    let sid = seed_session(&st, &user_id).await;

    let resp = call_start(&st, &token, &sid, "你好").await;
    assert!(resp.status().is_success(), "first turn must be accepted");
    let v = resp_json(resp).await;
    let run_id = v["runId"].as_str().expect("runId in accept receipt").to_string();
    spawn_drainer(&st, &run_id);

    // 等待回合完成：demo pack 自带开场 assistant 消息，须等「第二条」assistant（本回合回应）
    let st2 = st.clone();
    let sid2 = sid.clone();
    let done = wait_until(
        move || {
            st2.sessions_tavern
                .get(&sid2)
                .map(|s| s.messages.iter().filter(|m| m.role == "assistant").count() >= 2)
                .unwrap_or(false)
        },
        Duration::from_secs(25),
    )
    .await;
    assert!(done, "worker should persist assistant message");

    // 等回合完全收尾（worker 清理 hub + 释放会话锁），否则二次提交撞上并发护栏 409
    let st3 = st.clone();
    let sid3 = sid.clone();
    let released = wait_until(
        move || {
            st3.sessions_tavern
                .get(&sid3)
                .map(|s| s.active_run_id.is_none())
                .unwrap_or(false)
        },
        Duration::from_secs(15),
    )
    .await;
    assert!(released, "first turn must fully finish before second submit");

    // 同内容二次提交 → 幂等回执（无第二次 LLM 副作用）
    let resp2 = call_start(&st, &token, &sid, "你好").await;
    let v = resp_json(resp2).await;
    assert_eq!(
        v["accepted"],
        serde_json::Value::Bool(false),
        "second submit must be rejected: {v}"
    );
    assert_eq!(v["reason"], "duplicate_submit");
}

// ─────────────────────────────────────────────────────────────────
// 用例②：U11 resume —— 孤儿 running job（hub 无 live worker）
// ─────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn orphan_running_job_is_rearmed_and_superseded() {
    let st = orch_state("resume", Some(format!("http://127.0.0.1:{}/v1", spawn_mock("static"))));
    let (token, user_id, ws) = login_token(&st).await;
    let sid = seed_session(&st, &user_id).await;

    // 伪造孤儿 running job + active_run_id（无任何 live worker）
    let job = st
        .jobs
        .create(
            "tavern-turn",
            &user_id,
            &ws,
            serde_json::json!({"sessionId": sid}),
            None,
            None,
        )
        .expect("create orphan job");
    assert_eq!(job.status, "running");
    let updated = st
        .sessions_tavern
        .acquire_turn(&sid, &job.run_id)
        .expect("acquire");
    st.sessions_tavern.save(updated).expect("save active_run_id");

    let resp = call_start(&st, &token, &sid, "恢复后的新回合").await;
    assert!(
        resp.status().is_success(),
        "resume must not 409 when hub has no live worker"
    );
    let v = resp_json(resp).await;
    if let Some(new_run) = v["runId"].as_str() {
        spawn_drainer(&st, new_run);
    }

    // 旧 run 被 cancelled 终态收尾
    let st2 = st.clone();
    let orphan = job.run_id.clone();
    let superseded = wait_until(
        move || {
            st2.jobs
                .get(&orphan)
                .map(|j| j.status == "cancelled")
                .unwrap_or(false)
        },
        Duration::from_secs(10),
    )
    .await;
    assert!(superseded, "orphan job must be closed as cancelled");

    // 新回合正常完成（demo pack 开场 assistant 之外的新回应）
    let st3 = st.clone();
    let sid3 = sid.clone();
    let done = wait_until(
        move || {
            st3.sessions_tavern
                .get(&sid3)
                .map(|s| s.messages.iter().filter(|m| m.role == "assistant").count() >= 2)
                .unwrap_or(false)
        },
        Duration::from_secs(25),
    )
    .await;
    assert!(done, "new turn should complete after resume");
}

// ─────────────────────────────────────────────────────────────────
// 用例③：并发护栏 —— live worker 存在时第二请求 409
// ─────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_turn_returns_409_when_live_worker() {
    let st = orch_state("busy", None);
    let (token, user_id, ws) = login_token(&st).await;
    let sid = seed_session(&st, &user_id).await;

    let job = st
        .jobs
        .create(
            "tavern-turn",
            &user_id,
            &ws,
            serde_json::json!({"sessionId": sid}),
            None,
            None,
        )
        .expect("create running job");
    let updated = st
        .sessions_tavern
        .acquire_turn(&sid, &job.run_id)
        .expect("acquire");
    st.sessions_tavern.save(updated).expect("save");
    // 关键：hub 里有挂着的 worker（register 后未 cleanup）
    let _tx = st.hub.register(&job.run_id);

    let resp = call_start(&st, &token, &sid, "并发请求").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT, "live worker must 409");
    let v = resp_json(resp).await;
    assert_eq!(v["code"], "ST_TURN_BUSY");
}

// ─────────────────────────────────────────────────────────────────
// 用例④：worker panic → 看门狗兜底（job failed / 清锁 / hub 清理）
// ─────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn worker_panic_is_caught_by_watchdog() {
    let st = orch_state("panic", Some(format!("http://127.0.0.1:{}/v1", spawn_mock("static"))));
    let (token, user_id, _ws) = login_token(&st).await;
    let sid = seed_session(&st, &user_id).await;

    let resp = call_start(&st, &token, &sid, "__panic_probe__").await;
    assert!(resp.status().is_success());
    let v = resp_json(resp).await;
    if let Some(run) = v["runId"].as_str() {
        spawn_drainer(&st, run);
    }

    // 看门狗兜底：worker panic → job 以 failed 终态收尾（finish("error") 归一化为 failed）
    let st2 = st.clone();
    let uid2 = user_id.clone();
    let failed = wait_until(
        move || {
            st2.jobs
                .list(kaleido_core::JobListFilter {
                    status: Some("failed".into()),
                    kind: Some("tavern_turn".into()),
                    user_id: Some(uid2.clone()),
                    workspace_id: None,
                    limit: 10,
                })
                .map(|v| !v.is_empty())
                .unwrap_or(false)
        },
        Duration::from_secs(15),
    )
    .await;
    assert!(failed, "watchdog must mark panicked worker's job as failed");

    // 锁已释放：active_run_id 清空
    let sess = st.sessions_tavern.get(&sid).unwrap();
    assert!(
        sess.active_run_id.is_none(),
        "watchdog must clear active_run_id"
    );
}

// ─────────────────────────────────────────────────────────────────
// 用例⑤：客户端断连 → 取消收尾（job cancelled + 锁释放 + hub 清理）
// 模拟真实 SSE 订阅者：收到 STOPPED error 事件后断开（rx drop），
// worker 的 broadcast send 失败 → Stopped → 保守取消收尾。
// ─────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn client_disconnect_cancels_turn() {
    let st = orch_state(
        "cancel",
        Some(format!("http://127.0.0.1:{}/v1", spawn_mock("streaming"))),
    );
    let (token, user_id, _ws) = login_token(&st).await;
    let sid = seed_session(&st, &user_id).await;

    let resp = call_start(&st, &token, &sid, "讲个长故事").await;
    assert!(resp.status().is_success());

    // 等 active_run_id 出现（worker 已 spawn 并注册 hub）
    let st2 = st.clone();
    let sid2 = sid.clone();
    let got_run = wait_until(
        move || {
            st2.sessions_tavern
                .get(&sid2)
                .map(|s| s.active_run_id.is_some())
                .unwrap_or(false)
        },
        Duration::from_secs(15),
    )
    .await;
    assert!(got_run, "worker should set active_run_id");
    let run_id = st.sessions_tavern.get(&sid).unwrap().active_run_id.unwrap();

    // 真实断连：订阅者收到首个事件后离线（rx drop）。此后 worker 的 hub.send
    // 全部失败（无接收者）→ Stopped → 保守取消收尾。不调用 hub.cancel ——
    // 那是“用户点停止”路径；本用例验证的是纯客户端断连。
    let (mut rx, _replay) = st.hub.subscribe(&run_id).expect("subscribe");
    tokio::spawn(async move {
        let _ = rx.recv().await;
        // rx 在此 drop —— 客户端离线
    });

    let st3 = st.clone();
    let rid = run_id.clone();
    let cancelled = wait_until(
        move || {
            st3.jobs
                .get(&rid)
                .map(|j| j.status == "cancelled")
                .unwrap_or(false)
        },
        Duration::from_secs(15),
    )
    .await;
    assert!(
        cancelled,
        "streaming worker must finish job as cancelled on disconnect"
    );

    let st4 = st.clone();
    let sid4 = sid.clone();
    let released = wait_until(
        move || {
            st4.sessions_tavern
                .get(&sid4)
                .map(|s| s.active_run_id.is_none())
                .unwrap_or(false)
        },
        Duration::from_secs(10),
    )
    .await;
    assert!(released, "session lock must be released after cancellation");
    assert!(!st.hub.has_live_worker(&run_id), "hub must be cleaned up");
}
