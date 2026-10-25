//! D4 吞噬 drama-skills `novel_index.py`：小说章节索引器（统一切分真相）。
//!
//! 参考实现（2026-08-23 评估入库）：`/root/workspaces/drama-skills/skills/short-drama-novel-analyze/scripts/novel_index.py`
//! （651 行，纯 stdlib）。本模块为 Rust 移植：单一「切分真相」——章节边界只检测一次，
//! 绑定行号/字符跨度，编号无法解释的必须上报 `problems` 而非静默猜测。
//!
//! 相对旧 `text_chunker.rs` 的 regex 直接切分，补四类硬能力：
//! ① 中文数字含「千/两」（第两百章）② 目录页丢弃（密度+数字复现双重证据）
//! ③ 多卷重编号识别（segment_by_restart）④ problems 清单（不可读编号/重复/跳号/短章）。

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 单元顺序：由粗到细（选单元按深度而非频率——防止「4 章×3 节」被误判成 12 章）。
pub const CHAPTER_UNITS: [&str; 3] = ["章", "回", "节"];
/// 中文数字（含「两」）。
fn chinese_digits() -> HashMap<char, u32> {
    let mut m = HashMap::new();
    for (c, v) in [
        ('零', 0),
        ('一', 1),
        ('二', 2),
        ('两', 2),
        ('三', 3),
        ('四', 4),
        ('五', 5),
        ('六', 6),
        ('七', 7),
        ('八', 8),
        ('九', 9),
    ] {
        m.insert(c, v);
    }
    m
}
fn chinese_units() -> HashMap<char, u32> {
    let mut m = HashMap::new();
    for (c, v) in [('十', 10u32), ('百', 100), ('千', 1000)] {
        m.insert(c, v);
    }
    m
}

/// 章标题行宽上限（字符数）：正文段落若以「第三章」开头（角色回忆等）不得误判为边界。
const MAX_HEADING_LINE_WIDTH: usize = 50;
/// 两个标题才构成编号 run（单条「第三回」可能是角色台词）。
const MINIMUM_NUMBERING_RUN: usize = 2;
/// 目录页相邻条目最大行距。
const CONTENTS_MAX_GAP: usize = 3;
/// 目录页最少条目数。
const CONTENTS_MIN_ENTRIES: usize = 3;
/// 短于该章数的「卷」不成立（单条重复标题不得关闭全本编号校验）。
const MIN_VOLUME_CHAPTERS: usize = 3;

/// 章节标题正则（对齐 novel_index.py CHAPTER_RE，支持中文/阿拉伯数字 + 千两）。
fn chapter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t\u{3000}]*第\s*([0-9]+|[零一二三四五六七八九十百千两]+)\s*([章回节])[ \t\u{3000}]*(.*?)[ \t\u{3000}]*$",
        )
        .expect("chapter index regex must be valid")
    })
}

/// 一个章节标题（识别阶段原始信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterHeading {
    pub line_index: usize,
    pub raw: String,
    /// 源编号；无法解读为 None（上报 problems，不崩溃）。
    pub number: Option<u32>,
    pub unit: String,
    pub title: String,
}

/// 章节索引条目（build 后：行号/跨度/字符数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterIndexEntry {
    pub sequence: usize,
    pub source_number: Option<u32>,
    pub unit: String,
    pub title: String,
    pub heading: String,
    /// 1-based 起始行。
    pub line_start: usize,
    /// 结束行（下一章标题行 / 末行）。
    pub line_end: usize,
    pub char_count: usize,
}

/// 中文数字 → 整数；畸形返回 None（不 panic）。
///
/// 覆盖：一 / 十五 / 两百零三 / 一千零一 / 第两百章（「两」=2）。
/// 规则：`十五` 即 15（前置「十」隐式带 1）；单位乘当前小节，小节复位。
pub fn chinese_to_int(text: &str) -> Option<u32> {
    // 剥离包裹数字段的前后缀：开头的「第」与结尾的「章/回/节/卷/篇」（含空白）。
    // 只剥首尾，不动数字中间的字；剥完为空则按无数字返回 None。
    let mut text = text.trim();
    text = text.strip_prefix('第').unwrap_or(text).trim_start();
    text = text
        .trim_end_matches(|c| matches!(c, '章' | '回' | '节' | '卷' | '篇'))
        .trim_end();
    if text.is_empty() {
        return None;
    }
    if text.chars().all(|c| c.is_ascii_digit()) {
        return text.parse().ok();
    }
    let digits = chinese_digits();
    let units = chinese_units();
    let mut total: u32 = 0;
    let mut section: u32 = 0;
    let mut digit_seen = false;
    for ch in text.chars() {
        if let Some(&d) = digits.get(&ch) {
            section = d;
            digit_seen = true;
        } else if let Some(&u) = units.get(&ch) {
            // 十五 = 15：前置「十」时隐式 1。
            total += section.max(1).saturating_mul(u);
            section = 0;
            digit_seen = true;
        } else {
            return None;
        }
    }
    if !digit_seen {
        return None;
    }
    Some(total + section)
}

/// 可见宽度（组合字符不算，近似 unicodedata 不计 combining）。
fn visible_width(s: &str) -> usize {
    s.chars().count()
}

/// 扫描标题行：返回 (headings, 超宽被忽略的行数)。
pub fn find_heading_lines(lines: &[&str]) -> (Vec<ChapterHeading>, usize) {
    let re = chapter_re();
    let mut headings = Vec::new();
    let mut long_lines = 0usize;
    for (offset, &line) in lines.iter().enumerate() {
        let candidate = if offset == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        let Some(caps) = re.captures(candidate) else { continue };
        if visible_width(candidate.trim()) > MAX_HEADING_LINE_WIDTH {
            long_lines += 1;
            continue;
        }
        headings.push(ChapterHeading {
            line_index: offset,
            raw: candidate.trim().to_string(),
            number: caps.get(1).map(|m| chinese_to_int(m.as_str())).flatten(),
            unit: caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
            title: caps.get(3).map(|m| m.as_str().trim().to_string()).unwrap_or_default(),
        });
    }
    (headings, long_lines)
}

/// 选择唯一编号单元（按深度由粗到细，最低 run=2）。
///
/// 频率法会把「4 章×3 节」误判成 12 章（节总比章多）；按 CHAPTER_UNITS 的粗细顺序，
/// 首个形成真实编号 run 的最粗单元胜出。
pub fn select_chapter_unit(
    headings: &[ChapterHeading],
    forced: Option<&str>,
) -> (Option<String>, HashMap<String, usize>) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for unit in CHAPTER_UNITS {
        counts.insert(unit.to_string(), 0);
    }
    for h in headings {
        if let Some(c) = counts.get_mut(&h.unit) {
            *c += 1;
        }
    }
    if headings.is_empty() {
        return (None, counts);
    }
    if let Some(f) = forced {
        let ignored: HashMap<String, usize> = counts
            .iter()
            .filter(|(k, v)| *k != f && **v > 0)
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        return (Some(f.to_string()), ignored);
    }
    let chosen = CHAPTER_UNITS
        .iter()
        .copied()
        .find(|u| counts.get(*u).copied().unwrap_or(0) >= MINIMUM_NUMBERING_RUN)
        .map(|s| s.to_string());
    let chosen = match chosen {
        Some(c) => c,
        None => {
            // 无 run：取最频繁（并列按声明顺序，结果与标题先后无关）。
            CHAPTER_UNITS
                .iter()
                .copied()
                .max_by_key(|u| counts.get(*u).copied().unwrap_or(0))
                .map(|s| s.to_string())
                .unwrap_or_default()
        }
    };
    let ignored: HashMap<String, usize> = counts
        .iter()
        .filter(|(k, v)| *k != &chosen && **v > 0)
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    (Some(chosen), ignored)
}

/// 丢弃前导目录页：密度 + 数字复现双重证据。
///
/// 密度阈值会被大目录块拖入自身区间而失效，故必须要求：紧凑前导块的编号
/// 全部在后续标题中复现（目录总是重复它索引的章节；真正密集的开篇不会）。
pub fn drop_leading_toc(headings: Vec<ChapterHeading>) -> (Vec<ChapterHeading>, usize) {
    if headings.len() < CONTENTS_MIN_ENTRIES * 2 {
        return (headings, 0);
    }
    let numbers: Vec<Option<u32>> = headings.iter().map(|h| h.number).collect();
    if numbers[0].is_none() {
        return (headings, 0);
    }
    // 切点 = 编号重启处（目录块后接同一批章节）。
    let mut cut = 0usize;
    for (index, &n) in numbers.iter().enumerate().skip(1) {
        if let Some(nv) = n {
            if nv <= numbers[0].unwrap() {
                cut = index;
                break;
            }
        }
    }
    if cut < CONTENTS_MIN_ENTRIES || cut > headings.len() - cut {
        return (headings, 0);
    }
    // 目录条目间行距必须紧凑；否则是「编号重启但中间有真正文」= 多卷书，不是目录。
    for i in 0..cut.saturating_sub(1) {
        let gap = headings[i + 1].line_index - headings[i].line_index;
        if gap > CONTENTS_MAX_GAP {
            return (headings, 0);
        }
    }
    let dropped: std::collections::HashSet<u32> = numbers[..cut]
        .iter()
        .filter_map(|n| *n)
        .collect();
    let remaining: std::collections::HashSet<u32> = numbers[cut..]
        .iter()
        .filter_map(|n| *n)
        .collect();
    if dropped.is_empty() || !dropped.is_subset(&remaining) {
        return (headings, 0);
    }
    let dropped_count = cut;
    (headings[cut..].to_vec(), dropped_count)
}

/// 由标题列表构建章节条目（含行跨度与字符数）。
pub fn build_chapters(lines: &[&str], headings: &[ChapterHeading], text: &str) -> Vec<ChapterIndexEntry> {
    let line_offsets: Vec<usize> = {
        let mut offsets = Vec::with_capacity(lines.len());
        let mut cursor = 0usize;
        for line in lines {
            offsets.push(cursor);
            cursor += line.len() + 1; // +1 for '\n'
        }
        offsets
    };
    let mut chapters = Vec::with_capacity(headings.len());
    for (position, heading) in headings.iter().enumerate() {
        let start_line = heading.line_index;
        let end_line = if position + 1 < headings.len() {
            headings[position + 1].line_index
        } else {
            lines.len()
        };
        let start_byte = line_offsets[start_line];
        let end_byte = if end_line < line_offsets.len() {
            line_offsets[end_line]
        } else {
            text.len()
        };
        let body = text.get(start_byte..end_byte).unwrap_or_default();
        chapters.push(ChapterIndexEntry {
            sequence: position + 1,
            source_number: heading.number,
            unit: heading.unit.clone(),
            title: heading.title.clone(),
            heading: heading.raw.clone(),
            line_start: start_line + 1,
            line_end: end_line,
            char_count: visible_width(body),
        });
    }
    chapters
}

/// 编号停止递增处切分（每段 = 候选卷）。校验在卷内进行：各卷从第一章重来合法，
/// 卷内缺章仍上报。
pub fn segment_by_restart(chapters: &[ChapterIndexEntry]) -> Vec<Vec<ChapterIndexEntry>> {
    let mut segments: Vec<Vec<ChapterIndexEntry>> = Vec::new();
    let mut current: Vec<ChapterIndexEntry> = Vec::new();
    let mut previous: Option<u32> = None;
    for ch in chapters {
        if let Some(n) = ch.source_number {
            if let Some(p) = previous {
                if n <= p {
                    segments.push(std::mem::take(&mut current));
                }
            }
            previous = Some(n);
        }
        current.push(ch.clone());
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// 校验并产出 problems 清单（不可读编号/重复/跳号/短章/可疑重启）。
pub fn validate_chapters(
    chapters: &[ChapterIndexEntry],
    segments: &[Vec<ChapterIndexEntry>],
) -> Vec<String> {
    let mut problems = Vec::new();
    if chapters.is_empty() {
        problems.push("no chapter heading matched; the source may need manual spans".to_string());
        return problems;
    }
    let unreadable: Vec<&str> = chapters
        .iter()
        .filter(|c| c.source_number.is_none())
        .map(|c| c.heading.as_str())
        .take(5)
        .collect();
    if !unreadable.is_empty() {
        problems.push(format!(
            "unreadable chapter numbers: {}",
            unreadable.join(", ")
        ));
    }
    for (position, segment) in segments.iter().enumerate() {
        let multi = segments.len() > 1;
        let label = if multi {
            format!(" in volume {}", position + 1)
        } else {
            String::new()
        };
        if multi && segment.len() < MIN_VOLUME_CHAPTERS {
            if let Some(first) = segment.first() {
                problems.push(format!(
                    "numbering restarts after only {} chapter(s) at sequence {}; expected a volume boundary, check for a stray or duplicated heading{}",
                    segment.len(),
                    first.sequence,
                    label
                ));
            }
        }
        let numbers: Vec<u32> = segment
            .iter()
            .filter_map(|c| c.source_number)
            .collect();
        for w in numbers.windows(2) {
            let (prev, cur) = (w[0], w[1]);
            if cur == prev {
                problems.push(format!("duplicate chapter number {cur}{label}"));
                break;
            }
            if cur != prev + 1 {
                problems.push(format!(
                    "chapter numbering jumps from {prev} to {cur}{label}"
                ));
                break;
            }
        }
    }
    // 空章 = 标题行之外没有正文（spans 内只剩标题与空白，典型为目录残留）。
    // 短章合法（样章可能仅一句正文），不得以「总字符过小」误报。
    let empty: Vec<usize> = chapters
        .iter()
        .filter(|c| c.char_count <= visible_width(&c.heading))
        .map(|c| c.sequence)
        .take(5)
        .collect();
    if !empty.is_empty() {
        problems.push(format!(
            "chapters with almost no body text at sequence {}",
            empty
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    problems
}

/// 完整索引管线：识别 → 选单元 → 去目录 → 建章 → 分段 → 校验。
pub fn index_chapters(text: &str) -> (Vec<ChapterIndexEntry>, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let (headings, _long) = find_heading_lines(&lines);
    let (chosen, _ignored) = select_chapter_unit(&headings, None);
    // 只保留选中单元的标题（其他单元如「节」在「章」被选中时忽略）。
    let mut filtered: Vec<ChapterHeading> = headings;
    if let Some(ref c) = chosen {
        filtered.retain(|h| h.unit == *c);
    }
    let (filtered, _dropped) = drop_leading_toc(filtered);
    let chapters = build_chapters(&lines, &filtered, text);
    let segments = segment_by_restart(&chapters);
    let problems = validate_chapters(&chapters, &segments);
    (chapters, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_numerals_including_qian_and_liang() {
        assert_eq!(chinese_to_int("一"), Some(1));
        assert_eq!(chinese_to_int("十五"), Some(15));
        assert_eq!(chinese_to_int("两百零三"), Some(203));
        assert_eq!(chinese_to_int("一千零一"), Some(1001));
        assert_eq!(chinese_to_int("第两百章"), Some(200)); // 整串不匹配但取数字段时覆盖
        assert_eq!(chinese_to_int("两百"), Some(200));
        assert_eq!(chinese_to_int("二"), Some(2));
        assert_eq!(chinese_to_int("两"), Some(2));
        assert_eq!(chinese_to_int("120"), Some(120));
        assert_eq!(chinese_to_int(""), None);
        assert_eq!(chinese_to_int("abc"), None);
        assert_eq!(chinese_to_int(""), None);
    }

    #[test]
    fn heading_scan_extracts_number_unit_title() {
        let text = "第一章 初见\n窗外下雨。\n\n第两百章 重逢\n多年以后。\n";
        let lines: Vec<&str> = text.lines().collect();
        let (headings, long) = find_heading_lines(&lines);
        assert_eq!(long, 0);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].number, Some(1));
        assert_eq!(headings[0].unit, "章");
        assert_eq!(headings[0].title, "初见");
        assert_eq!(headings[1].number, Some(200));
        assert_eq!(headings[1].title, "重逢");
    }

    #[test]
    fn select_unit_by_depth_not_frequency() {
        // 4 章 × 3 节：频率法会选「节」，深度法必须选「章」。
        let mut headings = Vec::new();
        for c in 1..=4 {
            headings.push(ChapterHeading {
                line_index: c,
                raw: format!("第{c}章"),
                number: Some(c as u32),
                unit: "章".into(),
                title: String::new(),
            });
            for s in 1..=3 {
                headings.push(ChapterHeading {
                    line_index: 100 + c * 10 + s,
                    raw: format!("第{s}节"),
                    number: Some(s as u32),
                    unit: "节".into(),
                    title: String::new(),
                });
            }
        }
        let (chosen, _ignored) = select_chapter_unit(&headings, None);
        assert_eq!(chosen.as_deref(), Some("章"));
    }

    #[test]
    fn drop_toc_with_corroboration() {
        // 目录块：第1-3章紧凑复现于后文。
        let mut headings = Vec::new();
        for i in 0..3 {
            headings.push(ChapterHeading {
                line_index: i,
                raw: format!("第{}章 目录标题{i}", i + 1),
                number: Some(i as u32 + 1),
                unit: "章".into(),
                title: format!("目录标题{i}"),
            });
        }
        for i in 3..6 {
            headings.push(ChapterHeading {
                line_index: 20 + i,
                raw: format!("第{}章 正文标题{i}", i - 2),
                number: Some(i as u32 - 2),
                unit: "章".into(),
                title: format!("正文标题{}", i - 2),
            });
        }
        let (kept, dropped) = drop_leading_toc(headings);
        assert_eq!(dropped, 3);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].line_index, 23);
    }

    #[test]
    fn volume_restart_validated_inside_segments() {
        // 卷一卷章 1..3 后重启为第1章：不报错（各卷内递增合法）。
        let mut chapters = Vec::new();
        for seq in 1..=3 {
            chapters.push(ChapterIndexEntry {
                sequence: seq,
                source_number: Some(seq as u32),
                unit: "章".into(),
                title: String::new(),
                heading: String::new(),
                line_start: seq,
                line_end: seq + 1,
                char_count: 200,
            });
        }
        // 重启
        for seq in 4..=6 {
            chapters.push(ChapterIndexEntry {
                sequence: seq,
                source_number: Some(seq as u32 - 3),
                unit: "章".into(),
                title: String::new(),
                heading: String::new(),
                line_start: seq,
                line_end: seq + 1,
                char_count: 200,
            });
        }
        let segments = segment_by_restart(&chapters);
        assert_eq!(segments.len(), 2);
        let problems = validate_chapters(&chapters, &segments);
        assert!(problems.is_empty(), "多卷合法重排不应报错: {problems:?}");
    }

    #[test]
    fn duplicate_and_jump_reported() {
        let mk = |seq, n: Option<u32>| ChapterIndexEntry {
            sequence: seq,
            source_number: n,
            unit: "章".into(),
            title: String::new(),
            heading: String::new(),
            line_start: seq,
            line_end: seq + 1,
            char_count: 200,
        };
        // [1,2,2,9]：重复 2 触发编号重启（切成两卷），2->9 跳号 + 短卷可疑。
        let chapters = vec![mk(1, Some(1)), mk(2, Some(2)), mk(3, Some(2)), mk(4, Some(9))];
        let segments = segment_by_restart(&chapters);
        let problems = validate_chapters(&chapters, &segments);
        assert!(
            problems.iter().any(|p| p.contains("jumps from 2 to 9")),
            "跳号应上报: {problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("restarts after only")),
            "短卷重启（疑似重复标题）应上报: {problems:?}"
        );
    }

    #[test]
    fn index_pipeline_on_real_sample() {
        let text = "第一章 雨夜\n\n夜色里她推门而入。\n\n第二章 重逢\n\n多年以后，他们在桥头再见。\n\n第三章 真相\n\n抽屉里的信揭开了秘密。\n";
        let (chapters, problems) = index_chapters(text);
        assert_eq!(chapters.len(), 3, "应识别 3 章");
        assert_eq!(chapters[0].source_number, Some(1));
        assert_eq!(chapters[1].source_number, Some(2));
        assert!(problems.is_empty(), "干净样本不应有 problems: {problems:?}");
    }
}
