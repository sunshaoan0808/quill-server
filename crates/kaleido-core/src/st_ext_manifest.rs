//! ST 扩展 `manifest.json` 兼容层 → MuseAI 插件定义。
//!
//! 契约实测自 `/root/workspaces/TauriTavern/src/scripts/extensions/*/manifest.json`
//! 与 `src/scripts/extensions.js` 的消费点（`:500-542` hook 调用、`:723-801`
//! `getManifests` / `shouldActivateExtension`、`:1169` `optional` 展示、`:65`
//! `sortManifestsByOrder`）。
//!
//! # 契约字段
//! 必选（ST 侧语义）：`js`（入口）、`display_name`、`loading_order`、
//! `requires`（Extras 依赖数组）、`optional`。
//! 选：`css`、`author`、`version`、`homePage`、`hooks`（`{hook名: 导出函数名}`）、
//! `generate_interceptor`、`i18n`、`description`、`dependencies`、
//! `minimum_client_version`。
//!
//! # 兼容红线（与 ST 行为逐条对齐）
//! - **只有「是 JSON object」是硬校验**：`extensions.js:723-725` 对
//!   `!manifest || typeof !== 'object' || Array.isArray` 抛错，其余字段一律
//!   「缺失即宽容放行」。本模块 [`StExtensionManifest::from_value`] 同规则：
//!   非 object 返回 `None`，字段缺失只留 `None`，绝不因此拒载。
//! - **`requires` / `dependencies` 非数组只 warn 不拒载**（`:779`、`:801`
//!   两处 `console.warn` + 继续激活）。本模块把警告收进
//!   [`StExtensionManifest::warnings`]，判定结果留在
//!   [`StExtensionManifest::requires_satisfied`] /
//!   [`StExtensionManifest::dependencies_satisfied`]。
//! - **camel/snake 双写**：`displayName`/`display_name`、`loadingOrder`/
//!   `loading_order`、`homePage`/`home_page`、`generateInterceptor`/
//!   `generate_interceptor`、`minimumClientVersion`/`minimum_client_version`
//!   都认，两套都可出现（与 st_quick_reply / st_regex 保持一致）。
//! - **未知字段宽容**：解析后未识别的键进 [`StExtensionManifest::unknown_fields`]，
//!   不报错、不丢弃（值保留在 [`StExtensionManifest::raw`] 里）。
//!
//! # 关键诚实性边界
//! ST 扩展的 `js` 入口是依赖 ST DOM / jQuery / `window.SillyTavern` 的 JS，
//! **在 MuseAI 里跑不起来**。本模块因此**不产出任何「已执行 JS」的假象**：
//! 插件定义里带 `js_body_present: true` + `unsupported_reason`，
//! 由 [`crate::plugin`] 侧显式标注为不可执行。静默丢弃才是骗人。

use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;

/// MuseAI 的 hook 名（字符串形态，与 [`crate::plugin::NarrativeEvent`] 的变体
/// 一一对应）。本模块刻意**不复用** m1 的 Rust 类型，只产出字符串 + JSON，
/// 避免跨批次耦合。
pub type StPluginDefJson = Value;

/// ST hook 名 → MuseAI hook 名映射表。
///
/// 映射依据是 ST 扩展世界里的**语义位置**，不是名字字面相同：
/// `init`/`activate` 是扩展挂载时刻（≈ 会话/插件激活），
/// `main` 是入口主函数（MuseAI 里最接近的是玩家侧显式命令），
/// `chat` 在玩家输入之后、组装 prompt 之前触发，
/// `generation` 在组装 prompt 送模型之前触发，
/// `generation_after`/`message_received` 在拿到生成结果之后触发。
///
/// 映射不到的 hook 一律 `None` → 调用方**忽略**（不猜、不硬塞）。
pub const ST_HOOK_TO_MUSEAI: &[(&str, &str)] = &[
    ("init", "SessionCreated"),
    ("activate", "SessionCreated"),
    ("main", "SlashCommand"),
    ("chat", "UserMessageSubmitted"),
    ("generation", "PromptBuilding"),
    ("generation_before", "PromptBuilding"),
    ("generation_after", "MessageGenerated"),
    ("message_received", "MessageGenerated"),
];

/// 单个 ST hook → MuseAI hook 名。映射不到返回 `None`。
pub fn map_st_hook_to_museai(st_hook: &str) -> Option<&'static str> {
    let key = st_hook.trim().to_ascii_lowercase();
    ST_HOOK_TO_MUSEAI
        .iter()
        .find(|(from, _)| *from == key)
        .map(|(_, to)| *to)
}

/// 插件定义里 `js_body_present` 为 true 时填的 `unsupported_reason`。
///
/// 固定文案 = 固定事实：MuseAI 没有 ST 的 DOM / jQuery / `window.SillyTavern`，
/// 扩展入口 JS 无法执行。这一条**不允许被调用方静默吞掉**。
pub const JS_UNSUPPORTED_REASON: &str =
    "ST 扩展入口 JS 依赖 SillyTavern 的 DOM / jQuery / window.SillyTavern 全局，MuseAI 无此宿主，无法执行；\
     本插件定义只承载 manifest 元数据与 hook 名映射，JS 本体已标注未执行";

/// 一条宽容警告（对应 ST 侧的 `console.warn`，不阻断加载）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StManifestWarning {
    /// 扩展名（扫目录时才有；单 manifest 解析时可能为空）。
    pub extension: Option<String>,
    /// 触发警告的字段名（如 `"requires"`）。
    pub field: String,
    /// 人读警告文案。
    pub message: String,
}

impl StManifestWarning {
    fn new(extension: Option<String>, field: &str, message: impl Into<String>) -> Self {
        Self {
            extension,
            field: field.to_string(),
            message: message.into(),
        }
    }
}

/// 扫目录时单个 manifest 的加载失败（对应 `getManifests` 的 catch）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StManifestLoadError {
    /// 扩展目录名。
    pub extension: String,
    /// manifest.json 的完整路径。
    pub path: String,
    /// 失败原因。
    pub reason: String,
}

// ─── manifest 本体 ───────────────────────────────────────────────────────────

/// ST 扩展 manifest。**所有字段都是 `Option`**：ST 只硬校验「是不是 JSON
/// object」，字段缺失一律宽容放行（见模块文档「兼容红线」）。
#[derive(Debug, Clone, PartialEq)]
pub struct StExtensionManifest {
    /// 入口 JS 相对路径（ST 必选；缺失只标注，不拒载）。
    pub js: Option<String>,
    /// 展示名（camel `displayName` / snake `display_name` 都认）。
    pub display_name: Option<String>,
    /// 加载顺序（camel `loadingOrder` / snake `loading_order`）。
    pub loading_order: Option<f64>,
    /// Extras 模块依赖。`None` 表示「缺失或非数组」——两种情况都放行。
    pub requires: Option<Vec<String>>,
    /// 可选的 Extras 模块（纯展示用，`:1169`）。
    pub optional: Option<Vec<String>>,
    /// 入口 CSS。
    pub css: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    /// 主页（camel `homePage` / snake `home_page`）。
    pub home_page: Option<String>,
    /// `hooks`：`{ST hook名: 导出的函数名}`，只保留字符串值。
    pub hooks: Option<Vec<(String, String)>>,
    /// `generate_interceptor`：ST 会把这些扩展排到生成拦截链最前
    /// （`:2310`）。MuseAI 无对应拦截点，这里**只如实标记**。
    pub generate_interceptor: bool,
    /// i18n 映射（原样保留）。
    pub i18n: Option<Value>,
    pub description: Option<String>,
    /// 其它扩展依赖（ST 语义：依赖的扩展名）。`None` = 缺失或非数组 → 放行。
    pub dependencies: Option<Vec<String>>,
    /// 最低客户端版本（camel `minimumClientVersion` / snake `minimum_client_version`）。
    pub minimum_client_version: Option<String>,

    /// 未识别的字段名（宽容：不报错、不丢，值仍在 [`Self::raw`]）。
    pub unknown_fields: BTreeSet<String>,
    /// 宽容警告（非数组依赖、类型不符等）。
    pub warnings: Vec<StManifestWarning>,
    /// 原始 manifest（逐字节保留，未识别字段只在这里）。
    pub raw: Value,
}

impl StExtensionManifest {
    /// camel/snake 双写取第一个存在的键（值不要求存在）。
    fn pick<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
        keys.iter().find_map(|k| obj.get(*k))
    }

    /// 取字符串字段；键在但不是字符串 → warn + `None`（宽容，不拒载）。
    fn str_field(
        obj: &Map<String, Value>,
        keys: &[&str],
        ext: Option<&str>,
        warnings: &mut Vec<StManifestWarning>,
    ) -> Option<String> {
        match Self::pick(obj, keys) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => {
                warnings.push(StManifestWarning::new(
                    ext.map(str::to_string),
                    keys[0],
                    format!(
                        "manifest.json '{}' is not a string (found {}). Value ignored, loading allowed.",
                        keys[0],
                        type_name(other)
                    ),
                ));
                None
            }
        }
    }

    /// 取字符串数组字段。**非数组只 warn 不拒载**（`:779`、`:801`）。
    fn str_array_field(
        obj: &Map<String, Value>,
        keys: &[&str],
        ext: Option<&str>,
        warnings: &mut Vec<StManifestWarning>,
    ) -> Option<Vec<String>> {
        match Self::pick(obj, keys) {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    match it {
                        Value::String(s) => out.push(s.clone()),
                        other => warnings.push(StManifestWarning::new(
                            ext.map(str::to_string),
                            keys[0],
                            format!(
                                "manifest.json '{}' contains a non-string element ({}). Element ignored, loading allowed.",
                                keys[0],
                                type_name(other)
                            ),
                        )),
                    }
                }
                Some(out)
            }
            Some(_) => {
                warnings.push(StManifestWarning::new(
                    ext.map(str::to_string),
                    keys[0],
                    format!(
                        "Extension: manifest.json '{}' field is not an array. Loading allowed, but any intended requirements were not verified to exist.",
                        keys[0]
                    ),
                ));
                None
            }
        }
    }

    /// 解析 manifest。**唯一的硬校验是「必须是 JSON object」**
    /// （`extensions.js:723-725`）；数组 / null / 标量一律返回 `None`。
    pub fn from_value(value: &Value) -> Option<Self> {
        Self::from_value_named(value, None)
    }

    /// 同 [`Self::from_value`]，但带上扩展名让警告可定位（扫目录用）。
    pub fn from_value_named(value: &Value, extension: Option<&str>) -> Option<Self> {
        let obj = value.as_object()?;
        let ext = extension.map(str::to_string);
        let mut warnings: Vec<StManifestWarning> = Vec::new();

        let mut hooks: Option<Vec<(String, String)>> = None;
        match Self::pick(obj, &["hooks"]) {
            None | Some(Value::Null) => {}
            Some(Value::Object(hmap)) => {
                let mut list = Vec::new();
                for (k, v) in hmap {
                    match v {
                        Value::String(s) if !s.is_empty() => list.push((k.clone(), s.clone())),
                        Value::String(_) => {}
                        other => warnings.push(StManifestWarning::new(
                            ext.clone(),
                            "hooks",
                            format!(
                                "manifest.json hook '{k}' is not a non-empty string ({}). Hook ignored, loading allowed.",
                                type_name(other)
                            ),
                        )),
                    }
                }
                hooks = Some(list);
            }
            Some(_) => warnings.push(StManifestWarning::new(
                ext.clone(),
                "hooks",
                format!(
                    "manifest.json 'hooks' field is not an object ({}). All hooks ignored, loading allowed.",
                    type_name(Self::pick(obj, &["hooks"]).unwrap_or(&Value::Null))
                ),
            )),
        }

        const KNOWN: &[&str] = &[
            "js", "entry", "display_name", "displayName", "loading_order", "loadingOrder",
            "requires", "optional", "css", "author", "version", "homePage", "home_page", "hooks",
            "generate_interceptor", "generateInterceptor", "i18n", "description", "dependencies",
            "minimum_client_version", "minimumClientVersion",
        ];
        let unknown_fields: BTreeSet<String> = obj
            .keys()
            .filter(|k| !KNOWN.contains(&k.as_str()))
            .cloned()
            .collect();

        let e = ext.as_deref();
        let generate_interceptor = obj
            .get("generate_interceptor")
            .or_else(|| obj.get("generateInterceptor"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        Some(Self {
            js: Self::str_field(obj, &["js", "entry"], e, &mut warnings),
            display_name: Self::str_field(obj, &["display_name", "displayName"], e, &mut warnings),
            loading_order: Self::pick(obj, &["loading_order", "loadingOrder"])
                .and_then(value_to_f64),
            requires: Self::str_array_field(obj, &["requires"], e, &mut warnings),
            optional: Self::str_array_field(obj, &["optional"], e, &mut warnings),
            css: Self::str_field(obj, &["css"], e, &mut warnings),
            author: Self::str_field(obj, &["author"], e, &mut warnings),
            version: Self::str_field(obj, &["version"], e, &mut warnings),
            home_page: Self::str_field(obj, &["homePage", "home_page"], e, &mut warnings),
            hooks,
            generate_interceptor,
            i18n: Self::pick(obj, &["i18n"]).cloned(),
            description: Self::str_field(obj, &["description"], e, &mut warnings),
            dependencies: Self::str_array_field(obj, &["dependencies"], e, &mut warnings),
            minimum_client_version: Self::str_field(
                obj,
                &["minimum_client_version", "minimumClientVersion"],
                e,
                &mut warnings,
            ),
            unknown_fields,
            warnings,
            raw: value.clone(),
        })
    }

    /// 解析 manifest 文本。
    ///
    /// 返回 `Ok(None)` = 文本合法但不是 JSON object（ST 侧抛错，扫目录时跳过）；
    /// 返回 `Err` = JSON 语法错误（ST 侧同样落到 catch 分支）。
    pub fn from_json_str(text: &str) -> Result<Option<Self>, serde_json::Error> {
        let value: Value = serde_json::from_str(text)?;
        Ok(Self::from_value(&value))
    }

    /// `requires` 判定。`None`（缺失或非数组）→ **放行**（`requires_ok == true`）。
    pub fn requires_satisfied(&self, available_modules: &[&str]) -> (bool, Vec<String>) {
        match &self.requires {
            None => (true, Vec::new()),
            Some(req) => {
                let missing: Vec<String> = req
                    .iter()
                    .filter(|r| !available_modules.contains(&r.as_str()))
                    .cloned()
                    .collect();
                (missing.is_empty(), missing)
            }
        }
    }

    /// `dependencies` 判定。同 ST：缺失/非数组放行；是数组则查存在性 +
    /// 依赖项是否被禁用，被禁用的依赖同样算不满足（`:790-797`）。
    pub fn dependencies_satisfied(
        &self,
        discovered: &[&str],
        disabled: &[&str],
    ) -> (bool, Vec<String>, Vec<String>) {
        match &self.dependencies {
            None => (true, Vec::new(), Vec::new()),
            Some(deps) => {
                let missing: Vec<String> = deps
                    .iter()
                    .filter(|d| !discovered.contains(&d.as_str()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    return (false, missing, Vec::new());
                }
                let disabled_deps: Vec<String> = deps
                    .iter()
                    .filter(|d| disabled.contains(&d.as_str()))
                    .cloned()
                    .collect();
                // 依赖都存在，但其中有被 disable 的 → 同样不满足（:790-797）
                (disabled_deps.is_empty(), Vec::new(), disabled_deps)
            }
        }
    }

    /// ST 的 `sortManifestsByOrder`（`:65`）：先 `loading_order` 升序，
    /// 同序再按 `display_name` 字符串比。
    pub fn sort_key(&self) -> (f64, String) {
        (
            self.loading_order.unwrap_or(f64::MAX),
            self.display_name.clone().unwrap_or_default(),
        )
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        // JS 侧是 parseInt 语义：数字字符串也能吃
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

// ─── manifest → 插件定义 ─────────────────────────────────────────────────────

/// 把 ST manifest 映射成 MuseAI 能消费的插件定义（纯 JSON，无 m1 耦合）。
///
/// 产出的 JSON 形状：
/// ```json
/// {
///   "source": "sillytavern-extension",
///   "id": "assets",                       // 调用方给的扩展名（可为 null）
///   "name": "Assets",                     // display_name 兜底扩展名
///   "enabled": false,                     // MuseAI 里需显式启用，绝不假装已激活
///   "hooks": [{ "museai_hook": "SessionCreated",
///               "st_hook": "activate", "st_export": "init" }],
///   "js_body_present": true,
///   "unsupported_reason": "…JS_UNSUPPORTED_REASON…",
///   "js": { "entry": "index.js", "css": "style.css",
///           "source_bytes": 1234, "executable_in_museai": false },
///   "requirements": { "requires": [], "optional": [], "dependencies": [] },
///   "meta": { "author": "…", "version": "…", "home_page": "…",
///             "description": "…", "loading_order": 15.0,
///             "generate_interceptor": false,
///             "minimum_client_version": null, "i18n": null,
///             "unknown_fields": [], "warnings": [] }
/// }
/// ```
///
/// **诚实性**：`js` 字段只描述「有这个文件」，从不描述「已加载」。
/// `entry_script_source` 是入口脚本文本（可选）；**只量字节数**——它不执行、
/// 也不被当成可运行的插件体。真要真跑请看
/// [`crate::plugin`] 侧的注册路径，别在这里骗自己。
pub fn manifest_to_plugin_definition(
    manifest: &StExtensionManifest,
    extension_name: Option<&str>,
    entry_script_source: Option<&str>,
) -> StPluginDefJson {
    // hook 映射：按 ST_HOOK_TO_MUSEAI 表的顺序走（**不是** manifest 里的键序），
    // 保证同一 museai hook 只留首次、且输出顺序与映射表一致、可复现。
    let decls = manifest.hooks.clone().unwrap_or_default();
    let mut hooks: Vec<Value> = Vec::new();
    for (st_hook, museai_hook) in ST_HOOK_TO_MUSEAI {
        // 同一 museai hook 只留首次（表内先出现的 ST hook 胜出），不重复注册。
        if hooks
            .iter()
            .any(|h| h["museai_hook"].as_str() == Some(*museai_hook))
        {
            continue;
        }
        let st_export = decls
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(st_hook))
            .map(|(_, export)| export.clone());
        if let Some(st_export) = st_export {
            hooks.push(json!({
                "museai_hook": *museai_hook,
                "st_hook": st_hook,
                "st_export": st_export,
            }));
        }
    }
    // 映射不到的一律忽略（不猜、不硬塞），但记下来便于排查。
    // 显式排序：`serde_json::Map` 是 BTreeMap 支撑的，键序已被字母化，
    // 这里给一个与声明顺序无关但**稳定可复现**的顺序。
    let mut unmapped: Vec<String> = decls
        .iter()
        .map(|(name, _)| name)
        .filter(|name| {
            !ST_HOOK_TO_MUSEAI
                .iter()
                .any(|(from, _)| from.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect();
    unmapped.sort();

    let js_body_present = manifest.js.is_some();
    let name = manifest
        .display_name
        .clone()
        .or_else(|| extension_name.map(str::to_string))
        .unwrap_or_else(|| "unnamed-extension".to_string());

    let mut meta = Map::new();
    meta.insert("author".into(), json!(manifest.author));
    meta.insert("version".into(), json!(manifest.version));
    meta.insert("home_page".into(), json!(manifest.home_page));
    meta.insert("description".into(), json!(manifest.description));
    meta.insert("loading_order".into(), json!(manifest.loading_order));
    meta.insert(
        "generate_interceptor".into(),
        json!(manifest.generate_interceptor),
    );
    meta.insert(
        "minimum_client_version".into(),
        json!(manifest.minimum_client_version),
    );
    meta.insert("i18n".into(), json!(manifest.i18n));
    meta.insert("unknown_fields".into(), json!(manifest.unknown_fields));
    meta.insert(
        "warnings".into(),
        json!(manifest
            .warnings
            .iter()
            .map(|w| json!({
                "field": w.field,
                "message": w.message,
                "extension": w.extension,
            }))
            .collect::<Vec<_>>()),
    );
    if !unmapped.is_empty() {
        meta.insert("unmapped_st_hooks".into(), json!(unmapped));
    }

    let mut root = Map::new();
    root.insert("source".into(), json!("sillytavern-extension"));
    root.insert("id".into(), json!(extension_name));
    root.insert("name".into(), json!(name));
    root.insert("enabled".into(), json!(false));
    root.insert("hooks".into(), Value::Array(hooks));
    // ── 关键：不假装能跑 ──────────────────────────────────────────────
    root.insert("js_body_present".into(), json!(js_body_present));
    root.insert(
        "unsupported_reason".into(),
        if js_body_present {
            json!(JS_UNSUPPORTED_REASON)
        } else {
            Value::Null
        },
    );
    root.insert(
        "js".into(),
        json!({
            "entry": manifest.js,
            "css": manifest.css,
            "source_bytes": entry_script_source.map(str::len),
            // 永远 false：MuseAI 无 ST 宿主，见 unsupported_reason
            "executable_in_museai": false,
        }),
    );
    root.insert(
        "requirements".into(),
        json!({
            "requires": manifest.requires,
            "optional": manifest.optional,
            "dependencies": manifest.dependencies,
        }),
    );
    root.insert("meta".into(), Value::Object(meta));
    Value::Object(root)
}

// ─── 扫目录 ──────────────────────────────────────────────────────────────────

/// [`list_extension_manifests`] 的结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StExtensionManifestScan {
    /// 成功解析的 `(扩展名, manifest)`。
    pub manifests: Vec<(String, StExtensionManifest)>,
    /// 跳过的损坏 manifest（不中断整个扫描）。
    pub errors: Vec<StManifestLoadError>,
    /// 目录整体级问题（如目录不存在）；`None` = 目录读到了（哪怕是空的）。
    pub dir_error: Option<String>,
}

impl StExtensionManifestScan {
    /// 按 ST 的 `sortManifestsByOrder`（`:65`）排好的扩展名。
    pub fn sorted_names(&self) -> Vec<String> {
        let mut v: Vec<(f64, String, &str)> = self
            .manifests
            .iter()
            .map(|(n, m)| (m.sort_key().0, m.sort_key().1, n.as_str()))
            .collect();
        v.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
        v.into_iter().map(|(_, _, n)| n.to_string()).collect()
    }

    /// 便捷取值。
    pub fn get(&self, name: &str) -> Option<&StExtensionManifest> {
        self.manifests
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, m)| m)
    }
}

/// 扫 `extensions_dir` 下每个子目录的 `manifest.json`（照 `getManifests`，`:723`）。
///
/// **损坏文件只跳过、不中断**（ST 侧是 `Promise.all` + per-name `catch`，
/// `console.warn` 后继续）：JSON 语法错误、非 object、读不出来，都进
/// [`StExtensionManifestScan::errors`]。目录本身不存在则 `dir_error` 有值、
/// `manifests` 为空。
pub fn list_extension_manifests(extensions_dir: &Path) -> StExtensionManifestScan {
    let mut scan = StExtensionManifestScan::default();
    let entries = match std::fs::read_dir(extensions_dir) {
        Ok(e) => e,
        Err(err) => {
            scan.dir_error = Some(format!("{err}"));
            return scan;
        }
    };

    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        match p.file_name().and_then(|n| n.to_str()) {
            Some(n) if !n.starts_with('.') => names.push(n.to_string()),
            _ => {}
        }
    }
    names.sort();

    for name in names {
        let manifest_path = extensions_dir.join(&name).join("manifest.json");
        let path_str = manifest_path.display().to_string();
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(t) => t,
            Err(err) => {
                // 目录里没有 manifest.json 是 ST 的常态（它按 names 列表取），
                // 这里当警告而不是错误：ST 侧 fetch 404 也是 warn 后跳过。
                scan.errors.push(StManifestLoadError {
                    extension: name,
                    path: path_str,
                    reason: format!("{err}"),
                });
                continue;
            }
        };
        match StExtensionManifest::from_json_str(&text) {
            Ok(Some(m)) => {
                let mut m = m;
                m.warnings.iter_mut().for_each(|w| w.extension = Some(name.clone()));
                scan.manifests.push((name, m));
            }
            Ok(None) => scan.errors.push(StManifestLoadError {
                extension: name,
                path: path_str,
                reason: "manifest.json must contain a JSON object".to_string(),
            }),
            Err(err) => scan.errors.push(StManifestLoadError {
                extension: name,
                path: path_str,
                reason: format!("invalid JSON: {err}"),
            }),
        }
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "st_ext_manifest_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_ext(root: &Path, name: &str, manifest: &str) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("manifest.json"), manifest).unwrap();
    }

    /// 抄自 TauriTavern `src/scripts/extensions/caption/manifest.json`。
    const REAL_CAPTION: &str = r#"{
        "display_name": "Image Captioning",
        "loading_order": 4,
        "requires": [],
        "optional": ["caption"],
        "js": "index.js",
        "css": "style.css",
        "author": "Cohee#1207",
        "version": "1.0.0",
        "homePage": "https://github.com/SillyTavern/SillyTavern",
        "hooks": { "activate": "init" }
    }"#;

    // ── 解析 ────────────────────────────────────────────────────────────────

    #[test]
    fn parses_full_real_manifest() {
        let m = StExtensionManifest::from_json_str(REAL_CAPTION).unwrap().unwrap();
        assert_eq!(m.display_name.as_deref(), Some("Image Captioning"));
        assert_eq!(m.loading_order, Some(4.0));
        assert_eq!(m.js.as_deref(), Some("index.js"));
        assert_eq!(m.css.as_deref(), Some("style.css"));
        assert_eq!(m.author.as_deref(), Some("Cohee#1207"));
        assert_eq!(m.version.as_deref(), Some("1.0.0"));
        assert_eq!(m.home_page.as_deref(), Some("https://github.com/SillyTavern/SillyTavern"));
        assert_eq!(m.requires.as_deref(), Some(&[][..]));
        assert_eq!(m.optional.unwrap(), vec!["caption".to_string()]);
        assert_eq!(
            m.hooks.unwrap(),
            vec![("activate".to_string(), "init".to_string())]
        );
        assert!(!m.generate_interceptor);
        assert!(m.warnings.is_empty(), "真实 manifest 不该有警告");
        assert!(m.unknown_fields.is_empty());
    }

    #[test]
    fn empty_object_is_valid_missing_everything() {
        let m = StExtensionManifest::from_json_str("{}").unwrap().unwrap();
        assert!(m.js.is_none() && m.display_name.is_none() && m.loading_order.is_none());
        assert!(m.requires.is_none() && m.optional.is_none() && m.hooks.is_none());
        assert!(m.warnings.is_empty());
        // 依赖缺失 = 放行
        assert_eq!(m.requires_satisfied(&[]).0, true);
        assert_eq!(m.dependencies_satisfied(&[], &[]).0, true);
    }

    #[test]
    fn only_non_object_is_rejected() {
        // extensions.js:723-725 只认「是不是 JSON object」
        for bad in ["[]", "null", "1", "\"str\"", "true"] {
            assert!(
                StExtensionManifest::from_json_str(bad).unwrap().is_none(),
                "{bad} 不是 object，应被拒"
            );
        }
        // 语法错误是 Err（ST 侧同一 catch 分支）
        assert!(StExtensionManifest::from_json_str("{oops").is_err());
        assert!(StExtensionManifest::from_json_str("").is_err());
    }

    #[test]
    fn unknown_fields_tolerated_and_preserved() {
        let m = StExtensionManifest::from_json_str(
            r#"{"display_name":"X","loading_order":1,"brand_new_field":{"a":1},"zzz":7}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(m.display_name.as_deref(), Some("X"));
        assert_eq!(m.unknown_fields.len(), 2);
        assert!(m.unknown_fields.contains("brand_new_field"));
        // 值没丢，还在 raw 里
        assert_eq!(m.raw["brand_new_field"]["a"], json!(1));
        assert!(m.warnings.is_empty(), "未知字段不该 warn 也不该拒载");
    }

    #[test]
    fn camel_and_snake_both_accepted() {
        let camel = StExtensionManifest::from_json_str(
            r#"{"displayName":"Camel","loadingOrder":"7","homePage":"hp","minimumClientVersion":"1.2.3","generateInterceptor":true}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(camel.display_name.as_deref(), Some("Camel"));
        // parseInt 语义：数字字符串也吃
        assert_eq!(camel.loading_order, Some(7.0));
        assert_eq!(camel.home_page.as_deref(), Some("hp"));
        assert_eq!(camel.minimum_client_version.as_deref(), Some("1.2.3"));
        assert!(camel.generate_interceptor);

        let snake = StExtensionManifest::from_json_str(
            r#"{"display_name":"Snake","loading_order":7,"home_page":"hp","minimum_client_version":"1.2.3","generate_interceptor":true}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(snake.display_name.as_deref(), Some("Snake"));
        assert_eq!(snake.loading_order, Some(7.0));
        assert_eq!(snake.home_page.as_deref(), Some("hp"));
        assert_eq!(snake.minimum_client_version.as_deref(), Some("1.2.3"));
        assert!(snake.generate_interceptor);
    }

    // ── 非数组依赖：只 warn 不拒载（:779、:801）─────────────────────────────

    #[test]
    fn non_array_requires_warns_but_does_not_reject() {
        let m = StExtensionManifest::from_json_str(
            r#"{"display_name":"X","requires":"caption","dependencies":{"a":"b"},"optional":[]}"#,
        )
        .unwrap()
        .unwrap();
        assert!(m.requires.is_none(), "非数组 → None（不放行成数组）");
        assert!(m.dependencies.is_none());
        assert_eq!(m.warnings.len(), 2);
        assert!(m.warnings.iter().any(|w| w.field == "requires"));
        assert!(m.warnings.iter().any(|w| w.field == "dependencies"));
        // 关键：判定仍是「满足」，不是拒载
        assert_eq!(m.requires_satisfied(&[]).0, true);
        assert_eq!(m.dependencies_satisfied(&[], &[]).0, true);
    }

    #[test]
    fn array_requires_are_enforced() {
        let m =
            StExtensionManifest::from_json_str(r#"{"requires":["caption","vectors"]}"#)
                .unwrap()
                .unwrap();
        assert_eq!(m.requires_satisfied(&["caption"]).0, false);
        assert_eq!(
            m.requires_satisfied(&["caption"]).1,
            vec!["vectors".to_string()]
        );
        assert_eq!(m.requires_satisfied(&["caption", "vectors"]).0, true);
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn dependencies_enforced_and_disabled_dep_fails() {
        let m = StExtensionManifest::from_json_str(r#"{"dependencies":["memory","regex"]}"#)
            .unwrap()
            .unwrap();
        let (ok, missing, disabled) = m.dependencies_satisfied(&["memory"], &[]);
        assert!(!ok);
        assert_eq!(missing, vec!["regex".to_string()]);
        assert!(disabled.is_empty());

        // 依赖存在但被 disable → 同样不满足（:790-797）
        let (ok, missing, disabled) = m.dependencies_satisfied(&["memory", "regex"], &["regex"]);
        assert!(!ok);
        assert!(missing.is_empty());
        assert_eq!(disabled, vec!["regex".to_string()]);

        let (ok, _, _) = m.dependencies_satisfied(&["memory", "regex"], &[]);
        assert!(ok);
    }

    // ── hook 映射 ───────────────────────────────────────────────────────────

    #[test]
    fn hook_mapping_table() {
        assert_eq!(map_st_hook_to_museai("init"), Some("SessionCreated"));
        assert_eq!(map_st_hook_to_museai("activate"), Some("SessionCreated"));
        assert_eq!(map_st_hook_to_museai("main"), Some("SlashCommand"));
        assert_eq!(map_st_hook_to_museai("chat"), Some("UserMessageSubmitted"));
        assert_eq!(map_st_hook_to_museai("generation"), Some("PromptBuilding"));
        assert_eq!(map_st_hook_to_museai("generation_before"), Some("PromptBuilding"));
        assert_eq!(map_st_hook_to_museai("generation_after"), Some("MessageGenerated"));
        assert_eq!(map_st_hook_to_museai("message_received"), Some("MessageGenerated"));
        // 大小写/空白宽容；映射不到就 None（调用方忽略，不猜）
        assert_eq!(map_st_hook_to_museai("  Init "), Some("SessionCreated"));
        assert_eq!(map_st_hook_to_museai("INSTALL"), None);
        assert_eq!(map_st_hook_to_museai(""), None);
    }

    #[test]
    fn plugin_def_maps_and_ignores_unknown_hooks() {
        let m = StExtensionManifest::from_json_str(
            r#"{"display_name":"Multi","js":"index.js",
                "hooks":{"init":"boot","main":"onMain","chat":"onChat","generation":"beforeGen",
                         "generation_after":"afterGen","install":"onInstall","chatSurface":"onSurface"}}"#,
        )
        .unwrap()
        .unwrap();
        let def = manifest_to_plugin_definition(&m, Some("multi"), None);
        let hooks = def["hooks"].as_array().unwrap();
        let mapped: Vec<&str> = hooks.iter().map(|h| h["museai_hook"].as_str().unwrap()).collect();
        assert_eq!(
            mapped,
            vec![
                "SessionCreated",
                "SlashCommand",
                "UserMessageSubmitted",
                "PromptBuilding",
                "MessageGenerated"
            ]
        );
        // ST 侧的 hook 名和导出函数名都留着，方便回溯
        assert_eq!(hooks[0]["st_hook"], json!("init"));
        assert_eq!(hooks[0]["st_export"], json!("boot"));
        // 映射不到的记进 meta.unmapped_st_hooks，不硬塞。
        // 顺序按字母排序（serde_json::Map 是 BTreeMap，声明顺序本就拿不到）。
        let unmapped = def["meta"]["unmapped_st_hooks"].as_array().unwrap();
        let un: Vec<&str> = unmapped.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(un, vec!["chatSurface", "install"]);
    }

    #[test]
    fn duplicate_museai_hook_deduped() {
        let m = StExtensionManifest::from_json_str(
            r#"{"display_name":"Dup","hooks":{"init":"a","activate":"b"}}"#,
        )
        .unwrap()
        .unwrap();
        let def = manifest_to_plugin_definition(&m, Some("dup"), None);
        assert_eq!(def["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(def["hooks"][0]["st_hook"], json!("init"));
    }

    // ── js_body_present：明确标注，不静默丢 ────────────────────────────────

    #[test]
    fn js_entry_is_flagged_not_silently_dropped() {
        let m = StExtensionManifest::from_json_str(REAL_CAPTION).unwrap().unwrap();
        let src = "$(function(){ /* jQuery on ST DOM */ })();";
        let def = manifest_to_plugin_definition(&m, Some("caption"), Some(src));
        // 有 js 字段 → 明确标注存在 + 给出不可执行原因
        assert_eq!(def["js_body_present"], json!(true));
        assert_eq!(def["unsupported_reason"], json!(JS_UNSUPPORTED_REASON));
        assert!(def["unsupported_reason"]
            .as_str()
            .unwrap()
            .contains("SillyTavern"));
        // 入口路径与体量都留着（不丢信息），但绝不说「可执行」
        assert_eq!(def["js"]["entry"], json!("index.js"));
        assert_eq!(def["js"]["css"], json!("style.css"));
        assert_eq!(def["js"]["source_bytes"], json!(src.len()));
        assert_eq!(def["js"]["executable_in_museai"], json!(false));
        // 也不假装已启用
        assert_eq!(def["enabled"], json!(false));
        assert_eq!(def["source"], json!("sillytavern-extension"));
    }

    #[test]
    fn no_js_field_means_no_flag_and_null_reason() {
        let m = StExtensionManifest::from_json_str(r#"{"display_name":"MetaOnly"}"#)
            .unwrap()
            .unwrap();
        let def = manifest_to_plugin_definition(&m, Some("meta"), None);
        assert_eq!(def["js_body_present"], json!(false));
        assert!(def["unsupported_reason"].is_null());
        assert!(def["js"]["entry"].is_null());
        assert!(def["js"]["source_bytes"].is_null());
        // name 兜底到扩展名
        assert_eq!(def["name"], json!("MetaOnly"));
    }

    #[test]
    fn plugin_def_carries_metadata_and_requirements() {
        let m = StExtensionManifest::from_json_str(
            r#"{"display_name":"SD","js":"index.js","requires":["caption"],"optional":["vectors"],
                "dependencies":["memory"],"author":"Darkatse","version":"1.0.0",
                "generate_interceptor":true,"i18n":{"zh-cn":"i18n/zh-cn.json"},
                "description":"d","minimum_client_version":"1.12.0","weird":1}"#,
        )
        .unwrap()
        .unwrap();
        let def = manifest_to_plugin_definition(&m, Some("stable-diffusion"), None);
        assert_eq!(def["requirements"]["requires"], json!(["caption"]));
        assert_eq!(def["requirements"]["optional"], json!(["vectors"]));
        assert_eq!(def["requirements"]["dependencies"], json!(["memory"]));
        assert_eq!(def["meta"]["author"], json!("Darkatse"));
        assert_eq!(def["meta"]["generate_interceptor"], json!(true));
        assert_eq!(def["meta"]["minimum_client_version"], json!("1.12.0"));
        assert_eq!(def["meta"]["i18n"]["zh-cn"], json!("i18n/zh-cn.json"));
        assert_eq!(def["meta"]["unknown_fields"], json!(["weird"]));
    }

    // ── 扫目录 ──────────────────────────────────────────────────────────────

    #[test]
    fn scan_dir_reads_all_and_skips_broken() {
        let root = tmpdir("scan");
        write_ext(&root, "a-good", REAL_CAPTION);
        write_ext(&root, "b-good", r#"{"display_name":"B","loading_order":2,"js":"b.js"}"#);
        write_ext(&root, "c-bad-json", "{not json");
        write_ext(&root, "d-array", "[]");
        write_ext(&root, "e-null", "null");
        // 没有 manifest.json 的目录
        std::fs::create_dir_all(root.join("f-no-manifest")).unwrap();
        // 散落文件（非目录）必须被忽略
        std::fs::write(root.join("loose.txt"), "x").unwrap();

        let scan = list_extension_manifests(&root);
        assert!(scan.dir_error.is_none());
        assert_eq!(scan.manifests.len(), 2, "只收 2 个好的");
        assert!(scan.get("a-good").is_some());
        assert!(scan.get("b-good").is_some());
        // 坏的各自进 errors，不中断
        assert_eq!(scan.errors.len(), 4);
        let exts: Vec<&str> = scan.errors.iter().map(|e| e.extension.as_str()).collect();
        assert_eq!(exts, vec!["c-bad-json", "d-array", "e-null", "f-no-manifest"]);
        assert!(scan
            .errors
            .iter()
            .any(|e| e.reason.contains("must contain a JSON object")));
        assert!(scan.errors.iter().any(|e| e.reason.contains("invalid JSON")));
        // 扫描顺带标出 loading_order 顺序
        assert_eq!(scan.sorted_names(), vec!["b-good", "a-good"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_missing_dir_is_graceful() {
        let scan = list_extension_manifests(Path::new("/definitely/not/here/st_ext"));
        assert!(scan.dir_error.is_some());
        assert!(scan.manifests.is_empty());
        assert!(scan.errors.is_empty());
    }

    #[test]
    fn scan_empty_dir() {
        let root = tmpdir("empty");
        let scan = list_extension_manifests(&root);
        assert!(scan.dir_error.is_none());
        assert!(scan.manifests.is_empty());
        assert!(scan.errors.is_empty());
        assert!(scan.sorted_names().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_tags_warnings_with_extension_name() {
        let root = tmpdir("warn");
        write_ext(&root, "w", r#"{"display_name":"W","requires":"nope"}"#);
        let scan = list_extension_manifests(&root);
        let m = scan.get("w").unwrap();
        assert_eq!(m.warnings.len(), 1);
        assert_eq!(m.warnings[0].extension.as_deref(), Some("w"));
        assert_eq!(m.warnings[0].field, "requires");
        let _ = std::fs::remove_dir_all(&root);
    }
}