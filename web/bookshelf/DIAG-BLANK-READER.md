# Reader Blank-Content Root Cause — Verified Findings (2026-07-31)

## Setup
- Server: http://localhost:8899 serves bookshelf (standalone reader).
- Playwright probes: run `NODE_PATH=/root/scripts/node_modules xvfb-run -a node probe-*.js` from
  /root/workspaces/museai-web-android-brainstorm/museai-server/web/bookshelf
- Bookshelf card selector that works: `button img[src*="/api/v1/crawler/novels/"]` then closest('button').
  (`.book-card` class does NOT exist in current markup — do not use.)

## Symptom
- Reader overlay opens, chapter title shows, but **content area is blank**.
- No console errors during open (foliate catches/swallows them inside goTo).

## Root Cause (2 bugs in foliate-reader.js)
1. **`makeBookFromHTML()` (line 179-202) returns a plain book object WITHOUT `resolveHref`.**
   - `View.resolveNavigation()` (foliate-js/view.js:446-458) calls `this.book.resolveHref(target)`
     for ANY non-number, non-fraction, non-CFI target — including `{index:0}`.
   - Throws `TypeError: this.book.resolveHref is not a function` → goTo fails → paginator
     `#display` never runs → `#createView` never runs → `getContents()` = [] → BLANK.
2. **No initial navigation after open.**
   - `open()` (foliate-reader.js:1328-1334) only calls `view.goTo(saved.fraction)` when saved
     progress exists. With fresh localStorage, nothing displays section 0.

## Verified facts from probes
- renderer = Paginator (custom foliate-js/paginator.js), closed shadow root.
- renderer.sections.length = 1 (section-1, load() returns blob URL of full HTML).
- `fr.view.goTo({index:0})` BEFORE patch → throws resolveHref error, contents stay 0.
- Manual fix-test via monkeypatching `book.resolveHref` + `view.goTo({index:0})` → planned.
- foliate-js/view.js public API: goTo, goToFraction, next/prev/goLeft/goRight, goToTextStart;
  no `first()`; `lastLocation` property exists (not currentLocation()).

## Planned Fix (delegate code edit to claude CLI per SOP)
- Add `resolveHref(href)` to book object in makeBookFromHTML:
  - object target → return as-is; string `section-1#id` → {index, anchor(doc => getElementById)}
- In open(), after view.open(book), if no saved progress: `view.goTo({ index: 0 })` (after small timeout).
- Then re-run probe to confirm contents[0].doc.body text appears.

## Probe files created
- probe-goto.js, probe-detail.js, probe-fixmock.js, probe-geo.js (the latter used wrong selector)


---

## Fix Applied & Verified (2026-07-31)

### Fix 1 — blank content (root cause 1+2 above)
- `makeBookFromHTML()` book object now includes `resolveHref(href)` (line ~201):
  - object target → returned as-is; string `section-N#id` → `{index, anchor}`.
- `open()` now calls `view.goTo({ index: 0 })` when no saved progress (line ~1345).

### Fix 2 — dict popup never appeared after fresh open
- `_attachDictListener()` (line ~885) runs at open() BEFORE the paginator's content
  document exists → falls back to main document only; selectionchange in the content
  iframe never reaches it (popup/quote-image unreachable until a relocate re-attach).
- Now, when content doc is missing at attach time, keeps main-doc fallback AND polls
  `view.renderer.getContents()[0].doc` up to 15×400ms; on arrival, moves the listener
  to the content document.

### Verification (test-quote-image.js) — PASS ✅ 2026-07-31
- Reader opens with content (contents[0].doc.text ~119K chars, 222 pages).
- Select "链接" → dict popup appears with `🖼 生成图片` button.
- Click quote → 800×1000 canvas overlay, opaque, dataURL 1.1MB; PNG download OK;
  clipboard toBlob OK (845KB).
- Only console noise: dictionaryapi.dev CORS-blocked lookup for Chinese text
  → FIXED (2026-07-31): CJK text skips English dictionary, shows local hint
  + quote-image/wordbook buttons. test-quote-image.js PASS again, zero console errors.

### Lesson
- claude CLI (claude -p --allowedTools Edit) repeatedly *reports* success but does NOT
  apply edits in this sandbox (3 occurrences this session). Always verify with
  `grep`/`node --check` + served copy (`curl | grep`); fall back to file_patch.
