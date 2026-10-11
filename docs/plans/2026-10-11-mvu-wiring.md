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

- ✅ 已接线(2026-10-11 第三轮):
  - core st_mvu 新增 `init_sources_from_lore_entries` / `is_mvu_active` /
    `snapshot_stat_data` / `expand_variable_macros` / `expand_mvu_macros` 一组纯函数;
    `is_initvar_entry` 改零分配 ASCII-CI 匹配(护住提示词装配热路径)。
  - 接线点:`build_tavern_system_prompt` 返回前(p004/prompt.rs)——活性检测未命中
    (无 `[InitVar]` 条目且无消息带块)原样返回,热路径零开销;命中时基线+沿落盘
    消息链重放 → 按本轮快照展开 `{{get_message_variable::path}}`/`{{format_message_variable::path}}`
    (后者 v1 同样输出 JSON,YAML 为已知缺口),请求内固定。世界书条目内容经
    最终整串展开自然覆盖。
  - 已知 v1 缺口:开场白 `<initvar>` 覆盖未接(多数卡 init 在世界书);prev_anchor/
    raw_hash 增量缓存未做(全量重放,量级无忧);`{{tableData}}` 表格记忆(B2)未做。
- 验收:同一会话两次重建 stat_data 结果一致(确定性测试);宏大小写不敏感展开;
  非 MVU 会话字节级不变(直通测试);4 个 P3 单测入 core st_mvu。

## P4 与 actor_states 的关系(需要拍板)

- 短期并存:actor_states(手动属性面板)与 MVU stat_data(卡片变量)互不覆盖,面板只读展示 MVU。
- 长期:是否合并由 TavernWeave P4 决策;MVU 的 `$meta.strictSet` 语义(强设值校验)届时一并评估。

## 红线

- 每步不碰 X3 红线文件之外的回合语义;st_mvu.rs 保持纯函数零依赖。
- MVU 关闭路径:会话/包级开关缺省开(对齐 AIcove 放预设世界书页),关闭时 P2/P3 整链旁路。
