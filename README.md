# quill-server

A self-hosted narrative engine: turn-based, streaming story sessions driven by
configurable LLM backends, with story packs, world-info scanning, character
card distillation and a job system.

- Rust (axum + tokio + SQLite) server, vanilla-JS single-page frontend
- SillyTavern-compatible world info / lorebook scanner
- SSE streaming with replay & reconnect, job queue with concurrency limits
- OpenAI-compatible / Anthropic / Google provider dispatch
- chub.ai character import, SD WebUI (A1111) image generation channel
- Dynamic injection router (LLM-driven prompt block selection)
- Cognitive / relationship depth modules (epistemic, agency, intimacy, ...)
- ST ecosystem alignment (presets, quick replies, MVU, plugin manifest)
- e2e smoke tests, perf baseline with regression guards

## Build & run

```bash
npm ci && npm run build
cargo run --release -p kaleido-server
```

See `.env.example` for configuration; `ARCHITECTURE.md` has the module map.
