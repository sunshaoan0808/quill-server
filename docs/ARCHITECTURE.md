# ARCHITECTURE — Kaleido

> 面向公开版的一页架构图 + 依赖方向规则。工程流水账见 docs/ 其余文档（分桶见 DOC_BUCKETS_2026-10-07.md）。

## 总览

Kaleido 是一个"故事酒馆"（story tavern）引擎：上传剧本包（Pack）→ 开档（Session）→
以 LLM 流式回合驱动沉浸式叙事，附带角色卡蒸馏、世界书、记忆系统与阅读端。

```
kaleido-main/
├── crates/
│   ├── kaleido-core/      # 领域层：全部 Store（SQLite/JSON 持久化）+ 纯函数域逻辑
│   ├── kaleido-server/    # HTTP 层：axum 路由、回合编排、LLM 流式管道、蒸馏管线
│   └── kaleido-harness/   # 自动化验收工作台
├── web/                   # 前端：vanilla JS SPA（Vite 构建，无框架）
├── desktop/               # 可选 Tauri 桌面壳
└── scripts/               # e2e（Python）与 CI 守卫脚本
```

## 服务端模块地图（kaleido-server/src）

| 模块 | 职责 |
|---|---|
| `main.rs` | 组装：~70 个路由模块声明 + AppState 装配 |
| `state.rs` | AppState（~40 个 store 字段）+ StreamHub（SSE 广播/重放/取消） |
| `story_tavern/` | 酒馆 API 门面：`mod parts; pub use parts::*` |
| `story_tavern/parts/p001..p010/` | 语义部件：stores/契约/蒸馏/解析等（p006 已退役至 dual_agent 语义拆分） |
| `story_tavern/parts/turn/` | **回合编排内核**（15 文件） |
| `story_tavern/parts/protocol/` | 协议层纯函数：markers（8 个标记解析器）、receipt（回执协议） |
| `crawler/parts/p001..p004/` | 爬虫语义拆分 |
| `llm_stream.rs` | LLM 流式客户端（OpenAI/Anthropic/Google 分派、思维链、usage、.no_proxy 显式禁系统代理） |
| `error_codes.rs` | 语义错误码体系（ST_*） |

### turn/ 回合编排内核（依赖方向受 R7 断言保护）

```
entry.rs ── start_turn 外壳：幂等守卫 → U11 resume → 原子锁 acquire_turn
   │        → 建 job → hub.register → spawn worker → panic 看门狗
   ├─ ctx.rs           TurnCtx 20 字段参数包（borrow checker 友好）
   ├─ pre_refine.rs    回合前精炼（与 post_parse 互不引用，经 entry 传数据）
   ├─ stream.rs        LLM 流消费 / 取消 / 空响应重试
   ├─ post_parse.rs    回合后解析（director plan / state updates / receipt…）
   ├─ memory.rs        记忆抽取与 epoch 压缩
   ├─ persist.rs       落盘与快照
   ├─ checks.rs / clock.rs / director_exec.rs
   └─ 叶子：leak_strip / bg_llm / session_repair / prompt_ctx（不 import 兄弟模块）
```

**规则（CI 强制，scripts/check-dep-direction.py）：**
1. 部件模块不得反向引用 `entry::`（entry 只能被调用）；
2. 叶子模块不得显式 import 兄弟模块（数据经 `use super::super::*` glob 桥接传入）；
3. `pre_refine` 与 `post_parse` 互不引用。

## 关键机制

- **StreamHub**（state.rs）：run_id → broadcast sender + 事件重放历史（断线重连）+
  cancelled 标志。取消语义：`cancel()` 发 STOPPED 后即 cleanup；worker 侧 send 失败
  （无接收者 = 客户端断连）或 is_cancelled → 保守取消收尾。
- **JobStore v2**：running 并发上限、queued 提升槽位、`rearm_running` + cancel-wins
  终态（U11 resume：孤儿 running job 自动收尾放行新回合）。
- **幂等守卫**：同一回合同内容重复提交 → `accepted:false + duplicate_submit`，无二次 LLM 副作用。
- **P3 panic 看门狗**：回合 worker panic → job failed + 会话锁释放 + hub 清理。
- **依赖锁定**：Cargo.lock 入库，CI `cargo metadata --locked` 快检 + `cargo test --locked`。

## 测试与 CI 门禁

- `test.yml` 四 job：rust（全量 `--locked` + 锁快检）/ frontend（Vite 构建 + UI 初始化守卫）/
  scripts（pycompile + 依赖方向断言）/ security（cargo audit，RustSec 即红）。
- 回合编排行为测试：`turn_orch_tests.rs`（幂等 / resume / 409 护栏 / panic 看门狗 / 断连收尾），
  伪 LLM 为本地 TCP SSE mock，不依赖网络。
- 本地四重门禁（windows-gnu）：insert-ort shim → warnings 0 → no-run 0 err → 全量测试 →
  remove-ort + 还原 Cargo.lock（ort-sys 在 windows-gnu 不可编译，shim 仅限本地门禁，禁止带入 CI）。

## 数据流（单回合）

```
POST /st/sessions/:id/turn
  → 幂等守卫（内容 hash + assistant 已回应）
  → active_run_id 原子锁（pending-N）
  → jobs.create（并发上限，超限 429 ST_SERVER_BUSY）
  → hub.register + tokio::spawn worker
  → TurnCtx 装包 → pre_refine → LLM SSE 流（hub.send 广播 + 重放持久化）
  → post_parse（协议解析：director plan / state / receipt）→ checks → memory → persist
  → jobs.finish + clear_session_active_run + hub.cleanup
前端 GET /st/turns/:run/stream （SSE，断线经重放历史恢复）
```
