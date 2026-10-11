# 上下文真相源审计(build fa0a05c 快照,2026-10-11)

> 溯源说明:本审计在内部开发线的 fa0a05c 快照上完成,行号/路径均为当时基线。开源 kaleido(quill-server)为同一代码库改名:crates/museai-core → crates/kaleido-core,$MUSEAI_DATA → $KALEIDO_DATA(后者兼容回退),模块名未变,按符号名定位。其中 TOP5 #2(异步 save 竞态)核对确认已在本部 main 修复,勿重复施工。


> 配套 ADR-0004。逐调用点判定:P=PERSISTED(可溯源)/ M=MIXED / MO=MEMORY-ONLY / C=CLIENT-SENT(前端回传)。
> 行号基于本快照,合入主仓库后按符号名定位。

## 0. 总体结论

主线 Tavern 回合的上下文架构**基本符合不变式**:`start_turn` 只发 `[system, user]` 两条(story_tavern.rs:7366-7378),全部历史/记忆/状态烘焙进 system prompt,每个来源都可溯源(session JSON、pack.json+chapters/*.md、plot.sqlite、cross-session/memory.json、worldline.json、ledger.json、`state/wi-vector-index/sess-*.json`);玩家消息先落盘再调 LLM(6827-6838→7261-7263)。
但存在下述 TOP 5 破口,集中在**客户端覆写、异步 save 竞态、纯内存状态、压缩无归档、回滚不回滚记忆**五类。

## 1. 调用点清单

| # | 位置 | 用途 | 来源 | 备注 |
|---|---|---|---|---|
| 1 | story_tavern.rs 7366-7451 | 主线回合正文流式 | **P** | sys_prompt 由 build_tavern_system_prompt(3937-4859)组装:L0 近 12 条/L2-L4/actor_states/chapter_diaries/director_plan/game_clock ← session JSON;角色/lore/locked_beats ← pack.json;章节摘录 6000 字 ← chapters/*.md;伏笔 ← plot.sqlite;跨会话 ← memory.json;S7 召回 ← 向量索引;worldline ← worldline.json |
| 2 | story_tavern.rs 7842-7847 | 首回合场景纠偏重试 | **P** | 复用同一 sys_prompt |
| 3 | story_tavern.rs 561-666(调 8002) | standard/heavy 质量管道 | **P** | heavy MemoryPatch 回写 actor_states(8018-8027,get→save 无 CAS) |
| 4 | story_tavern.rs 8606(spawn 8605) | TurnEnd roster LLM 兜底识别 | **P** | **异步 save() 无 CAS**,与主流程 9431 save 竞态 |
| 5 | story_tavern.rs 8729(spawn 8728) | 每 3 回合 L2/L3 抽取 | **P** | 输入近 6 条持久化消息;**异步 save() 无 CAS** |
| 6 | story_tavern.rs 6982/8770/9008 | embedding | **P** | 文本来自 session 消息与事件账本 |
| 7 | story_tavern.rs 8939 | 游戏时钟剧情评估 | **P** | |
| 8 | story_tavern.rs 9199(spawn 9198) | epoch 压缩 LLM 摘要 | **P** | **异步 save() 无 CAS** |
| 9 | story_tavern.rs 9324/9342 | L4 情感层提炼 | **P** | 同步落盘 |
| 10 | story_tavern.rs 9395 | 章节摘要 diary | **P** | |
| 11 | story_tavern.rs 9805(含 9904) | 开局导演计划+因果推演 | **P** | CAS 落盘 6890 |
| 12 | story_tavern.rs 2371(assistant_chat) | 会话内剧情助手 | **M** | 剧情 ctx 持久化;**助手历史 body.history 为 C**(2358-2369) |
| 13 | story_tavern.rs 2485 | 冒险/跑团助手 | **C** | 注释自认走 localStorage(2424-2425),仅世界书向量检索在服务端 |
| 14 | story_tavern.rs 11288 | 角色记忆分析 | **P** | |
| 15 | story_tavern.rs 11415 | 记忆去重优化 | **P** | |
| 16 | story_tavern.rs 11621/11652 | 手动压缩 | **P** | **直接丢消息不归档 S7**(11673-11674),与 epoch 压缩不一致 |
| 17 | story_tavern.rs 11766 | 支线摘要 | **P** | |
| 18 | story_tavern.rs 10766 | 文风分析 | **C** | body.source_text 前端传入;产物落盘 |
| 19 | story_tavern.rs 11044/11109 | 角色档案分析/精编 | **M/P** | 无 session 时用 body.recent_messages(C) |
| 20 | story_tavern.rs 10845 | 公共非流式 helper | — | |
| 21 | main.rs 3230-3943 | ST 兼容伴侣/对话 chat | **M+MO** | transcript C(req.messages);WI/角色卡/正则库 P;timedWorldInfo P;**world_state 是内存 HashMap(299-300/4317)= MO,重启即空** |
| 22 | dual_agent.rs 1197/1428/1498/1531/1774/1971/3255/3278 | 双 Agent 写作流水线 | **P** | DualAgentSession 全量落盘 |
| 23 | harness_bridge.rs 99 | harness refine 协议层 | **P** | |
| 24 | crawler.rs 2482 | 自动角色谱抽取 | **P** | |
| 25 | convert.rs 898/1045/1092/1267/1346/2298 | 蒸馏管线 | **P** | 诊断文件 911-919 写 CWD,违反 $MUSEAI_DATA 约定(小问题) |
| 26 | review_tavern.rs 116/210 | 审稿/修复复核 | **C** | 审稿对象 body.content;审稿历史持久化 |
| 27 | agent_sessions.rs 652 | Agent 会话一轮 | **P** | |
| 28 | analysis.rs 705/779 | 作品分析任务 | **P** | |
| 29 | moa_api.rs 72 | MoA 多模型对比 | **C** | body.prompt |
| 30 | deai.rs 92/185/230 | 去 AI 味/标题/记忆分析 | **C** | body 传入 |
| 31 | outline.rs 566 | 逆大纲润色 | **P** | |
| 32 | background.rs 1353 | 背景设定生成 | **C**(任务输入) | 产物落 pack |
| 33 | book_travel.rs 256/1029 | 书旅分类/规划 | **C**(任务输入) | 快照落 works |
| 34 | main.rs 748 / llm_test.rs 109 | embedding 网关/探针 | **C** | 工具端点 |

C 类里 #26/#29/#30/#32/#33 属"工具型端点"(用户显式提交的待处理文本),不属违反;**真违反是 #12/#13(助手历史)、#19(无 session 分支)、#21(world_state)**。

## 2. 持久化事实

- 会话 `tavern-session-{uuid}.json` 全文内嵌;`get/save` 全局 Mutex 但 **save() 无版本 CAS**(4207-4208),CAS 仅 save_with_revision(4636)。
- checkpoint 每回合 push(core 5020-5065),messages_snapshot 上限 8、cap 30。
- S7 向量索引/timedWorldInfo 均已落盘,无内存态风险。

## 3. TOP 5(按危害排序)

1. **PUT save_session 允许客户端整体覆写会话**(story_tavern.rs:1166-1210):前端回传的 TavernSession 直接 `*existing = body_session`,messages/memory_l1-l4/actor_states/chapter_diaries/director_plan 全可被前端重建后喂给 LLM。修法:字段白名单合并(仅 title/present_character_ids/focus 等 UI 可编辑字段),历史与记忆层服务端权威。
2. **start_turn 三个异步任务 save() 与主流程 save 竞态**(roster 8605→8677、抽取 8728→8778、epoch 压缩 9198→9210):均无 CAS,主流程 9431 save 前还要 await 三次 LLM(最长 120s×3);交错时记忆层被旧快照覆盖=整回合记忆回滚。修法:统一改 core 的 update_session(4657,锁内 read-modify-write)。
3. **start_chat_from_body 的 world_state 纯内存**(main.rs:299-300/4317,写 3363,注入 3587-3600):重启即丢、多端不一致,直接违反不变式。修法:落 `$DATA/state/world-state/{workspace}.json`(仿 TimedWorldInfoStore)。
4. **两条压缩路径不一致 + 摘要无溯源**:手动 compact_session(11673-11674)直接丢消息、不归档 S7、无 checkpoint——压缩后早期剧情在向量召回中永久消失;apply_memory_compression(core tavern_engine.rs:1006-1038)替换 L1/砍 L2-L3 无归档。修法:手动压缩复用 epoch 归档链路,压缩写 epoch 边界(消息 id 区间→摘要)。
5. **reroll/rewind 后记忆层不同步**(core restore_checkpoint 5070-5090):只回滚 turn/messages/L1/L2/panels/director_plan,不回滚 L3/L4/actor_states/chapter_diaries/game_clock/check_history——回退后 system prompt 仍注入"未来"状态(已回滚的表白/检定后果仍在情感块)。修法:checkpoint 增补上述字段,restore 一并恢复。

次级:KEEP_RECENT_MESSAGES=12 窗口 + S7 只在压缩时归档 → 两次压缩之间的早期消息无向量索引(召回缺口);#12 助手 CLIENT-SENT 历史与服务端上下文混流;convert.rs:911 诊断写 CWD。
