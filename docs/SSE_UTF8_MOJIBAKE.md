# SSE UTF-8 split → `�` (U+FFFD) in Story Tavern

**Date:** 2026-07-28  
**Symptom:** 剧场「白昼之下 · P3」最新剧情里部分汉字显示为 `�`  
**Session:** `tavern-session-35f8e919-5240-4099-888c-5e5084bfa8f6`（title `白昼之下 · P3`）  
**Not:** W1 background slug/JSON template issue

## Root cause

Upstream LLM SSE is read as `reqwest` **byte chunks**. Multi-byte UTF-8 (CJK = 3 bytes) often **straddles chunk boundaries**.

Old code (three places):

```rust
buf.push_str(&String::from_utf8_lossy(&bytes));
```

`from_utf8_lossy` on an incomplete trailing sequence **inserts U+FFFD and drops the partial bytes**. Later chunks cannot reconstruct the character → permanent `�` in `messages[].content`, then copied into `memoryL2` / `memoryL3`.

Evidence on P3 session disk JSON:

| range | U+FFFD count |
|-------|----------------|
| messages[0..60] | **0** |
| messages[61..] (from ~18:28 UTC) | **261+** in content; 286 file-wide |
| latest asst #69 | 50× `�` |
| user #64 / #68 | 2× each (`��身` ≈ 转 split; `��的` similar) |

Frontend already uses `TextDecoder('utf-8', { stream: true })` — **not** the bug.

## Fix

- New helper: `crates/kaleido-server/src/utf8_stream.rs`  
  - `push_utf8_chunk(carry, chunk)` — keep 0..=3 incomplete trailing bytes  
  - `flush_utf8_carry` at EOS  
- Wired into:
  - `story_tavern.rs` (剧场主路径)  
  - `llm_stream.rs` (Background / BookTravel)  
  - `main.rs` (generic chat stream)  
- Unit tests: CJK split every 1–2 bytes → no U+FFFD (`cargo test -p kaleido-server utf8_stream`)

## Existing P3 data

**Already-written messages stay corrupted** (bytes were discarded at stream time; cannot losslessly restore).

Options for the user:

1. **Continue from a save before ~18:28 UTC** (first bad asst is message #63 / 2026-07-28T18:28:08Z), or  
2. **Keep playing** — **new** turns after this deploy will be clean; old `�` lines remain in history unless manually edited / rolled back.

No automatic “repair” of historical text in this change (would be guesswork).

## Verify

```bash
cargo test -p kaleido-server utf8_stream
# live: play one turn on P3 after deploy; new assistant content must have zero U+FFFD
python3 -c "import json;..."  # count \\ufffd on latest message only
```

## Historical repair (P3 session)

**Session:** `tavern-session-35f8e919-5240-4099-888c-5e5084bfa8f6`（白昼之下 · P3）  
**Backup:** `…json.bak-before-fffd-repair-20260728T190001Z`  
**Method:** context reconstruction per corrupted message (#63–#73 content + options + memoryL2/L3), not byte recovery.  
**Result:** disk JSON `U+FFFD` count **0**.

Repaired messages: 63,64,65,67,68,69,71,73 (+ option chips).  
Note: wording is best-effort from surrounding canon (档案局 / 蝴蝶结 / 琥珀色 / 转身 / 门框…); not bit-identical to original model output.

