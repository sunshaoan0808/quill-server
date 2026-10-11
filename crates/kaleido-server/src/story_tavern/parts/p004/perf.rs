//! p004 性能基线（R8；R6-2② 自 p004.rs 迁入，零改动）。
#![allow(unused_imports)]
use super::*;

#[cfg(test)]
pub(crate) mod perf_baseline {
    use super::*;

    /// R8 性能基线：build_tavern_system_prompt（demo pack × 40 回合会话）。
    /// 运行：cargo test -p kaleido-server --release build_tavern_system_prompt -- --ignored --nocapture
    #[test]
    #[ignore = "perf baseline: run explicitly with --ignored"]
    fn perf_build_tavern_system_prompt() {
        let tmp = std::env::temp_dir().join(format!("kaleido-perf-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let data = kaleido_core::DataRoot::new(tmp.join("d")).unwrap();
        let packs = kaleido_core::PackStore::new(data.clone());
        let pack = packs.ensure_demo_pack().expect("demo pack");
        let sessions = kaleido_core::TavernSessionStore::new(data.clone());
        let req: kaleido_core::CreateSessionRequest = serde_json::from_value(serde_json::json!({
            "packId": pack.id,
            "owner": "perf",
        }))
        .unwrap();
        let mut sess = sessions.create_from_pack(&packs, req).unwrap();
        // 模拟 40 回合对话（user/assistant 各 400 汉字）
        for t in 0..40 {
            for role in ["user", "assistant"] {
                sess.messages.push(kaleido_core::TavernMessage {
                    id: format!("msg-{t}-{role}"),
                    role: role.into(),
                    content: format!("第{t}回合 {}正文。", "叙述填充".repeat(60)),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    options: vec![],
                    engine_tag: None,
                    program: None,
                    reasoning: None,
                    swipes: vec![],
                    swipe_index: 0,
                    tokens: 0,
                });
            }
        }
        let prompt = build_tavern_system_prompt(
            &pack, &sess, "", &tmp.join("cross"), None, &[], None
        );
        assert!(!prompt.is_empty());
        let n = 100;
        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let _ = build_tavern_system_prompt(
                &pack, &sess, "", &tmp.join("cross"), None, &[], None
            );
            samples.push(t0.elapsed());
        }
        samples.sort();
        let med = samples[n / 2].as_secs_f64() * 1e6;
        let p90 = samples[(n as f64 * 0.9) as usize].as_secs_f64() * 1e6;
        println!("[perf] prompt_40turns: median={med:.0}us p90={p90:.0}us len={}", prompt.len());
        assert!(med < 10_000.0, "prompt 组装劣化：{med:.0}us（告警阈值 10ms）");
    }
}
