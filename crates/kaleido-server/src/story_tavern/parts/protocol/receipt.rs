//! parts/protocol/receipt — [2026-10-06 二期①] 【回执】协议（自 p010.rs 整体迁入）：
//! TurnReceipt 解析/merge + split_turn_receipt_from_narrative 单尾 JSON 剥离。

use super::super::*;

/// 回执协议的字符串感知平衡 JSON 提取：跳过 "..."（含 \" 转义）内的花括号。
/// 返回 (起始偏移, 结束偏移)（相对 `s`，end 为闭花括号后一位）。
fn find_balanced_object(s: &str) -> Option<(usize, usize)> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, start + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// 本回合结构化回执（【回执】JSON 尾块的类型化视图，全部字段可选）。
#[derive(Debug, Default, Clone)]
pub(crate) struct TurnReceipt {
    /// 玩家选项（覆盖旧【选项】/【询问】；询问停笔 = options 有值而正文为空）。
    pub(crate) options: Option<Vec<String>>,
    /// 剧情出口推进（旧【节点推进:ID】）。
    pub(crate) advance: Option<String>,
    /// 章节摘要（旧【章节摘要】）。
    pub(crate) chapter_diary: Option<String>,
    /// 时间推进（旧 [时间推进: X]）。
    pub(crate) time_advance: Option<String>,
    /// 角色状态更新（旧【状态更新】）。
    pub(crate) state_updates: Option<Vec<ActorStateUpdate>>,
    /// 规则检定请求（旧【检定】；单对象或数组均接受）。
    pub(crate) check: Option<Vec<kaleido_core::TurnCheckRequest>>,
    /// 自建面板（旧【面板】；含 eventbook 档）。
    pub(crate) panels: Option<Vec<kaleido_core::TavernPanel>>,
    /// 程序卡 HTML（旧【程序】…【/程序】）。
    pub(crate) program: Option<String>,
    /// 导演计划（旧【导演计划】{...}｜none；skip/none = 无需更新）。
    pub(crate) director_plan: Option<DirectorPlanUpdate>,
}

impl TurnReceipt {
    /// 字段级合并：`self`（管道后 post）优先，`fallback`（管道前 pre stash）补缺。
    pub(crate) fn merge(self, fallback: TurnReceipt) -> TurnReceipt {
        let has = |x: &Option<Vec<String>>| x.as_ref().map_or(false, |v| !v.is_empty());
        let mut out = TurnReceipt {
            options: if has(&self.options) { self.options } else { fallback.options },
            advance: self.advance.or(fallback.advance),
            chapter_diary: self
                .chapter_diary
                .filter(|s| !s.trim().is_empty())
                .or(fallback.chapter_diary),
            time_advance: self
                .time_advance
                .filter(|s| !s.trim().is_empty())
                .or(fallback.time_advance),
            state_updates: match &self.state_updates {
                Some(v) if !v.is_empty() => self.state_updates,
                _ => fallback.state_updates,
            },
            check: match &self.check {
                Some(v) if !v.is_empty() => self.check,
                _ => fallback.check,
            },
            panels: match &self.panels {
                Some(v) if !v.is_empty() => self.panels,
                _ => fallback.panels,
            },
            program: self
                .program
                .filter(|s| !s.trim().is_empty())
                .or(fallback.program),
            director_plan: self.director_plan.or(fallback.director_plan),
        };
        // 全空 → 归一为 Default（Option 全 None），让调用方 is_none() 判断可靠。
        if out.options.is_none()
            && out.advance.is_none()
            && out.chapter_diary.is_none()
            && out.time_advance.is_none()
            && out.state_updates.is_none()
            && out.check.is_none()
            && out.panels.is_none()
            && out.program.is_none()
            && out.director_plan.is_none()
        {
            out = TurnReceipt::default();
        }
        out
    }

    /// 回执是否携带任意字段。
    pub(crate) fn is_empty(&self) -> bool {
        self.options.is_none()
            && self.advance.is_none()
            && self.chapter_diary.is_none()
            && self.time_advance.is_none()
            && self.state_updates.is_none()
            && self.check.is_none()
            && self.panels.is_none()
            && self.program.is_none()
            && self.director_plan.is_none()
    }
}

/// 从叙述文本提取「【回执】{...}」单一 JSON 尾块并剥离。
/// 返回 (clean_text, Option<TurnReceipt>)：
/// - 无标记 → 原文原样返回，None（零开销快速路径）；
/// - 有标记但 JSON 缺失/失败 → 剥掉标记起的所有尾段（零残留）+ warn，None；
/// - 成功 → 剥掉整个尾块，Some(回执)。容忍 ```json 围栏与闭括号后的杂散文字。
pub(crate) fn split_turn_receipt_from_narrative(full: &str) -> (String, Option<TurnReceipt>) {
    const MARKER: &str = "【回执】";
    let Some(mark) = full.rfind(MARKER) else {
        return (full.to_string(), None);
    };
    let after = &full[mark + MARKER.len()..];
    let receipt = match find_balanced_object(after) {
        Some((s, e)) => match serde_json::from_str::<Value>(&after[s..e]) {
            Ok(v) => receipt_from_value(&v),
            Err(err) => {
                warn!(error = %err, "st turn receipt JSON parse failed; 尾块剥离降级走旧标记");
                None
            }
        },
        None => {
            warn!("st turn receipt marker present but no JSON object; 尾块剥离降级走旧标记");
            None
        }
    };
    // 剥离范围：标记起（含其前导换行）到闭括号后；其后残留杂散文字一并丢弃
    //（协议约定回执是最后一段）。回执前若全为空白正文，保留标记前原文。
    let head = full[..mark].trim_end();
    (head.to_string(), receipt)
}

/// Value → TurnReceipt 的逐字段容错映射：单字段类型不符只丢该字段。
fn receipt_from_value(v: &Value) -> Option<TurnReceipt> {
    let obj = v.as_object()?;
    let mut r = TurnReceipt::default();

    if let Some(opts) = obj.get("options") {
        let mut out: Vec<String> = Vec::new();
        if let Value::Array(items) = opts {
            for it in items {
                match it {
                    Value::String(s) => {
                        if !s.trim().is_empty() {
                            out.push(s.clone());
                        }
                    }
                    Value::Object(m) => {
                        if let Some(s) = m.get("label").and_then(|x| x.as_str()) {
                            if !s.trim().is_empty() {
                                out.push(s.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !out.is_empty() {
            r.options = Some(out);
        }
    }

    if let Some(a) = obj.get("advance").and_then(|x| x.as_str()) {
        let a = a.trim();
        if !a.is_empty() {
            r.advance = Some(a.to_string());
        }
    }

    if let Some(d) = obj.get("chapterDiary").and_then(|x| x.as_str()) {
        let d = d.trim();
        if !d.is_empty() {
            r.chapter_diary = Some(d.to_string());
        }
    }

    if let Some(t) = obj.get("timeAdvance").and_then(|x| x.as_str()) {
        let t = t.trim();
        if !t.is_empty() {
            r.time_advance = Some(t.to_string());
        }
    }

    if let Some(su) = obj.get("stateUpdates") {
        let items: Vec<Value> = match su {
            Value::Array(a) => a.clone(),
            Value::Object(_) => vec![su.clone()],
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        for it in items {
            if let Ok(u) = serde_json::from_value::<ActorStateUpdate>(it) {
                out.push(u);
            }
        }
        if !out.is_empty() {
            r.state_updates = Some(out);
        }
    }

    if let Some(c) = obj.get("check") {
        let items: Vec<Value> = match c {
            Value::Array(a) => a.clone(),
            Value::Object(_) => vec![c.clone()],
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        for it in items {
            if let Ok(u) = serde_json::from_value::<kaleido_core::TurnCheckRequest>(it) {
                out.push(u);
            }
        }
        if !out.is_empty() {
            r.check = Some(out);
        }
    }

    if let Some(p) = obj.get("panels") {
        let items: Vec<Value> = match p {
            Value::Array(a) => a.clone(),
            Value::Object(_) => vec![p.clone()],
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        for it in items {
            if let Ok(panel) = serde_json::from_value::<kaleido_core::TavernPanel>(it.clone()) {
                if !panel.name.is_empty() && !panel.content.is_empty() {
                    out.push(panel);
                }
                continue;
            }
            // eventbook 变体：content 为 JSON 对象（旧协议里模型可能不给字符串）→ 序列化兜底
            let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("").trim();
            if name.is_empty() {
                continue;
            }
            let kind = it
                .get("kind")
                .and_then(|x| x.as_str())
                .unwrap_or("markdown")
                .to_string();
            let content = match it.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(o @ Value::Object(_)) => o.to_string(),
                _ => continue,
            };
            if content.is_empty() {
                continue;
            }
            out.push(kaleido_core::TavernPanel::new(name.to_string(), kind, content));
        }
        if !out.is_empty() {
            r.panels = Some(out);
        }
    }

    if let Some(pg) = obj.get("program").and_then(|x| x.as_str()) {
        let pg = pg.trim();
        if !pg.is_empty() {
            r.program = Some(pg.to_string());
        }
    }

    if let Some(dp) = obj.get("directorPlan") {
        match dp {
            Value::String(s) => {
                let s = s.trim();
                if s.is_empty() || s == "none" {
                    r.director_plan = Some(DirectorPlanUpdate::Skip);
                }
            }
            Value::Object(m) => {
                if m.get("skip").and_then(|x| x.as_bool()) == Some(true) {
                    r.director_plan = Some(DirectorPlanUpdate::Skip);
                } else if let Ok(plan) = serde_json::from_value::<DirectorPlanOutput>(dp.clone()) {
                    if !plan.goal.trim().is_empty() {
                        r.director_plan = Some(DirectorPlanUpdate::Set(plan));
                    }
                }
            }
            _ => {}
        }
    }

    if r.is_empty() {
        None
    } else {
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_absent_is_noop() {
        let (clean, r) = split_turn_receipt_from_narrative("正文一段。");
        assert_eq!(clean, "正文一段。");
        assert!(r.is_none());
    }

    #[test]
    fn receipt_full_parse_and_strip() {
        let full = "正文。\n【回执】\n{\"options\":[\"a\",\"b\"],\"advance\":\"n2\",\"timeAdvance\":\"次日清晨\"}";
        let (clean, r) = split_turn_receipt_from_narrative(full);
        assert_eq!(clean, "正文。");
        let r = r.expect("receipt parsed");
        assert_eq!(r.options.as_deref(), Some(&["a".to_string(), "b".to_string()][..]));
        assert_eq!(r.advance.as_deref(), Some("n2"));
        assert_eq!(r.time_advance.as_deref(), Some("次日清晨"));
    }

    #[test]
    fn receipt_fenced_and_trailing_prose_tolerated() {
        let full = "正文。\n【回执】\n```json\n{\"options\":[\"x\"]}\n```\n以上。";
        let (clean, r) = split_turn_receipt_from_narrative(full);
        assert_eq!(clean, "正文。");
        assert_eq!(r.unwrap().options.as_deref(), Some(&["x".to_string()][..]));
    }

    #[test]
    fn receipt_braces_inside_strings_are_string_aware() {
        let full = "正文。\n【回执】{\"program\":\"<script>if(a){b()}</script>\",\"options\":[\"y\"]}";
        let (_, r) = split_turn_receipt_from_narrative(full);
        let r = r.expect("string-aware match");
        assert_eq!(r.program.as_deref(), Some("<script>if(a){b()}</script>"));
        assert_eq!(r.options.as_deref(), Some(&["y".to_string()][..]));
    }

    #[test]
    fn receipt_malformed_json_stripped_zero_residue() {
        let full = "正文。\n【回执】{\"options\":[\"a\", oops]}";
        let (clean, r) = split_turn_receipt_from_narrative(full);
        assert_eq!(clean, "正文。");
        assert!(r.is_none());
    }

    #[test]
    fn receipt_marker_without_json_stripped() {
        let full = "正文。\n【回执】然后就没了";
        let (clean, r) = split_turn_receipt_from_narrative(full);
        assert_eq!(clean, "正文。");
        assert!(r.is_none());
    }

    #[test]
    fn receipt_wrong_field_type_only_drops_that_field() {
        let full = "正文。\n【回执】{\"options\":\"不是数组\",\"advance\":\"n3\",\"stateUpdates\":[{\"characterId\":\"c1\",\"fields\":{\"hp\":-2}}]}";
        let (_, r) = split_turn_receipt_from_narrative(full);
        let r = r.expect("partial receipt");
        assert!(r.options.is_none(), "类型不符的字段仅缺席");
        assert_eq!(r.advance.as_deref(), Some("n3"));
        assert_eq!(r.state_updates.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn receipt_director_plan_skip_and_none_variants() {
        let (_, r1) = split_turn_receipt_from_narrative("【回执】{\"directorPlan\":\"none\"}");
        assert!(matches!(r1.unwrap().director_plan, Some(DirectorPlanUpdate::Skip)));
        let (_, r2) = split_turn_receipt_from_narrative("【回执】{\"directorPlan\":{\"skip\":true}}");
        assert!(matches!(r2.unwrap().director_plan, Some(DirectorPlanUpdate::Skip)));
        let (_, r3) =
            split_turn_receipt_from_narrative("【回执】{\"directorPlan\":{\"goal\":\"逼她摊牌\"}}");
        match r3.unwrap().director_plan {
            Some(DirectorPlanUpdate::Set(p)) => assert_eq!(p.goal, "逼她摊牌"),
            other => panic!("expected Set, got {:?}", other.is_none()),
        }
    }

    #[test]
    fn receipt_eventbook_content_object_serialized() {
        let full = "正文。\n【回执】{\"panels\":[{\"name\":\"事件书\",\"kind\":\"eventbook\",\"content\":{\"events\":[{\"title\":\"约定\",\"done\":false}]}}]}";
        let (_, r) = split_turn_receipt_from_narrative(full);
        let panels = r.unwrap().panels.unwrap();
        assert_eq!(panels.len(), 1);
        assert!(panels[0].content.contains("\"events\""));
    }

    #[test]
    fn receipt_options_accept_label_objects() {
        let full = "正文。\n【回执】{\"options\":[{\"label\":\"追问\"},{\"label\":\"离开\"}]}";
        let (_, r) = split_turn_receipt_from_narrative(full);
        assert_eq!(
            r.unwrap().options.as_deref(),
            Some(&["追问".to_string(), "离开".to_string()][..])
        );
    }

    #[test]
    fn receipt_merge_post_wins_and_fallback_fills() {
        let mut post = TurnReceipt::default();
        post.options = Some(vec!["post".into()]);
        let mut pre = TurnReceipt::default();
        pre.advance = Some("n5".into());
        pre.options = Some(vec!["pre".into()]);
        let merged = post.clone().merge(pre.clone());
        assert_eq!(merged.options.as_deref(), Some(&["post".to_string()][..]));
        assert_eq!(merged.advance.as_deref(), Some("n5"));
        // post 全空 → 用 fallback
        let merged2 = TurnReceipt::default().merge(pre);
        assert_eq!(merged2.options.as_deref(), Some(&["pre".to_string()][..]));
        assert_eq!(merged2.advance.as_deref(), Some("n5"));
        let _ = post; // clone 已用
    }

    #[test]
    fn receipt_empty_object_is_none() {
        let (_, r) = split_turn_receipt_from_narrative("正文。\n【回执】{}");
        assert!(r.is_none());
    }
}
