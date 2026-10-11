# fixtures 说明
`ls_real.json` 是真实用户 localStorage 快照(含 token),**禁止入库**。
本地生成:从 CDP 9222 现有浏览器 dump localStorage(keys 含 kaleido_token/kaleido_session_id/kaleido_appearance_v1 等),
appearance 强制 {"mode":"day","syncServer":false}。
参考 docs/TAVERN_E2E_NOTES.md。
