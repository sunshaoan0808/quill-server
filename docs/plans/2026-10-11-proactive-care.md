# 主动关怀系统设计(吞噬自 AIcove 主动关怀系统)

> 日期:2026-10-11 | 状态:设计稿,未实现 | 参考:AIcove docs/04_功能模块规范/主动关怀系统/01-05
> 红线:不改变回合语义;所有上下文重建遵守 ADR-0004;先纯后端,Web Push 后置

## 1. 产品语义(借鉴部分)

角色在用户静默期主动发起一条消息("主动想你"),三种触发:

| 触发源 | 优先级 | 说明 |
|---|---|---|
| 用户显式要求(对话中让角色"明早叫我") | high | 永不作废;对齐 AIcove `create_reminder` |
| 管家自动分析(对话结束 N 分钟无新消息) | medium/low | LLM 分析"是否值得主动+说什么",对齐 `schedule_proactive` |
| 会话钩子(伏笔到期/导演计划 due) | medium | MuseAI 特有:复用 director_due/opening_plan_due 机制 |

## 2. 触发器模型(新增 `museai-core/src/proactive_trigger.rs`,纯模型+JSON store)

```
ProactiveTrigger {
  id, session_id,
  state: created → prepared → fired | expired | cancelled,
  priority: high | medium | low,
  fire_at: DateTime,            // 到期时间
  allow_night: bool = false,    // 23:00-07:00 静默(服务器本地时区)
  prompt: String,               // 主动消息的系统提示(Proactive Agent)
  cached_content: Option,       // 预生成结果
  anchor: { last_user_message_id, last_user_message_at },  // 作废比对基准(真相源锚点)
  created_at, fired_at,
}
存储:$KALEIDO_DATA/proactive/{session_id}.json(触发器数组,随目录布局已预留)
```

## 3. 生命周期

1. **created**:创建即投预生成 job(jobs v2 `kind=agent`,独立分析上下文;调度器与分析器分离——分析器只出决策,副作用归调度器,对齐 AIcove 已踩过的坑)。
2. **prepared**:cached_content 就绪;到期由调度器发火。
3. **fired**:把预生成内容(或实时补生成)落为正常 assistant 消息 + SSE `proactive_message` 事件;记审计。
4. **expired/cancelled**:到期但被作废规则拦截 → expired(留审计);用户显式取消 → cancelled。

## 4. 作废与防打扰(对齐 AIcove 优先级语义)

- high:永不作废。
- medium:锚点比对——最后一条**用户**消息的 id/at 变了(用户来过新消息)→ expired。
- low:锚点变化 **或** 用户在线(该会话有活跃 SSE 订阅)→ expired。
- 用户发消息时同步取消该会话全部 pending 的 low(同步作废,不发通知)。
- 夜间静默 23:00-07:00:`allow_night=false` 的触发器顺延到早晨 07:00 发火。
- 全局上限:每日每会话 proactive ≤ 2 条;触发后该会话进入冷却期(默认 6h,settings-store 可调)。

## 5. 服务端架构(kaleido-server (quill)/src/proactive.rs)

- `AnalyzerScheduler`:tokio interval(60s)扫描 `$DATA/proactive/`,对 state=created/prepared 且到期者按第 4 节判定。
- 上下文构建:从 tavern-session 落盘消息**重建**(ADR-0004),独立快照,不复用回合内存态;预生成与分析各用独立 prompt(会话隔离:聊天连续上下文 / 管家一次性 / 预生成 per-trigger,对齐 AIcove 三 session 隔离)。
- 送达:SSE 事件 + 消息落库用现有 TavernMessage 通道;前端零改动即可收(消息自然出现),后续可加"主动消息"角标。
- Web Push(VAPID):后置到移动端阶段;桌面 Web 用 SSE 足够。

## 6. 阶段划分(按 SOP 小步)

- ✅ P1(2026-10-11):core 触发器模型 + store + 单测(纯函数,零接线)。
- ✅ P2(2026-10-11 第三轮):server 调度器 + 发火落库 + 手动 API。
  - `crates/kaleido-server/src/proactive.rs`(新模块):
    `POST/GET /api/v1/story-tavern/sessions/{id}/proactive`、
    `DELETE .../proactive/{trigger_id}`(high 优先级显式提醒,create 时锚点取
    最后一条 user 消息 id);`spawn_scheduler`(main 启动拉起,45s 冷启动 + 60s 扫描)。
  - 发火:`cachedContent` 优先,否则 `call_llm_nonstream` 角色化生成;LLM 失败
    fireAt 顺延 5 分钟有界重试。落库走 `update_session` CAS 原子写(ADR-0004),
    协议块按回合 P2 同口径剥离落 `mvuBlocks`。
  - 审计:`$KALEIDO_DATA/audit/proactive.log`(create/fire/expire/cancel)。
  - v2 缺口(记入 P3):SSE 推送(StreamHub 是 run_id 键,无常驻会话通道,消息
    以落库为准)、medium/low 管家自动分析、冷却期、在线探测(v1 恒 false,low 仅
    靠锚点作废)。
- P3:medium/low 管家分析 + 冷却期 + SSE 推送。
- P4:对话工具 `create_reminder`(接入现有 tool 通道)。

## 7. 验收

- 显式提醒:创建→预生成→到期→落库+PUSH 全链路;重启进程后触发器从磁盘恢复。
- 作废:medium 触发器在用户来新消息后不发送。
- 静默:23:00 创建的 low 触发器 fire_at 顺延到 07:00。
- 审计:$DATA/audit/proactive.log 可追溯每次发火/作废原因。
