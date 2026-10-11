//! R8 性能基线（零依赖方案：`#[ignore]` perf 测试，显式运行）：
//!   cargo test -p kaleido-core --release --test perf_baseline -- --ignored --nocapture
//! 基线记录于 docs/PERF_BASELINE_2026-10-07.md。机器相关，仅作相对回归参照。

use kaleido_core::{
    check_world_info_timed, parse_wi_entry, vector_cosine_similarity, TimedWorldInfo, WiEntry,
    WiSettings,
};
use std::time::Instant;

fn synthetic_entries(n: usize) -> Vec<WiEntry> {
    (0..n)
        .filter_map(|i| {
            parse_wi_entry(
                &serde_json::json!({
                    "uid": format!("e{}", i),
                    "keys": [format!("关键词{}", i), format!("keyword{}", i)],
                    "keysecondary": [format!("次要{}", i)],
                    "content": format!(
                        "这是条目 {i} 的正文内容，用于模拟世界书条目。{}",
                        "包含一些占位文本。".repeat(20)
                    ),
                    "comment": format!("entry {i}"),
                    "constant": i % 50 == 0,
                    "selective": i % 3 == 0,
                    "order": i,
                    "position": 0,
                    "probability": 100,
                    "extensions": { "vectorized": i % 7 == 0 },
                }),
                "perf",
                i,
            )
        })
        .collect()
}

/// WI 扫描：500 条目 × 24 轮聊天缓冲（check_world_info_timed 热路径）。
#[test]
#[ignore = "perf baseline: run explicitly with --ignored"]
fn perf_wi_scan_500_entries() {
    let entries = synthetic_entries(500);
    let mut chat: Vec<String> = Vec::new();
    for t in 0..24 {
        chat.push(format!(
            "第{t}回合叙述：沈棠走进雨巷，{{user}}提起关键词{}，远处传来钟声。{}",
            if t == 0 { "keyword0" } else { "无关词" },
            "叙述正文填充。".repeat(30)
        ));
    }
    chat.reverse(); // newest first
    let settings = WiSettings::default();
    let timed = Some(TimedWorldInfo::default());

    for _ in 0..5 {
        let _ = check_world_info_timed(&entries, &chat, 8192, &settings, timed.clone(), true, None);
    }
    let n = 50;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let _ = check_world_info_timed(&entries, &chat, 8192, &settings, timed.clone(), true, None);
        samples.push(t0.elapsed());
    }
    samples.sort();
    let med = samples[n / 2].as_secs_f64() * 1e6;
    let p90 = samples[(n as f64 * 0.9) as usize].as_secs_f64() * 1e6;
    println!("[perf] wi_scan_500: median={med:.0}us p90={p90:.0}us");
    assert!(med < 50_000.0, "WI 扫描中位数劣化：{med:.0}us（基线告警阈值 50ms）");
}

/// 向量召回打分层：20k 候选 × 64 维余弦相似度（混合检索打分核心，纯函数）。
#[test]
#[ignore = "perf baseline: run explicitly with --ignored"]
fn perf_cosine_20k() {
    let query: Vec<f32> = (0..64).map(|i| (i as f32 * 0.011).sin()).collect();
    let candidates: Vec<Vec<f32>> = (0..20_000)
        .map(|r| (0..64).map(|d| ((r * 64 + d) as f32 * 0.013).cos()).collect())
        .collect();

    let _ = vector_cosine_similarity(&query, &candidates[0]); // 预热
    let n = 10;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let mut best = 0.0f64;
        for c in &candidates {
            let s = vector_cosine_similarity(&query, c);
            if s > best {
                best = s;
            }
        }
        assert!(best > 0.0);
        samples.push(t0.elapsed());
    }
    samples.sort();
    let med = samples[n / 2].as_secs_f64() * 1e6;
    println!("[perf] cosine_20k: median={med:.0}us");
    assert!(med < 200_000.0, "余弦打分 20k 劣化：{med:.0}us（基线告警阈值 200ms）");
}
