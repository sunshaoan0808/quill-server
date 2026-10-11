# MVU 兼容层接线计划(st_mvu.rs → 回合引擎)

> 日期:2026-10-11 | 模块本体已合入:`crates/kaleido-core/src/st_mvu.rs`(ADR-0005)
> 本文只排接线步骤;每步独立可验证,遵守 SOP AUDIT→GATE→CHANGE→VERIFY

## P1 导入侧预检(只读,不改行为)

- ✅ 已接线(2026-10-11 第三轮):`POST /api/v1/partner/st-import` 响应新增
  `"mvu": MvuPrecheckReport`(camelCase:hasMvu/initvarEntryCount/initFailed/strictSet/
  diagnosticCount/diagnostics)。取自 `bundle.card.raw_data`(v1 平卡为整卡、v2/v3 为
  data 对象,`card_mvu_precheck` 两者兼容),在 `persist_bundle` 消费 bundle 前计算。
  无 MVU 卡 `hasMvu=false` 零影响;单测覆盖三种卡形态。

## P2 回合输出侧落块

- ✅ 已接线(2026-10-11 第三轮):
  - `TavernMessage` 新增 `mvu_blocks: Vec<String>`(`#[serde(default,
    skip_serializing_if = "Vec::is_empty")]`,旧会话 JSON 零影响加载,空时不序列化;
    全部 21 处构造点已补 `Vec::new()`,serde 兼容回归测试在 core story_tavern p002)。
  - `turn/persist.rs::assemble_and_persist_turn`:全部剥离链(思维/自白/导演前缀)之后、
    消息组装之前,`extract_update_blocks` 提取 + `strip_update_blocks` 剥离——展示与
    后续喂模型的正文不含协议块,块原文落 `msg.mvuBlocks`;有块时 tracing::info 记数。
  - 已知 v1 缺口:swipes(重掷备选正文)不经此路径,不带块;event_extract 后台任务读
    `full_text` 原文仍会看到更新块(启发式提取噪音,低风险,后续统一)。
- 验收:带更新块的回复,历史正文干净、块落 `mvuBlocks`;旧会话加载零影响(回归测试)。

## P3 状态重放与提示词注入

- 每轮请求前:基线(导入时冻结,存 session)→ 沿消息链重放 `meta.mvu_blocks`(首版全量重放,500+ 消息实测 AIcove 170ms,量级无忧;prev_anchor/raw_hash 增量缓存后置)。
- 注入:`{{get_message_variable::path}}` 宏在提示词装配时展开(`format_variable_json`),每轮取一次快照,请求内固定。
- 验收:同一会话两次重建 stat_data 结果一致;宏展开进 system prompt。

## P4 与 actor_states 的关系(需要拍板)

- 短期并存:actor_states(手动属性面板)与 MVU stat_data(卡片变量)互不覆盖,面板只读展示 MVU。
- 长期:是否合并由 TavernWeave P4 决策;MVU 的 `$meta.strictSet` 语义(强设值校验)届时一并评估。

## 红线

- 每步不碰 X3 红线文件之外的回合语义;st_mvu.rs 保持纯函数零依赖。
- MVU 关闭路径:会话/包级开关缺省开(对齐 AIcove 放预设世界书页),关闭时 P2/P3 整链旁路。
