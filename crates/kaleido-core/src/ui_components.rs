//! # UI Components — 交互式组件目录与提示词注入
//!
//! 吞噬 DEEIX-Chat `application/uicomponent`（Go 原版仅作设计蓝本，实现自研）：
//! 模型在回复正文里写 `deeix-ui` 围栏 JSON，客户端渲染成可交互组件（图表/
//! 表格/面板…）。本模块负责后端一侧：
//!
//! - 内置组件规格（specs）：定位/适用/交互/入参/规则；
//! - 给模型的 `<catalog>` 目录（静态版与 live 可交互版两套 header）；
//! - 能力判定（panel/widget 勾选）与目录裁剪（去能力行、去被 panel 取代的
//!   calculator、上限 32 个）。
//!
//! 提示词文案是产品规格的一部分，按原语义保留（中文）；渲染实现在前端。
//!
//! 零依赖、纯函数，可脱离服务器单测。

/// 模型输出组件块时的 Markdown 围栏语言标识。
pub const FENCE_LANGUAGE: &str = "deeix-ui";
/// 内联交互图解（HTML 沙箱 iframe）的围栏语言标识。
pub const WIDGET_FENCE_LANGUAGE: &str = "deeix-widget";

/// 块格式：静态（component/id/props）。
pub const BLOCK_FORMAT_STATIC: u8 = 1;
/// 块格式：live（state/computed/children 与布局控件原语）。
pub const BLOCK_FORMAT_LIVE: u8 = 2;

/// 目录里组件数量上限。
const MAX_CATALOG_COMPONENTS: usize = 32;
/// 内置描述里「定位/适用/交互」的分隔（目录只取定位与适用）。
const DESCRIPTION_INTERACTIONS: &str = "。交互：";
/// 入参摘要里「入参/约定」的分隔（目录拆成 <props> 与 <rules>）。
const PROPS_RULES_SEPARATOR: &str = "。约定：";

/// 块格式能力（不是可写进围栏的组件，只决定目录是否说明 panel/widget）。
pub const CAPABILITY_PANEL: &str = "panel";
pub const CAPABILITY_WIDGET: &str = "widget";

/// 一个内置组件的规格。
pub struct BuiltinSpec {
    /// 组件名（围栏里的 component 值）。
    pub name: &'static str,
    /// 版本（>1 时目录带 version 属性）。
    pub version: u32,
    /// 一句话定位。
    pub what: &'static str,
    /// 适用场景。
    pub when: &'static str,
    /// 用户可做的交互（给选择器看；目录提示只保留 what+when）。
    pub interactions: &'static str,
    /// 入参约定（可空；进 <rules>）。
    pub rules: &'static str,
    /// 入参摘要 `{...}`（进 <props>）。
    pub props: &'static str,
}

impl BuiltinSpec {
    /// 完整描述：「定位。适用：…。交互：…」。
    fn description(&self) -> String {
        format!("{}。适用：{}。交互：{}", self.what, self.when, self.interactions)
    }

    /// 入参摘要：「{...}」或「{...}。约定：…」。
    fn props_summary(&self) -> String {
        if self.rules.is_empty() {
            self.props.to_string()
        } else {
            format!("{}{}{}", self.props, PROPS_RULES_SEPARATOR, self.rules)
        }
    }
}

/// 内置组件规格（10 个内容组件 + panel/widget 两个能力行）。
pub fn builtin_specs() -> Vec<BuiltinSpec> {
    vec![
        BuiltinSpec {
            name: "card-grid", version: 1,
            what: "卡片网格",
            when: "新闻、搜索结果、候选方案等同构条目",
            interactions: "按 tag 筛选（条目带 tag 时自动出现）；带 url 的卡片可点击",
            rules: "",
            props: "{title, items: [{title, summary?, tag?, source?, url?}]}",
        },
        BuiltinSpec {
            name: "chart", version: 1,
            what: "数据图表",
            when: "折线、柱状、面积、横向柱状、散点、饼图、雷达、漏斗、进度环",
            interactions: "图例切换序列显隐；悬停读数",
            rules: "scatter 的 x 为数值；series.type 可单独改画柱/线/面积，series.axis: right 用第二 Y 轴；percent 需配合 stacked",
            props: "{title?, type: line|bar|area|horizontal-bar|scatter|pie|radar|funnel|radial, x?: (string|number)[], series: [{name, data: number[], type?: line|bar|area, axis?: left|right}], unit?, stacked?: boolean, percent?: boolean}",
        },
        BuiltinSpec {
            name: "function-plot", version: 1,
            what: "函数图像与方程曲线",
            when: "讲解数学函数、方程、几何曲线",
            interactions: "编辑、增删表达式实时重绘；滚轮缩放、拖动平移；显隐单条曲线",
            rules: "expr 可写 x 的表达式、y = …、x = …（关于 y）或含 x、y 的方程；乘法必须写 *；支持常见数学函数（sin cos tan sqrt abs exp ln log pow floor ceil round min max 等）与 pi、e",
            props: "{title?, functions: [{expr}], x?: [min, max], y?: [min, max]}",
        },
        BuiltinSpec {
            name: "stat-grid", version: 1,
            what: "关键指标卡片组",
            when: "几个并列的数字：KPI、涨跌幅、统计值",
            interactions: "带 history 的指标显示迷你走势线，悬停逐点读数",
            rules: "trend 决定颜色；history 从旧到新，periods 是各点标签",
            props: "{title?, periods?: string[], items: [{label, value, unit?, delta?, trend?: up|down|flat, note?, history?: number[]}]}",
        },
        BuiltinSpec {
            name: "gantt", version: 1,
            what: "甘特图",
            when: "项目排期、里程碑、任务依赖",
            interactions: "日/周/月刻度切换；点击任务高亮整条依赖链；分组折叠；标出今天与周末",
            rules: "日期 YYYY-MM-DD；end 与 days 二选一；depends 引用其它任务的 id（缺省为 name）",
            props: "{title?, tasks: [{id?, name, start, end?, days?: number, group?, progress?: 0-100, milestone?: boolean, depends?: string[]}]}",
        },
        BuiltinSpec {
            name: "data-table", version: 1,
            what: "数据表格",
            when: "十行以上的结构化数据",
            interactions: "排序、搜索、分页、导出 CSV",
            rules: "column.type 决定排序方式与对齐",
            props: "{title?, columns: [{key, label, type?: string|number|date|boolean}], rows: [{<key>: value}], pageSize?}",
        },
        BuiltinSpec {
            name: "calculator", version: 1,
            what: "交互式计算器",
            when: "贷款、利率、单位换算、预算等 what-if 分析",
            interactions: "拖动输入滑块，输出按公式实时重算",
            rules: "",
            props: "{title, inputs: [{label, value, unit?, min?, max?, step?}], formulas: [{label, expr, unit?}]}",
        },
        BuiltinSpec {
            name: "decision-tree", version: 1,
            what: "决策树 / 流程",
            when: "排障、选型、资格判断等一步步走的判断",
            interactions: "点节点展开分支；点击路径高亮",
            rules: "node.id 被 children[].when 引用；root 指向根节点 id",
            props: "{title?, root: id, nodes: [{id, question?, answer?, children?: [{when, to}], result?}]}",
        },
        BuiltinSpec {
            name: "diff", version: 1,
            what: "修改前后对照",
            when: "改写、润色、代码或文本的前后对比",
            interactions: "逐块展开/折叠；按行对齐查看",
            rules: "side: split 并排、line 逐行；语言标识进 code 的 language",
            props: "{title?, side?: split|line, before: {label?, text, language?}, after: {label?, text, language?}}",
        },
        BuiltinSpec {
            name: "quiz", version: 1,
            what: "测验 / 自测题",
            when: "自测、练习、讲解后检验理解",
            interactions: "选项作答即时判分与解析；多题进度",
            rules: "answer 为选项下标（0 起）；explanation 作答后显示",
            props: "{title?, questions: [{q, options: [string], answer: 0-N, explanation?}]}",
        },
        BuiltinSpec {
            name: CAPABILITY_PANEL, version: 1,
            what: "可交互面板（live 块格式能力）",
            when: "状态、公式、滑块与原语组成的交互面板",
            interactions: "state/computed 实时求值、控件绑定",
            rules: "",
            props: "",
        },
        BuiltinSpec {
            name: CAPABILITY_WIDGET, version: 1,
            what: "内联交互图解（widget 块格式能力）",
            when: "动画、三维示意等 panel 表达不了的内容",
            interactions: "沙箱 iframe 运行模型写的 HTML",
            rules: "",
            props: "",
        },
    ]
}

/// 运行时组件（内置或用户自定义）。
#[derive(Debug, Clone)]
pub struct Component {
    /// 内置 = builtin（前端注册表按 name 分发）；自定义 = sandbox。
    pub renderer_kind: String,
    pub name: String,
    pub version: u32,
    /// 完整描述（定位/适用/交互）。
    pub description: String,
    /// 入参摘要（props + 约定）。
    pub props_summary: String,
    /// JSON Schema 文本（自定义组件前端校验用；内置为空）。
    pub props_schema: String,
    /// sandbox 渲染时的 HTML 源码；内置为空。
    pub renderer_source: String,
    pub enabled: bool,
}

/// 全部内置组件（渲染 kind = builtin）。
pub fn builtin_components() -> Vec<Component> {
    builtin_specs()
        .into_iter()
        .map(|s| Component {
            renderer_kind: "builtin".into(),
            name: s.name.to_string(),
            version: s.version,
            description: s.description(),
            props_summary: s.props_summary(),
            props_schema: String::new(),
            renderer_source: String::new(),
            enabled: true,
        })
        .collect()
}

/// 本次勾选里有哪些块格式能力（只认内置行；自定义组件不能用这两个名字）。
pub fn capabilities(components: &[Component]) -> (bool, bool) {
    let (mut panel, mut widget) = (false, false);
    for c in components {
        if c.renderer_kind != "builtin" {
            continue;
        }
        match c.name.as_str() {
            CAPABILITY_PANEL => panel = c.enabled,
            CAPABILITY_WIDGET => widget = c.enabled,
            _ => {}
        }
    }
    (panel, widget)
}

/// 目录里的组件 = 去掉能力行；panel 勾选时再去掉被 panel 取代的 calculator。
fn catalog_only(components: &[Component], panel: bool) -> Vec<&Component> {
    components
        .iter()
        .filter(|c| c.enabled && c.name != CAPABILITY_PANEL && c.name != CAPABILITY_WIDGET)
        .filter(|c| !(panel && c.name == "calculator"))
        .collect()
}

/// 给模型的组件目录。空目录且无能力时返回空串（上层跳过注入）。
pub fn catalog_prompt(components: &[Component], block_format: u8) -> String {
    let live = block_format >= BLOCK_FORMAT_LIVE;
    let (raw_panel, raw_widget) = capabilities(components);
    let (panel, widget) = if live { (raw_panel, raw_widget) } else { (false, false) };
    let mut catalog = catalog_only(components, panel);
    if catalog.len() > MAX_CATALOG_COMPONENTS {
        catalog.truncate(MAX_CATALOG_COMPONENTS);
    }
    if catalog.is_empty() && !panel && !widget {
        return String::new();
    }
    let mut out = String::new();
    if live {
        out.push_str(&live_catalog_header(panel, widget));
        if catalog.is_empty() {
            return out;
        }
        out.push_str("\n<catalog>");
    } else {
        out.push_str(STATIC_CATALOG_HEADER);
    }
    for c in catalog {
        out.push_str(&format!("\n  <component name=\"{}\"", c.name));
        if c.version > 1 {
            out.push_str(&format!(" version=\"{}\"", c.version));
        }
        out.push('>');
        let use_text = c.description.split(DESCRIPTION_INTERACTIONS).next().unwrap_or(&c.description);
        out.push_str(&format!("\n    <use>{}</use>", use_text));
        let (props, rules) = match c.props_summary.split_once(PROPS_RULES_SEPARATOR) {
            Some((p, r)) => (p, Some(r)),
            None => (c.props_summary.as_str(), None),
        };
        out.push_str(&format!("\n    <props>{}</props>", props));
        if let Some(rules) = rules {
            out.push_str(&format!("\n    <rules>{}</rules>", rules));
        }
        out.push_str("\n  </component>");
    }
    out.push_str(STATIC_CATALOG_FOOTER);
    out
}

/// 静态格式目录层固定部分（结尾就是 `<catalog>` 起头）。
const STATIC_CATALOG_HEADER: &str = concat!(
    "<nature>交互式组件是回复正文里的一段 Markdown 代码块，由客户端渲染成界面。它是输出格式，不是工具：不要通过 function calling / tool call 调用组件，也不要等待它返回结果。</nature>\n",
    "<when>只在组件明显比纯 Markdown 更清楚、更好用时使用（需要筛选、排序、计算、绘图、排期、逐步引导、对比、判分）；否则用 Markdown。数量由内容决定，可以为零。</when>\n",
    "<syntax>语言标识为 `deeix-ui` 的代码块，内容是合法 JSON，只有 component、id（简短英文、对话内唯一）、props 三个字段；字符串内的英文双引号写成 \\\"，不要尾随逗号、注释或目录外的字段。代码块放在正文中它所属的位置，前后用自然语言承接，不要在组件外重复组件里的数据。\n",
    "```deeix-ui\n",
    "{\"component\": \"stat-grid\", \"id\": \"kpi-week\", \"props\": {\"title\": \"本周指标\", \"items\": [{\"label\": \"日活\", \"value\": \"128,430\", \"delta\": \"+4.2%\", \"trend\": \"up\"}]}}\n",
    "```\n",
    "<catalog>",
);

const STATIC_CATALOG_FOOTER: &str = "\n</catalog>";

/// live 可交互格式的目录层固定部分：intro + use/avoid/choose/rules +（panel?）+（widget?）。
fn live_catalog_header(panel: bool, widget: bool) -> String {
    let mut sections: Vec<String> = vec![
        LIVE_PROMPT_INTRO.to_string(),
        live_use_rule(panel, widget),
        LIVE_PROMPT_AVOID.to_string(),
        live_choose_rule(panel, widget),
        LIVE_PROMPT_RULES.to_string(),
    ];
    if panel {
        sections.push(LIVE_PROMPT_PANEL.to_string());
    }
    if widget {
        sections.push(LIVE_PROMPT_WIDGET.to_string());
    }
    sections.join("\n")
}

fn live_use_rule(panel: bool, widget: bool) -> String {
    let mut signals: Vec<&str> = vec![
        "同类条目超过五行（清单、候选、商品、城市、条目），或者需要筛选、排序、搜索",
        "用户在估算、比较方案或做 what-if，哪怕参数由你假设",
    ];
    if panel {
        signals.push("讲解公式、定律或有数量关系的概念（物理、化学、金融、统计、几何），用 panel 放公式、滑块和示意图，让用户拖动参数看结果怎么变，正文照常讲解");
    }
    if widget {
        signals.push("讲解随时间变化的过程或空间结构（动画、三维示意、流场、机构运动），用 <widget>");
    }
    signals.extend_from_slice(&[
        "给到或查到一组数，有趋势、分布、构成或多组对比",
        "过程要一步步走（排障、选型、资格判断、自测）",
        "有日期、时长或先后关系",
        "修改前后的对照",
    ]);
    format!(
        "  <use>满足任一项就出组件：{}。拿不准时看用户接下来会不会想改一个数、筛一遍或点一下；会，就出组件。引用工具结果时按原样放进组件，不要改写成文字。</use>",
        signals.join("；")
    )
}

fn live_choose_rule(panel: bool, widget: bool) -> String {
    let mut tiers: Vec<&str> = vec!["目录组件"];
    let mut rules: Vec<&str> = Vec::new();
    if panel {
        tiers.push("panel");
        rules.push("只有存在会改变结果的控件，或几个部件要共享状态时才用 panel，不要用 panel 包一个静态组件");
    }
    if widget {
        tiers.push("widget");
        rules.push("只有需要动画、三维或大量图形时才用 widget");
    }
    let mut choose = String::from("  <choose>一条回复一般一个块，最多三个，一个块对应一个需求。");
    if !rules.is_empty() {
        choose.push_str(&format!(
            "由轻到重：{}，能用轻的就不用重的。{}。",
            tiers.join(" → "),
            rules.join("；")
        ));
    }
    choose.push_str("</choose>");
    choose
}

const LIVE_PROMPT_INTRO: &str = concat!(
    "<nature>交互式组件是回复正文里的一段 Markdown 代码块，由客户端渲染成界面。它是输出格式，不是工具：不要通过 function calling / tool call 调用组件，也不要等待它返回结果。</nature>\n",
    "<when>\n",
    "  <principle>组件是让用户直接操作、直接读数的，不是装饰。命中 <use> 时默认就该出组件，只把数据讲成一段话不算完成；没有任何命中时才用最轻的表达：文字 → Markdown 列表或表格。结论和要点写在正文，明细与交互放进组件：去掉组件，回复仍然回答了问题。该用而没用，和不该用而用了，同样是错。</principle>",
);

const LIVE_PROMPT_AVOID: &str = "  <avoid>这些条件优先于 <use>，命中就退回文字或 Markdown：闲聊、观点、没有可调的量的定性概念、写作类请求（邮件、文案、代码）、一两句话就能答完；数据很少（三个以内且没有趋势的数字），直接写在正文或 Markdown 表格里；用户要可复制的纯文本、指定格式，或说过不要组件；没有可靠数据，不要为了画图编造数字，估算须在正文写明依据；组件只是把正文已有的内容再摆一遍；用组件代替回答，比如先发一张表单让用户选题材、篇幅、风格再动手，能直接回答就按合理的默认直接答，需要确认时用一句话问。</avoid>";

const LIVE_PROMPT_RULES: &str = "  <followup>用户在组件里的改动会在下一轮以 ui_state 提供。追问已有组件时，直接用其中的当前值作答，不要让用户复述，也不重复输出同一个块；用户要求改组件时，输出一个新块（新 id），初始值取用户当前的值。</followup>\n  <check>写完回看一遍：有明确的 <use> 信号却没有组件（用户明确要纯文本时除外），或者没有信号却放了组件，都算错，按本节重写。</check>\n</when>\n<syntax>语言标识为 `deeix-ui` 的代码块，内容是 JSON：component、id（简短英文，只用字母、数字、- 和 _，对话内唯一）与组件属性；属性可以平铺在顶层，也可以放进 props。字符串内的英文双引号写成 \\\"，不要尾随逗号或注释。代码块放在正文中它所属的位置，前后用自然语言承接，不要在组件外重复组件里的数据。\n```deeix-ui\n{\"component\": \"stat-grid\", \"id\": \"kpi-week\", \"title\": \"本周指标\", \"items\": [{\"label\": \"日活\", \"value\": \"128,430\", \"delta\": \"+4.2%\", \"trend\": \"up\"}]}\n```</syntax>";

const LIVE_PROMPT_PANEL: &str = concat!(
    "<live>\n",
    "  <state>state 是用户可改的初始值；computed 是由 state 推导的值，写不带花括号的公式，可以互相引用，不能循环。</state>\n",
    "  <children>children 从上到下排列，可以是下面的原语，也可以是 catalog 里的组件（嵌套时不写 id）。控件用 bind 指向 state 名；visible: \"{条件}\" 为假时隐藏该节点。</children>\n",
    "  <formula>属性值里的 {公式} 实时求值：整个值只有一个 {…} 时保留原类型（数字、列表），与文字混写时数字按千分位、最多两位小数显示；{{ 和 }} 表示字面花括号。公式只能用数字、'文字'、+ - * / % ^、比较、&& || !、a ? b : c、obj.key、list[i]、[a, b]，函数 abs sqrt cbrt floor ceil sign log ln log10 log2 exp sin cos tan asin acos atan sinh cosh tanh pow min max round(x, 位数?) fixed(x, 位数) clamp(x, 下限, 上限) range(起, 止, 步长?) sum avg len contains，常量 pi e；没有赋值、循环和自定义函数。range 包含两端，最多 1000 项。</formula>\n",
    "  <data>chart 的 series[].data 可以写成关于 x 的公式，按 x 列表逐点计算（i 是下标）；data-table 与 card-grid 可以加 where: \"{关于 row 的条件}\" 筛选行。</data>\n",
    "  <actions>button 三选一：ask: \"文字\" 显示为一个可编辑的追问框，预填这段文字（可以含 {公式}），用户可以改写后点发送，作为下一条消息发出，所以写成用户可能想问的完整问题；set: {state 名: 值} 修改状态；copy: \"文字\" 复制到剪贴板。</actions>\n",
    "  <primitives>\n",
    "    布局：panel {title?, description?}、row、stack、grid {columns?: 1-4}、card {title?}、tabs {items: [标签], bind?}（第 i 个子节点是第 i 个标签页）、collapse {title, open?}、divider\n",
    "    展示：text {text, tone?, size?: sm|base|lg}、stat {label, value, unit?, note?, precision?, highlight?, tone?}、badge {text, tone?}、progress {value, max?, label?}、callout {text, title?, tone?: info|positive|warning|negative}\n",
    "    讲解：formula {expr, name?, unit?, precision?, label?, with?}（expr 是不带花括号的公式，按当前值代入，显示「公式 = 代入 = 结果」；写成课本里的形式如 G * M / r^2，常数放进 computed，不要把算好的数折进公式；滑块用的单位与公式不同时，用 with: {\"M\": \"{M * 1e24}\"} 给出代入公式的值）、scene {width?, height?, shapes: [{type: rect|circle|line|arrow|text, 坐标, label?, tone?}]}（示意图，按实际像素画，宽 240–400 为宜，原点在左上；rect {x, y, w, h}、circle {cx, cy, r}、line/arrow {x1, y1, x2, y2}、text {x, y, text}，坐标与 label 都可以写 {公式}；只画说明关系必需的几个图形，离边缘与彼此留出 16 以上）\n",
    "    版式：panel 的 children 平铺写即可。有 scene 时，客户端自动把公式与控件放左边、示意图放右边、ask 按钮放下方，不用自己写 row\n",
    "    控件：slider {bind, label, min, max, step?, unit?}、select {bind, label, options}、segmented {bind, label?, options}、switch {bind, label}、input {bind, label, type?: text|number, placeholder?}、button {label?, ask | set | copy, variant?: default|outline}\n",
    "    tone 取 default|muted|positive|negative|warning|info；options 写成 [\"文字\"] 或 [{label, value}]\n",
    "  </primitives>\n",
    "  <example>\n",
    "```deeix-ui\n",
    include_str!("ui_live_example.json"),
    "\n```\n",
    "  </example>\n",
    "  <example>\n",
    "```deeix-ui\n",
    include_str!("ui_explorable_example.json"),
    "\n```\n",
    "  </example>\n",
    "</live>",
);

const LIVE_PROMPT_WIDGET: &str = concat!(
    "<widget>\n",
    "  <use>讲解需要动画、时间推进、三维或透视示意、粒子与流场、几何作图、机构或工程过程演示，而 panel 与 scene 表达不了时，在正文里写一个内联交互图解。用户要完整网页、小游戏或可下载的代码时仍用 html Artifact。</use>\n",
    "  <format>语言标识为 `deeix-widget` 的代码块，内容是自包含的 HTML 片段，不写 html、head、body 标签：<title> 写图解标题，<style> 与 <script> 内联，用 SVG 或 canvas 绘制，图例、播放控件、说明文字都放在图解里。它在沙箱里运行且没有网络：不要引用外部脚本、样式、字体、图片或 CDN，不要用 alert、prompt、表单提交、localStorage、cookie。代码块写完后才会运行。</format>\n",
    "  <design>宽度占满容器，高度不超过 640px；颜色只用主题变量 var(--foreground)、var(--muted-foreground)、var(--primary)、var(--border)、var(--card)、var(--chart-1) 到 var(--chart-5)，深色与浅色都要可读，背景留透明；标签不要压在图形上；有动画时提供播放与暂停，并尊重 prefers-reduced-motion；代码控制在 12KB 以内，写得完整，不要省略号或待补充。</design>\n",
    "  <layout>宿主已留出 16px 内边距，并给 button、select、input 设好了与界面一致的样式，外层不要再加边距，控件不要另写样式。从上到下：标题与一句说明、图形、随图形变化的说明文字、控件行。控件放进一个 class=\"controls\" 的容器排成一行（播放/暂停、速度、进度），按下状态用 aria-pressed=\"true\"。随状态变化的文字不要直接改写：把每种情况的文案都写成 class=\"stable\" 容器的子元素，当前那段加 class=\"on\"，切换时只移动 on，容器高度自动取最长那段；变化的数字加 class=\"num\" 并给最小宽度。控件不能因为文字长短或行数变化而移动。追问不要自己写输入框或按钮：调用 deeix.suggest(\"问题\") 给出一个用户可能想问的完整问题（120 字以内，可以带上当前的参数），宿主会在图解下方显示与界面一致的追问框，用户可以改写后发送；状态变化时可以再次调用来更新。</layout>\n",
    "  <api>window.deeix.getState()、onState(fn)、setState({名: 值}) 保存用户的选择（名用字母、数字、- 和 _，值是 JSON，合计 4KB 内）：刷新后恢复，下一轮以 ui_state 提供给你。deeix.suggest(\"文字\") 设置图解下方的追问，传空字符串则不显示。deeix.onTheme(fn) 在用户切换深浅色时回调。</api>\n",
    "</widget>",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_specs_cover_content_and_capabilities() {
        let specs = builtin_specs();
        assert_eq!(specs.len(), 12);
        let names: Vec<&str> = specs.iter().map(|s| s.name).collect();
        for want in ["card-grid", "chart", "function-plot", "stat-grid", "gantt", "data-table", "calculator", "decision-tree", "diff", "quiz"] {
            assert!(names.contains(&want), "missing builtin {want}");
        }
        assert!(names.contains(&CAPABILITY_PANEL));
        assert!(names.contains(&CAPABILITY_WIDGET));
    }

    #[test]
    fn description_and_props_summary_follow_separator_rules() {
        let comps = builtin_components();
        let chart = comps.iter().find(|c| c.name == "chart").unwrap();
        // 描述：定位。适用：…。交互：…
        assert!(chart.description.starts_with("数据图表。适用："));
        assert!(chart.description.contains("。交互："));
        // 摘要：props。约定：rules —— chart 有 rules，calculator 没有
        assert!(chart.props_summary.contains(PROPS_RULES_SEPARATOR));
        let calc = comps.iter().find(|c| c.name == "calculator").unwrap();
        assert!(!calc.props_summary.contains(PROPS_RULES_SEPARATOR));
    }

    #[test]
    fn catalog_static_format() {
        let out = catalog_prompt(&builtin_components(), BLOCK_FORMAT_STATIC);
        // 能力行不进目录
        assert!(!out.contains("<component name=\"panel\""));
        assert!(!out.contains("<component name=\"widget\""));
        // header/footer 成对
        assert!(out.starts_with("<nature>"));
        assert!(out.contains("<catalog>"));
        assert!(out.trim_end().ends_with("</catalog>"));
        // 组件行：use/props/rules 拆分
        assert!(out.contains("<component name=\"chart\">"));
        assert!(out.contains("<props>{title?, type:"));
        assert!(out.contains("<rules>scatter 的 x 为数值"));
        // 无 rules 的组件不产生 <rules>
        let calc_line = out.split("<component name=\"calculator\">").nth(1).unwrap();
        assert!(calc_line.split("</component>").next().unwrap().contains("<props>{title, inputs:"));
        assert!(!calc_line.split("</component>").next().unwrap().contains("<rules>"));
        // 交互段不进提示词（目录只留定位与适用）
        assert!(!out.contains("。交互："));
    }

    #[test]
    fn panel_capability_supersedes_calculator() {
        // 内置能力行默认启用（全吞形态 = panel+widget 全开）
        let comps = builtin_components();
        let (p, w) = capabilities(&comps);
        assert!(p && w);
        let out = catalog_prompt(&comps, BLOCK_FORMAT_LIVE);
        // calculator 被 panel 取代，不出现在目录
        assert!(!out.contains("<component name=\"calculator\""));
        // live header 出现
        assert!(out.contains("<live>"));
        assert!(out.contains("<primitives>"));
        // 两个 example 都在
        assert!(out.contains("\"component\": \"panel\""));
    }

    #[test]
    fn live_header_only_when_format_live() {
        // 静态格式：即使勾了 panel，也不进 live 分支
        let mut comps = builtin_components();
        comps.iter_mut().find(|c| c.name == CAPABILITY_PANEL).unwrap().enabled = true;
        let out = catalog_prompt(&comps, BLOCK_FORMAT_STATIC);
        assert!(!out.contains("<live>"));
        assert!(out.contains("<component name=\"calculator\""), "静态分支无 superseded 规则");
        assert!(!out.contains("<component name=\"panel\""));
    }

    #[test]
    fn widget_capability_adds_widget_section() {
        // 只开 widget：panel 关掉才能断言无 <live> 节
        let mut comps = builtin_components();
        comps.iter_mut().find(|c| c.name == CAPABILITY_PANEL).unwrap().enabled = false;
        comps.iter_mut().find(|c| c.name == CAPABILITY_WIDGET).unwrap().enabled = true;
        let out = catalog_prompt(&comps, BLOCK_FORMAT_LIVE);
        assert!(out.contains("<widget>"));
        assert!(out.contains(WIDGET_FENCE_LANGUAGE));
        assert!(!out.contains("<live>"), "没勾 panel 不应有 <live> 节");
    }

    #[test]
    fn empty_when_nothing_enabled_and_no_capability() {
        let mut comps = builtin_components();
        for c in &mut comps {
            c.enabled = false;
        }
        assert_eq!(catalog_prompt(&comps, BLOCK_FORMAT_STATIC), "");
        assert_eq!(catalog_prompt(&comps, BLOCK_FORMAT_LIVE), "");
        // 只勾能力：live 下 header 仍在（无组件行）
        comps.iter_mut().find(|c| c.name == CAPABILITY_PANEL).unwrap().enabled = true;
        let out = catalog_prompt(&comps, BLOCK_FORMAT_LIVE);
        assert!(out.contains("<live>"));
        assert!(!out.contains("<component name="));
    }

    #[test]
    fn catalog_capped_at_32() {
        let mut comps: Vec<Component> = builtin_components();
        for i in 0..40 {
            comps.push(Component {
                renderer_kind: "sandbox".into(),
                name: format!("custom-{i}"),
                version: 1,
                description: "自定义".into(),
                props_summary: "{}".into(),
                props_schema: "{}".into(),
                renderer_source: "<div></div>".into(),
                enabled: true,
            });
        }
        let out = catalog_prompt(&comps, BLOCK_FORMAT_STATIC);
        let count = out.matches("<component name=").count();
        assert_eq!(count, 32);
        assert!(out.contains("<component name=\"custom-21\""));
        assert!(!out.contains("<component name=\"custom-22\""));
    }

    #[test]
    fn sandbox_components_enter_catalog_with_props_schema() {
        let mut comps = builtin_components();
        comps.push(Component {
            renderer_kind: "sandbox".into(),
            name: "mood-ring".into(),
            version: 2,
            description: "心情环".into(),
            props_summary: "{tone}".into(),
            props_schema: "{\"type\":\"object\"}".into(),
            renderer_source: "<div class=\"ring\"></div>".into(),
            enabled: true,
        });
        let out = catalog_prompt(&comps, BLOCK_FORMAT_STATIC);
        assert!(out.contains("<component name=\"mood-ring\" version=\"2\">"));
        assert!(out.contains("<use>心情环</use>"));
    }

    #[test]
    fn disabled_components_skipped() {
        let mut comps = builtin_components();
        comps.iter_mut().find(|c| c.name == "gantt").unwrap().enabled = false;
        let out = catalog_prompt(&comps, BLOCK_FORMAT_STATIC);
        assert!(!out.contains("<component name=\"gantt\""));
        assert!(out.contains("<component name=\"chart\""));
    }
}
