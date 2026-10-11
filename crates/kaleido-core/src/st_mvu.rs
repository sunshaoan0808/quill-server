//! [MVU 2026-10-11, 吞噬自 AIcove《MVU变量与表格记忆兼容方案》]
//! SillyTavern MVU（MagVarUpdate，MIT）变量系统的**只解析、不执行**兼容层。
//!
//! 设计原则（对齐 AIcove 实测方案 + 本仓库红线）：
//! - 不引入 JS 运行时（JS-Slash-Runner 为 PolyForm NonCommercial 且深耦合酒馆内部，
//!   明确拒绝）；命令参数只支持「引号字符串 / 数字四则运算 / JSON 字面量」。
//! - 失败语义 = 逐条跳过 + 诊断记录，不中断回合、不假装成功。
//! - 状态 = 冻结基线 + 沿持久化消息链重放 `<UpdateVariable>` 块；
//!   stat_data 永远可以从落盘消息重建（配合 ADR-0004 真相源不变式）。
//! - 本模块为纯函数层，零 IO、零新依赖，不直接接触回合引擎；
//!   接线（st_import 识别 InitVar 条目、server 回合循环落块/注入）按 SOP 后续小步合入。
//!
//! 兼容子集（与 AIcove 对齐）：
//! - 命令：`_.set` / `_.add` / `_.insert` / `_.assign` / `_.remove`（别名 `_.unset` `_.delete`）
//! - 块：`<UpdateVariable>…</UpdateVariable>`、`<JSONPatch>[…]</JSONPatch>`（replace/add/remove/delta）
//! - `<initvar>` 块（开场白内，JSON）覆盖世界书 `[InitVar]` 条目基线
//! - 已知缺口（v1）：YAML 基线、JSON5 单引号、`move`/`math.*`、表格记忆 `<tableEdit>`
//!   ——检测到即诊断跳过，不假装成功；按需再小步补。

use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// 诊断
// ---------------------------------------------------------------------------

/// 单条 MVU 解析/执行诊断（永远只追加、不 panic、不阻断）。
#[derive(Debug, Clone, PartialEq)]
pub struct MvuDiag {
    pub severity: MvuDiagSeverity,
    /// 机器可读类别，如 `unknown_command` / `bad_args` / `unsupported_expr`。
    pub code: String,
    pub message: String,
    /// 出错时的原始行/块片段（截断到 200 字符，方便定位）。
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvuDiagSeverity {
    /// 跳过该条，继续。
    Skip,
    /// 初始化整体失败（基线不可信）。
    InitFailed,
}

impl MvuDiag {
    fn skip(code: &str, message: impl Into<String>, raw: &str) -> Self {
        Self {
            severity: MvuDiagSeverity::Skip,
            code: code.into(),
            message: message.into(),
            raw: clip(raw, 200),
        }
    }

    fn init_failed(code: &str, message: impl Into<String>, raw: &str) -> Self {
        Self {
            severity: MvuDiagSeverity::InitFailed,
            code: code.into(),
            message: message.into(),
            raw: clip(raw, 200),
        }
    }
}

fn clip(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------------------
// 标签块定位（对原始文本做 ASCII 大小写不敏感匹配，避免大小写折叠改变字节长度）
// ---------------------------------------------------------------------------

/// 在 `text` 中 ASCII 大小写不敏感地查找 `needle`（needle 必须是 ASCII），
/// 返回字节偏移。保证返回的偏移对 `text` 是合法切片边界。
fn find_ci(text: &str, needle: &str) -> Option<usize> {
    debug_assert!(needle.is_ascii());
    let t = text.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || t.len() < n.len() {
        return None;
    }
    let eq_ci = |a: u8, b: u8| a.eq_ignore_ascii_case(&b);
    'outer: for i in 0..=(t.len() - n.len()) {
        for j in 0..n.len() {
            if !eq_ci(t[i + j], n[j]) {
                continue 'outer;
            }
        }
        // needle 纯 ASCII 且以 '<' 开头/结尾，命中处必是字符边界
        return Some(i);
    }
    None
}

fn tag_open_lower(tag: &str) -> String {
    format!("<{}>", tag.to_ascii_lowercase())
}

fn tag_close_lower(tag: &str) -> String {
    format!("</{}>", tag.to_ascii_lowercase())
}

/// 提取所有 `<tag>…</tag>` 块体（大小写不敏感，ASCII 安全）。
fn find_tag_blocks(text: &str, tag: &str) -> Vec<String> {
    let open = tag_open_lower(tag);
    let close = tag_close_lower(tag);
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(start) = find_ci(&text[from..], &open) {
        let body_from = from + start + open.len();
        match find_ci(&text[body_from..], &close) {
            Some(rel) => {
                out.push(text[body_from..body_from + rel].trim().to_string());
                from = body_from + rel + close.len();
            }
            None => break, // 未闭合：丢弃剩余（防泄漏）
        }
    }
    out
}

/// 移除所有 `<tag>…</tag>` 跨度（含未闭合时吞到结尾，防泄漏）。
fn remove_tag_spans(text: &str, tag: &str) -> String {
    let open = tag_open_lower(tag);
    let close = tag_close_lower(tag);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        match find_ci(rest, &open) {
            Some(start) => {
                let body_from = start + open.len();
                let end = match find_ci(&rest[body_from..], &close) {
                    Some(rel) => body_from + rel + close.len(),
                    None => rest.len(),
                };
                out.push_str(&rest[..start]);
                rest = &rest[end..];
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 路径（dot 段 + [index] 后缀，lodash 子集；容忍 `stat_data.` / `$` 前缀）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum PathSeg {
    Key(String),
    Index(usize),
}

fn parse_path(raw: &str) -> Result<Vec<PathSeg>, String> {
    let mut s = raw.trim();
    // 容忍 MVU 常见前缀写法（stat_data 即根的别名）
    if let Some(rest) = s.strip_prefix("stat_data.") {
        s = rest;
    } else if s == "stat_data" {
        return Err("path is root alias only".into());
    }
    let s = s.trim_start_matches('$');
    if s.is_empty() {
        return Err(format!("empty path in `{raw}`"));
    }
    let mut segs = Vec::new();
    for part in s.split('.') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("empty path segment in `{raw}`"));
        }
        let (key, mut rest) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        if !key.is_empty() {
            segs.push(PathSeg::Key(key.to_string()));
        }
        while let Some(start) = rest.find('[') {
            let end = rest[start..]
                .find(']')
                .ok_or_else(|| format!("unbalanced bracket in `{raw}`"))?
                + start;
            let idx_raw = rest[start + 1..end].trim();
            let idx: usize = idx_raw
                .parse()
                .map_err(|_| format!("bad array index `{idx_raw}` in `{raw}`"))?;
            segs.push(PathSeg::Index(idx));
            rest = &rest[end + 1..];
        }
    }
    if segs.is_empty() {
        return Err(format!("no segments in path `{raw}`"));
    }
    Ok(segs)
}

/// 读取路径（只读，不创建中间层）。路径语义相对 stat_data 根。
pub fn get_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let segs = parse_path(path).ok()?;
    let mut cur = root;
    for seg in segs {
        cur = match seg {
            PathSeg::Key(k) => cur.get(k)?,
            PathSeg::Index(i) => cur.get(i)?,
        };
    }
    Some(cur)
}

/// 写路径：中间层缺失时自动创建（文本段→object，下标段→array）。
pub fn set_path(root: &mut Value, path: &str, value: Value) -> Result<(), String> {
    let segs = parse_path(path)?;
    let mut cur = root;
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        match seg {
            PathSeg::Key(k) => {
                if last {
                    match cur {
                        Value::Object(m) => {
                            m.insert(k.clone(), value);
                            return Ok(());
                        }
                        Value::Null => {
                            let mut m = Map::new();
                            m.insert(k.clone(), value);
                            *cur = Value::Object(m);
                            return Ok(());
                        }
                        _ => return Err(format!("path `{path}`: parent is not an object")),
                    }
                }
                if !cur.is_object() {
                    if cur.is_null() {
                        *cur = Value::Object(Map::new());
                    } else {
                        return Err(format!("path `{path}`: intermediate `{k}` is not an object"));
                    }
                }
                let m = cur.as_object_mut().unwrap();
                cur = m.entry(k.clone()).or_insert(Value::Null);
            }
            PathSeg::Index(idx) => {
                if last {
                    match cur {
                        Value::Array(a) => {
                            if *idx > a.len() {
                                return Err(format!(
                                    "path `{path}`: index {idx} beyond len {}",
                                    a.len()
                                ));
                            }
                            if *idx == a.len() {
                                a.push(value);
                            } else {
                                a[*idx] = value;
                            }
                            return Ok(());
                        }
                        Value::Null => {
                            let mut a: Vec<Value> = Vec::new();
                            a.resize_with(*idx + 1, || Value::Null);
                            a[*idx] = value;
                            *cur = Value::Array(a);
                            return Ok(());
                        }
                        _ => return Err(format!("path `{path}`: parent is not an array")),
                    }
                }
                if !cur.is_array() {
                    if cur.is_null() {
                        *cur = Value::Array(Vec::new());
                    } else {
                        return Err(format!("path `{path}`: intermediate is not an array"));
                    }
                }
                let a = cur.as_array_mut().unwrap();
                while a.len() <= *idx {
                    a.push(Value::Null);
                }
                cur = &mut a[*idx];
            }
        }
    }
    Ok(())
}

/// 删除路径（object key 或 array 元素），返回被删的值。
pub fn remove_path(root: &mut Value, path: &str) -> Result<Value, String> {
    let segs = parse_path(path)?;
    let (last_seg, parents) = segs.split_last().ok_or("no segments")?;
    let mut cur = root;
    for seg in parents {
        cur = match seg {
            PathSeg::Key(k) => cur.get_mut(k).ok_or("missing parent")?,
            PathSeg::Index(i) => cur
                .as_array_mut()
                .ok_or("parent not array")?
                .get_mut(*i)
                .ok_or("missing index")?,
        };
    }
    match last_seg {
        PathSeg::Key(k) => match cur.as_object_mut() {
            Some(m) => m.remove(k).ok_or_else(|| format!("missing key `{k}`")),
            None => Err("parent not object".into()),
        },
        PathSeg::Index(i) => {
            let a = cur.as_array_mut().ok_or("parent not array")?;
            if *i >= a.len() {
                return Err(format!("index {i} out of range"));
            }
            Ok(a.remove(*i))
        }
    }
}

// ---------------------------------------------------------------------------
// 值字面量：引号字符串 / 数字四则 / JSON 字面量（不执行任何 JS）
// ---------------------------------------------------------------------------

/// 安全求值：只允许数字、`+ - * / ( )`、空白。其余一律 None（拒绝执行）。
pub fn eval_number_expr(expr: &str) -> Option<f64> {
    let tokens = tokenize_expr(expr)?;
    let mut pos = 0usize;
    let v = parse_expr_add(&tokens, &mut pos)?;
    if pos != tokens.len() {
        return None;
    }
    if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

fn tokenize_expr(s: &str) -> Option<Vec<Tok>> {
    let mut toks = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
            }
            '+' => {
                toks.push(Tok::Plus);
                chars.next();
            }
            '-' => {
                toks.push(Tok::Minus);
                chars.next();
            }
            '*' => {
                toks.push(Tok::Star);
                chars.next();
            }
            '/' => {
                toks.push(Tok::Slash);
                chars.next();
            }
            '(' => {
                toks.push(Tok::LParen);
                chars.next();
            }
            ')' => {
                toks.push(Tok::RParen);
                chars.next();
            }
            '0'..='9' | '.' => {
                let mut num = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_digit() || n == '.' {
                        num.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let v: f64 = num.parse().ok()?;
                toks.push(Tok::Num(v));
            }
            _ => return None, // 任何其它字符 → 拒绝（不 eval）
        }
    }
    Some(toks)
}

fn parse_expr_add(toks: &[Tok], pos: &mut usize) -> Option<f64> {
    let mut left = parse_expr_mul(toks, pos)?;
    while *pos < toks.len() {
        match toks[*pos] {
            Tok::Plus => {
                *pos += 1;
                left += parse_expr_mul(toks, pos)?;
            }
            Tok::Minus => {
                *pos += 1;
                left -= parse_expr_mul(toks, pos)?;
            }
            _ => break,
        }
    }
    Some(left)
}

fn parse_expr_mul(toks: &[Tok], pos: &mut usize) -> Option<f64> {
    let mut left = parse_expr_unary(toks, pos)?;
    while *pos < toks.len() {
        match toks[*pos] {
            Tok::Star => {
                *pos += 1;
                left *= parse_expr_unary(toks, pos)?;
            }
            Tok::Slash => {
                *pos += 1;
                let d = parse_expr_unary(toks, pos)?;
                left /= d;
            }
            _ => break,
        }
    }
    Some(left)
}

fn parse_expr_unary(toks: &[Tok], pos: &mut usize) -> Option<f64> {
    match toks.get(*pos)? {
        Tok::Minus => {
            *pos += 1;
            Some(-parse_expr_unary(toks, pos)?)
        }
        Tok::Plus => {
            *pos += 1;
            parse_expr_unary(toks, pos)
        }
        Tok::Num(v) => {
            *pos += 1;
            Some(*v)
        }
        Tok::LParen => {
            *pos += 1;
            let v = parse_expr_add(toks, pos)?;
            if toks.get(*pos) == Some(&Tok::RParen) {
                *pos += 1;
                Some(v)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 把求值结果规整为干净 JSON 数值（整数优先）。
fn num_to_value(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// 解析命令参数中的值字面量：
/// 引号字符串原样保留；true/false/null；JSON 对象/数组；
/// 纯数字四则表达式求值；其余 → Err（诊断跳过，绝不 eval）。
pub fn parse_value_literal(raw: &str) -> Result<Value, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty value".into());
    }
    let first = s.chars().next().unwrap();
    let last = s.chars().last().unwrap();
    if (first == '\'' || first == '"') && last == first && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut escaped = false;
        for c in inner.chars() {
            if escaped {
                match c {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    other => out.push(other),
                }
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else {
                out.push(c);
            }
        }
        if escaped {
            out.push('\\');
        }
        return Ok(Value::String(out));
    }
    match s {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "null" => return Ok(Value::Null),
        _ => {}
    }
    if first == '{' || first == '[' {
        return serde_json::from_str(s).map_err(|e| format!("invalid JSON literal: {e}"));
    }
    // 数字或四则表达式（无引号才求值）
    if let Some(v) = eval_number_expr(s) {
        return Ok(num_to_value(v));
    }
    Err(format!("unsupported value expression: `{}`", clip(s, 60)))
}

// ---------------------------------------------------------------------------
// 命令解析
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum MvuCommand {
    Set { path: String, value: Value },
    Add { path: String, delta: f64 },
    Insert { path: String, index: Option<usize>, value: Value },
    Remove { path: String },
}

/// 解析一行命令，如 `_.set('角色.好感', 10)`。
/// 未知命令 / 无法解析 → Err(诊断)。
pub fn parse_command_line(line: &str) -> Result<MvuCommand, MvuDiag> {
    let t = line.trim();
    let raw = t;
    let bad = |code: &str, msg: String| MvuDiag::skip(code, msg, raw);

    let body = t.strip_prefix("_.").ok_or_else(|| {
        bad("unknown_command", format!("not a MVU command: `{}`", clip(t, 60)))
    })?;
    let open = body.find('(').ok_or_else(|| {
        bad("unknown_command", format!("no argument list: `{}`", clip(t, 60)))
    })?;
    let close = body.rfind(')').ok_or_else(|| {
        bad("bad_args", format!("missing closing paren: `{}`", clip(t, 60)))
    })?;
    if close < open {
        return Err(bad("bad_args", format!("paren order: `{}`", clip(t, 60))));
    }
    let name = body[..open].trim();
    let args_raw = &body[open + 1..close];

    // 拆参（顶层逗号，忽略引号/括号/嵌套 JSON 内的逗号）
    let args = split_top_level_args(args_raw);

    let cmd = match name {
        "set" | "assign" => {
            if args.len() != 2 {
                return Err(bad("bad_args", format!("{name} expects 2 args")));
            }
            let path = parse_string_arg(&args[0]).ok_or_else(|| {
                bad("bad_args", format!("{name}: path must be a quoted string"))
            })?;
            let value = parse_value_literal(&args[1]).map_err(|e| bad("unsupported_expr", e))?;
            MvuCommand::Set { path, value }
        }
        "add" => {
            if args.len() != 2 {
                return Err(bad("bad_args", "add expects 2 args".to_string()));
            }
            let path = parse_string_arg(&args[0]).ok_or_else(|| {
                bad("bad_args", "add: path must be quoted string".to_string())
            })?;
            let v = parse_value_literal(&args[1]).map_err(|e| bad("unsupported_expr", e))?;
            let delta = match &v {
                Value::Number(n) => n.as_f64().unwrap_or(0.0),
                _ => {
                    return Err(bad(
                        "bad_args",
                        "add: delta must be numeric (MVU math.* not supported)".to_string(),
                    ))
                }
            };
            MvuCommand::Add { path, delta }
        }
        "insert" => {
            // 两种形态：_.insert(path, index, value) / _.insert(path, value)（追加）
            match args.len() {
                3 => {
                    let path = parse_string_arg(&args[0]).ok_or_else(|| {
                        bad("bad_args", "insert: path must be quoted string".to_string())
                    })?;
                    let idx_raw = args[1].trim();
                    let index: usize = idx_raw
                        .parse()
                        .map_err(|_| bad("bad_args", format!("insert: bad index `{idx_raw}`")))?;
                    let value = parse_value_literal(&args[2])
                        .map_err(|e| bad("unsupported_expr", e))?;
                    MvuCommand::Insert { path, index: Some(index), value }
                }
                2 => {
                    let path = parse_string_arg(&args[0]).ok_or_else(|| {
                        bad("bad_args", "insert: path must be quoted string".to_string())
                    })?;
                    let value = parse_value_literal(&args[1])
                        .map_err(|e| bad("unsupported_expr", e))?;
                    MvuCommand::Insert { path, index: None, value }
                }
                _ => return Err(bad("bad_args", "insert expects 2 or 3 args".to_string())),
            }
        }
        "remove" | "unset" | "delete" => {
            if args.len() != 1 {
                return Err(bad("bad_args", format!("{name} expects 1 arg")));
            }
            let path = parse_string_arg(&args[0])
                .ok_or_else(|| bad("bad_args", format!("{name}: path must be quoted string")))?;
            MvuCommand::Remove { path }
        }
        "move" | "get" => {
            return Err(bad(
                "unsupported_command",
                format!("`{name}` 不在本兼容子集内（对齐 AIcove：move/get 不支持）"),
            ))
        }
        other => {
            return Err(bad(
                "unknown_command",
                format!("unknown MVU command `{other}`"),
            ))
        }
    };
    Ok(cmd)
}

fn parse_string_arg(arg: &str) -> Option<String> {
    let s = arg.trim();
    let first = s.chars().next()?;
    let last = s.chars().last()?;
    if (first == '\'' || first == '"') && last == first && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut esc = false;
        for c in inner.chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else {
                out.push(c);
            }
        }
        if esc {
            out.push('\\');
        }
        Some(out)
    } else {
        None
    }
}

/// 顶层逗号拆参：跳过引号串、()、[]、{} 内部的逗号。
fn split_top_level_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in s.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                args.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        args.push(cur);
    }
    args.into_iter().map(|a| a.trim().to_string()).collect()
}

// ---------------------------------------------------------------------------
// <UpdateVariable> 块提取
// ---------------------------------------------------------------------------

/// 从模型输出中提取所有 `<UpdateVariable>` 块体（大小写不敏感）。
/// 跳过 `<think>…</think>` 内的块（对齐 AIcove：思考区不执行）。
pub fn extract_update_blocks(text: &str) -> Vec<String> {
    let scrubbed = remove_tag_spans(text, "think");
    find_tag_blocks(&scrubbed, "UpdateVariable")
}

/// 从展示文本中移除所有 UpdateVariable 块（历史副本清理用）。
pub fn strip_update_blocks(text: &str) -> String {
    remove_tag_spans(text, "UpdateVariable").trim().to_string()
}

// ---------------------------------------------------------------------------
// 执行
// ---------------------------------------------------------------------------

/// 应用单条命令；Err = 诊断（调用方跳过并记录）。
pub fn apply_command(root: &mut Value, cmd: &MvuCommand) -> Result<(), String> {
    match cmd {
        MvuCommand::Set { path, value } => set_path(root, path, value.clone()),
        MvuCommand::Add { path, delta } => {
            let cur = get_path(root, path).ok_or_else(|| format!("add: missing path `{path}`"))?;
            let n = cur
                .as_f64()
                .ok_or_else(|| format!("add: `{path}` is not numeric"))?;
            set_path(root, path, num_to_value(n + delta))
        }
        MvuCommand::Insert { path, index, value } => {
            let segs = parse_path(path)?;
            match segs.last() {
                Some(PathSeg::Index(i)) => {
                    // _.insert('arr[2]', value) 形态：arr 为目标数组
                    let parent_path = join_prefix(&segs[..segs.len() - 1]);
                    let parent =
                        get_path(root, &parent_path).ok_or_else(|| format!("insert: missing array `{parent_path}`"))?;
                    if !parent.is_array() {
                        return Err(format!("insert: `{parent_path}` is not an array"));
                    }
                    let idx = index.unwrap_or(*i);
                    let mut arr = parent.clone();
                    let a = arr.as_array_mut().unwrap();
                    let at = idx.min(a.len());
                    a.insert(at, value.clone());
                    set_path(root, &parent_path, arr)
                }
                _ => {
                    // _.insert('key', [index,] value) 形态：key 指向（或即将成为）数组
                    match get_path(root, path) {
                        Some(Value::Array(a)) => {
                            let mut arr = a.clone();
                            match index {
                                Some(i) => {
                                    let at = (*i).min(arr.len());
                                    arr.insert(at, value.clone());
                                }
                                None => arr.push(value.clone()),
                            }
                            set_path(root, path, Value::Array(arr))
                        }
                        Some(Value::Null) | None => {
                            let arr = match index {
                                Some(i) => {
                                    let mut a: Vec<Value> = Vec::new();
                                    a.resize_with(*i + 1, || Value::Null);
                                    a[*i] = value.clone();
                                    a
                                }
                                None => vec![value.clone()],
                            };
                            set_path(root, path, Value::Array(arr))
                        }
                        Some(_) => Err(format!("insert: `{path}` is not an array")),
                    }
                }
            }
        }
        MvuCommand::Remove { path } => remove_path(root, path).map(|_| ()),
    }
}

fn join_prefix(segs: &[PathSeg]) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg {
            PathSeg::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
            PathSeg::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// 解析并应用一个 `<UpdateVariable>` 块体（多行命令）。
/// 逐条跳过失败项并返回诊断（对齐 AIcove：不整块回滚）。
pub fn apply_update_block(root: &mut Value, block: &str) -> Vec<MvuDiag> {
    let mut diags = Vec::new();
    for line in block.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match parse_command_line(t) {
            Ok(cmd) => {
                if let Err(e) = apply_command(root, &cmd) {
                    diags.push(MvuDiag::skip("apply_failed", e, t));
                }
            }
            Err(d) => diags.push(d),
        }
    }
    diags
}

/// 解析并应用 `<JSONPatch>[…]</JSONPatch>`（replace/add/remove/delta 子集）。
pub fn apply_json_patch_block(root: &mut Value, block: &str) -> Vec<MvuDiag> {
    let mut diags = Vec::new();
    let ops: Vec<Value> = match serde_json::from_str(block.trim()) {
        Ok(v) => v,
        Err(e) => {
            return vec![MvuDiag::skip(
                "bad_json_patch",
                format!("invalid JSON: {e}"),
                block,
            )];
        }
    };
    for op in ops {
        let kind = op.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let path = op.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            diags.push(MvuDiag::skip("bad_json_patch", "missing path", &op.to_string()));
            continue;
        }
        let r = match kind {
            "replace" | "add" => {
                let value = op.get("value").cloned().unwrap_or(Value::Null);
                set_path(root, path, value)
            }
            "remove" => remove_path(root, path).map(|_| ()),
            "delta" => match op.get("value").and_then(|v| v.as_f64()) {
                None => Err("delta: value must be numeric".to_string()),
                Some(d) => match get_path(root, path).and_then(|v| v.as_f64()) {
                    Some(n) => set_path(root, path, num_to_value(n + d)),
                    None => Err("delta: target missing or not numeric".into()),
                },
            },
            other => Err(format!("unsupported JSONPatch op `{other}`")),
        };
        if let Err(e) = r {
            diags.push(MvuDiag::skip("apply_failed", e, &op.to_string()));
        }
    }
    diags
}

// ---------------------------------------------------------------------------
// 初始化：[InitVar] 世界书条目 + 开场白 <initvar> 覆盖
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct MvuInitSource {
    /// 条目名（含 `[InitVar]` 标记的那批，包括已禁用条目——对齐 AIcove）。
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct MvuInitResult {
    pub stat_data: Value,
    /// `$meta.strictSet` 是否开启。
    pub strict_set: bool,
    pub diagnostics: Vec<MvuDiag>,
    /// 初始化整体失败（有源但全部不可用时为 true）。
    pub init_failed: bool,
}

fn is_initvar_entry(name: &str) -> bool {
    // 零分配 ASCII 大小写不敏感匹配（find_tag_blocks 同款），热路径友好
    find_ci(name, "[initvar]").is_some()
}

/// 从世界书条目构建 stat_data 基线：识别名字含 `[InitVar]` 的条目，
/// 逐条解析（JSON 优先）并深合并；`$meta`/`$__META_EXTENSIBLE__$` 清理。
pub fn init_from_entries(entries: &[MvuInitSource]) -> MvuInitResult {
    let mut stat = Value::Object(Map::new());
    let mut strict_set = false;
    let mut diags = Vec::new();
    let mut any_source = false;
    let mut any_ok = false;

    for e in entries {
        if !is_initvar_entry(&e.name) {
            continue;
        }
        let body = extract_initvar_payload(&e.content).unwrap_or_else(|| e.content.trim().to_string());
        if body.is_empty() {
            continue;
        }
        any_source = true;
        // 内容里出现 EJS 模板 → 整条跳过（对齐 AIcove）
        if body.contains("<%") {
            diags.push(MvuDiag::init_failed(
                "ejs_skipped",
                "entry contains EJS template; skipped",
                &e.name,
            ));
            continue;
        }
        match parse_loose_json(&body) {
            Ok(v) => {
                if let Some(meta) = v.get("$meta") {
                    strict_set = strict_set
                        || meta
                            .get("strictSet")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false);
                }
                let cleaned = strip_meta_keys(&v);
                deep_merge(&mut stat, &cleaned);
                any_ok = true;
            }
            Err(err) => {
                diags.push(MvuDiag::init_failed(
                    "bad_init_json",
                    format!("InitVar entry `{}` unparseable: {err}", e.name),
                    &body,
                ));
            }
        }
    }

    let init_failed = any_source && !any_ok;
    MvuInitResult {
        stat_data: stat,
        strict_set,
        diagnostics: diags,
        init_failed,
    }
}

/// 开场白 `<initvar>` 块覆盖世界书基线（对齐 AIcove：开场白优先）。
pub fn apply_first_message_initvar(base: &mut Value, first_message: &str) -> Vec<MvuDiag> {
    let mut diags = Vec::new();
    for block in find_tag_blocks(first_message, "initvar") {
        match parse_loose_json(&block) {
            Ok(v) => deep_merge(base, &strip_meta_keys(&v)),
            Err(e) => diags.push(MvuDiag::skip(
                "bad_initvar_block",
                format!("first-message initvar unparseable: {e}"),
                &block,
            )),
        }
    }
    diags
}

/// `<initvar>`/世界书条目内容可能是 ```json 代码围栏或裸 JSON。
fn extract_initvar_payload(content: &str) -> Option<String> {
    let t = content.trim();
    if t.starts_with('{') || t.starts_with('[') {
        return Some(t.to_string());
    }
    // ```json ... ``` 围栏
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.trim_start_matches(['j', 's', 'o', 'n']).trim_start();
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }
    // 首个 { 到最后一个 } 的切片
    match (t.find('{'), t.rfind('}')) {
        (Some(a), Some(b)) if b > a => Some(t[a..=b].to_string()),
        _ => None,
    }
}

/// 宽松 JSON v1：标准 JSON 失败后，剥 // 行注释 + 去尾逗号重试
/// （单引号键值等 JSON5 特性 v1 不支持，诊断跳过）。
fn parse_loose_json(s: &str) -> Result<Value, String> {
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Ok(v);
    }
    let normalized = normalize_loose_json(s);
    serde_json::from_str::<Value>(&normalized).map_err(|e| {
        format!(
            "not valid JSON ({e}); JSON5 单引号/模板语法 v1 不支持"
        )
    })
}

/// 字符串感知地剥 // 行注释与 ,} / ,] 形式的尾逗号。
fn normalize_loose_json(s: &str) -> String {
    // 第一步：剥行注释（保持字符串内容不动）
    let mut no_comment = String::with_capacity(s.len());
    let mut in_str: Option<char> = None;
    let mut escaped = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            no_comment.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                in_str = Some(c);
                no_comment.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        no_comment.push('\n');
                        break;
                    }
                }
            }
            _ => no_comment.push(c),
        }
    }
    // 第二步：去 `}` / `]` 前的尾逗号
    let mut out = String::with_capacity(no_comment.len());
    let mut in_str: Option<char> = None;
    let mut escaped = false;
    for c in no_comment.chars() {
        if let Some(q) = in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                in_str = Some(c);
                out.push(c);
            }
            '}' | ']' => {
                let trimmed = out.trim_end();
                let mut cut = out.len();
                if trimmed.ends_with(',') {
                    cut = trimmed.len() - 1;
                }
                out.truncate(cut);
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// 递归剥离 `$meta` / `$__META_EXTENSIBLE__$` 等以 `$` 开头的键。
fn strip_meta_keys(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, val) in m {
                if k.starts_with('$') {
                    continue;
                }
                out.insert(k.clone(), strip_meta_keys(val));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(strip_meta_keys).collect()),
        other => other.clone(),
    }
}

/// 深合并：object 递归，其余（含数组）整体覆盖。
pub fn deep_merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) if slot.is_object() && v.is_object() => deep_merge(slot, v),
                    Some(slot) => *slot = v.clone(),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

// ---------------------------------------------------------------------------
// 注入宏：{{get_message_variable::path}} → JSON（剥离 $ 键）
// ---------------------------------------------------------------------------

/// 供提示词装配层调用：取路径值并序列化为 JSON 字符串；路径为空 → 整个 stat_data。
/// 找不到路径 → `null` 字符串（"每轮快照固定"的语义由调用方保证：传入当轮快照）。
pub fn format_variable_json(stat_data: &Value, path: &str) -> String {
    let target = if path.trim().is_empty() {
        Some(stat_data)
    } else {
        get_path(stat_data, path)
    };
    match target {
        Some(v) => serde_json::to_string(&strip_meta_keys(v)).unwrap_or_else(|_| "null".into()),
        None => "null".into(),
    }
}

// ---------------------------------------------------------------------------
// [P3 2026-10-11] 会话快照重放 + 提示词宏展开
// ---------------------------------------------------------------------------

/// lore 条目名兜底键序：title → name → comment（quill pack.lore_entries 用 title，
/// ST character_book 条目多用 name/comment）。
fn lore_entry_name(e: &Value) -> &str {
    e.get("title")
        .or_else(|| e.get("name"))
        .or_else(|| e.get("comment"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn lore_entry_content(e: &Value) -> &str {
    e.get("content").and_then(|v| v.as_str()).unwrap_or("")
}

/// 从 pack.lore_entries 收集 `[InitVar]` 源（含已禁用条目，对齐 AIcove）。
pub fn init_sources_from_lore_entries(entries: &[Value]) -> Vec<MvuInitSource> {
    entries
        .iter()
        .filter(|e| is_initvar_entry(lore_entry_name(e)))
        .map(|e| MvuInitSource {
            name: lore_entry_name(e).to_string(),
            content: lore_entry_content(e).to_string(),
        })
        .collect()
}

/// 会话是否"MVU 活跃"：任一消息带更新块，或存在非空 `[InitVar]` 条目。
/// 非活跃会话走宏展开直通路径（零拷贝、零解析），保住提示词装配热路径。
pub fn is_mvu_active(lore_entries: &[Value], message_blocks: &[String]) -> bool {
    if message_blocks.iter().any(|b| !b.trim().is_empty()) {
        return true;
    }
    init_sources_from_lore_entries(lore_entries)
        .iter()
        .any(|s| !s.content.trim().is_empty())
}

/// 基线（`[InitVar]` 条目）+ 沿落盘消息链顺序重放更新块 → 当前 stat_data。
/// 每轮请求构建一次、请求内固定（对齐 AIcove"每轮一次快照"语义；ADR-0004
/// 真相源：快照永远可由 `[InitVar]` + 落盘消息链重建）。
/// 块形态：P2 落盘的是已剥标签的裸块体（`extract_update_blocks` 产物）；
/// 对仍带 `<UpdateVariable>` 包裹的输入同样容忍（防御）。
/// 已知 v1 缺口：开场白 `<initvar>` 覆盖、prev_anchor/raw_hash 增量缓存未做。
pub fn snapshot_stat_data(lore_entries: &[Value], message_blocks: &[String]) -> MvuInitResult {
    let sources = init_sources_from_lore_entries(lore_entries);
    let mut r = init_from_entries(&sources);
    for b in message_blocks {
        // 重放沿用逐条跳过+诊断语义；此处诊断不影响主链路（init_from_entries 的
        // 诊断已含在返回值里，调用方需要时可记日志）
        let bodies = extract_update_blocks(b);
        if bodies.is_empty() {
            let _ = apply_update_block(&mut r.stat_data, b);
        } else {
            for body in bodies {
                let _ = apply_update_block(&mut r.stat_data, &body);
            }
        }
    }
    r
}

/// 展开 prompt 中的 `{{get_message_variable::path}}`（ASCII 大小写不敏感）。
/// `{{format_message_variable::path}}` v1 同样输出 JSON（YAML 输出为已知缺口）。
/// 找不到路径 → `null` 字符串；非 MVU 宏的花括号原样保留。
pub fn expand_variable_macros(prompt: &str, stat_data: &Value) -> String {
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    loop {
        let Some(start) = rest.find("{{") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(rel) = after.find("}}") else {
            // 未闭合：原样保留剩余
            out.push_str(&rest[start..]);
            break;
        };
        let inner = after[..rel].trim();
        let mut replaced = false;
        for p in ["get_message_variable::", "format_message_variable::"] {
            if inner.len() > p.len() && inner[..p.len()].eq_ignore_ascii_case(p) {
                out.push_str(&format_variable_json(stat_data, &inner[p.len()..]));
                replaced = true;
                break;
            }
        }
        if replaced {
            rest = &after[rel + 2..];
        } else {
            // 非 MVU 宏：原样吐回 "{{"，从 "}}" 之外继续扫描（防吞嵌套）
            out.push_str("{{");
            rest = after;
        }
    }
    out
}

/// P3 一步入口：活性检测 → 基线+重放 → 宏展开。
/// 非 MVU 会话原样返回调用方字符串（零解析、零额外分配）。
pub fn expand_mvu_macros(prompt: &str, message_blocks: &[String], lore_entries: &[Value]) -> String {
    if !is_mvu_active(lore_entries, message_blocks) {
        return prompt.to_string();
    }
    let snap = snapshot_stat_data(lore_entries, message_blocks);
    expand_variable_macros(prompt, &snap.stat_data)
}

// ---------------------------------------------------------------------------
// [P1 2026-10-11] 卡片级 MVU 预检（只读）：供导入层出诊断报告，不改导入行为
// ---------------------------------------------------------------------------

/// 预检报告（camelCase，直接进导入报告 JSON）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MvuPrecheckReport {
    /// 卡片是否使用 MVU（有 `[InitVar]` 条目，或开场白含 `<initvar>`/`<UpdateVariable>`）。
    pub has_mvu: bool,
    pub initvar_entry_count: usize,
    /// 基线整体失败（有源但全部不可用）——导入可继续，但 MVU 状态不可信。
    pub init_failed: bool,
    pub strict_set: bool,
    pub diagnostic_count: usize,
    /// 前 5 条诊断摘要（截断展示，全量走导入诊断通道）。
    pub diagnostics: Vec<String>,
}

impl Default for MvuPrecheckReport {
    fn default() -> Self {
        Self {
            has_mvu: false,
            initvar_entry_count: 0,
            init_failed: false,
            strict_set: false,
            diagnostic_count: 0,
            diagnostics: Vec::new(),
        }
    }
}

/// 对解析后的角色卡 JSON（v1 平卡 / v2/v3 带 data 信封）做 MVU 预检。
/// 只读、无副作用；调用方自行决定是否把报告挂进导入产物。
pub fn card_mvu_precheck(card: &Value) -> MvuPrecheckReport {
    let mut report = MvuPrecheckReport::default();
    // v2/v3 信封取 data，v1 平卡取根
    let body = card.get("data").unwrap_or(card);

    // 1) 世界书条目（character_book.entries；名字取 name/comment 兜底）
    let mut sources: Vec<MvuInitSource> = Vec::new();
    if let Some(entries) = body
        .get("character_book")
        .and_then(|b| b.get("entries"))
        .and_then(|e| e.as_array())
    {
        for e in entries {
            let name = e
                .get("name")
                .or_else(|| e.get("comment"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = e.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if is_initvar_entry(name) {
                report.initvar_entry_count += 1;
                sources.push(MvuInitSource {
                    name: name.to_string(),
                    content: content.to_string(),
                });
            }
        }
    }

    // 2) 开场白（first_mes + alternate_greetings）里的 MVU 标记
    let mut first_meses: Vec<&str> = Vec::new();
    if let Some(fm) = body.get("first_mes").and_then(|v| v.as_str()) {
        first_meses.push(fm);
    }
    if let Some(alts) = body.get("alternate_greetings").and_then(|v| v.as_array()) {
        for a in alts.iter().filter_map(|v| v.as_str()) {
            first_meses.push(a);
        }
    }
    let mut greeting_has_tags = false;
    for fm in &first_meses {
        if !find_tag_blocks(fm, "initvar").is_empty() || !extract_update_blocks(fm).is_empty() {
            greeting_has_tags = true;
            break;
        }
    }

    report.has_mvu = report.initvar_entry_count > 0 || greeting_has_tags;
    if !report.has_mvu {
        return report;
    }

    // 3) 空跑基线构建，收集诊断
    let r = init_from_entries(&sources);
    report.init_failed = r.init_failed;
    report.strict_set = r.strict_set;
    report.diagnostic_count = r.diagnostics.len();
    report.diagnostics = r
        .diagnostics
        .iter()
        .take(5)
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    report
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deep_merge_overrides_leaf_and_replaces_array() {
        let mut base = json!({"a": {"x": 1, "y": 2}, "list": [1, 2]});
        deep_merge(&mut base, &json!({"a": {"y": 9}, "list": [7]}));
        assert_eq!(base["a"]["x"], json!(1));
        assert_eq!(base["a"]["y"], json!(9));
        assert_eq!(base["list"], json!([7]));
    }

    #[test]
    fn init_reads_initvar_entries_and_strips_meta() {
        let entries = vec![
            MvuInitSource {
                name: "普通条目".into(),
                content: "不该被解析".into(),
            },
            MvuInitSource {
                name: "[InitVar]角色状态".into(),
                content: r#"{"角色": {"好感度": 10}, "$meta": {"strictSet": true}}"#.into(),
            },
        ];
        let r = init_from_entries(&entries);
        assert!(!r.init_failed);
        assert!(r.strict_set);
        assert_eq!(r.stat_data["角色"]["好感度"], json!(10));
        assert!(r.stat_data.get("$meta").is_none());
    }

    #[test]
    fn init_bad_json_diags_init_failed() {
        let entries = vec![MvuInitSource {
            name: "[InitVar]坏条目".into(),
            content: "{不是json".into(),
        }];
        let r = init_from_entries(&entries);
        assert!(r.init_failed);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].code, "bad_init_json");
    }

    #[test]
    fn first_message_initvar_overrides_worldbook() {
        let mut base = json!({"好感度": 10, "地点": "教室"});
        let diags =
            apply_first_message_initvar(&mut base, "开场白正文\n<initvar>{\"好感度\": 50}</initvar>\n结尾");
        assert!(diags.is_empty());
        assert_eq!(base["好感度"], json!(50));
        assert_eq!(base["地点"], json!("教室"));
    }

    #[test]
    fn extracts_update_blocks_case_insensitive_and_skips_think() {
        let text = "<updatevariable>\n_.set('a', 1)\n</UpdateVariable>中段<think>\n<UpdateVariable>_.set('hack', 1)</UpdateVariable>\n</think><UPDATEVARIABLE>_.set('b', 2)</UPDATEVARIABLE>";
        let blocks = extract_update_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("_.set('a', 1)"));
        assert!(blocks[1].contains("_.set('b', 2)"));
        let stripped = strip_update_blocks(text);
        assert!(!stripped.contains("_.set"));
        assert!(stripped.contains("中段"));
    }

    #[test]
    fn unclosed_update_block_is_dropped() {
        let text = "正文<UpdateVariable>_.set('a', 1)";
        assert!(extract_update_blocks(&text).is_empty());
        let stripped = strip_update_blocks(text);
        assert!(!stripped.contains("_.set"));
    }

    #[test]
    fn set_creates_nested_path_and_types() {
        let mut v = json!({});
        let cmds = [
            "_.set('角色.林诗语.好感度', 10)",
            "_.set('角色.林诗语.备注', 'it\\'s ok')",
            "_.set('flags.on', true)",
            "_.set('obj', {\"x\": 1})",
        ];
        for c in cmds {
            apply_update_block(&mut v, c);
        }
        assert_eq!(v["角色"]["林诗语"]["好感度"], json!(10));
        assert_eq!(v["角色"]["林诗语"]["备注"], json!("it's ok"));
        assert_eq!(v["flags"]["on"], json!(true));
        assert_eq!(v["obj"]["x"], json!(1));
    }

    #[test]
    fn unquoted_arithmetic_is_evaluated_but_js_is_not() {
        let mut v = json!({});
        apply_update_block(&mut v, "_.set('n', 10+5*2)");
        assert_eq!(v["n"], json!(20));
        apply_update_block(&mut v, "_.set('p', (1+2)*3)");
        assert_eq!(v["p"], json!(9));
        // 含字母的表达式 → 不求值、诊断跳过
        let diags = apply_update_block(&mut v, "_.set('bad', process.exit(1))");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "unsupported_expr");
        assert!(v.get("bad").is_none());
    }

    #[test]
    fn add_requires_existing_number() {
        let mut v = json!({"hp": 100});
        apply_update_block(&mut v, "_.add('hp', -30)");
        assert_eq!(v["hp"], json!(70));
        let diags = apply_update_block(&mut v, "_.add('missing', 5)");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "apply_failed");
    }

    #[test]
    fn insert_append_and_indexed() {
        let mut v = json!({"items": ["a"]});
        apply_update_block(&mut v, "_.insert('items', 'b')");
        apply_update_block(&mut v, "_.insert('items', 0, 'z')");
        assert_eq!(v["items"], json!(["z", "a", "b"]));
        // 新建数组
        apply_update_block(&mut v, "_.insert('tags', '新')");
        assert_eq!(v["tags"], json!(["新"]));
        // arr[i] 形态的插入点
        let mut w = json!({"arr": [1, 3]});
        apply_update_block(&mut w, "_.insert('arr[1]', 2)");
        assert_eq!(w["arr"], json!([1, 2, 3]));
    }

    #[test]
    fn remove_and_aliases() {
        let mut v = json!({"a": 1, "arr": [10, 20, 30]});
        apply_update_block(&mut v, "_.remove('a')");
        apply_update_block(&mut v, "_.unset('arr[1]')");
        assert!(v.get("a").is_none());
        assert_eq!(v["arr"], json!([10, 30]));
        // 缺键 → 诊断（不假装成功）
        let diags = apply_update_block(&mut v, "_.delete('不存在的键')");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "apply_failed");
    }

    #[test]
    fn assign_alias_and_unknown_commands() {
        let mut v = json!({});
        apply_update_block(&mut v, "_.assign('x', 'y')");
        assert_eq!(v["x"], json!("y"));
        let d1 = apply_update_block(&mut v, "_.move('a', 'b')");
        let d2 = apply_update_block(&mut v, "_.hack('a')");
        assert_eq!(d1[0].code, "unsupported_command");
        assert_eq!(d2[0].code, "unknown_command");
    }

    #[test]
    fn json_patch_block_supported_subset() {
        let mut v = json!({"hp": 100, "mood": "平静"});
        let diags = apply_json_patch_block(
            &mut v,
            r#"[{"op":"replace","path":"mood","value":"开心"},{"op":"delta","path":"hp","value":-20},{"op":"add","path":"新键","value":1}]"#,
        );
        assert!(diags.is_empty());
        assert_eq!(v["mood"], json!("开心"));
        assert_eq!(v["hp"], json!(80));
        assert_eq!(v["新键"], json!(1));
    }

    #[test]
    fn replay_chain_is_deterministic() {
        let base = json!({"gold": 0, "log": []});
        let b1 = "_.set('gold', 10+1)\n_.insert('log', '拾取金币')";
        let b2 = "_.add('gold', -3)\n_.insert('log', '买面包')";
        let mut a = base.clone();
        apply_update_block(&mut a, b1);
        apply_update_block(&mut a, b2);
        let mut b = base.clone();
        apply_update_block(&mut b, b1);
        apply_update_block(&mut b, b2);
        assert_eq!(a, b);
        assert_eq!(a["gold"], json!(8));
        assert_eq!(a["log"], json!(["拾取金币", "买面包"]));
    }

    #[test]
    fn format_variable_json_strips_meta_and_supports_paths() {
        let stat = json!({"$meta": {"x": 1}, "角色": {"好感度": 42}});
        assert_eq!(format_variable_json(&stat, ""), r#"{"角色":{"好感度":42}}"#);
        assert_eq!(format_variable_json(&stat, "角色.好感度"), "42");
        assert_eq!(format_variable_json(&stat, "不存在"), "null");
    }

    #[test]
    fn path_tolerates_stat_data_prefix_and_dollar() {
        let root = json!({"arr": [{"n": 1}]});
        // stat_data / $ 是根的别名
        assert_eq!(get_path(&root, "stat_data.arr[0].n"), Some(&json!(1)));
        assert_eq!(get_path(&root, "$arr[0].n"), Some(&json!(1)));
        let mut w = json!({});
        apply_update_block(&mut w, "_.set('stat_data.k', 3)");
        assert_eq!(w["k"], json!(3));
    }

    #[test]
    fn ejs_entries_are_skipped_with_diag() {
        let entries = vec![MvuInitSource {
            name: "[InitVar]模板".into(),
            content: "```json\n{\"a\": \"<%= x %>\"}\n```".into(),
        }];
        let r = init_from_entries(&entries);
        assert!(r.init_failed);
        assert_eq!(r.diagnostics[0].code, "ejs_skipped");
    }

    #[test]
    fn fenced_json_and_line_comments_and_trailing_comma_parse() {
        let entries = vec![MvuInitSource {
            name: "[InitVar]围栏".into(),
            content: "说明文字\n```json\n{\n  // 注释\n  \"hp\": 5,\n}\n```\n".into(),
        }];
        let r = init_from_entries(&entries);
        assert!(!r.init_failed, "diags={:?}", r.diagnostics);
        assert_eq!(r.stat_data["hp"], json!(5));
    }

    #[test]
    fn chinese_text_roundtrip_through_extract_and_strip() {
        let text = "【正文】她推开门，冷风灌入。\n<UpdateVariable>\n_.set('场景.温度', '冷')\n_.add('角色.好感度', 1)\n</UpdateVariable>\n【完】";
        let blocks = extract_update_blocks(text);
        assert_eq!(blocks.len(), 1);
        let mut stat = json!({"场景": {"温度": "温"}, "角色": {"好感度": 7}});
        apply_update_block(&mut stat, &blocks[0]);
        assert_eq!(stat["场景"]["温度"], json!("冷"));
        assert_eq!(stat["角色"]["好感度"], json!(8));
        let shown = strip_update_blocks(text);
        assert!(shown.contains("她推开门"));
        assert!(shown.contains("【完】"));
        assert!(!shown.contains("UpdateVariable"));
    }

    // ── P3: 快照重放 + 宏展开 ──

    #[test]
    fn p3_snapshot_replays_blocks_in_message_order() {
        let lore = vec![
            json!({"title": "[InitVar]状态", "content": r#"{"好感度": 10, "地点": "教室"}"#}),
            json!({"title": "普通条目", "content": "无关内容"}),
        ];
        let b1 = "<UpdateVariable>_.set('好感度', 20)</UpdateVariable>".to_string();
        let b2 = "<UpdateVariable>_.add('好感度', -5)</UpdateVariable>".to_string();
        let snap = snapshot_stat_data(&lore, &[b1, b2]);
        assert_eq!(snap.stat_data["好感度"], json!(15));
        assert_eq!(snap.stat_data["地点"], json!("教室"));
    }

    #[test]
    fn p3_expand_macros_case_insensitive_and_passthrough() {
        let stat = json!({"角色": {"好感度": 42}});
        let p = "前文 {{get_message_variable::角色.好感度}} 中 {{GET_MESSAGE_VARIABLE::角色}} 尾";
        let out = expand_variable_macros(p, &stat);
        assert_eq!(out, "前文 42 中 {\"好感度\":42} 尾");

        // 非 MVU 宏原样保留
        let p2 = "{{char}} 与 {{get_message_variable::不存在}}";
        assert_eq!(expand_variable_macros(p2, &stat), "{{char}} 与 null");

        // 未闭合花括号原样保留
        let p3 = "abc {{ get_message_variable::x";
        assert_eq!(expand_variable_macros(p3, &stat), p3);
    }

    #[test]
    fn p3_expand_mvu_macros_gates_on_activity() {
        let lore = vec![json!({"title": "[InitVar]状态", "content": r#"{"hp": 5}"#})];
        let p = "{{get_message_variable::hp}}";
        // 有 InitVar → 展开
        assert_eq!(expand_mvu_macros(p, &[], &lore), "5");
        // 块重放覆盖 InitVar 基线
        let blocks = vec!["<UpdateVariable>_.set('hp', 99)</UpdateVariable>".to_string()];
        assert_eq!(expand_mvu_macros(p, &blocks, &lore), "99");

        // 无 InitVar 无块 → 原样返回（热路径直通）
        let empty_lore: Vec<Value> = vec![];
        assert_eq!(expand_mvu_macros(p, &[], &empty_lore), p);
        // 有块无 InitVar → 以空基线重放,命令自建路径
        let gold_blocks = vec!["<UpdateVariable>_.set('gold', 7)</UpdateVariable>".to_string()];
        assert_eq!(expand_mvu_macros("{{get_message_variable::gold}}", &gold_blocks, &empty_lore), "7");
    }

    #[test]
    fn precheck_detects_v3_card_with_initvar() {
        let card = json!({
            "spec": "chara_card_v3",
            "data": {
                "name": "测试卡",
                "first_mes": "开场<initvar>{\"好感度\": 0}</initvar>",
                "character_book": {
                    "entries": [
                        {"name": "普通设定", "content": "背景"},
                        {"comment": "[InitVar]状态", "content": "{\"体力\": 100}", "enabled": false}
                    ]
                }
            }
        });
        let r = card_mvu_precheck(&card);
        assert!(r.has_mvu);
        assert_eq!(r.initvar_entry_count, 1);
        assert!(!r.init_failed);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn precheck_flat_card_without_mvu_is_clean_false() {
        let card = json!({"name": "旧卡", "description": "x", "first_mes": "你好"});
        let r = card_mvu_precheck(&card);
        assert!(!r.has_mvu);
        assert_eq!(r.initvar_entry_count, 0);
        assert_eq!(r.diagnostic_count, 0);
    }

    #[test]
    fn precheck_reports_init_failure_with_diagnostics() {
        let card = json!({
            "spec": "chara_card_v2",
            "data": {
                "first_mes": "hi",
                "character_book": {"entries": [
                    {"name": "[InitVar]坏", "content": "{oops"}
                ]}
            }
        });
        let r = card_mvu_precheck(&card);
        assert!(r.has_mvu);
        assert!(r.init_failed);
        assert!(r.diagnostic_count >= 1);
        assert!(!r.diagnostics.is_empty());
    }
}
