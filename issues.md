# Issues

## 2026-10-11 — AIcove 首批移动同步切片

- 现象：移动端会话与 app-state 原先走全局文件，无法安全支撑多用户薄客户端同步。
- 病根：`AgentSessionStore` 与 `AppStateStore` 缺少按认证 user scope 的读写入口；移动路由虽有 Bearer 鉴权，但数据访问仍未绑定用户。
- 修法：新增 per-user 会话目录与 app-state 路径；`/api/mobile/sessions*`、`/api/v1/agent/sessions*`、`/api/mobile/state*` 与 sync manifest/export 统一走认证用户 scope；保存时写入并校验 `owner_user_id`；导出递归脱敏 API key/token/password/secret/credentials。
- 验证：`cargo check -p kaleido-server` 通过；后续 owner-scope 回归为 574 passed / 1 failed（`pack_owner_scope_rejects_owner_mismatch_and_ownerless_legacy`，`save_for_owner` 未把新 pack 的 owner 写回返回值）；`cargo test -p kaleido-server --bin kaleido-server` 另有测试 fixture 缺少 `owner_user_id`；本轮未重启生产服务。
