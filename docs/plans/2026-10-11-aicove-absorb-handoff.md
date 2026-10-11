# 交接:AIcove 吞噬(MVU 兼容层 + 主动关怀 P1 + 真相源审计)

> 日期:2026-10-11 | 执行:GLM(经 ZCode) | 基线:main@cc36665 | CI:test.yml(rust job 全量 `cargo test --workspace --locked`)

## 本提交内容

1. **st_mvu.rs(kaleido-core,新增)** — 酒馆 MVU(MagVarUpdate,MIT)变量系统"只解析、不执行"兼容层,吸收自 AIcove《MVU变量与表格记忆兼容方案》:
   - `[InitVar]` 世界书条目(含禁用条目)+ 开场白 `<initvar>` 建基线(深合并、`$meta`/`$__META_EXTENSIBLE__$` 清理、strictSet 识别)
   - 命令:`_.set/_.add/_.insert/_.assign/_.remove`(别名 unset/delete);值字面量仅引号字符串/数字四则(手写递归下降,拒绝任何 JS)/JSON 字面量
   - 块:`<UpdateVariable>` 提取/剥离(大小写不敏感、ASCII 安全定位)、`<JSONPatch>`(replace/add/remove/delta)、`<think>` 内块跳过、未闭合块防泄漏
   - 失败语义:逐条跳过+诊断,不中断回合、不假装成功;`card_mvu_precheck()` 卡片级只读预检
   - 22 个单测;零新依赖(只用 serde_json),`--locked`/cargo audit 无影响
2. **proactive_trigger.rs(kaleido-core,新增)** — 主动关怀 P1(吞噬自 AIcove 主动关怀系统):触发器状态机(Created→Prepared→Fired/Expired/Cancelled,非法迁移拒绝)、优先级作废纯函数(high 永不/medium 锚点动/low 锚点动或在线)、夜间静默(23:00-07:00 本地时区顺延 07:00,allow_night 旁路)、ProactiveStore(`$KALEIDO_DATA/proactive/{session_id}.json`)。5 个单测,零 tokio 依赖。
3. **ADR-0004(上下文真相源不变式)/ ADR-0005(MVU 只解析不执行,否决 JS 沙箱)** — 决策与理由。
4. **真相源审计(docs/plans/2026-10-11-truth-source-audit.md)** — 34 个 LLM 调用点定级(P/M/MO/C)+ TOP5 破口与修法。基线为内部 fa0a05c 快照,溯源说明见文内横幅;其中 #2(异步 save 竞态)已在本部 main 修复并标注。
5. **MVU 接线计划 / 主动关怀设计(docs/plans/2026-10-11-*)** — P1-P4 小步路线。

## 集成点(后续小步,均已有单测护栏)

- MVU P1:导入 handler 一行 `card_mvu_precheck(&card_json)` 进导入报告
- MVU P2/P3:回合循环落 `<UpdateVariable>` 块到消息 meta → 基线+重放建快照 → `{{get_message_variable::path}}` 宏注入
- 主动关怀 P2:kaleido-server 调度器 + SSE 送达(设计见 plans)

## 验证

- 模块开发期间在本机(fa0a05c 同源基线)实跑 `cargo test -p museai-core` 523/0(含全部新测试);本机已停编译(按 SOP),编译与全量回归以本仓 test.yml 的 `cargo test --workspace --locked` 为准。
