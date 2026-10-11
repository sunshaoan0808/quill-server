//! crawler/parts/p002/toc —— TOC 规则表 + 章节切分族（split_novel_chapters/编号/缺口/中文数字/窗口回退）。（R6-3① 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) struct TocRule {
    #[allow(dead_code)] // [P7] 抓取源展示名预留
    name: &'static str,
    enabled: bool,
    chapter: &'static str,
}

/// Legado 内置目录规则（enabled 子集），适配 Rust `regex`（无 look-around）。
/// 数字字符集覆盖：阿拉伯 + 中文小写(零一二…) + 中文大写(壹贰叁…) + 〇。
pub(crate) const TOC_RULES: &[TocRule] = &[
    TocRule {
        name: "目录",
        enabled: true,
        chapter: r"(?m)^[ \t　#]{0,4}(?:序章|楔子|正文|终章|后记|尾声|番外|第[ \t　]{0,4}[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]+?[ \t　]{0,4}(?:章|节|卷|集|部|篇)).{0,40}$",
    },
    TocRule {
        name: "数字 可选分隔符 标题名称",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}[0-9]{1,5}[:：,.， 、_—\-]?.{1,40}$",
    },
    TocRule {
        name: "大写数字 分隔符 标题名称",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}(?:序章|楔子|正文|终章|后记|尾声|番外|[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,8}章?)[ 、_—\-].{1,40}$",
    },
    TocRule {
        name: "数字混合 分隔符 标题名称",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}(?:序章|楔子|正文|终章|后记|尾声|番外|[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,8}章?[ 、_—\-]|[0-9]{1,5}章?[:：,.， 、_—\-]).{0,40}$",
    },
    TocRule {
        name: "Chapter/Section/Part/Episode 序号 标题",
        enabled: true,
        chapter: r"(?mi)^[ \t　]{0,4}(?:[Cc]hapter|[Ss]ection|[Pp]art|ＰＡＲＴ|[Nn][oO][.、]|[Ee]pisode|[Cc]h\.?)[ \t　]{0,4}[0-9]{1,4}.{0,40}$",
    },
    TocRule {
        name: "特殊符号 序号 标题",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}[【〔〖「『〈［\[](?:第|[Cc]hapter)[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,10}[章节].{0,30}$",
    },
    TocRule {
        name: "章/卷 序号 标题",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}[卷章][0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,8}[ \t　]{0,4}.{0,40}$",
    },
    TocRule {
        name: "字数分割 分节阅读",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}(?:.{0,15}分[页节章段]阅读[-_ ]|第[ \t　]{0,4}[0-9〇零一二两三四五六七八九十百千万]{1,6}[ \t　]{0,4}[页节]).{0,40}$",
    },
    TocRule {
        name: "顶格标题",
        enabled: false,
        chapter: r"(?m)^[^ \t　].{1,20}$",
    },
    TocRule {
        name: "通用规则",
        enabled: false,
        chapter: r"(?m)^.{0,6}(?:[引楔]子|正文|[引序前]言|[序终]章|[上中下][部篇卷]|后记|尾声|番外|={2,4}|第[ \t　]{0,4}[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]+?[ \t　]{0,4}(?:章|节|卷|部|篇)).{0,40}$",
    },
    // 2026-08-19: Markdown 列表前缀破折号（安全屋/老板你也没说/孕船 等 txt 导出经常带 `- 第X章`）
    TocRule {
        name: "列表破折号 章标题",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}[-–—·•*]{1,2}[ \t　]{0,2}(?:第[ \t　]{0,4}[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]+?[ \t　]{0,4}(?:章|节|卷|集|部|篇)).{0,40}$",
    },
    // 2026-08-19: 全角括号序号标题（度蜜月 `　　（5）释放（手交口交）` 内嵌章号）
    // 仅作为独立切分规则候选；混合格式源（第X章 主流 + 内嵌（N））需多规则合并切分兜底（见 split 逻辑）。
    TocRule {
        name: "全角括号序号 标题",
        enabled: true,
        chapter: r"(?m)^[ \t　]{0,4}[（(【［\[]\s*[0-9０-９]{1,8}\s*[）)】］\]](?:[ \t　]{0,4}\S.{0,40})?$",
    },
];

/// 分章时长章节的阈值（超出则按字数窗口二次拆分），参考 Legado「拆分超长章节」。
#[allow(dead_code)] // [P7] 长文切分阈值预留
pub(crate) const SPLIT_LONG_THRESHOLD: usize = 30_000;

/// 无法用目录规则切分时的窗口大小（字符），对齐换行落点。
pub(crate) const FALLBACK_WINDOW: usize = 2400;

/// Split novel text into chapters — 移植 Legado txtTocRule 的多规则自动甄别方案。
/// 与旧实现（单正则 + 中文数字统一归一化为 "n" 导致误合并）不同：
///  1. 内置多条启用的目录规则，对全文逐条统计"匹配数"，选取匹配最多者作为本章规则；
///  2. 匹配统计时要求相邻标题间隔 >1000 字符，过滤把正文当标题的误报；
///  3. 找不到 ≥2 个标题时退回字数窗口切分，并对齐到换行符（不硬切在句中）。
pub(crate) fn split_novel_chapters(text: &str) -> Vec<(String, String)> {
    // 选中最合适的目录规则（匹配数最多，且相邻标题间隔 >1000）
    let best = pick_toc_rule(text);
    if let Some(rule) = best {
        let regex = match regex::Regex::new(rule.chapter) {
            Ok(re) => re,
            Err(_) => return fallback_windows(text),
        };
        // 收集所有标题的 (byte 起点, 标题文本)
        let mut titles: Vec<(usize, String)> = Vec::new();
        for m in regex.find_iter(text) {
            let title = m
                .as_str()
                .trim()
                .trim_start_matches(|c| c == '#' || c == '-' || c == '*' || c == ' ')
                .trim()
                .chars()
                .take(80)
                .collect::<String>();
            if !title.is_empty() {
                titles.push((m.start(), title));
            }
        }
        // 至少 2 个标题才算切章成功，否则 fallback
        if titles.len() >= 2 {
            // 2026-08-19: 补充规则合并——混合格式源（主流「第X章」 + 内嵌「（N）标题」如度蜜月
            // 第五章 `　　（5）释放`）单规则会漏。用「全角括号序号」规则额外扫描主规则未覆盖的
            // 标题：仅并入 chapter_value 能解析（含括号前导序号扩展）且与主序列章号构成 gap 的标题，
            // 防正文误报（正文里的 `（3）` 数字通常不成章号序列）。
            if let Ok(bracket_re) = regex::Regex::new(
                r"(?m)^[ \t　]{0,4}[（(【\[][ \t　]{0,4}[0-9]{1,5}[ \t　]{0,4}[）)】\]][^\n]{0,60}$",
            ) {
                let main_vals: Vec<i64> = titles
                    .iter()
                    .filter_map(|(_, t)| chapter_value(t))
                    .collect();
                let mut extra: Vec<(usize, String)> = Vec::new();
                for m in bracket_re.find_iter(text) {
                    let mstart = m.start();
                    // 与主标题位置过近（<300 字符）视为同一标题变体，跳过
                    if titles.iter().any(|(idx, _)| mstart.abs_diff(*idx) < 300) {
                        continue;
                    }
                    let raw = m.as_str();
                    let t = raw.trim().chars().take(80).collect::<String>();
                    if t.is_empty() {
                        continue;
                    }
                    if let Some(v) = chapter_value(&t) {
                        // 章号在主序列 gap 内（如主序列 1..4,6 时 5 是 gap）→ 并入
                        let is_gap = main_vals.iter().any(|mv| *mv > v + 0)
                            && !main_vals.contains(&v);
                        // 或主序列尚无该序号且后续有更大章号 → 补进
                        let fills = main_vals.iter().any(|mv| *mv > v) && !main_vals.contains(&v);
                        if is_gap || fills {
                            extra.push((mstart, t));
                        }
                    }
                }
                if !extra.is_empty() {
                    titles.extend(extra);
                    titles.sort_by_key(|(idx, _)| *idx);
                    // 位置过近的重复（同章双标题）交给 dedup_same_value 按章号合并
                }
            }
            // 合并"同真实章号"的相邻重复（如 `# 第1章` 与裸 `第1章` 双标题），
            // 只按真实数值合并，避免旧实现"中文数字统一归一为 n"导致的误合并。
            let titles = dedup_same_value(titles);
            if titles.len() >= 2 {
                // A0: 章号连续性检测——解析每章真实章号（如「第六章」→6），检测跳号/缺章，
                // 出现 gap 时 warn 记录（源文件缺章/标题解析失败 → 后续 chXX 编号会整体错位，
                // 污染 worldline/relations/事件卡等带章节产物；此处先可见化，供人工修正源）。
                let gaps = chapter_gaps(&titles);
                if !gaps.is_empty() {
                    tracing::warn!(
                        n_titles = titles.len(),
                        gaps = ?gaps,
                        "章节切分检测到章号不连续（缺章/跳号），下游 chXX 编号将与该序列对齐"
                    );
                }
                return slice_chapters(text, &titles);
            }
        }
    }
    fallback_windows(text)
}

/// 多规则自动甄别：逐条统计匹配数（要求相邻标题间隔 >1000），返回匹配最多且启用者。
pub(crate) fn pick_toc_rule(text: &str) -> Option<&'static TocRule> {
    let mut best: Option<&'static TocRule> = None;
    let mut max_num: usize = 0;
    for rule in TOC_RULES.iter().filter(|r| r.enabled) {
        let Ok(re) = regex::Regex::new(rule.chapter) else {
            continue;
        };
        let mut num = 0usize;
        let mut prev_end: Option<usize> = None;
        for m in re.find_iter(text) {
            let ok_gap = match prev_end {
                None => true,
                Some(pe) => m.start().saturating_sub(pe) > 1000,
            };
            if ok_gap {
                num += 1;
            }
            prev_end = Some(m.end());
        }
        // Legado 用 `num >= maxNum`（后匹配者平局时胜）。这里取首次达到最大者的近似即可。
        if num > max_num {
            max_num = num;
            best = Some(rule);
        }
    }
    best
}

/// 相邻同真实章号去重：`# 第1章` 与裸 `第1章` 这类双标题各识别到一次，章号数值相同 →
/// 合并为最后出现的标题（保留正文标题而非页眉）。仅按真实数值判定相等，不同数值保留。
pub(crate) fn dedup_same_value(titles: Vec<(usize, String)>) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (idx, title) in titles {
        if let Some((last_idx, last_title)) = out.last_mut() {
            // 两条标题真实章号都存在且相等 → 认为同章重复，更新为此条的字节起点（更靠后、更接近正文）
            if let (Some(a), Some(b)) = (chapter_value(&title.clone()), chapter_value(last_title)) {
                if a == b {
                    *last_idx = idx;
                    *last_title = title_truncate(&title);
                    continue;
                }
            }
        }
        out.push((idx, title_truncate(&title)));
    }
    out
}

/// 从标题头部提取章号并转成真实数值（中文数字支持到万位），失败返回 None。
/// 例："第一章 续一" → 1；"第一千零六十章 嫌弃的对象" → 1060；"第 1 章 得加钱" → 1。
pub(crate) fn chapter_value(title: &str) -> Option<i64> {
    let t = title.trim();
    // 2026-08-19: 支持全角/半角括号前导序号（度蜜月 `　　（5）释放`、`(7) xxx`）→ 返回括号内数值。
    // 模式：^(（|() N (）|)) [可带 章/话/集] 可选标题
    for (open, close) in [('（', '）'), ('(', ')'), ('【', '】'), ('[', ']')] {
        let trimmed = t.trim_start_matches(['\u{3000}', ' ', '\t']);
        if let Some(rest) = trimmed.strip_prefix(open) {
            let digits: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                // ASCII 数字 1 char == 1 byte，digits.len() 即字节偏移
                let after = &rest[digits.len()..];
                if let Some(_after_close) = after.strip_prefix(close) {
                    // 括号后紧跟 章/话/集 或直接标题 → 视作章号
                    if let Some(v) = digits.parse::<i64>().ok() {
                        // 允许括号后有标题（如「（5）释 放」）或前后缀（「（5）章」）
                        return Some(v);
                    }
                }
            }
        }
    }
    // 去掉可选的 "第 / 章 / 节 / 卷 / 部分 / 序 段落" 前后缀，取中间的数字段
    let inner = t
        .strip_prefix("第")
        .map(|s| {
            // 去掉结尾的 章|节|卷|集|部|篇 及其后所有内容
            let s = s.trim_start();
            if let Some(pos) = s.find(|c| matches!(c, '章' | '节' | '卷' | '集' | '部' | '篇')) {
                &s[..pos]
            } else {
                s
            }
        })
        .unwrap_or(t);
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    cn_number_to_i64(inner)
}

/// A0 完整版 (2026-08-19)：为章节标题序列分配 chXX 编号 —— **chXX = max(原著章号, 递增游标)**。
/// - 原著章号优先（「第六章」→ 6）：缺章自动跳号，让 chXX 与原著标题对齐
///   （源缺第五章时，标题「第六章 初恋」所在的第 5 个切片 → ch06，ch05 空缺可见）。
/// - 解析失败（楔子/序章/番外等非「第N章」格式）→ 用递增游标续号；
/// - 章号回退/重复（如续章「第一章 续一」跟在楔子后，章号=1 却排在切片 2）
///   → 用游标顺延去重，绝不产出重复 chXX。
/// 语义等价于一次遍历维护 `next_id`：`id = max(v, next_id)`，`next_id = id + 1`。
/// 该编号体系同时用于 pack.chapters.id / chapters/{id}.md 文件名 / nodes.chapter_id /
/// distil_chapters 蒸馏前缀 / build_roster_input 的【chXX】标注，保证下游
/// worldline / relations / 事件卡 chapterRange 全部与原著标题对齐（修复 D1「章号错位」）。
pub(crate) fn chapter_id_seq(titles: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::with_capacity(titles.len());
    let mut next_id: i64 = 1;
    for (t, _b) in titles {
        let v = chapter_value(t).unwrap_or(0);
        let id = v.max(next_id);
        out.push(format!("ch{:02}", id));
        next_id = id + 1;
    }
    out
}

/// 将"一~万"中文数字串转成数值。
/// A0: 检测章节标题序列的章号连续性（解析「第X章」→ 数值）。
/// 返回跳号描述列表（如 ["5→6 之间缺章(应为 5)", "6→7…"]）。
/// 规则：章号必须严格递增；相邻差值 >1 视为缺章（gap：前章号+1..后章号-1 全缺）。
/// 解析失败（标题不是「第N章」格式，如「序章/番外」）跳过不参与判断。
pub(crate) fn chapter_gaps(titles: &[(usize, String)]) -> Vec<String> {
    let mut gaps = Vec::new();
    let mut prev: Option<i64> = None;
    for (_, title) in titles {
        let Some(cur) = chapter_value(title) else { continue };
        if let Some(p) = prev {
            if cur <= p {
                gaps.push(format!("章号不递增：{p} → {cur}（{}）", title_truncate(title)));
            } else if cur - p > 1 {
                let missing: Vec<i64> = ((p + 1)..cur).collect();
                gaps.push(format!(
                    "{p} → {cur} 缺章：{}（缺 {}）",
                    title_truncate(title),
                    missing.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
                ));
            }
        }
        prev = Some(cur);
    }
    gaps
}

pub(crate) fn cn_number_to_i64(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    let digits = |c: char| -> Option<i64> {
        match c {
            '零' | '〇' => Some(0),
            '一' => Some(1),
            '二' | '两' | '俩' => Some(2),
            '三' => Some(3),
            '四' => Some(4),
            '五' => Some(5),
            '六' => Some(6),
            '七' => Some(7),
            '八' => Some(8),
            '九' => Some(9),
            _ => None,
        }
    };
    let unit = |c: char| -> Option<i64> {
        match c {
            '十' => Some(10),
            '百' => Some(100),
            '千' => Some(1000),
            '万' => Some(10_000),
            _ => None,
        }
    };
    // 万位分割：lv * 10000 + rv
    if let Some(pos) = s.find('万') {
        let left = &s[..pos];
        let right = &s[pos + 1..];
        let lv = if left.is_empty() { 1 } else { cn_number_to_i64(left)? };
        let rv = if right.is_empty() { 0 } else { cn_number_to_i64(right)? };
        return Some(lv * 10_000 + rv);
    }
    let mut result = 0i64;
    let mut num = 0i64;
    for c in s.chars() {
        if let Some(d) = digits(c) {
            num = d;
        } else if let Some(u) = unit(c) {
            if num == 0 {
                num = 1;
            }
            result += num * u;
            num = 0;
        } else {
            return None;
        }
    }
    result += num;
    if result > 0 {
        Some(result)
    } else {
        None
    }
}

pub(crate) fn title_truncate(title: &str) -> String {
    title.trim().chars().take(80).collect()
}

/// 按标题 byte 起点切分正文为 (title, content)。content 含标题行。
pub(crate) fn slice_chapters(text: &str, titles: &[(usize, String)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, (idx, title)) in titles.iter().enumerate() {
        let end = if i + 1 < titles.len() {
            titles[i + 1].0
        } else {
            text.len()
        };
        let content = text[*idx..end].trim().to_string();
        if !content.is_empty() {
            out.push((title.clone(), content));
        }
    }
    if out.is_empty() {
        out.push(("第1章".into(), text.to_string()));
    }
    out
}

/// 兜底切分：按固定窗口切，并对齐到最近的换行符（不在句中硬切）。
pub(crate) fn fallback_windows(text: &str) -> Vec<(String, String)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return vec![("第1章".into(), text.to_string())];
    }
    let mut parts: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    let mut part_i = 1;
    while i < chars.len() {
        let start_byte = chars[i].0;
        let mut end_i = (i + FALLBACK_WINDOW).min(chars.len());
        if end_i < chars.len() {
            let min_j = i + FALLBACK_WINDOW / 2;
            let mut j = end_i;
            while j > min_j {
                if chars[j].1 == '\n' {
                    end_i = j;
                    break;
                }
                j -= 1;
            }
        }
        let end_byte = if end_i >= chars.len() {
            text.len()
        } else {
            chars[end_i].0
        };
        let chunk = text[start_byte..end_byte].trim();
        if !chunk.is_empty() {
            parts.push((format!("第{part_i}章"), chunk.to_string()));
            part_i += 1;
        }
        if end_i >= chars.len() {
            break;
        }
        i = end_i;
    }
    if parts.is_empty() {
        parts.push(("第1章".into(), text.to_string()));
    }
    parts
}

pub(crate) fn is_clean_name(name: &str) -> bool {
    kaleido_core::is_clean_cast_name(name)
}

/// [fix 2026-08-15] 从 LLM 输出中提取完整 JSON 数组：从第一个 `[` 起按深度配对，
/// 处理 LLM 常在数组后追加说明文字（「以上是角色…」「```json」）或嵌套 `]` 的情况。
pub(crate) fn extract_json_array(raw: &str) -> Option<Vec<serde_json::Value>> {
    let start = raw.find('[')?;
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'"' if !escaped => in_str = !in_str,
            b'\\' if in_str && !escaped => escaped = true,
            _ => escaped = false,
        }
        if in_str {
            continue;
        }
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let e = end?;
    serde_json::from_str(&raw[start..=e]).ok()
}

/// [fix 2026-08-15] 章节目录页判别：短文件（<5KB）+ 大量「- 第N章：标题」行 = 导入时
/// 把目录页当章节存了（安全屋 pack ch01-03.md 实证）。目录页不是正文，LLM 会从标题瞎抽。
pub(crate) fn is_chapter_toc(content: &str) -> bool {
    if content.len() > 5000 {
        return false;
    }
    let toc_lines = content
        .lines()
        .filter(|l| l.trim_start().starts_with("- ") && l.contains("第") && l.contains('章'))
        .count();
    toc_lines >= 5
}
