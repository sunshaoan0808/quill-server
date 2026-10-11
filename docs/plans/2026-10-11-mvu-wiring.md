# MVU 兼容层接线计划(st_mvu.rs → 回合引擎)

> 日期:2026-10-11 | 模块本体已合入:`crates/kaleido-core/src/st_mvu.rs`(ADR-0005)
> 本文只排接线步骤;每步独立可验证,遵守 SOP AUDIT→GATE→CHANGE→VERIFY

## P1 导入侧预检(只读,不改行为)

- ✅ 已落地(2026-10-11 第二轮):`st_mvu::card_mvu_precheck(&Value) -> MvuPrecheckReport`
  ——对 v1 平卡/v2/v3 信封的解析后卡片 JSON 做只读预检([InitVar] 条目数、基线空跑
  诊断 top5、开场白含 `<initvar>`/`<UpdateVariable>` 标记,alternate_greetings 一并扫),
  camelCase 可直接进导入报告 JSON。**有意不接 StImportBundle**(跨 crate 结构体,
  加字段波及全部构造点);接线 = 导入 handler 一行 `let mvu = st_mvu::card_mvu_precheck(&card_json);`,
  作为独立小 patch。
- 验收:带 MVU 的测试卡导入出报告;无 MVU 卡零影响。(单测已覆盖三种卡形态)

## P2 回合输出侧落块

- server 回合循环:模型回复落库前,`extract_update_blocks` 提取更新块 → 存入消息 meta(如 `message.meta.mvu_blocks`);`strip_update_blocks` 后的文本进展示/历史。
- `<think>` 内块天然被 extract 跳过。
- 验收:带更新块的回复,历史里正文干净、块在 meta 可查;turn 回放不丢。

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
