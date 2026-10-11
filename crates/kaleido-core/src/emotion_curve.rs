//! Emotion curve analysis — per-chapter emotional intensity estimation.
//!
//! Pure heuristic (no LLM): keyword frequency + punctuation density map to
//! 0-100 intensity, dominant emotion label, and curve shape. Absorbed from
//! novel2hermes_jp's planning-workflow "情感曲线 master view" pattern.

use serde::{Deserialize, Serialize};

/// Minimal chapter input contract (module-local, no external deps).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterText {
    pub chapter: String,
    pub text: String,
}

/// One chapter's emotion snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterEmotion {
    pub chapter: String,
    /// Peak intensity 0-100 (heuristic).
    pub peak_intensity: u8,
    /// Dominant emotion label (喜/怒/哀/惧/惊/思/平).
    pub dominant_emotion: String,
    /// Curve shape: 上升 / 下降 / 峰 / 谷 / 平台.
    pub curve_shape: String,
}

/// Full-story emotion curve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionCurve {
    pub chapters: Vec<ChapterEmotion>,
    /// Overall arc summary (concise Chinese phrase).
    pub overall_arc: String,
}

/// Emotion keyword lexicon (simplified Chinese fiction).
const EMOTION_LEXICON: &[(&str, &str)] = &[
    ("喜", "喜"),
    ("高兴", "喜"),
    ("开心", "喜"),
    ("笑", "喜"),
    ("幸福", "喜"),
    ("欢", "喜"),
    ("怒", "怒"),
    ("生气", "怒"),
    ("愤怒", "怒"),
    ("恨", "怒"),
    ("吼", "怒"),
    ("怒骂", "怒"),
    ("哀", "哀"),
    ("哭", "哀"),
    ("悲伤", "哀"),
    ("难过", "哀"),
    ("伤心", "哀"),
    ("悲痛", "哀"),
    ("落泪", "哀"),
    ("惧", "惧"),
    ("害怕", "惧"),
    ("恐惧", "惧"),
    ("惊", "惧"),
    ("颤抖", "惧"),
    ("冷汗", "惧"),
    ("惊", "惊"),
    ("震惊", "惊"),
    ("惊讶", "惊"),
    ("意外", "惊"),
    ("骇", "惊"),
    ("思", "思"),
    ("想", "思"),
    ("回忆", "思"),
    ("思念", "思"),
    ("沉思", "思"),
    ("怀念", "思"),
];

/// Count keyword hits per emotion label.
fn count_emotions(text: &str) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (word, label) in EMOTION_LEXICON {
        if text.contains(word) {
            *counts.entry((*label).to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// Heuristic peak intensity: emotion hits + exclamation density.
fn estimate_intensity(text: &str, hits: usize) -> u8 {
    let exclaim = text.chars().filter(|c| *c == '！' || *c == '!' || *c == '？' || *c == '?').count();
    let chars = text.chars().count().max(1);
    let exclaim_density = exclaim as f64 / chars as f64;
    let mut score = hits.min(20) as f64 * 4.0; // up to 80 from keywords
    score += (exclaim_density * 500.0).min(20.0); // up to 20 from punctuation
    score.min(100.0) as u8
}

/// Build a full-story emotion curve from chapter texts.
pub fn build_emotion_curve(chapters: &[ChapterText]) -> EmotionCurve {
    let mut per_chapter: Vec<ChapterEmotion> = Vec::with_capacity(chapters.len());
    for ch in chapters {
        let counts = count_emotions(&ch.text);
        let total: usize = counts.values().sum();
        let dominant = counts
            .iter()
            .max_by_key(|(_, n)| *n)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| "平".to_string());
        per_chapter.push(ChapterEmotion {
            chapter: ch.chapter.clone(),
            peak_intensity: estimate_intensity(&ch.text, total),
            dominant_emotion: dominant,
            curve_shape: "平台".to_string(), // filled in second pass
        });
    }
    // Second pass: compute curve shape relative to neighbors.
    for i in 0..per_chapter.len() {
        let prev = if i > 0 { Some(per_chapter[i - 1].peak_intensity) } else { None };
        let next = per_chapter.get(i + 1).map(|c| c.peak_intensity);
        let cur = per_chapter[i].peak_intensity;
        let shape = match (prev, next) {
            (Some(p), Some(n)) if cur > p && cur > n => "峰",
            (Some(p), Some(n)) if cur < p && cur < n => "谷",
            (Some(p), Some(n)) if cur > p && cur > n.saturating_sub(5) => "上升",
            (Some(p), _) if cur > p => "上升",
            (Some(p), _) if cur < p => "下降",
            (_, Some(n)) if cur < n => "上升",
            (_, Some(n)) if cur > n => "下降",
            _ => "平台",
        };
        per_chapter[i].curve_shape = shape.to_string();
    }
    let overall_arc = summarize_arc(&per_chapter);
    EmotionCurve { chapters: per_chapter, overall_arc }
}

/// Summarize the overall arc from chapter intensities.
fn summarize_arc(chapters: &[ChapterEmotion]) -> String {
    if chapters.is_empty() {
        return "无章节数据".to_string();
    }
    let first = chapters.first().unwrap().peak_intensity;
    let last = chapters.last().unwrap().peak_intensity;
    let avg: u32 = chapters.iter().map(|c| c.peak_intensity as u32).sum::<u32>() / chapters.len() as u32;
    let dominant_set: std::collections::HashSet<String> =
        chapters.iter().map(|c| c.dominant_emotion.clone()).collect();
    let mut desc = format!("平均强度 {avg}，主导情绪 {}，", dominant_set.iter().take(3).cloned().collect::<Vec<_>>().join("/"));
    if last as i32 - first as i32 >= 15 {
        desc.push_str("整体升温");
    } else if first as i32 - last as i32 >= 15 {
        desc.push_str("整体降温");
    } else {
        desc.push_str("整体平稳");
    }
    desc
}


// ---------------------------------------------------------------------------
// 情感快照（backport 自 museai-server PR #108，源吸收 dramatica-flow EmotionalSnapshot：
// emotion / intensity 1-10 / trigger 每章一条，供长篇弧线追踪与审计对照）。
// 与既有 0-100 heuristic 曲线互补：build_emotion_curve 看整书形态；
// 这里给「单章情感快照」，强度换 1-10 口径。
// ---------------------------------------------------------------------------

/// 单章情感快照（dramatica 吸收：1-10 强度 + 触发原因）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EmotionalSnapshot {
    pub chapter: String,
    /// 主导情绪标签（喜/怒/哀/惧/惊/思/平）。
    pub emotion: String,
    /// 强度 1-10（dramatica 口径；从 0-100 heuristic 折算）。
    pub intensity: u8,
    /// 触发原因（该章首个命中情感词所在句，截断 ≤80 字；无则空）。
    pub trigger: String,
}

/// 0-100 强度 → 1-10（dramatica 口径：100→10，0→1）。
pub fn intensity_100_to_10(v: u8) -> u8 {
    let v = v.clamp(0, 100) as u16;
    ((v * 9) / 100 + 1).clamp(1, 10) as u8
}

/// 从单章正文提取情感快照。
pub fn chapter_emotion_snapshot(chapter: &str, text: &str) -> EmotionalSnapshot {
    let counts = count_emotions(text);
    let total: usize = counts.values().sum();
    let emotion = counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| "平".to_string());
    let intensity = intensity_100_to_10(estimate_intensity(text, total));
    // 触发原因：首个命中情感词的句子
    let trigger = find_emotion_trigger_sentence(text, &emotion);
    EmotionalSnapshot {
        chapter: chapter.to_string(),
        emotion,
        intensity,
        trigger,
    }
}

/// 全章情感快照序列（dramatica 吸收：逐章 append 供弧线追踪）。
pub fn build_emotional_snapshots(chapters: &[ChapterText]) -> Vec<EmotionalSnapshot> {
    chapters
        .iter()
        .map(|c| chapter_emotion_snapshot(&c.chapter, &c.text))
        .collect()
}

/// 找到第一个含主导情绪词的句子（截断 ≤80 字）。
fn find_emotion_trigger_sentence(text: &str, emotion: &str) -> String {
    for seg in text.split(['。', '！', '？', '！', '?', '\n']) {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        // 该句命中该情绪标签的任一词
        let hit = EMOTION_LEXICON
            .iter()
            .any(|(word, label)| *label == emotion && seg.contains(word));
        if hit {
            return seg.chars().take(80).collect();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let c = build_emotion_curve(&[]);
        assert!(c.chapters.is_empty());
        assert_eq!(c.overall_arc, "无章节数据");
    }

    #[test]
    fn happy_chapter_marks_xi() {
        let c = build_emotion_curve(&[ChapterText {
            chapter: "c1".into(),
            text: "他高兴地笑了，幸福极了！哈哈！".into(),
        }]);
        assert_eq!(c.chapters[0].dominant_emotion, "喜");
        assert!(c.chapters[0].peak_intensity > 0);
    }

    #[test]
    fn sad_chapter_marks_ai() {
        let c = build_emotion_curve(&[ChapterText {
            chapter: "c2".into(),
            text: "她伤心地哭了，悲痛欲绝，眼泪止不住。".into(),
        }]);
        assert_eq!(c.chapters[0].dominant_emotion, "哀");
        assert!(c.chapters[0].peak_intensity > 0);
    }

    #[test]
    fn rising_trend() {
        let c = build_emotion_curve(&[
            ChapterText { chapter: "c1".into(), text: "他静静地坐着。".into() },
            ChapterText { chapter: "c2".into(), text: "他震惊地站了起来！！".into() },
        ]);
        assert!(c.chapters[1].peak_intensity >= c.chapters[0].peak_intensity);
    }

    #[test]
    fn intensity_100_to_10_bounds() {
        assert_eq!(intensity_100_to_10(0), 1);
        assert_eq!(intensity_100_to_10(100), 10);
        assert_eq!(intensity_100_to_10(50), 5);
        assert!(intensity_100_to_10(20) >= 1 && intensity_100_to_10(20) <= 10);
    }

    #[test]
    fn snapshot_extracts_emotion_intensity_trigger() {
        let snap = chapter_emotion_snapshot("c1", "他高兴地笑了，幸福极了！哈哈！");
        assert_eq!(snap.emotion, "喜");
        assert!(snap.intensity >= 1 && snap.intensity <= 10);
        assert!(snap.trigger.contains("高兴") || snap.trigger.contains("幸福"), "trigger 应命中情感句: {}", snap.trigger);
        assert_eq!(snap.chapter, "c1");
    }

    #[test]
    fn snapshots_sequence_per_chapter() {
        let snaps = build_emotional_snapshots(&[
            ChapterText { chapter: "c1".into(), text: "他高兴地笑了。".into() },
            ChapterText { chapter: "c2".into(), text: "她伤心地哭了，悲痛欲绝。".into() },
        ]);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].emotion, "喜");
        assert_eq!(snaps[1].emotion, "哀");
        assert_eq!(snaps[0].chapter, "c1");
        assert_eq!(snaps[1].chapter, "c2");
        // 高情绪密度章强度更高
        let weak = chapter_emotion_snapshot("c0", "他静静坐着，窗外下雨。");
        assert!(weak.intensity <= snaps[1].intensity, "无情绪词章强度应低: {} vs {}", weak.intensity, snaps[1].intensity);
    }
}