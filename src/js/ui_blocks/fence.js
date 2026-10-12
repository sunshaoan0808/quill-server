/**
 * UI block fence protocol (quill port of DEEIX ui-block/fence.ts).
 *
 * The model may emit fenced blocks in assistant text:
 *   ```deeix-ui\n{json}\n```
 *   ```deeix-widget\n{html}\n```
 * Rendering pulls them out of the text and mounts them as interactive cards.
 *
 * Contract notes (kept for compatibility with upstream behaviour):
 * - Code-fence scanning is intentionally greedy-safe: language token may be
 *   empty (plain ``` blocks are preserved untouched — markdown handles them).
 * - Body index arithmetic uses the ORIGINAL text (never normalized \r\n),
 *   otherwise replace() misses match positions.
 */

export const UI_BLOCK_LANGUAGE_DEEIX = 'deeix-ui';
export const WIDGET_BLOCK_LANGUAGE_DEEIX = 'deeix-widget';
const SUPPORTED_BLOCK_LANGUAGES = new Set([
  UI_BLOCK_LANGUAGE_DEEIX,
  // WIDGET_BLOCK_LANGUAGE_DEEIX: mounted in the live-panel PR (sandbox iframe);
  // until then widget fences must stay in the text (not extracted, not swallowed).
]);

const FENCE_PATTERN = /```([A-Za-z0-9_+.-]*)[ \t]*\r?\n([\s\S]*?)\r?\n?```/g;

function stripTrailingNewline(body) {
  return body.replace(/\r?\n$/, '');
}

function stripCRLF(text) {
  return String(text || '').replace(/\r\n/g, '\n').replace(/\r/g, '\n');
}

/** Stable-enough body identity for change detection (DEEIX hashText-compatible FNV-1a). */
export function hashFenceBody(rawBody) {
  const s = String(rawBody || '');
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, '0');
}

/** Scan all fence matches. Returns structural entries (no language filter). */
export function scanUIBlockFences(text) {
  const src = String(text || '');
  const matches = src.matchAll(FENCE_PATTERN);
  const fences = [];
  for (const m of matches) {
    fences.push({
      raw: m[0],
      rawStart: m.index,
      rawEnd: m.index + m[0].length,
      language: m[1],
      normalizedBody: stripCRLF(stripTrailingNewline(m[2])),
      rawBody: stripTrailingNewline(m[2]),
    });
  }
  return { fences, invalid: [] };
}

/** Remove supported UI-block fences from the text (plain code fences kept). */
export function stripUIBlockFences(text) {
  const src = String(text || '');
  let out = '';
  let last = 0;
  for (const m of src.matchAll(FENCE_PATTERN)) {
    if (!SUPPORTED_BLOCK_LANGUAGES.has(m[1])) continue;
    out += src.slice(last, m.index);
    last = m.index + m[0].length;
  }
  out += src.slice(last);
  return out;
}

/**
 * Extract supported UI blocks (deeix-ui / deeix-widget) out of the message.
 * Returns { cleanText, blocks } where blocks carry rawBody + hash for
 * rendering-side change detection. Plain code fences stay in the text.
 */
export function extractUIBlocks(text) {
  const src = String(text || '');
  const blocks = [];
  let out = '';
  let last = 0;
  for (const m of src.matchAll(FENCE_PATTERN)) {
    const lang = m[1];
    if (!SUPPORTED_BLOCK_LANGUAGES.has(lang)) continue; // plain fence → keep
    out += src.slice(last, m.index);
    blocks.push({
      language: lang,
      rawBody: stripTrailingNewline(m[2]),
      normalizedBody: stripCRLF(stripTrailingNewline(m[2])),
      hash: hashFenceBody(m[2]),
    });
    last = m.index + m[0].length;
  }
  out += src.slice(last);
  return { cleanText: out, blocks };
}

/** Replace only supported UI fences (replacer receives fence objects). */
export function replaceUIBlockFences(text, replacer) {
  const src = String(text || '');
  return src.replace(FENCE_PATTERN, (full, lang, body) => {
    if (!SUPPORTED_BLOCK_LANGUAGES.has(lang)) return full;
    return String(
      replacer({
        raw: full,
        rawStart: 0,
        rawEnd: full.length,
        language: lang,
        normalizedBody: stripCRLF(stripTrailingNewline(body)),
        rawBody: stripTrailingNewline(body),
      }),
    );
  });
}
