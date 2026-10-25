// tavern_paint.js — 故事馆（st-messages）正文全面上色外挂
//
// 2026-08-30 彩读修正：上色源改为「当前 pack 蒸馏角色表」（window.__getTavernRoster，
// 由 _chat-part.js 暴露，与说话人 chip 白名单同源），不再是"历史出现过且带标记的说话人"，
// 修掉"只有庄眉有颜色、其他角色没有"的问题。所有蒸馏角色名 + 说话人台词一律字体色。
//
// 三类上色：
//   1. 说话人 chip：.st-speaker-name 按角色名稳定 hash 分配角色字体色（data-role-color 0..7）
//   2. 说话人行台词：.st-para.st-speaker 内容整段包 ct-role（同角色字体色），
//      不再被引号玫红覆盖（ct-role 在 SKIP_TAGS 内）
//   3. 正文：content-paint 的角色名上色（roles=蒸馏角色表）+ 引号台词归属染色（dialogue）+ 内容色
//
// 纪律：
//   - 流式（.bubble.is-streaming）不上色，等完成；WeakSet/标记 幂等，重染不嵌套
//   - content-paint 自带 SKIP_TAGS（script/style/code/button/mark/ct-*），不破坏交互
//   - 说话人名先加 .ct-role 再 paintDocument，使其内部被跳过、不被二次包裹
//
// 用法：index.html 以 <script type="module" src="./assets/tavern_paint.js"> 注入。

import { paintDocument, PAINT_CSS, ROLE_COLORS, buildRolePlan } from '/web/bookshelf/foliate-reader/content-paint.js?v=20260830B';

// ── 注入上色 CSS（幂等；书架/故事馆共用 ct-* 类）──
function ensureCss() {
  if (document.getElementById('tavern-paint-css')) return;
  const s = document.createElement('style');
  s.id = 'tavern-paint-css';
  s.textContent = PAINT_CSS + '\n' +
    // 故事馆说话人名：角色字体色 + 加粗（字体色来自 .ct-role）
    '.st-speaker-name.ct-role{font-weight:700;}' +
    // 说话人行内角色色台词：略加重，区分于普通正文
    '.st-para.st-speaker .ct-role[data-role-color]{font-weight:500;}';
  document.head.appendChild(s);
}

// 角色名 → 角色色：优先用 buildRolePlan 的按序色表（与正文角色名同色）；
// 不在表内（兜底收集到的名字）退化为稳定 hash。
function colorForName(name, roleMap) {
  if (roleMap && roleMap.has(name)) return roleMap.get(name);
  let h = 0;
  for (const ch of String(name)) h = (h * 31 + ch.codePointAt(0)) >>> 0;
  return h % ROLE_COLORS;
}

// 当前蒸馏角色名单：优先 _chat-part 暴露的 pack 蒸馏角色表；兜底收集已渲染说话人。
function collectRoster() {
  const names = new Set();
  try {
    if (typeof window !== 'undefined' && typeof window.__getTavernRoster === 'function') {
      for (const n of window.__getTavernRoster()) if (n) names.add(n);
    }
  } catch (_) {}
  document.querySelectorAll('#st-messages .st-speaker-name').forEach((n) => {
    const t = (n.textContent || '').trim();
    if (t) names.add(t);
  });
  return Array.from(names);
}

const paintedBodies = new WeakSet();

// 说话人 chip 染色 + 说话人行台词整段同角色字体色（幂等）
function paintSpeakerLines(body, roleMap) {
  body.querySelectorAll('.st-para.st-speaker').forEach((p) => {
    const nameEl = p.querySelector(':scope > .st-speaker-name');
    if (!nameEl) return;
    const name = (nameEl.textContent || '').trim();
    if (!name) return;
    const c = String(colorForName(name, roleMap));
    // 1) 名字 chip 字体色
    if (!nameEl.classList.contains('ct-role')) {
      nameEl.classList.add('ct-role');
      nameEl.setAttribute('data-role-color', c);
    }
    // 2) 台词内容（名字/头像之外的子节点）包成同角色色 span（幂等）
    if (p.dataset.roleLinePainted) return;
    p.dataset.roleLinePainted = '1';
    const wrap = [];
    for (const child of p.childNodes) {
      if (child === nameEl) continue;
      if (child.nodeType === 1 && child.classList && child.classList.contains('st-speaker-avatar')) continue;
      if (child.nodeType === 3) {
        if ((child.textContent || '').trim()) wrap.push(child);
      } else if (child.nodeType === 1) {
        wrap.push(child);
      }
    }
    if (wrap.length) {
      const span = p.ownerDocument.createElement('span');
      span.className = 'ct-role';
      span.setAttribute('data-role-color', c);
      for (const child of wrap) span.appendChild(child);
      p.appendChild(span);
    }
  });
}

function paintBubbleBody(body) {
  if (!body || paintedBodies.has(body)) return;
  // 流式输出中：等完成（不标记，下次 observer 触发再染）
  const bubble = body.closest('.bubble');
  if (bubble && bubble.classList.contains('is-streaming')) return;
  paintedBodies.add(body);

  const roster = collectRoster();
  const roleMap = buildRolePlan(roster).nameToColor;

  // 1) 说话人 chip + 台词行 同角色字体色（先于 paintDocument，使其内部被 ct- 跳过）
  paintSpeakerLines(body, roleMap);

  // 2) 正文：角色名上色 + 引号台词归属染色 + 内容色
  try {
    paintDocument(body, { roles: roster, dialogue: true, content: true });
  } catch (e) {
    // 上色失败绝不影响阅读：静默降级为不上色
    console.warn('[tavern_paint] paint failed:', e);
  }
}

function scan() {
  document.querySelectorAll('#st-messages .bubble-body').forEach(paintBubbleBody);
}

let scanTimer = null;
function scheduleScan() {
  clearTimeout(scanTimer);
  scanTimer = setTimeout(scan, 250);
}

function init() {
  ensureCss();
  const root = document.getElementById('st-messages');
  if (!root) {
    // 故事馆视图尚未进入，轮询等待（脚本在 head/body 早期加载时）
    setTimeout(init, 800);
    return;
  }
  // childList（新消息/新段落）+ characterData（流式文本）+ attributes（is-streaming 移除）
  const mo = new MutationObserver(scheduleScan);
  mo.observe(root, {
    childList: true,
    subtree: true,
    characterData: true,
    attributes: true,
    attributeFilter: ['class'],
  });
  scan();
}

init();
