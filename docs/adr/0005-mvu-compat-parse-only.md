# 酒馆 MVU 兼容层采用"只解析、不执行"，不做 JS 沙箱

（吸收自 AIcove《MVU变量与表格记忆兼容方案》，2026-10-11）

决定：对 ST 生态 MVU（MagVarUpdate，MIT）做**原生命令解释器**兼容：解析 `[InitVar]` 世界书条目（含禁用条目）与开场白 `<initvar>` 块建立 stat_data 基线，支持 `_.set/_.add/_.insert/_.assign/_.remove`（别名 unset/delete）与 `<UpdateVariable>`/`<JSONPatch>`（replace/add/remove/delta）块，值字面量仅引号字符串/数字四则/JSON；模型输出中的更新块逐条落为消息级快照，状态 = 基线 + 沿持久化消息链重放（服务 ADR-0004）。实现于 `crates/kaleido-core/src/st_mvu.rs`（纯函数、零新依赖、22 单测）。

否决方案：内嵌 QuickJS/boa 执行卡片脚本。理由：(1) 酒馆助手 JS-Slash-Runner 为 PolyForm NonCommercial 且从 CDN 拉库、可读密钥，与红线"不引入 ST 运行时依赖"冲突；(2) AIcove 已实测"只解析不执行"覆盖 MVU 主流用法，模板类/EJS 检测到即诊断跳过；(3) 零 C 依赖让 CI/本地工具链无额外要求。

失败语义：逐条跳过 + 诊断（`unknown_command`/`unsupported_expr`/`bad_init_json`/`ejs_skipped` 等），不中断回合、不假装成功；`init_failed` 表示基线不可信。接线计划（st_import 识别、server 回合循环落块、提示词宏 `{{get_message_variable::path}}`）见 `docs/plans/2026-10-11-mvu-wiring.md`。
