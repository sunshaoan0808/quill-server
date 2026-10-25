// content-paint.js — 纯 DOM 后处理：关键词高亮 + 内容上色（引号/括号/数字/英文）
//
// 吞噬自 ColorTxt（MPL-2.0）的「内容上色 + 自定义高亮词」理念，但用 KISS 的 DOM
// 后处理实现（不引入 Monaco 编辑器、不替换 foliate 渲染器）：
//   1. 关键词高亮：来自后端 /book/{slug}/highlights + global 的词表，命中处包 <mark>
//   2. 内容上色：成对引号/括号内侧、数字、英文各自加 class，用 CSS 控制颜色
//
// 设计原则：
//   - 纯字符串分词函数（tokenize*）零 DOM 依赖，node 可单测
//   - paint* 只遍历文本节点、原地包裹，幂等（重跑不嵌套）
//   - 跳过 <script>/<style>/<code>/<pre>/<textarea>/已有的 <mark>，绝不破坏交互元素
//   - 引号/括号配对在单个文本节点内闭合；跨节点未闭合的引号优雅降级为不着色（安全）
//
// 用法：
//   import { paintDocument } from './content-paint.js';
//   paintDocument(iframeDoc, { keywords: termToColorMap, content: true });

// ── 关键词：转义正则元字符（与后端 book_highlights::escape_regex 同口径）──
const REGEX_META = /[.*+?^${}()|[\]\\]/g;
export function escapeRegex(s) {
  return s.replace(REGEX_META, '\\$&');
}

// 把 HighlightWordsByIndex（colorIndex → 词组数组）展开成「词→色」映射，
// 并构造一个按长度降序的总匹配正则（长词优先，避免「林」抢先匹配「林婉清」）。
// 数据模型保证一个词只属于一个色（upsert 时从其它色剔除），故查表唯一。
export function buildKeywordPlan(map) {
  const termToColor = new Map();
  if (!map) return { regex: null, termToColor };
  for (const [color, groups] of Object.entries(map)) {
    if (!Array.isArray(groups)) continue;
    for (const group of groups) {
      if (!Array.isArray(group)) continue;
      for (const raw of group) {
        const term = String(raw || '').trim();
        if (term) termToColor.set(term, color);
      }
    }
  }
  if (termToColor.size === 0) return { regex: null, termToColor };
  const terms = [...termToColor.keys()].sort((a, b) => b.length - a.length);
  const regex = new RegExp(terms.map(escapeRegex).join('|'), 'g');
  return { regex, termToColor };
}

// ── 角色名上色（字体色，区别于关键词背景高亮 .ct-hl）──
// 蒸馏角色表（pack.characters）每名一色，稳定 hash → data-role-color 0..7。
// 角色名无论出现在说话人 chip 还是正文提及，一律用字体色标出。
export const ROLE_COLORS = 8;

export function buildRolePlan(names) {
  const nameToColor = new Map();
  if (!Array.isArray(names)) return { regex: null, nameToColor };
  // 按角色表顺序分配色号（0..ROLE_COLORS-1 顺序取），保证 ≤ROLE_COLORS 个角色零撞色，
  // 且同一 pack 内角色顺序稳定 → 同一角色颜色稳定。区别于 hash（会撞色）。
  let idx = 0;
  for (const raw of names) {
    const t = String(raw || '').trim();
    if (!t) continue;
    if (!nameToColor.has(t)) {
      nameToColor.set(t, String(idx % ROLE_COLORS));
      idx++;
    }
  }
  if (nameToColor.size === 0) return { regex: null, nameToColor };
  const terms = [...nameToColor.keys()].sort((a, b) => b.length - a.length);
  const regex = new RegExp(terms.map(escapeRegex).join('|'), 'g');
  return { regex, nameToColor };
}

// 角色名高亮：遍历 root 下文本节点，命中处包 <span class="ct-role" data-role-color="n">。
// 幂等：跳过已在 ct-* 内的节点（由 shouldSkip 保证）。
export function paintRoleKeywords(root, plan) {
  if (!root || !plan || !plan.regex) return 0;
  const doc = ownerDoc(root);
  const walker = doc.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (shouldSkip(n) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const targets = [];
  let cur;
  while ((cur = walker.nextNode())) targets.push(cur);

  let count = 0;
  for (const node of targets) {
    const text = node.nodeValue;
    plan.regex.lastIndex = 0;
    if (!plan.regex.test(text)) continue;
    plan.regex.lastIndex = 0;
    const frag = doc.createDocumentFragment();
    let last = 0; let m;
    while ((m = plan.regex.exec(text)) !== null) {
      if (m.index > last) frag.appendChild(doc.createTextNode(text.slice(last, m.index)));
      const span = doc.createElement('span');
      span.className = 'ct-role';
      span.setAttribute('data-role-color', plan.nameToColor.get(m[0]));
      span.textContent = m[0];
      frag.appendChild(span);
      last = m.index + m[0].length;
      count++;
    }
    if (last < text.length) frag.appendChild(doc.createTextNode(text.slice(last)));
    node.parentNode.replaceChild(frag, node);
  }
  return count;
}

// 引号内台词按就近角色归属染色（覆盖默认玫红 ct-quote）。
// 启发式：同一块级内，引号内容之后/之前 ~28 字符内出现角色名 → 判定为该角色台词。
// 找不到归属则保持默认玫红，绝不臆造归属。
function findAttributionName(plan, before, after) {
  const pick = (s) => {
    plan.regex.lastIndex = 0;
    const m = plan.regex.exec(s);
    return m ? m[0] : null;
  };
  // 优先看引号后（“……他/庄眉说”），再看引号前（“庄眉说：……”）
  const a = pick(after);
  if (a) return a;
  return pick(before);
}

export function paintQuotedDialogue(root, plan) {
  if (!root || !plan || !plan.regex || !root.querySelectorAll) return 0;
  const quotes = Array.from(root.querySelectorAll('span.ct-quote'));
  let count = 0;
  for (const q of quotes) {
    if (q.classList.contains('ct-role')) continue;
    const text = (q.textContent || '').trim();
    if (text.length < 2) continue; // 单字引号（“啊”）命中歧义大，跳过
    const block = q.closest('p, div, li') || root;
    const full = block.textContent || '';
    const qText = q.textContent || '';
    const idx = full.indexOf(qText);
    if (idx < 0) continue;
    const after = full.slice(idx + qText.length, idx + qText.length + 28);
    const before = full.slice(Math.max(0, idx - 20), idx);
    const name = findAttributionName(plan, before, after);
    if (!name) continue;
    q.classList.remove('ct-quote');
    q.classList.add('ct-role');
    q.setAttribute('data-role-color', plan.nameToColor.get(name));
    count++;
  }
  return count;
}

// ── 内容上色：成对引号/括号（对齐 ColorTxt txtrTextMonarch 完整字符集）──
// open → close 映射。引号与括号分两类（上色 class 不同）。
const QUOTE_OPEN = new Map([
  ['\u201c', '\u201d'], // “ ”
  ['\u300c', '\u300d'], // 「 」
  ['\u300e', '\u300f'], // 『 』
  ['\u2018', '\u2019'], // ‘ ’
  ['"', '"'],           // 半角双引号（开闭同形）
  ['\uff02', '\uff02'], // 全角双引号 ＂（开闭同形）
]);
const BRACKET_OPEN = new Map([
  ['\u300a', '\u300b'], // 《 》
  ['\u3008', '\u3009'], // 〈 〉
  ['\uff08', '\uff09'], // （ ）
  ['\u3010', '\u3011'], // 【 】
  ['\u3016', '\u3017'], // 〖 〗
  ['\uff1c', '\uff1e'], // ＜ ＞（全角尖括号）
  ['\uff3b', '\uff3d'], // ［ ］
  ['\uff5b', '\uff5d'], // ｛ ｝
  ['[', ']'], ['(', ')'], ['{', '}'],
]);
const CLOSE_TO_OPEN = new Map();
for (const [o, c] of QUOTE_OPEN) CLOSE_TO_OPEN.set(c, o);
for (const [o, c] of BRACKET_OPEN) CLOSE_TO_OPEN.set(c, o);
const OPEN_CHARS = new Set([...QUOTE_OPEN.keys(), ...BRACKET_OPEN.keys()]);

// 数字：ASCII + 全角 ０-９；续接 . , : 及全角 ．，：（3.14 / 12:30 / １，０００）
const isDigit = (c) => (c >= '0' && c <= '9') || (c >= '\uff10' && c <= '\uff19');
const isDigitContinue = (c) => isDigit(c) || '.,:．，：'.includes(c);
// 拉丁：ASCII + 全角 Ａ-ｚ + Latin-1（跳 ×÷）+ Latin Extended（含拼音 ā/ǖ）+ NFD 组合符
const isLatin = (c) => {
  const code = c.codePointAt(0);
  if ((code >= 0x41 && code <= 0x5a) || (code >= 0x61 && code <= 0x7a)) return true;      // A-Z a-z
  if ((code >= 0xff21 && code <= 0xff3a) || (code >= 0xff41 && code <= 0xff5a)) return true; // 全角
  if ((code >= 0xc0 && code <= 0xd6) || (code >= 0xd8 && code <= 0xf6) || (code >= 0xf8 && code <= 0xff)) return true; // Latin-1
  if (code >= 0x100 && code <= 0x24f) return true;  // Latin Extended-A/B
  if (code >= 0x300 && code <= 0x36f) return true;  // NFD 组合变音符
  return false;
};
const isLatinContinue = (c) => isLatin(c) || c === "'" || c === '-' || c === '\u2019';
// 特殊标记（对齐 ColorTxt SPECIAL_MARKERS + 常见强调符）：项目符号/星花/心形/勾选/叉/音附
const MARKER_CHARS = new Set('·•▪*＊✲❈※☆♡♥○●√✔☑×✘☒♪★◇◆■▲△'.split(''));
const isMarker = (c) => MARKER_CHARS.has(c);
// 中文数字字符（含「两」）。连续串表示数量/序号：十二、三百、两千、一万五。
const CN_NUM_CHARS = new Set('〇零一二三四五六七八九两十百千万亿'.split(''));

// 把纯字符串切成片段 [{t:文本, c:class|null}]，使用传入的配对栈（支持跨文本节点）。
// class: 'ct-quote' | 'ct-bracket' | 'ct-number' | 'ct-latin' | null(普通)
// stackIn: [{cls, close}]，返回 {segs, stackOut}。调用方在文档顺序的节点间传递 stack，
// 这样即便关键词 <mark> 把一个引号对话切成多个文本节点，配对仍能跨节点闭合。
export function tokenizeContent(text, stackIn = []) {
  const out = [];
  const stack = stackIn; // 复用同一数组
  let buf = '';
  let bufClass = stack.length ? stack[stack.length - 1].cls : null;

  const flush = () => {
    if (buf) {
      out.push({ t: buf, c: bufClass });
      buf = '';
      bufClass = stack.length ? stack[stack.length - 1].cls : null;
    }
  };
  const pushCls = (s, cls) => {
    if (cls !== bufClass) { flush(); bufClass = cls; }
    buf += s;
  };
  // 连续 token 提取（返回 {text,end} 或 null）
  const matchDigit = (t, i) => {
    if (!isDigit(t[i])) return null;
    let j = i; let s = '';
    while (j < t.length && isDigitContinue(t[j])) { s += t[j]; j++; }
    return { text: s, end: j };
  };
  const matchLatin = (t, i) => {
    if (!isLatin(t[i])) return null;
    let j = i; let s = '';
    while (j < t.length && isLatinContinue(t[j])) { s += t[j]; j++; }
    return { text: s, end: j };
  };
  const matchCnNumber = (t, i) => {
    if (!CN_NUM_CHARS.has(t[i])) return null;
    let j = i; let s = '';
    while (j < t.length && CN_NUM_CHARS.has(t[j])) { s += t[j]; j++; }
    const prevCh = i > 0 ? t[i - 1] : '';
    if (s.length >= 2 || prevCh === '第') return { text: s, end: j };
    return null;
  };

  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (stack.length > 0) {
      const top = stack[stack.length - 1];
      // 闭符跟随容器色（引号闭符玫红、括号闭符蓝），不做独立标点色以免浅背景满屏碎色
      if (ch === top.close) {
        pushCls(ch, top.cls);
        stack.pop();
        flush();
        continue;
      }
      // 引号内嵌套成对括号（《书名》/（注释））→ 进括号态
      if (top.isQuote && BRACKET_OPEN.has(ch)) {
        flush();
        stack.push({ cls: 'ct-bracket', close: BRACKET_OPEN.get(ch), isQuote: false });
        pushCls(ch, 'ct-bracket');
        continue;
      }
      // 内侧：数字/英文/标记独立着色；标点与普通文字回退容器色（对话玫红/注释蓝）
      let m;
      if ((m = matchDigit(text, i))) { pushCls(m.text, 'ct-number'); i = m.end - 1; continue; }
      if ((m = matchCnNumber(text, i))) { pushCls(m.text, 'ct-number'); i = m.end - 1; continue; }
      if ((m = matchLatin(text, i))) { pushCls(m.text, 'ct-latin'); i = m.end - 1; continue; }
      if (isMarker(ch)) { pushCls(ch, 'ct-marker'); continue; }
      pushCls(ch, top.cls);
      continue;
    }
    if (OPEN_CHARS.has(ch)) {
      flush();
      const isQuote = QUOTE_OPEN.has(ch);
      const cls = isQuote ? 'ct-quote' : 'ct-bracket';
      stack.push({ cls, close: QUOTE_OPEN.get(ch) || BRACKET_OPEN.get(ch), isQuote });
      pushCls(ch, cls);
      continue;
    }
    // root：特殊标记 → 数字（阿拉伯/中文）→ 英文；标点与普通文字不着色（黑色，保持阅读流畅）
    let m;
    if (isMarker(ch)) { pushCls(ch, 'ct-marker'); continue; }
    if ((m = matchDigit(text, i))) { pushCls(m.text, 'ct-number'); i = m.end - 1; continue; }
    if ((m = matchCnNumber(text, i))) { pushCls(m.text, 'ct-number'); i = m.end - 1; continue; }
    if ((m = matchLatin(text, i))) { pushCls(m.text, 'ct-latin'); i = m.end - 1; continue; }
    pushCls(ch, null);
  }
  flush();
  return { segs: out, stackOut: stack };
}

// ── DOM 层 ──
const SKIP_TAGS = new Set([
  'SCRIPT', 'STYLE', 'NOSCRIPT', 'CODE', 'PRE', 'TEXTAREA',
  'INPUT', 'BUTTON', 'SELECT', 'OPTION', 'MARK', 'SVG', 'CANVAS',
]);

function shouldSkip(node) {
  let p = node.parentElement;
  while (p) {
    if (SKIP_TAGS.has(p.tagName)) return true;
    // 幂等：已着色的 <mark class=ct-hl> / <span class=ct-*> 内部不再重复包裹。
    // 注意用「分词后逐 token 前缀判断」而非整体 startsWith——说话人名字 chip 的 class
    // 是 "st-speaker-name ct-role"（以 st- 开头），整体前缀判断会漏判导致二次嵌套。
    const cls = typeof p.className === 'string' ? p.className : '';
    if (cls.split(/\s+/).some((c) => c === 'ct-hl' || c.startsWith('ct-'))) return true;
    p = p.parentElement;
  }
  return false;
}

// 拿到 root 所属 document（createTreeWalker/createElement/createDocumentFragment 都是 document 方法，
// root 可能是 document 本身，也可能是 body/element）。
function ownerDoc(root) {
  return root.nodeType === 9 /* DOCUMENT */ ? root : root.ownerDocument || root;
}

// 关键词高亮：遍历 root 下文本节点，命中处切分并包 <mark data-hl-color="n">。
// 幂等：跳过已在 <mark> 内的节点（由 shouldSkip 保证）。
export function paintKeywords(root, plan) {
  if (!root || !plan || !plan.regex) return 0;
  const doc = ownerDoc(root);
  const walker = doc.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (shouldSkip(n) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const targets = [];
  let cur;
  while ((cur = walker.nextNode())) targets.push(cur);

  let count = 0;
  for (const node of targets) {
    const text = node.nodeValue;
    plan.regex.lastIndex = 0;
    if (!plan.regex.test(text)) continue;
    plan.regex.lastIndex = 0;
    const frag = doc.createDocumentFragment();
    let last = 0; let m;
    while ((m = plan.regex.exec(text)) !== null) {
      if (m.index > last) frag.appendChild(doc.createTextNode(text.slice(last, m.index)));
      const mark = doc.createElement('mark');
      mark.className = 'ct-hl';
      mark.setAttribute('data-hl-color', plan.termToColor.get(m[0]));
      mark.textContent = m[0];
      frag.appendChild(mark);
      last = m.index + m[0].length;
      count++;
    }
    if (last < text.length) frag.appendChild(doc.createTextNode(text.slice(last)));
    node.parentNode.replaceChild(frag, node);
  }
  return count;
}

// 内容上色：遍历文本节点，用 tokenizeContent 切分并包 <span class="ct-*">。
// 配对栈在节点间传递，保证关键词 <mark> 切碎引号后仍能跨节点正确闭合。
export function paintContent(root) {
  if (!root) return 0;
  const doc = ownerDoc(root);
  const walker = doc.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (shouldSkip(n) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const targets = [];
  let cur;
  while ((cur = walker.nextNode())) targets.push(cur);

  let count = 0;
  const stack = []; // 跨文本节点的引号/括号配对栈
  for (const node of targets) {
    const text = node.nodeValue;
    if (!text) continue;
    const { segs } = tokenizeContent(text, stack);
    // 整段都没有着色 class 且当前不在引号内 → 无需替换（避免无意义 DOM 操作）
    if (!segs.some((s) => s.c) && stack.length === 0) continue;
    // 但如果该节点全是普通文本且无引号态，跳过；否则建片段
    if (!segs.some((s) => s.c)) continue;
    const frag = doc.createDocumentFragment();
    for (const s of segs) {
      if (!s.c) {
        frag.appendChild(doc.createTextNode(s.t));
      } else {
        const span = doc.createElement('span');
        span.className = s.c;
        span.textContent = s.t;
        frag.appendChild(span);
      }
      count++;
    }
    node.parentNode.replaceChild(frag, node);
  }
  return count;
}

// ── 合并全局与本书词表（复刻后端 book_highlights::merge_global_and_book）──
// book 同词优先：book 整组写入时，先从结果中剔除组内各词，再放入 book 的色桶。
export function mergeHighlights(global, book) {
  const g = global && typeof global === 'object' ? global : {};
  const b = book && typeof book === 'object' ? book : {};
  if (Object.keys(g).length === 0 && Object.keys(b).length === 0) return null;
  // 浅拷贝 global
  const out = JSON.parse(JSON.stringify(g));
  const removeTerm = (term) => {
    for (const k of Object.keys(out)) {
      const groups = out[k];
      if (!Array.isArray(groups)) continue;
      out[k] = groups.filter((grp) => !Array.isArray(grp) || !grp.includes(term));
      if (out[k].length === 0) delete out[k];
    }
  };
  for (const [k, groups] of Object.entries(b)) {
    const idx = Number(k);
    if (!Number.isInteger(idx) || idx < 0 || !Array.isArray(groups)) continue;
    for (const grp of groups) {
      if (!Array.isArray(grp) || grp.length === 0) continue;
      for (const w of grp) removeTerm(w);
      if (!out[k]) out[k] = [];
      out[k].push(grp);
    }
  }
  return Object.keys(out).length ? out : null;
}

// 总入口：对一个 document/element 跑角色名上色 + 关键词高亮 + 内容上色。
// opts.roles: 蒸馏角色名数组（字体色，每名一色）；opts.keywords: HighlightWordsByIndex（背景高亮）；
// opts.content: bool 是否上内容色；opts.dialogue: bool 是否做引号台词归属染色（需 roles）。
//
// 顺序：角色名先跑（包 <span class=ct-role>，在 SKIP_TAGS 内，内容色不再进入），
// 再跑关键词（<mark class=ct-hl>），再上内容色，最后做引号台词归属染色。
export function paintDocument(root, opts = {}) {
  if (!root) return { keywords: 0, content: 0, roles: 0, dialogue: 0 };
  const result = { keywords: 0, content: 0, roles: 0, dialogue: 0 };
  const rolePlan = opts.roles && opts.roles.length ? buildRolePlan(opts.roles) : null;
  if (rolePlan) {
    result.roles = paintRoleKeywords(root, rolePlan);
  }
  if (opts.keywords) {
    result.keywords = paintKeywords(root, buildKeywordPlan(opts.keywords));
  }
  if (opts.content) {
    result.content = paintContent(root);
  }
  if (opts.dialogue && rolePlan) {
    result.dialogue = paintQuotedDialogue(root, rolePlan);
  }
  return result;
}

// 注入内容上色 + 关键词高亮的 CSS（默认调色板，可被读者主题覆盖）。
export const PAINT_CSS = `
.ct-hl { border-radius: 2px; padding: 0 1px; }
.ct-hl[data-hl-color="0"] { background: #fff59d; color: inherit; }
.ct-hl[data-hl-color="1"] { background: #b9f6ca; color: inherit; }
.ct-hl[data-hl-color="2"] { background: #84ffff; color: inherit; }
.ct-hl[data-hl-color="3"] { background: #f8bbd0; color: inherit; }
.ct-hl[data-hl-color="4"] { background: #ffcc80; color: inherit; }
.ct-role[data-role-color="0"] { color: #d32f2f; }
.ct-role[data-role-color="1"] { color: #7b1fa2; }
.ct-role[data-role-color="2"] { color: #1565c0; }
.ct-role[data-role-color="3"] { color: #e65100; }
.ct-role[data-role-color="4"] { color: #2e7d32; }
.ct-role[data-role-color="5"] { color: #00838f; }
.ct-role[data-role-color="6"] { color: #9e9d24; }
.ct-role[data-role-color="7"] { color: #5d4037; }
.ct-quote   { color: var(--ct-quote, #c2185b); }
.ct-bracket { color: var(--ct-bracket, #1565c0); }
.ct-number  { color: var(--ct-number, #2e7d32); }
.ct-latin   { color: var(--ct-latin, #6a1b9a); font-style: italic; }
.ct-marker  { color: var(--ct-marker, #e53935); font-weight: 600; }
@media (prefers-color-scheme: dark) {
  .ct-hl[data-hl-color="0"] { background: #7c6f00; }
  .ct-hl[data-hl-color="1"] { background: #1b5e20; }
  .ct-hl[data-hl-color="2"] { background: #006064; }
  .ct-hl[data-hl-color="3"] { background: #880e4f; }
  .ct-hl[data-hl-color="4"] { background: #e65100; }
  .ct-role[data-role-color="0"] { color: #ff8a80; }
  .ct-role[data-role-color="1"] { color: #ce93d8; }
  .ct-role[data-role-color="2"] { color: #90caf9; }
  .ct-role[data-role-color="3"] { color: #ffab91; }
  .ct-role[data-role-color="4"] { color: #a5d6a7; }
  .ct-role[data-role-color="5"] { color: #80cbc4; }
  .ct-role[data-role-color="6"] { color: #e6ee9c; }
  .ct-role[data-role-color="7"] { color: #bcaaa4; }
  .ct-quote   { color: #f48fb1; }
  .ct-bracket { color: #90caf9; }
  .ct-number  { color: #a5d6a7; }
  .ct-latin   { color: #ce93d8; }
  .ct-marker  { color: #f56c6c; }
}
`;
