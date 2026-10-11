/**
 * foliate-bridge.js — ReadAware-style integration layer for the bookshelf
 *
 * This script runs alongside the React SPA and provides:
 * 1. Content restructuring (fix chapter fragmentation)
 * 2. HTML sanitization for dangerouslySetInnerHTML
 * 3. foliate-js EPUB/PDF rendering support
 * 4. foliate-view CSS injection
 *
 * Architecture (inspired by ReadAware's foliate-engine.ts):
 * ┌─────────────────────────────────────────────────┐
 * │  loader.js (runs first)                         │
 * │  → registers <foliate-view> custom element       │
 * │  → exports __readawareFoliate.{makeBook, ...}    │
 * ├─────────────────────────────────────────────────┤
 * │  foliate-bridge.js (this file)                  │
 * │  → restructures API content to fix chapters      │
 * │  → sanitizes HTML in reader-content              │
 * │  → provides openFoliateBook() for EPUB/PDF      │
 * ├─────────────────────────────────────────────────┤
 * │  React SPA (index-DiShLB2e.js)                  │
 * │  → renders shelf + reader UI                    │
 * │  → uses dangerouslySetInnerHTML                 │
 * └─────────────────────────────────────────────────┘
 */

// ─── Content Restructurer ──────────────────────────────────────────────────
// The bundle's chapter parser splits content by `#`/`##`/`###` headings and
// creates a chapter object for EACH heading. This causes problems when the
// API returns:
//   • Book titles as `# Title` headings (creates spurious chapter)
//   • TOC sections like `## 目录` (creates spurious chapter)
//   • Duplicate chapter headings (`## 第1章` then `# 第一章` = 2 chapters)
//   • Content with NO headings (6 novels = 0 chapters = blank reader)
//
// This restructurer cleans up the markdown content before the bundle parses it.

// 2026-08-29 修复：删除重复的简版 chineseToNumber（与后段万位完整版同名，
// ES module 顶层重复 function 声明直接 SyntaxError 导致整个桥接模块崩溃、
// 书架阅读器打不开）。只保留后段完整版（支持百十千万 + 大写壹贰叁），
// function 声明提升，前后 extractChapterNumber/chapterValue 均可调用。

// Extract chapter number from a heading title.
// Returns a normalized number (1-based), or null if not a chapter heading.
function extractChapterNumber(title) {
  // Arabic: "第1章", "第 12 章", "第3节", "第4篇"
  const arabicMatch = title.match(/第\s*(\d+)\s*[章节篇回部]/);
  if (arabicMatch) return parseInt(arabicMatch[1], 10);
  // Chinese: "第一章", "第十二章"
  const chineseMatch = title.match(/第\s*([一二三四五六七八九十百千零]+)\s*[章节篇回部]/);
  if (chineseMatch) return chineseToNumber(chineseMatch[1]);
  // English: "Chapter 1", "CHAPTER 12"
  const enMatch = title.match(/[Cc]hapter\s+(\d+)/);
  if (enMatch) return parseInt(enMatch[1], 10);
  return null;
}

// Heading patterns
const RE_ANY_HEADING = /^(#{1,3})\s+(.+)$/;
const RE_CHAPTER_HEADING = /^#{1,3}\s+(第\s*[\d一二三四五六七八九十百千零]+\s*[章节篇回部]|[Cc]hapter\s+\d+)/;
const RE_TOC_HEADING = /^#{1,3}\s+(目录|章节目录|目次|索引|[Tt]able\s+[Oo]f\s+[Cc]ontents|[Cc]ontents|TOC)/;
const RE_BOOK_TITLE = /^#{1,3}\s+(序章?|前言|引言|[Pp]rologue|[Ii]ntroduction|楔子)/;

function restructureContent(content) {
  if (!content || typeof content !== 'string') return content;
  const lines = content.split('\n');
  const headingLines = lines.filter(l => RE_ANY_HEADING.test(l));

  // ── CASE: No headings at all — wrap entire content as one chapter ──
  if (headingLines.length === 0) {
    return `# 正文\n\n${content}`;
  }

  // ── CASE: Has headings — clean up ──
  let result = [];
  let foundFirstChapter = false;
  let lastChapterNum = null;
  let inTOC = false;

  for (const line of lines) {
    const headingMatch = line.match(RE_ANY_HEADING);

    if (!headingMatch) {
      // Non-heading line
      if (!foundFirstChapter) continue;        // skip pre-first-chapter
      if (inTOC) continue;                      // skip TOC body
      result.push(line);
      continue;
    }

    const title = headingMatch[2].trim();

    // TOC heading → skip the heading and subsequent list items
    if (RE_TOC_HEADING.test(line)) {
      inTOC = true;
      continue;
    }
    // Clear TOC flag on any other heading
    inTOC = false;

    // Check if it's a chapter heading
    if (RE_CHAPTER_HEADING.test(line)) {
      const num = extractChapterNumber(title);

      if (!foundFirstChapter) {
        foundFirstChapter = true;
        lastChapterNum = num;
        result.push(line);
        continue;
      }

      // Deduplicate: same number → skip (e.g. `## 第1章` followed by `# 第一章`)
      if (num !== null && lastChapterNum !== null && num === lastChapterNum) {
        continue;
      }
      lastChapterNum = num;
      result.push(line);
      continue;
    }

    // Before first chapter: skip non-chapter headings (book title, author, etc.)
    if (!foundFirstChapter) continue;

    // After first chapter: keep subsection headings (like ### 人物介绍)
    result.push(line);
  }

  return result.join('\n');
}

// ─── HTML Sanitizer ────────────────────────────────────────────────────────
const SAFE_TAGS = new Set([
  'p', 'br', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
  'div', 'span', 'section', 'article',
  'strong', 'em', 'b', 'i', 'u', 's', 'del', 'ins', 'mark', 'sub', 'sup',
  'code', 'pre', 'blockquote', 'cite',
  'ul', 'ol', 'li', 'dl', 'dt', 'dd',
  'table', 'thead', 'tbody', 'tr', 'th', 'td',
  'img', 'figure', 'figcaption', 'picture',
  'a', 'abbr', 'address', 'hr', 'time',
  'ruby', 'rt', 'rp', 'wbr',
]);

const SAFE_ATTRS = new Set([
  'href', 'src', 'alt', 'title', 'rel', 'target',
  'class', 'id', 'style',
  'width', 'height', 'loading', 'decoding',
  'lang', 'dir',
  'colspan', 'rowspan',
  'start', 'type', 'value', 'placeholder',
  'cite', 'datetime',
]);

const RE_EVENT = /^on/i;
const RE_JAVASCRIPT = /^\s*javascript\s*:/i;

function sanitize(html) {
  if (!html || typeof html !== 'string') return '';
  const parser = new DOMParser();
  const doc = parser.parseFromString(
    `<div id="__sanitize">${html}</div>`,
    'text/html'
  );
  const root = doc.getElementById('__sanitize');
  if (!root) return html;

  function cleanNode(node) {
    if (node.nodeType === Node.TEXT_NODE) return;
    if (node.nodeType === Node.ELEMENT_NODE) {
      const tag = node.tagName.toLowerCase();
      if (!SAFE_TAGS.has(tag) && !tag.startsWith('svg')) {
        const text = document.createTextNode(node.textContent || '');
        node.parentNode.replaceChild(text, node);
        return;
      }
      const attrs = Array.from(node.attributes);
      for (const attr of attrs) {
        const name = attr.name.toLowerCase();
        const value = attr.value;
        if (RE_EVENT.test(name) || RE_JAVASCRIPT.test(value)) {
          node.removeAttribute(attr.name);
        } else if (!SAFE_ATTRS.has(name) && !name.startsWith('data-') && !name.startsWith('aria-')) {
          node.removeAttribute(attr.name);
        } else if (name === 'href' && RE_JAVASCRIPT.test(value)) {
          node.removeAttribute('href');
        }
      }
      Array.from(node.childNodes).forEach(cleanNode);
    }
  }

  Array.from(root.childNodes).forEach(cleanNode);
  return root.innerHTML;
}

// ─── Chapter Title Normalizer ──────────────────────────────────────────────
// Some content has chapter titles without markdown heading prefixes (e.g.
// just "第一章 雨巷来客" on a line by itself). Convert those to `##` format.
// Only applies to content that has NO existing markdown headings.

const CHAPTER_RE = /(^|\n)(?:\s*第\s*[一二三四五六七八九十零百千万\d]+\s*[章节篇回部]\s*[:：]?\s*(.*?)\s*(?=\n|$)|(?:\n|^)\s*Chapter\s+\d+\s*[:：]?\s*(.*?)\s*(?=\n|$)|(?:\n|^)\s*CHAPTER\s+\d+\s*[:：]?\s*(.*?)\s*(?=\n|$))/gim;

function normalizeChapters(text) {
  if (!text || typeof text !== 'string') return text;
  // Only apply if content has NO markdown headings yet
  if (/^#{1,3}\s/m.test(text)) return text;
  return text.replace(CHAPTER_RE, (match, newline, title1, title2, title3) => {
    const title = (title1 || title2 || title3 || '').trim() || '章节';
    return `${newline}## ${title}`;
  });
}

// ─── Content Pipeline ─────────────────────────────────────────────────────
// The bundle's parser splits by `#{1,3}`, so we restructure+normalize BEFORE
// the bundle sees the content. Sanitization happens AFTER rendering via
// MutationObserver (see below).
//
// Pipeline: raw API content → restructureContent() → normalizeChapters() → bundle parser

const ORIGINAL_FETCH = window.fetch.bind(window);
window.fetch = async function(input, init) {
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
  const response = await ORIGINAL_FETCH(input, init);

  // Only intercept content API calls
  if (url && url.includes('/api/v1/crawler/novels/') && url.includes('/content')) {
    const cloned = response.clone();
    try {
      const text = await cloned.text();
      let data;
      try { data = JSON.parse(text); } catch { data = text; }

      if (typeof data === 'string') {
        const processed = normalizeChapters(restructureContent(data));
        return new Response(JSON.stringify(processed), {
          status: response.status,
          headers: response.headers,
        });
      }
      if (data && typeof data.content === 'string') {
        data.content = normalizeChapters(restructureContent(data.content));
        return new Response(JSON.stringify(data), {
          status: response.status,
          headers: response.headers,
        });
      }
    } catch (e) {
      console.warn('[foliate-bridge] Content restructuring failed:', e.message);
    }
  }
  return response;
};

// ─── Post-Render Sanitization ──────────────────────────────────────────────
// After React renders chapter content via dangerouslySetInnerHTML, sanitize
// the resulting HTML to remove any dangerous tags/attributes.

function sanitizeElement(el) {
  if (!el || el.getAttribute('data-sanitized')) return;
  el.innerHTML = sanitize(el.innerHTML);
  el.setAttribute('data-sanitized', 'true');
}

const sanitizerObserver = new MutationObserver(mutations => {
  for (const mutation of mutations) {
    for (const node of mutation.addedNodes) {
      if (node.nodeType === Node.ELEMENT_NODE) {
        // Reader content areas
        const contentAreas = node.matches
          ? (node.matches('[class*="reader-content"], [class*="chapter-content"]')
              ? [node]
              : [])
          : [];
        for (const el of contentAreas) sanitizeElement(el);
        // Also search children
        const children = node.querySelectorAll
          ? node.querySelectorAll('[class*="reader-content"], [class*="chapter-content"]')
          : [];
        for (const el of children) sanitizeElement(el);
      }
    }
  }
});

sanitizerObserver.observe(document.body, {
  childList: true,
  subtree: true,
});

// ─── foliate-js EPUB/PDF Rendering Support ─────────────────────────────────
// ReadAware pattern: use __readawareFoliate (from loader.js) to
// render EPUB/PDF/MOBI files in a <foliate-view> element.

// ─── TXT Plain-Text Support ────────────────────────────────────────────────
// foliate-js 的 makeBook 不识别 .txt（会抛 UnsupportedTypeError）。
// 这里把纯文本转成 HTML blob URL，包装成 foliate book 对象：
// 按「第X章 / Chapter N / 楔子 / 序章 / 尾声 / 番外」拆分成多个 section，
// 无章节标题的 txt 整本作为一个 section。

// ── 乱码修复：集成自开源阅读器 ReadAware（apps/web/src/features/reader/lib/decode-text.ts）──
// 原版 `decodeTextBook()` 逐字移植：BOM 判定 → 候选编码严格解码 → mojibake 检测 → lossy 兜底。
// 覆盖 GB18030(GBK/GB2312)/Big5/Shift_JIS/EUC-KR，避免源 txt 被按 UTF-8 硬解成 �。
const DECODE_BOMS = [
  { bytes: [0xef, 0xbb, 0xbf], encoding: "utf-8" },
  { bytes: [0xff, 0xfe], encoding: "utf-16le" },
  { bytes: [0xfe, 0xff], encoding: "utf-16be" },
];
const DECODE_CANDIDATE_ENCODINGS = ["utf-8", "gb18030", "big5", "shift_jis", "euc-kr"];

function decodeTextBook(bytes) {
  for (const { bytes: bom, encoding } of DECODE_BOMS) {
    if (bom.every((byte, index) => bytes[index] === byte)) {
      return stripBom(decodeOrNull(bytes, encoding) ?? decodeLossy(bytes));
    }
  }
  for (const encoding of DECODE_CANDIDATE_ENCODINGS) {
    const decoded = decodeOrNull(bytes, encoding);
    if (decoded != null && !looksLikeMojibake(decoded)) return decoded;
  }
  return decodeLossy(bytes);
}

function decodeOrNull(bytes, encoding) {
  try {
    return new TextDecoder(encoding, { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

function decodeLossy(bytes) {
  return new TextDecoder("windows-1252").decode(bytes);
}

function stripBom(value) {
  return value.charCodeAt(0) === 0xfeff ? value.slice(1) : value;
}

function looksLikeMojibake(value) {
  const sample = value.slice(0, 4096);
  if (!sample) return false;
  let suspicious = 0;
  for (const char of sample) {
    const code = char.codePointAt(0);
    if (
      code === 0xfffd ||
      (code >= 0xe000 && code <= 0xf8ff) ||
      (code >= 0x0080 && code <= 0x00a0)
    ) {
      suspicious++;
    }
  }
  return suspicious > sample.length * 0.02;
}

// 2026-08-25: 从单正则升级为后端 crawler::split_novel_chapters 的 Legado 多规则甄别方案。
// 旧 RE_TXT_CHAPTER 漏掉三类标题：列表破折号前缀 `- 第X章`、全角括号序号 `（5）释放`、
// 以及无「第X章」字样的纯数字/大写数字标题；且无间隔过滤，会把正文短句误判成标题。
// 这里逐条规则统计匹配（要求相邻标题间隔 >1000 字符过滤误报），选命中最多的规则，
// 再按真实章号去重（`# 第1章` 与裸 `第1章` 双标题合并）。
//
// 注意：前端只做展示用拆章，<2 个标题时整本作为一个 section（不复刻后端的字数窗口兜底，
// 窗口拆分会打断阅读流）。规则集须与 crates/museai-server/src/crawler.rs TOC_RULES 保持同步。
const TXT_TOC_RULES = [
  // 目录：序章/楔子/正文/终章/后记/尾声/番外/第X章/节/卷/集/部/篇
  { name: '目录', re: /^[ \t　#]{0,4}(?:序章|楔子|正文|终章|后记|尾声|番外|第[ \t　]{0,4}[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]+?[ \t　]{0,4}(?:章|节|卷|集|部|篇)).{0,40}$/gm },
  // 数字 可选分隔符 标题名称（如 `12 雨夜` / `3:归来`）
  { name: '数字标题', re: /^[ \t　]{0,4}[0-9]{1,5}[:：,.， 、_—\-]?.{1,40}$/gm },
  // 大写数字 分隔符 标题名称（如 `十二、缘起`）
  { name: '大写数字标题', re: /^[ \t　]{0,4}(?:序章|楔子|正文|终章|后记|尾声|番外|[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,8}章?)[ 、_—\-].{1,40}$/gm },
  // 数字混合 分隔符
  { name: '数字混合标题', re: /^[ \t　]{0,4}(?:序章|楔子|正文|终章|后记|尾声|番外|[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,8}章?[ 、_—\-]|[0-9]{1,5}章?[:：,.， 、_—\-]).{0,40}$/gm },
  // Chapter/Section/Part/Episode/No./Ch.
  { name: 'Chapter英文', re: /^[ \t　]{0,4}(?:[Cc]hapter|[Ss]ection|[Pp]art|ＰＡＲＴ|[Nn][oO][.、]|[Ee]pisode|[Cc]h\.?)[ \t　]{0,4}[0-9]{1,4}.{0,40}$/gm },
  // 特殊符号序号 标题（【第X章】/〖Chapter 1〗）
  { name: '特殊符号序号', re: /^[ \t　]{0,4}[【〔〖「『〈［\[](?:第|[Cc]hapter)[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,10}[章节].{0,30}$/gm },
  // 章/卷 序号 标题（章十二 / 卷三）
  { name: '章卷序号', re: /^[ \t　]{0,4}[卷章][0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]{1,8}[ \t　]{0,4}.{0,40}$/gm },
  // 2026-08-19: 列表破折号前缀（安全屋/老板你也没说/孕船 等 txt 导出常带 `- 第X章`）
  { name: '列表破折号', re: /^[ \t　]{0,4}[-–—·•*]{1,2}[ \t　]{0,2}(?:第[ \t　]{0,4}[0-9〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]+?[ \t　]{0,4}(?:章|节|卷|集|部|篇)).{0,40}$/gm },
  // 2026-08-19: 全角括号序号标题（度蜜月 `　　（5）释放`）
  { name: '全角括号序号', re: /^[ \t　]{0,4}[（(【［\[]\s*[0-9０-９]{1,8}\s*[）)】］\]](?:[ \t　]{0,4}\S.{0,40})?$/gm },
];

// 旧正则保留为兜底/导出兼容（新逻辑命中 <2 时整本一节，不再单独用它拆）。
const RE_TXT_CHAPTER = /^\s*(?:第\s*[一二三四五六七八九十百千万零〇0-9]+\s*[章节篇回部]|楔子|序章?|尾声|番外(?:篇)?|Chapter\s+\d+|CHAPTER\s+\d+)[^\n]{0,60}\s*$/gm;

// 中文数字 → 数值（支持到万位），与后端 chapter_value 同口径。无法解析返回 null。
const CHINESE_DIGITS = { 〇:0, 零:0, 一:1, 二:2, 两:2, 三:3, 四:4, 五:5, 六:6, 七:7, 八:8, 九:9, 十:10, 百:100, 千:1000, 万:10000, 壹:1, 贰:2, 叁:3, 肆:4, 伍:5, 陆:6, 柒:7, 捌:8, 玖:9, 拾:10, 佰:100, 仟:1000 };

function chapterValue(title) {
  if (!title) return null;
  // 全角括号前导序号：`（5）释放` → 5
  let m = title.match(/[（(【［\[]\s*([0-9０-９]{1,8})\s*[）)】］\]]/);
  if (m) return parseInt(m[1].replace(/[０-９]/g, d => '0123456789'['０１２３４５６７８９'.indexOf(d)]), 10);
  // 阿拉伯数字：第 12 章 / 第12节
  m = title.match(/第[ \t　]{0,4}([0-9]+)[ \t　]{0,4}(?:章|节|卷|集|部|篇|回)/);
  if (m) return parseInt(m[1], 10);
  // 中文数字：第十二章 → 12
  m = title.match(/第[ \t　]{0,4}([〇零一二两三四五六七八九十百千万壹贰叁肆伍陆柒捌玖拾佰仟]+)[ \t　]{0,4}(?:章|节|卷|集|部|篇|回)/);
  if (m) return chineseToNumber(m[1]);
  // 纯数字开头：`12 雨夜`
  m = title.match(/^[ \t　]{0,4}([0-9]{1,8})/);
  if (m) return parseInt(m[1], 10);
  return null;
}

function chineseToNumber(s) {
  // 简版中文数字解析（万位以内），与后端 chapter_value 一致。
  let total = 0, section = 0, num = 0;
  for (const ch of s) {
    const v = CHINESE_DIGITS[ch];
    if (v == null) continue;
    if (v === 10000) { total += (section + num) * v; section = 0; num = 0; }
    else if (v >= 10) { if (num === 0) num = 1; section += num * v; num = 0; }
    else { num = v; }
  }
  return total + section + num;
}

// 多规则甄别：逐条统计匹配（相邻标题间隔 >1000 才算），返回命中最多的规则的全部匹配。
function pickTxtChapters(text) {
  let best = null;
  let bestMatches = [];
  for (const rule of TXT_TOC_RULES) {
    rule.re.lastIndex = 0;
    const matches = [];
    let prevEnd = null;
    let m;
    while ((m = rule.re.exec(text)) !== null) {
      const okGap = prevEnd == null || (m.index - prevEnd) > 1000;
      if (okGap) matches.push({ index: m.index, end: m.index + m[0].length, title: m[0].trim() });
      prevEnd = m.index + m[0].length;
    }
    if (matches.length > bestMatches.length) {
      best = rule;
      bestMatches = matches;
    }
  }
  return { rule: best, matches: bestMatches };
}

// 同真实章号去重：相邻两条标题章号相同 → 合并为后一条（保留正文标题而非页眉）。
function dedupSameValue(matches) {
  const out = [];
  for (const cur of matches) {
    if (out.length > 0) {
      const last = out[out.length - 1];
      const a = chapterValue(cur.title);
      const b = chapterValue(last.title);
      if (a != null && b != null && a === b) {
        out[out.length - 1] = cur;
        continue;
      }
    }
    out.push(cur);
  }
  return out;
}

function escapeHtmlText(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function splitTxtChapters(text) {
  // 多规则甄别：选命中最多（间隔>1000 过滤误报）的规则，再按真实章号去重。
  const { matches: raw } = pickTxtChapters(text);
  const matches = dedupSameValue(raw);
  // 少于 2 个章节标题（含整本 0 个）→ 整本一个 section
  if (matches.length < 2) return [{ title: '', body: text }];
  const parts = [];
  for (let i = 0; i < matches.length; i++) {
    const start = matches[i].end;
    const end = i + 1 < matches.length ? matches[i + 1].index : text.length;
    parts.push({ title: matches[i].title, body: text.slice(start, end) });
  }
  return parts;
}

function makeTxtHtml(chapter) {
  const title = escapeHtmlText(chapter.title);
  const body = escapeHtmlText(chapter.body).replace(/\r?\n/g, '<br>');
  return `<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${title || '正文'}</title>
<style>
  html { background: var(--foliate-view-bg, #fff); }
  body { font-family: var(--foliate-view-font-family, "Noto Serif SC", "Source Han Serif SC", Georgia, serif);
         line-height: var(--foliate-view-line-height, 1.9);
         font-size: var(--foliate-view-font-size, 16px);
         padding: 1em 1.2em; color: var(--foliate-view-color, #333); }
  h1 { font-size: 1.3em; text-align: center; margin: 0.5em 0 1.2em; font-weight: 600; }
</style>
</head>
<body>
<h1>${title}</h1>
${body}
</body>
</html>`;
}

async function makeTxtBook(file) {
  // 用集成自 ReadAware 的乱码修复解码（编码嗅探），替代 file.text()（仅按 UTF-8 硬解）
  const text = decodeTextBook(new Uint8Array(await file.arrayBuffer()));
  const title = (file.name.replace(/\.txt$/i, '') || '未命名书籍').trim();
  const chapters = splitTxtChapters(text);
  const urls = [];
  const sections = chapters.map((ch, i) => ({
    id: 'txt-' + i,
    load: async () => {
      const url = URL.createObjectURL(
        new Blob([makeTxtHtml(ch)], { type: 'text/html;charset=utf-8' }));
      urls.push(url);
      return url;
    },
    size: ch.body.length,
  }));
  return {
    metadata: { title, language: 'zh-CN' },
    getCover: async () => null,
    sections,
    toc: chapters.map((ch, i) => ({ label: ch.title || `第 ${i + 1} 部分`, href: 'txt-' + i })),
    resolveHref: href => ({ index: sections.findIndex(s => s.id === href) }),
    splitTOCHref: href => [href, null],
    getTOCFragment: doc => doc.documentElement,
    destroy: () => { for (const url of urls) URL.revokeObjectURL(url); },
  };
}

async function openFoliateBook(file, container) {
  if (typeof __readawareFoliate === 'undefined') {
    await new Promise(resolve => {
      const check = () => {
        if (typeof __readawareFoliate !== 'undefined') resolve();
        else setTimeout(check, 50);
      };
      check();
    });
  }
  const { makeBook } = globalThis.__readawareFoliate;

  let view = container.querySelector('foliate-view');
  if (!view) {
    view = document.createElement('foliate-view');
    container.innerHTML = '';
    container.appendChild(view);
  }

  try {
    // .txt 纯文本走自定义转换（foliate makeBook 不认 txt），其余格式走 foliate-js
    const book = /\.txt$/i.test(file.name)
      ? await makeTxtBook(file)
      : await makeBook(file);
    await view.open(book);
    // 官方 foliate 流程：open 后必须 init 才会渲染第一页（paginator.open 只存数据）
    if (typeof view.init === 'function') await view.init({ showTextStart: true });
    return view;
  } catch (err) {
    console.error('[foliate-bridge] Failed to open book:', err);
    container.innerHTML = `<div class="error">${err.message}</div>`;
    throw err;
  }
}

// ─── Inject foliate-view CSS ───────────────────────────────────────────────
const STYLE_ID = 'foliate-bridge-styles';
if (!document.getElementById(STYLE_ID)) {
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = `
    foliate-view {
      display: block;
      width: 100%;
      height: 100%;
      min-height: 60vh;
      --foliate-view-bg: #fff;
      --foliate-view-color: #333;
      --foliate-view-font-size: 16px;
      --foliate-view-line-height: 1.8;
      --foliate-view-padding: 16px 24px;
      --foliate-view-font-family: -apple-system, BlinkMacSystemFont, "Segoe UI",
        Roboto, "Noto Sans", "Noto Serif", "Source Han Serif SC", Georgia, serif;
    }
    @media (prefers-color-scheme: dark) {
      foliate-view {
        --foliate-view-bg: #1a1a1a;
        --foliate-view-color: #ddd;
      }
    }
    .foliate-error {
      padding: 2rem;
      text-align: center;
      color: #b91c1c;
      font-size: 0.9rem;
    }
  `;
  document.head.appendChild(style);
}

// ─── Export for external use ───────────────────────────────────────────────
globalThis.__foliateBridge = {
  sanitize,
  openFoliateBook,
  restructureContent,
  normalizeChapters,
  makeTxtBook,
  splitTxtChapters,
};

console.log('[foliate-bridge] ReadAware-style integration loaded');
console.log('[foliate-bridge] foliate-js available:', typeof __readawareFoliate !== 'undefined');
