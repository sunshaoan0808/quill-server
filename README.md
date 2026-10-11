# quill-server

A self-hosted narrative engine: turn-based, streaming story sessions driven by
configurable LLM backends, with story packs, world-info scanning, character
card distillation and a job system.

- Rust (axum + tokio + SQLite) server, vanilla-JS single-page frontend
- SillyTavern-compatible world info / lorebook scanner
- SSE streaming with replay & reconnect, job queue with concurrency limits
- OpenAI-compatible / Anthropic / Google provider dispatch

## Build & run

```bash
# build frontend (Node 20)
npm ci && npm run build

# build & run server (Rust stable)
cargo run --release -p kaleido-server
```

See `.env.example` for configuration (admin password, LLM backend, ports);
`ARCHITECTURE.md` has the module map and data flow.
