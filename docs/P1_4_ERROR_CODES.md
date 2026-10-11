# P1-4 统一错误码 — 实施记录

## 契约
`{"error": <人类可读>, "code": <STABLE_CODE>, ...details}`（details: object 合并 / 标量嵌套 / Null 省略）
源头：`error_codes.rs::err_with_code` + wrappers（bad_request/not_found/unauthorized/forbidden/conflict/internal/service_unavailable/too_many_requests/unprocessable/bad_gateway）。`error_map.rs`(map_core_err) 为 CoreError 路径契约源头，内部字面量保留。

## 迁移轨迹（commits）
| 步 | commit | 文件 | 迁移 |
|---|---|---|---|
| S1 | fb56f29 | error_codes.rs 新建 + main.rs mod | helpers+4 测试 |
| S2 | ded258c+569e362+2423d89 | author.rs | 29+1→0, AUTHOR_*×16 |
| S3 | f50cb71 | dual_agent.rs | 26+1→0, DUAL_*×22 |
| S4 | 4cf9a18 | routes_partner.rs + service_unavailable helper | 27→0 |
| S5 | 16df187 | story_tavern.rs + 3 helpers | 42→0, ST_*×~20 |
| S6 | 2ddfaae | 33 文件批量 | ~230 处 |
| S7 | 565a4e3 | kaleido_tools/harness_api/story_tavern 裸 Json | 全归零 |
| S8 | 42a64f1 | api_shell.js friendlyError code 白名单 ×9 |

**判据达成**：`rg -U 'StatusCode::…Json\(json!\(\{?\s*"error"' ` 仅剩 error_map.rs（8 处，契约源头）。
裸 `Json(json!({"error"}))`（HTTP200 语义）全部消除——前端 api() 对 !ok 抛错路径文案不变。

## 主要域码注册表（节选，完整见各文件 grep `"CODE"`）
- AUTHOR_*(16) BAD_ID/INDEX_CORRUPT/SERIALIZE/NOT_FOUND/SESSION_REQUIRED/BAD_PLAYABLE/COMPOSE_EMPTY/SOURCE_DOC_READ/LAUNCH_ARITY/PATH_REQUIRED/PATH_ESCAPES/BAD_KIND/NO_PACK/PACK_EMPTY/CHAPTER_MISSING/WB_TARGET_MISSING
- DUAL_*(22) BAD_ID/CREATE_DIR/SERIALIZE/WRITE/COMMIT/NOT_FOUND/CORRUPT/FORBIDDEN/WORKID_REQUIRED/NO_PLAN/EMPTY_MESSAGE/EMPTY_CONTENT/NO_HANDOFF/WINDOW_NOT_FOUND/NOT_WRITTEN/WORKSPACE_ROOT/CREATE_WORK_DIR/PATH_ESCAPE/WRITE_FILE/PENDING_WINDOWS/BAD_STATUS/BAD_STAGE
- ST_*(~20) TURN_BUSY/CONCURRENT_WRITE/RUN_FORBIDDEN/DIRECTOR_BUSY/LLM_NOT_CONFIGURED/SERVER_BUSY(+runId,retryable)/ACTOR_NOT_FOUND/ACTOR_NO_ARCHIVE/CHAR_NOT_IN_PACK/NO_VALID_UPDATES/CONTEXT_REQUIRED/CONTENT_REQUIRED/RUNID_REQUIRED/DIFF_PARSE/REFINE_PARSE/OPT_PARSE/DIRECTOR_CONFIG/UPSTREAM_LLM/LLM_CALL_FAILED/ASSISTANT_LLM_FAILED/EMPTY_MESSAGE
- 通用 RATE_LIMITED(429,+retryAfterSecs)、BAD_JSON、BAD_ENTRIES_BODY、BAD_REGEX_IMPORT、EMBED_FAIL、EMBED_UNAVAILABLE、EMBED_MISMATCH、CONFIRM_REQUIRED、AGENT_WRITE_DISABLED、HARNESS_*(按状态)、TOOLS_*(TTS/ASR 链路)
- 长尾域前缀：SKILL_/BG_/IMG_/BT_/VER_/FS_/NOVEL_/JOB_/AN_/CHAT_/MOA_/APPEAR_/WKX_/REV_/CARD_/GRAPH_/STX_/EMB_/ATOOL_/UAS_/OUT_/DLG_/DEAI_/TODO_/STYLE_/WK_/AUTHEP_/TICKET_/STI_/PANEL_/DEC_/CRAWL_/CHPIPE_/ASESS_

## 教训（本轮新增）
- (m) 分段正则通过 ≠ 整体通过；平衡括号 `(?:[^()]|\([^()]*\))*`
- (n) raw 计数必须 rg -U 双形态（单行+多行），否则漏检
- (o) 正则 extra-keys 组误吞值文本 → json!({msg}) 垃圾；脚本生成必须编译闭环
- (p) Result<_,Response> 函数中是 Err(helper(..)) 不是 return；format-string 模板会产生 json!({{}})
- (q) match 臂内改写后尾随符需区分 `;`（语句）与 `,`（臂分隔）

## 验证
- cargo build --release ✓；cargo test error_codes ✓（envelope/details/status/null 四组）
- s4_gate :18982/:18983/:18984 全 PASS（fresh KALEIDO_DATA，用毕即焚）
- 实测探针带 code：AUTHOR_NOT_FOUND/DUAL_BAD_ID/DUAL_WORKID_REQUIRED/DUAL_BAD_STAGE(+validStages)/DUAL_BAD_STATUS(+valid)/DUAL_NO_PLAN/DUAL_NO_HANDOFF/PARTNER_BAD_STATE/JOB_NOT_FOUND/HARNESS_NOT_FOUND
- 前端 battery @42a64f1: routes 109=109 wins 61 CJK 9693 len +~700B（friendlyError 分支）
