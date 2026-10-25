# museai-server → kaleido 迁移计划（高价值清单）— 2026-10-07

> 来源：对标 museai-server（私有，2026-10-23，未重构原版血统，领先 kaleido 分叉 12 天）。
> 候选合计约 2.8 万行。**迁移 = 适配不是复制**：museai 代码基于未拆分的
> museai_core 单体 API，进 kaleido 需按 kaleido_core 命名与模块边界适配；
> 每批次独立编译轮 + 四重门禁 + 双仓同步（私有先上、公开 cherry-pick）。
> 对标快照：`Downloads/museai-compare`（浅克隆 f152b7c）。

## M1 图体系补全 【高价值 · 低风险 · ~900 行 · 建议首迁】

| 模块 | 行数 | 价值 |
|---|---|---|
| examples/graph_audit.rs | 254 | 只读审计 CLI：逐表 dump + 7维/delta 取证——立刻验证 kaleido 图数据流是否真通 |
| examples/memory_backfill.rs | 127 | 记忆回填工具 |
| examples/sidecar_import_cli.rs | 34 | sidecar 增量入图入口（museai 最新提交主题） |
| inject_router.rs | 519 | 注入通道化：把写死的 preload_asset_blocks 变成可配置注入路由（graph_memory 等 5+ 通道），对齐 ST 的可插拔注入 |

依赖：无新增 crate。M1 完成后图记忆链路可实测（回应 P2-5 遗留）。

## M2 认知/关系深化 【高价值 · 中成本 · ~3900 行】

| 模块 | 行数 | 价值 |
|---|---|---|
| relationship_stage.rs | 981 | 关系阶段状态机（阶段化推进，而非字符串描述） |
| intimacy2.rs | 674 | 亲密度第二代模型 |
| epistemic.rs | 565 | 认知状态（谁知道什么——防全知 NPC） |
| agency.rs | 649 | 角色自主性 |
| proactive_trigger.rs | 458 | 主动触发（NPC 先行动） |
| pending_memory.rs + pending_store.rs | 567 | 待定记忆暂存 |

依赖：相互关联，建议整批迁；下沉到 kaleido_core。

## M3 图原生记忆 + 时序图谱 【最大件 · ~9300 行 · 单独一轮】

| 模块 | 行数 |
|---|---|
| memory_graph.rs（SurrealDB 双写影子底座，feature=graph-memory） | 543 |
| temporal/turn_knowledge.rs | 2298 |
| temporal/graph_context.rs | 1467 |
| temporal_api.rs | 1537 |
| temporal/worldline_fork.rs（世界线分叉） | 574 |
| temporal/snapshot.rs | 632 |
| temporal/scene_context.rs | 221 |
| temporal/worldline.rs | 122 |

依赖：surrealdb crate；配套测试 graph_context/graph_context_d4/l4_graph_mirror/snapshot_chapter 一起迁。
先决：M1 的审计 CLI 先跑通 kaleido 图流，并用 R8 flamegraph 确认收益点。

## M4 ST 生态对齐 【高用户价值 · ~9500 行 · 拉新钩子】

| 模块 | 行数 | 价值 |
|---|---|---|
| st_preset + store + order + sampling | 2427 | ST 式预设管理器（提示词预设/排序/采样） |
| st_plugin_loader + st_plugin_examples + st_ext_manifest | 2862 | 用户插件系统（ST 生态护城河的对位件；kaleido 有 MCP 但无用户级扩展） |
| st_quick_reply.rs | 1182 | 快捷回复 |
| st_group.rs | 934 | 群聊（多角色同场） |
| st_chat_import.rs | 532 | ST 聊天记录导入（存量用户迁移通道） |
| st_mvu.rs | 1534 | MVU 变量框架（ST MagVarUpdate 对位） |

依赖：preset 依赖 stable_seq；plugin_loader 需配前端面板（前端工作量另计）。

## M5 质量卫生基建 【低成本 · 随任意批次顺路】

text_guards.rs (312) / format_health.rs (438) / stable_seq.rs (215) / voice2.rs (142)

## M6 媒体与云 provider 【按需 · ~5100 行】

media_services (1054) / st_media_config (1487) / model_catalog (523) /
auth_vertex (1160) / auth_aws (377) / workers_ai (501)
——仅在需要对应云厂商（Vertex/AWS/Cloudflare Workers AI）时迁。

## 明确不迁

- museai 的未拆分巨石本体（story_tavern.rs 等）——逻辑已被 kaleido 拆分版覆盖或按上述批次择优移植；
- museai 旧命名层（MUSEAI_* 兼容代码 kaleido 已有等价物）；
- 与 kaleido 已有实现重复且 kaleido 版本更新的部分（e2e 测试、turn resume、A1111 通道等）。

## 执行纪律

1. 每批次 = 独立编译轮：移植 → 适配 kaleido_core → 四重门禁 → 双仓同步。
2. 禁止 cargo fix；模块进 kaleido 时同步迁配套测试。
3. 迁移顺序建议：M1 → M2 → M4 → M3 → M6（价值/成本比递减，M3 需专项决策）。
4. 双仓：私有先合，公开仓按里程碑 cherry-pick。
