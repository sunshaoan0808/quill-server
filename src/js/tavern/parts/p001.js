/**
 * src/js/tavern.js — Story Tavern domain as a real ES module (P1-3 S2.10).
 *
 * Merged from 10 former IIFE fragments (parts.json order preserved):
 *   _tavern-core / _tavern-pack / _tavern-session / _tavern-send / _tavern-shelf /
 *   _tavern-packmgmt / _tavern-lore / _tavern-side / _tavern-char-summary /
 *   _tavern-bg-immerse (its wrapper IIFE was unwrapped).
 *
 * S2.21: 书架域（原 _tavern-packmgmt.js 书架段 4355-5061 行 + 3982/3984 两个
 *   shelf lets）逐字抽取为 src/js/shelf.js；本文件经 import 引用，
 *   loadBookshelf 保持再导出（tabs.js 与 converted[] 行消费路径不变）。
 *
 * Boundary decisions (docs/P1_3_VITE_MIGRATION.md S2.10):
 * - tavern lets (tavernPacks/Sessions/Session/Pack/Streaming/RunId/stTavernUserScrolled)
 *   moved here as canonical module state; remaining closure consumers read via
 *   stCurrentSession()/stCurrentPack().
 * - stHistoryExpanded is owned by state.js; writes go through setStHistoryExpanded().
 * - closure-only deps accessed via facades: __authToken/__appSettings/__curTab/
 *   __setSuppressHashWrite/__chatState/__storyState.
 * - escapeHtml/esc are local copies (keyboard.js precedent).
 * - ST_ICONS/PLAYABLE_LABELS/stripChoicesBlock/resolveMessageOptions now live in utils.js.
 * - All top-level DOM/window wiring runs from initTavernUI(), called by main.js.
 */
import { $ } from './dom.js';
import { api } from './api.js';
import { apiBase, friendlyError } from './api_shell.js';
import { showToast } from './toast.js';
import { showConfirm, showPrompt } from './dialog.js';
import { applyStRegexScripts } from './st_regex.js';
import { cssEscape, buildBubbleEl, fillBubbleBody } from './chat.js';
import { switchTab, closeToolsSheet, exitImmersive, updateImmersive } from './tabs_bridge.js';
import {
  uid, ADULT_OK_KEY, TAVERN_SID_KEY, ST_READPOS_PREFIX,
  PLAYABLE_LABELS, stripChoicesBlock, resolveMessageOptions,
} from './utils.js';
import { ST_VISIBLE_TURNS, setStHistoryExpanded, stHistoryExpanded } from './state.js';
import { ST_ICONS } from './utils.js';
import { switchAzView } from './tabs_bridge.js';
import { readSSE } from './jobs.js';
import { sessionId } from './state_core.js';
// S2.21: 书架域抽出为 src/js/shelf.js（shelf/bookshelf/distil-world 全段，函数体逐字保留）；
// loadBookshelf 在文件尾继续再导出（tabs.js 与 converted[] 行的消费路径不变）。
import {
  loadBookshelf, shelfPublishChat, shelfSaveSchedule, shelfRunScheduleNow,
  shelfImportFile, shelfStatus, closeShelfReader, shelfPromoteToPack, shelfExport,
  shelfActiveNovel, stDecodeTextFile,
} from './shelf.js';

/** closure-state accessors (canonical lets remain inside the virtual-module IIFE) */
const __authToken = () => window.__kaleidoAuthState.token;
const __appSettings = () => window.__kaleidoSettingsState.settings;
const __curTab = () => window.__kaleidoTabs.getCurrentTab();
const __setSuppressHashWrite = (v) => window.__kaleidoTabs.setSuppressHashWrite(v);
const __chatState = () => window.__kaleidoChatState;
const __storyState = () => window.__kaleidoStoryState;

/** local copies (_works-part/_chapter-diary-part keep their own; keyboard.js precedent) */
function escapeHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}
function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/* ===================== canonical tavern state (moved from _state-part.js) ===================== */
let tavernPacks = [];
let tavernSessions = [];
export let tavernSession = null;
export let tavernPack = null;
let tavernStreaming = false;
// S8.31: 用户手动滚动过消息区（用于流式期间保持视口在开头，尊重用户滚动）
let stTavernUserScrolled = false;
let tavernRunId = null;

/** remaining closure consumers (_tabs-part/_drift-part/_world-part) read these */
export function stCurrentSession() { return tavernSession; }
export function stCurrentPack() { return tavernPack; }

function stIcon(name) { return ST_ICONS[name] || ''; }

function stEmpty(text, action) {
  return `<div class="st-empty"><svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z"/></svg><span>${escapeHtml(text)}</span>${action ? `<span class="action">${escapeHtml(action)}</span>` : ''}</div>`;
}

const EMPTY_ICONS = {
  chat: '<svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z"/></svg>',
  book: '<svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1H6.5a1 1 0 0 1 0-5H20"/></svg>',
  user: '<svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>',
  folder: '<svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>',
  sparkle: '<svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275L12 3Z"/><path d="M5 3v4"/><path d="M19 17v4"/><path d="M3 5h4"/><path d="M17 19h4"/></svg>',
};

function emptyState(icon, title, subtitle, ctaText, ctaAction) {
  const svg = EMPTY_ICONS[icon] || EMPTY_ICONS.sparkle;
  const cta = ctaText ? '<button class="es-cta" type="button">' + escapeHtml(ctaText) + '</button>' : '';
  const html = '<div class="empty-state">' + svg + 
    (title ? '<div class="es-title">' + escapeHtml(title) + '</div>' : '') + 
    (subtitle ? '<div class="es-sub">' + escapeHtml(subtitle) + '</div>' : '') + 
    cta + '</div>';
  if (ctaAction) {
    setTimeout(() => {
      const btn = document.querySelector('.empty-state .es-cta');
      if (btn) btn.onclick = ctaAction;
    }, 0);
  }
  return html;
}

function stSkeleton(count) {
  return Array.from({ length: count || 3 }, () => '<div class="st-skeleton"><div class="line"></div><div class="line short"></div></div>').join('');
}

function stSwitchView(view) {
  const entry = $('st-view-entry');
  const wizard = $('st-view-wizard');
  const play = $('st-view-play');
  const pack = $('st-view-pack');
  if (entry) entry.classList.toggle('hidden', view !== 'entry');
  if (wizard) wizard.classList.toggle('hidden', view !== 'wizard');
  if (play) play.classList.toggle('hidden', view !== 'play');
  if (pack) pack.classList.toggle('hidden', view !== 'pack');
}

function stDisplayTitle(title) {
  let t = String(title || '').trim();
  if (!t) return '未命名剧本';
  t = t.replace(/^\[[^\]]+\]\s*/, '');
  t = t.replace(/^【([^】]{1,40})】.*/, '$1');
  t = t.replace(/（重置版）.*/, '');
  t = t.replace(/\s*1-\d+.*未完结.*/, '');
  t = t.replace(/\s*作者[:：].*$/, '');
  if (t.length > 28) t = t.slice(0, 28) + '…';
  return t;
}

function stCleanCast(pack) {
  const chars = (pack && pack.characters) || [];
  const names = (pack && pack.castNames) || [];
  if (names.length) return names.slice(0, 64);
  const junkRe = /^(露出|眼角|换鞋|随口|轻声|低头|抬起|转身|伸手|走过去|看向|听见|突然)/;
  return chars
    .filter((c) => {
      const role = String(c.role || '').toLowerCase();
      const n = String(c.name || '').trim();
      if (!n || role.includes('narrator') || role.includes('player')) return false;
      if (n === '旁白' || n === '读者' || n === '玩家') return false;
      if (n.length < 2 || n.length > 8) return false;
      if (junkRe.test(n)) return false;
      if (/[的了着在把被]/.test(n)) return false;
      return true;
    })
    .map((c) => c.name.trim())
    .slice(0, 64);
}

function stPackBlurb(pack) {
  if (pack && pack.blurb) return String(pack.blurb).slice(0, 160);
  const lore = (pack && pack.loreEntries) || [];
  const hit = lore.find((e) => /简介|概述|blurb/i.test(String(e.title || '')))
    || lore.find((e) => e.permanent)
    || lore[0];
  if (hit) {
    const t = hit.text || hit.content || '';
    if (t) return String(t).slice(0, 160);
  }
  const n0 = ((pack && pack.nodes) || [])[0];
  if (n0 && n0.summary) return String(n0.summary).slice(0, 120);
  return '';
}

function stSourceLabel(p) {
  const t = String((p && (p.sourceType || (p.source && p.source.type))) || '').toLowerCase();
  if (t === 'novel') return '小说';
  if (t === 'manual') return '自制';
  if (t === 'demo') return 'Demo';
  return t || '剧本';
}

async function stApi(path, opts = {}) {
  return api('/api/v1/story-tavern' + path, opts);
}

function adultOk() {
  // Server-side setting takes priority (persistent across devices/browser resets)
  if (__appSettings() && __appSettings().tavernAdultOk) return true;
  try { return localStorage.getItem(ADULT_OK_KEY) === '1'; } catch (_) { return false; }
}

async function setAdultOk() {
  let localWritten = false;
  try { localStorage.setItem(ADULT_OK_KEY, '1'); localWritten = true; } catch (_) {}
  // Persist first-class consent server-side; rollback the optimistic local gate on failure.
  try {
    await api('/api/v1/settings', {
      method: 'PATCH',
      body: JSON.stringify({ tavernAdultOk: true }),
    });
    __appSettings().tavernAdultOk = true;
  } catch (e) {
    if (localWritten) { try { localStorage.removeItem(ADULT_OK_KEY); } catch (_) {} }
    try { __appSettings().tavernAdultOk = false; } catch (_) {}
    throw e;
  }
}

function stStatus(text, opts) {
  const el = $('st-status');
  if (el) el.textContent = text || '';
  // S8.29: immersive mode hides the status row (top bar already shows the
  // title). Surface actionable/error messages via toast instead; drop the
  // session-state status lines (turn/focus/etc) entirely.
  if (document.documentElement.getAttribute('data-immersive') === '1') {
    const t = (text || '').trim();
    if (!t) return;
    if (/\bturn\b|\b焦点\b|\bmode\b/.test(t)) return; // status line, redundant
    if (opts && opts.silent) return; // intermediate state — no toast, only result toasts
    try {
      if (typeof showToast === 'function') showToast(t, /失败/.test(t) ? 'error' : 'info', undefined, true);
    } catch (_) {}
  }
}

let stNavFrom = '';

function stOverlayVisible(el) {
  if (!el || el.classList.contains('hidden')) return false;
  const host = el.closest('.tab-panel');
  return !host || !host.classList.contains('hidden');
}

function stHasOpenOverlay() {
  if (stOverlayVisible($('st-view-wizard'))) return true;
  if (stOverlayVisible($('st-side-panel'))) return true;
  if (stOverlayVisible($('tools-sheet'))) return true;
  const layout = $('st-layout');
  if (layout && layout.classList.contains('st-side-open')) return true;
  if (stOverlayVisible($('st-view-play'))) return true;
  return false;
}

function stPinViewHash() {
  try {
    const keep = '#/' + __curTab();
    if (location.hash !== keep) {
      __setSuppressHashWrite(true);
      history.replaceState(null, '', keep);
      setTimeout(() => { __setSuppressHashWrite(false); }, 50);
    }
  } catch (_) {}
}

function stCancelWizard(fromPop) {
  $('st-wizard').classList.add('hidden');
  $('st-view-wizard').classList.add('hidden');
  const listview = $('st-packs-listview');
  const packDetail = $('st-view-pack');
  if (stNavFrom === 'story-entry') {
    // 恢复故事馆，且不显示档案馆 pack
    stSwitchView('entry');
    if (listview) listview.classList.remove('hidden');
    if (packDetail) packDetail.classList.add('hidden');
  } else if (stNavFrom === 'packs-detail') {
    // 恢复档案馆当前包详情
    stSwitchView('pack');
    if (listview) listview.classList.add('hidden');
  } else if (fromPop) {
    // fromPop 兜底：不额外写 hash，仅恢复视图
    if (__curTab() === 'packs') {
      if (listview) listview.classList.remove('hidden');
      if (packDetail) packDetail.classList.add('hidden');
    } else {
      stSwitchView('entry');
    }
  } else {
    // 兜底：回档案馆列表
    switchTab('packs');
  }
  stPinViewHash();
}

function stExitPlay() {
  const play = $('st-view-play');
  if (play) play.classList.add('hidden');
  // 退出播放:隐藏语义记忆召回条(避免残留占满正文上方)
  const recallBar = $('st-recall-bar');
  if (recallBar) recallBar.classList.add('hidden');
  const listview = $('st-packs-listview');
  const packDetail = $('st-view-pack');
  if (stNavFrom === 'packs-detail' && __curTab() === 'packs') {
    // 从包详情进 play → 恢复包详情
    stSwitchView('pack');
    if (listview) listview.classList.add('hidden');
  } else if (__curTab() === 'packs') {
    // 从档案馆列表进 play → 回列表
    if (listview) listview.classList.remove('hidden');
    if (packDetail) packDetail.classList.add('hidden');
    const entry = $('st-view-entry');
    if (entry) entry.classList.add('hidden');
  } else {
    // 故事馆 / 其他 → entry
    stSwitchView('entry');
    if (listview) listview.classList.remove('hidden');
    if (packDetail) packDetail.classList.add('hidden');
  }
  exitImmersive();
  stRenderContinueCard();
  stPinViewHash();
}

function stGoBack(fromPop) {
  // 1) 向导可见 → 复用 R1 的取消逻辑
  if (stOverlayVisible($('st-view-wizard'))) {
    stCancelWizard(fromPop);
    return true;
  }
  // 2) st 侧栏 / 工具 sheet 可见 → 只关弹层，不跳 tab
  if (stOverlayVisible($('st-side-panel'))) {
    stCloseSidePanel();
    return true;
  }
  const layout = $('st-layout');
  if (layout && layout.classList.contains('st-side-open')) {
    layout.classList.remove('st-side-open');
    return true;
  }
  if (stOverlayVisible($('tools-sheet'))) {
    closeToolsSheet();
    return true;
  }
  // 3) play 剧场沉浸可见 → 退出 + 按来源恢复入口视图 + 还原入口 hash
  if (stOverlayVisible($('st-view-play'))) {
    stExitPlay();
    return true;
  }
  // 4) 交还给浏览器 history.back()
  if (!fromPop) history.back();
  return false;
}

let stStageEl = null;

let stStageEditing = false;

let stStageSd = null;

let stStageLastActorStates = null;

function stStageCss() {
  const css = [
    '#st-stage-modal{position:fixed;inset:0;z-index:980;display:flex;align-items:center;justify-content:center;background:rgba(8,10,18,.62);backdrop-filter:blur(3px);padding:16px}',
    '#st-stage-modal.hidden{display:none}',
    '#st-stage-modal .st-stage-box{width:min(760px,100%);max-height:86vh;overflow:auto;background:var(--bg-card,#161b28);border:1px solid var(--border,#2a3247);border-radius:16px;box-shadow:0 18px 60px rgba(0,0,0,.5);display:flex;flex-direction:column}',
    '#st-stage-modal .st-stage-head{display:flex;align-items:center;gap:10px;padding:14px 18px;border-bottom:1px solid var(--border,#2a3247);position:sticky;top:0;background:inherit;z-index:2}',
    '#st-stage-modal .st-stage-head h3{margin:0;font-size:16px;font-weight:700}',
    '#st-stage-modal .st-stage-close{margin-left:auto;background:none;border:none;color:var(--text-dim,#8b93a7);font-size:20px;cursor:pointer;line-height:1;padding:6px}',
    '#st-stage-modal .st-stage-close:hover{color:var(--text,#e8eaf2)}',
    '#st-stage-modal .st-stage-body{padding:16px 18px 20px;display:flex;flex-direction:column;gap:14px}',
    '#st-stage-modal .st-stage-sec{border:1px solid var(--border,#2a3247);border-radius:12px;padding:12px 14px;background:rgba(255,255,255,.02)}',
    '#st-stage-modal .st-stage-sec h4{margin:0 0 8px;font-size:13px;font-weight:700;color:var(--text,#e8eaf2);display:flex;align-items:center;gap:6px}',
    '#st-stage-modal .st-stage-sec h4 .st-stage-tag{margin-left:auto;font-size:11px;font-weight:500;color:var(--text-dim,#8b93a7);background:rgba(255,255,255,.06);padding:2px 8px;border-radius:99px}',
    '#st-stage-modal .st-stage-row{display:flex;flex-wrap:wrap;gap:6px 10px;font-size:12.5px;color:var(--text-dim,#aab2c5);line-height:1.5}',
    '#st-stage-modal .st-stage-row b{color:var(--text,#e8eaf2);font-weight:600}',
    '#st-stage-modal .st-stage-ev{margin-top:8px;border-left:3px solid #5b7cfa;background:rgba(91,124,250,.08);border-radius:6px;padding:8px 10px;font-size:12.5px;color:var(--text-dim,#aab2c5)}',
    '#st-stage-modal .st-stage-chip{display:inline-block;font-size:11px;background:rgba(91,124,250,.14);color:#9db4ff;border:1px solid rgba(91,124,250,.3);border-radius:99px;padding:1px 8px;margin:1px 2px}',
    '#st-stage-modal .st-stage-acts{display:flex;gap:8px;margin-top:10px;flex-wrap:wrap}',
    '#st-stage-modal .st-stage-acts button{font-size:12px;padding:5px 12px;border-radius:8px;border:1px solid var(--border,#2a3247);background:rgba(255,255,255,.04);color:var(--text,#e8eaf2);cursor:pointer}',
    '#st-stage-modal .st-stage-acts button:hover{background:rgba(91,124,250,.16);border-color:#5b7cfa}',
    '#st-stage-modal .st-stage-acts button:disabled{opacity:.5;cursor:default}',
    '#st-stage-modal .st-stage-empty{color:var(--text-dim,#6b7285);font-size:12.5px;padding:6px 2px}',
    '#st-stage-modal .st-stage-load{color:var(--text-dim,#8b93a7);font-size:12.5px;padding:8px 2px}',
    '#st-stage-modal .st-stage-err{color:#ff8a8a;font-size:12.5px;padding:8px 2px}',
    '#st-stage-modal .st-stage-sub{font-size:12px;color:var(--text-dim,#8b93a7);padding:10px 14px;border-top:1px solid var(--border,#2a3247);text-align:center}',
    /* 日间模式：浅色卡片 + 暗发丝（与剧情助手/剧场日间一致） */
    'html[data-color-scheme="day"] #st-stage-modal{background:rgba(28,25,20,.4)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-box{background:var(--surface-0);border-color:rgba(28,25,20,.16);box-shadow:0 18px 60px rgba(28,25,20,.18)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-head{border-color:rgba(28,25,20,.14)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-sec{border-color:rgba(28,25,20,.14);background:rgba(28,25,20,.03)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-sec h4{color:var(--text)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-tag{color:rgba(28,25,20,.55);background:rgba(28,25,20,.07)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-row{color:rgba(28,25,20,.72)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-row b{color:var(--text)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-close{color:rgba(28,25,20,.55)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-close:hover{color:var(--text)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-acts button{background:rgba(28,25,20,.05);border-color:rgba(28,25,20,.16);color:var(--text)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-empty{color:rgba(28,25,20,.5)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-load{color:rgba(28,25,20,.55)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-ev{color:rgba(28,25,20,.75)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-chip{color:#3350a8;background:rgba(91,124,250,.12);border-color:rgba(91,124,250,.3)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-sub{color:rgba(28,25,20,.55);border-color:rgba(28,25,20,.14)}',
    /* G16: 导演策略编辑表单 */
    '#st-stage-modal .st-stage-form{display:flex;flex-direction:column;gap:8px;margin-top:10px;padding-top:10px;border-top:1px dashed var(--border,#2a3247)}',
    '#st-stage-modal .st-stage-form .st-f-row{display:flex;align-items:center;gap:8px;flex-wrap:wrap}',
    '#st-stage-modal .st-stage-form label{font-size:12px;color:var(--text-dim,#8b93a7);min-width:84px}',
    '#st-stage-modal .st-stage-form select,#st-stage-modal .st-stage-form input[type=number]{flex:1;min-width:120px;background:rgba(255,255,255,.05);border:1px solid var(--border,#2a3247);color:var(--text,#e8eaf2);border-radius:8px;padding:5px 8px;font-size:12.5px}',
    '#st-stage-modal .st-stage-form select:focus,#st-stage-modal .st-stage-form input[type=number]:focus{outline:none;border-color:#5b7cfa}',
    '#st-stage-modal .st-stage-form .st-f-btns{display:flex;gap:8px;margin-top:6px}',
    '#st-stage-modal .st-stage-form .st-f-btns button{font-size:12px;padding:5px 14px;border-radius:8px;border:1px solid var(--border,#2a3247);background:rgba(255,255,255,.04);color:var(--text,#e8eaf2);cursor:pointer}',
    '#st-stage-modal .st-stage-form .st-f-btns button:hover{background:rgba(91,124,250,.16);border-color:#5b7cfa}',
    '#st-stage-modal .st-stage-form .st-f-btns button:disabled{opacity:.5;cursor:default}',
    '#st-stage-modal .st-stage-form .st-f-btns button.st-f-save{background:rgba(91,124,250,.18);border-color:#5b7cfa;color:#c8d4ff}',
    '#st-stage-modal .st-stage-edit-btn{font-size:11px;padding:2px 8px;border-radius:99px;border:1px solid var(--border,#2a3247);background:rgba(255,255,255,.04);color:var(--text-dim,#8b93a7);cursor:pointer;margin-left:6px}',
    '#st-stage-modal .st-stage-edit-btn:hover{background:rgba(91,124,250,.16);border-color:#5b7cfa;color:var(--text,#e8eaf2)}',
    '#st-stage-modal .st-stage-edit-form{margin:4px 0 8px;border-left:3px solid #5b7cfa;background:rgba(91,124,250,.06);border-radius:8px;padding:8px 10px}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-form{border-color:rgba(28,25,20,.14)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-form label{color:rgba(28,25,20,.6)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-form select,html[data-color-scheme="day"] #st-stage-modal .st-stage-form input[type=number]{background:rgba(28,25,20,.04);border-color:rgba(28,25,20,.16);color:var(--text)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-form .st-f-btns button{background:rgba(28,25,20,.05);border-color:rgba(28,25,20,.16);color:var(--text)}',
    'html[data-color-scheme="day"] #st-stage-modal .st-stage-form .st-f-btns button.st-f-save{color:#3350a8}',
  ].join('');
  let el = document.getElementById('st-stage-css');
  if (!el) { el = document.createElement('style'); el.id = 'st-stage-css'; document.head.appendChild(el); }
  el.textContent = css;
}

function stStagePick(v, keys) {
  if (v == null) return undefined;
  for (let k = 0; k < keys.length; k++) { if (v[keys[k]] !== undefined) return v[keys[k]]; }
  return undefined;
}

function stStageVal(v) {
  if (v == null || v === '') return '—';
  // 安全：所有调用点均将返回值拼进 innerHTML（含双引号属性），统一经 escapeHtml 转义
  return escapeHtml(String(v));
}

async function stStageFetch(path) {
  return stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + path);
}

function stStageSec(title, tag, html) {
  // 安全：title/tag 均为纯文本（各调用点无 HTML 标记），统一转义防注入
  return '<div class="st-stage-sec"><h4>' + escapeHtml(title) + (tag ? '<span class="st-stage-tag">' + escapeHtml(tag) + '</span>' : '') + '</h4>' + html + '</div>';
}

const DC_MAINLINE_OPTS = [
  ['strong_arc', '严格遵循（strong_arc）'],
  ['balanced', '主线优先（balanced）'],
  ['soft', '宽松探索（soft）'],
  ['soft_guidance', '宽松探索（soft_guidance）'],
];

const DC_MODE_OPTS = [
  ['on_demand', '按需（on_demand）'],
  ['manual', '手动（manual）'],
  ['interval', '按回合间隔（interval）'],
];

const DC_FAILURE_OPTS = [
  ['fail_forward', '失败继续推进（fail_forward）'],
  ['success_at_cost', '代价成功（success_at_cost）'],
  ['blocked', '阻断（blocked）'],
  ['hard_failure', '硬失败（hard_failure）'],
];

const DC_PACING_OPTS = [
  ['', '不指定（自由发挥）'],
  ['wave', '波浪（wave）'],
  ['goal-pressure-payoff', '目标-压力-回报（goal-pressure-payoff）'],
  ['linear', '线性（linear）'],
];

const DC_EVENT_FREQ_OPTS = [
  ['off', '关闭（off）'],
  ['sparse', '稀疏（sparse）'],
  ['balanced', '均衡（balanced）'],
  ['frequent', '频繁（frequent）'],
];

const DC_RULE_VIS_OPTS = [
  ['audit_only', '仅审计（audit_only）'],
  ['public_roll', '公开掷骰（public_roll）'],
];

function stStageOpts(opts, val) {
  return opts.map(function (o) {
    return '<option value="' + escapeHtml(o[0]) + '"' + (String(val) === o[0] ? ' selected' : '') + '>' + escapeHtml(o[1]) + '</option>';
  }).join('');
}

function stStageEditForm(sd, sid) {
  const rp = (sd && sd.runPolicy) || {};
  const v = function (o, k, fb) { return (o && o[k] !== undefined && o[k] !== null) ? o[k] : fb; };
  const fRow = function (label, inner) {
    return '<div class="st-f-row"><label>' + label + '</label>' + inner + '</div>';
  };
  return '<form class="st-stage-form" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">' +
    fRow('主线强度', '<select data-dc="mainlineStrength">' + stStageOpts(DC_MAINLINE_OPTS, v(sd, 'mainlineStrength', 'balanced')) + '</select>') +
    fRow('运行模式', '<select data-dc="mode">' + stStageOpts(DC_MODE_OPTS, v(rp, 'mode', 'on_demand')) + '</select>') +
    fRow('回合间隔', '<input type="number" min="0" step="1" data-dc="intervalTurns" value="' + escapeHtml(String(v(rp, 'intervalTurns', 0))) + '">') +
    fRow('失败策略', '<select data-dc="failurePolicy">' + stStageOpts(DC_FAILURE_OPTS, v(rp, 'failurePolicy', 'fail_forward')) + '</select>') +
    fRow('节奏曲线', '<select data-dc="pacingCurve">' + stStageOpts(DC_PACING_OPTS, v(rp, 'pacingCurve', '')) + '</select>') +
    fRow('事件频率', '<select data-dc="eventFrequency">' + stStageOpts(DC_EVENT_FREQ_OPTS, v(rp, 'eventFrequency', 'balanced')) + '</select>') +
    fRow('检定可见', '<select data-dc="ruleVisibilityMode">' + stStageOpts(DC_RULE_VIS_OPTS, v(rp, 'ruleVisibilityMode', 'audit_only')) + '</select>') +
    fRow('分支规划', '<input type="number" min="1" step="1" data-dc="branchPlanningTurns" value="' + escapeHtml(String(v(rp, 'branchPlanningTurns', 5))) + '">') +
    '<div class="st-f-btns">' +
    '<button type="submit" class="st-f-save">保存</button>' +
    '<button type="button" data-stage-edit-cancel>取消</button>' +
    '</div>' +
    '</form>';
}

async function stStageEditSave(form) {
  const sid = form.getAttribute('data-sid');
  const base = stStageSd || {};
  const read = function (name, fb) {
    const el = form.querySelector('[data-dc="' + name + '"]');
    if (!el) return fb;
    if (el.type === 'number') {
      const n = parseInt(el.value, 10);
      return isFinite(n) ? n : fb;
    }
    return el.value;
  };
  const cfg = {
    runPolicy: {
      mode: read('mode', 'on_demand'),
      intervalTurns: read('intervalTurns', 0),
      failurePolicy: read('failurePolicy', 'fail_forward'),
      pacingCurve: read('pacingCurve', ''),
      eventFrequency: read('eventFrequency', 'balanced'),
      ruleVisibilityMode: read('ruleVisibilityMode', 'audit_only'),
      branchPlanningTurns: read('branchPlanningTurns', 5),
    },
    modules: base.modules || {},
    mainlineStrength: read('mainlineStrength', 'balanced'),
    resolvedSnapshot: base.resolvedSnapshot || null,
  };
  const saveBtn = form.querySelector('[type="submit"]');
  if (saveBtn) { saveBtn.disabled = true; saveBtn.textContent = '保存中…'; }
  try {
    await stApi('/sessions/' + sid + '/director-config', { method: 'PUT', body: JSON.stringify(cfg) });
    stStatus('导演策略已保存到 Pack');
    stStageEditing = false;
    stStageSd = null;
    await stStageRender();
  } catch (e) {
    stStatus('保存失败：' + (e.message || e));
    if (saveBtn) { saveBtn.disabled = false; saveBtn.textContent = '保存'; }
  }
}

function stStageToggleEdit() {
  stStageEditing = !stStageEditing;
  if (!stStageEditing) stStageSd = null;
  stStageRender().catch(function () {});
}

async function stStageRender() {
  if (!tavernSession || !tavernSession.sessionId) return;
  const body = $('st-stage-body');
  if (!body) return;
  body.innerHTML = '<div class="st-stage-load">读取中…</div>';
  let out = '';
  // 1) 导演台（实际响应: {stageDirector, directorPlan, directorPending}）
  try {
    const dc = await stStageFetch('/director-config');
    const sd = dc.stageDirector || {};
    stStageSd = sd;
    const rp = sd.runPolicy || {};
    const plan = dc.directorPlan || {};
    const modules = sd.modules || {};
    const pending = !!dc.directorPending;
    // G2: 状态机——lastRun.status 优先（ready/running/conflict），旧数据无 lastRun 时按 pending/goal 推断
    const lr = plan.lastRun || {};
    const st = lr.status || (pending ? 'pending' : (plan.goal ? 'running' : 'idle'));
    const stLabel = { 'ready': '运行中', 'running': '执行中', 'conflict': '冲突', 'pending': '待执行', 'idle': '未启动' }[st] || escapeHtml(st);
    let h = '<div class="st-stage-row"><b>策略</b><span>' + stStageVal(rp.mode) + '</span>' +
      '<b>导演</b><span>' + stLabel + '</span>' +
      '<b>间隔</b><span>' + (rp.intervalTurns ? escapeHtml(rp.intervalTurns) + ' 回合' : '—') + '</span>' +
      '<b>主线</b><span>' + ({ 'strong_arc': '严格遵循', 'balanced': '主线优先', 'soft': '宽松探索', 'soft_guidance': '宽松探索' }[sd.mainlineStrength] || stStageVal(sd.mainlineStrength) || '主线优先') + '</span></div>';
    if (plan.goal || pending) {
      h += '<div class="st-stage-row" style="margin-top:6px"><b>计划</b><span>' + stStageVal(plan.goal) + '</span>';
      if (pending) h += '<span style="color:#ffd166">（待执行）</span>';
      h += '</div>';
      if (plan.pressure) h += '<div class="st-stage-ev">压力：' + stStageVal(plan.pressure) + '</div>';
      if (plan.cost) h += '<div class="st-stage-ev">代价：' + stStageVal(plan.cost) + '</div>';
      // G2: conflict 时显示错误原因
      if (st === 'conflict' && lr.error) h += '<div class="st-stage-ev" style="color:#ff6b6b">冲突：' + stStageVal(lr.error) + '</div>';
      // G1: 三文档展示（有内容才显示）
      if (plan.agentBrief) h += '<div class="st-stage-ev"><b>本回合</b>' + stStageVal(plan.agentBrief) + '</div>';
      if (plan.loreContext) h += '<div class="st-stage-ev"><b>铺垫</b>' + stStageVal(plan.loreContext) + '</div>';
    } else {
      h += '<div class="st-stage-empty" style="margin-top:4px">导演按需运行：点击下方按钮排程，下一回合由 LLM 生成导演计划</div>';
    }
    const pkgIds = Array.isArray(modules.eventPackageIds) ? modules.eventPackageIds : [];
    if (pkgIds.length) {
      h += '<div class="st-stage-row" style="margin-top:6px"><b>事件包</b><span>' + escapeHtml(pkgIds.join('、')) + '</span></div>';
    }
    h += '<div class="st-stage-acts">' +
      '<button type="button" data-stage-act="director" data-sid="' + encodeURIComponent(tavernSession.sessionId) + '">▶ 运行导演计划</button>' +
      '<button type="button" data-stage-act="director-edit" data-sid="' + encodeURIComponent(tavernSession.sessionId) + '">' + (stStageEditing ? '✕ 收起编辑' : '✎ 编辑策略') + '</button>' +
      '</div>';
    // G16: 编辑态展开表单
    if (stStageEditing && stStageSd) {
      h += stStageEditForm(stStageSd, tavernSession.sessionId);
    }
    // [主线完结] pack 走完终点 → 标题提示自由生长（dc.mainlineExhausted）
    try {
      if (dc.mainlineExhausted) {
        out += stStageSec('🎛 导演台', '主线已完结·自由生长中', h);
      } else {
        out += stStageSec('🎛 导演台', pending ? '待执行' : (plan.goal ? '运行中' : '空闲'), h);
      }
    } catch (_) {
      out += stStageSec('🎛 导演台', pending ? '待执行' : (plan.goal ? '运行中' : '空闲'), h);
    }
    // H3 (吞噬 humanizer-zh): 去 AI 味评分展示（lastTurnDiagnostic 直带）
    // 声线漂移：voice_drift_score ≥ 0.6 显示（确定性指纹比对，零 LLM）
    try {
      const dg = tavernSession.lastTurnDiagnostic || tavernSession.last_turn_diagnostic || null;
      const vd = dg ? (dg.voiceDriftScore || dg.voice_drift_score || 0) : 0;
      if (vd >= 0.25) {
        const vc = dg.voiceDriftChar || dg.voice_drift_char || '';
        const vr = dg.voiceDriftReasons || dg.voice_drift_reasons || [];
        out += stStageSec('🎭 声线', (vc || '某角色') + ' 漂移 ' + Math.round(vd * 100) + '%',
          '<div class="st-stage-row"><b>角色</b><span>' + stStageVal(vc) +
          '</span><b>原因</b><span>' + stStageVal(Array.isArray(vr) ? vr.join('；') : vr) + '</span></div>');
      }
      if (dg && (dg.diceForgery || dg.dice_forgery || dg.diceContradiction || dg.dice_contradiction)) {
        const dr = dg.diceReasons || dg.dice_reasons || [];
        out += stStageSec('🎲 骰审', '命中',
          '<div class="st-stage-row"><b>审计</b><span>' + stStageVal(Array.isArray(dr) ? dr.join('；') : dr) + '</span></div>');
      }
      const hallu = dg.halluNames || dg.hallu_names || [];
      const conf = dg.conflicts || [];
      if ((Array.isArray(hallu) && hallu.length) || (Array.isArray(conf) && conf.length)) {
        let hh = '';
        if (Array.isArray(hallu) && hallu.length) hh += '<div class="st-stage-row"><b>幻觉候选</b><span>' + stStageVal(hallu.join('、')) + '</span></div>';
        if (Array.isArray(conf) && conf.length) hh += '<div class="st-stage-row"><b>冲突</b><span>' + stStageVal(conf.join('；')) + '</span></div>';
        out += stStageSec('👁 幻觉/冲突', (hallu.length || 0) + ' 幻觉 · ' + (conf.length || 0) + ' 冲突', hh);
      }
      if (dg && dg.humanizeTotal) {
        const col = dg.humanizeTotal >= 45 ? '#7CFC98' : (dg.humanizeTotal >= 35 ? '#ffd166' : '#ff6b6b');
        out += stStageSec('✍️ 去 AI 味', dg.humanizeTotal + '/50 ' + (dg.humanizeGrade || ''),
          '<div class="st-stage-row"><b>评分</b><span style="color:' + col + '">' + escapeHtml(dg.humanizeTotal) + '/50 ' +
          escapeHtml(dg.humanizeGrade || '') + '</span><b>命中</b><span>' + escapeHtml(dg.humanizeHits || 0) + ' 处</span></div>');
      }
    } catch (_) {}
    // P2-3 叙界守卫事件回放（director-config 返回 guardEvents: [{...}] 最近 20 条，格式 [high|med][维度] 消息）
    const gEvents = Array.isArray(dc.guardEvents) ? dc.guardEvents : [];
    if (gEvents.length) {
      let gh = '<div class="st-stage-guard">';
      gEvents.forEach(function (ev) {
        const isHigh = /\[high\]/.test(ev);
        gh += '<div class="st-stage-ev" style="color:' + (isHigh ? '#ff6b6b' : '#ffd166') + '">' +
          (isHigh ? '🔴' : '🟡') + ' ' + stStageVal(ev) + '</div>';
      });
      gh += '</div>';
      out += stStageSec('🛡 叙界守卫', gEvents.length + ' 事件', gh);
    } else {
      out += stStageSec('🛡 叙界守卫', '0 事件', '<div class="st-stage-empty">暂无守卫事件（主线未走偏）</div>');
    }
    // X3: 虾米质检展示区（data source: dc.skimming（原 xiami 契约键已更名））。渲染失败不影响导演台其它功能。
    try {
      const xi = dc.skimming || null;
      if (xi && Array.isArray(xi.skimIssues)) {
        const issues = xi.skimIssues;
        const sample = xi.skimSample || '';
        const turn = xi.lastCheckedTurn;
        if (!issues.length) {
          out += stStageSec('🧪 速读质检', '速读', '<div class="st-stage-empty">✅ 速读质检通过</div>');
        } else {
          let xh = '<div class="st-stage-row" style="margin-top:2px"><b>速读质检</b>' +
            '<span>' + issues.length + ' 个问题' + (turn != null ? '（turn ' + escapeHtml(turn) + ' 检查）' : '') + '</span></div>';
          issues.forEach(function (it) {
            const sev = it && it.severity;
            const color = sev === 1 ? '#ff6b6b' : (sev === 2 ? '#ffd166' : '#8b93a7');
            const tag = sev === 1 ? 'P1' : (sev === 2 ? 'P2' : 'P?');
            const msg = it && it.message ? it.message : '未命名问题';
            const fix = it && it.fix ? it.fix : (it && it.evidence ? it.evidence : '');
            xh += '<div class="st-stage-ev" style="color:' + color + '">· [' + tag + '] ' + stStageVal(msg) +
              (fix ? '（修复建议：' + stStageVal(fix) + '）' : '') + '</div>';
          });
          out += stStageSec('🧪 速读质检', issues.length + ' 个问题', xh);
        }
        if (sample) {
          out += '<div class="st-stage-ev" style="margin-top:2px"><b>正文摘录（前 200 字）</b>' + stStageVal(sample) + '</div>';
        }
      } else {
        out += stStageSec('🧪 速读质检', '', '<div class="st-stage-empty">无质检记录</div>');
      }
    } catch (xe) {
      out += stStageSec('🧪 速读质检', '', '<div class="st-stage-err">质检展示失败：' + escapeHtml(xe.message || xe) + '</div>');
    }
  } catch (e) {
    out += stStageSec('🎛 导演台', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>');
  }
  // 2) 事件卡包（实际响应: {packId, packages:[{id,name,enabled,cards:[{id,title,prompt,weight}]}]}）
  try {
    const ep = await stStageFetch('/event-packages');
    const packs = Array.isArray(ep.packages) ? ep.packages : [];
    if (!packs.length) {
      out += stStageSec('🃏 事件卡包', '', '<div class="st-stage-empty">暂无事件卡包</div>');
    } else {
      let h = '';
      packs.forEach(function (p) {
        const id = stStagePick(p, ['id', 'packageId']);
        const name = stStagePick(p, ['name', 'title']) || id || '未命名包';
        const on = !!(p.enabled);
        const cards = Array.isArray(p.cards) ? p.cards : (Array.isArray(p.events) ? p.events : []);
        h += '<div class="st-stage-row" style="margin-top:2px"><b>' + stStageVal(name) + '</b>' +
          '<span style="color:' + (on ? '#7ee2a8' : '#8b93a7') + '">' + (on ? '● 启用' : '○ 停用') + '</span>' +
          '<span>' + cards.length + ' 卡</span></div>';
        if (cards.length) {
          const chips = cards.slice(0, 12).map(function (ev) {
            // G7: 展示 category/intensity/typeName/tags（旧数据无新字段则防御式退化）
            const base = stStagePick(ev, ['title', 'name', 'id']) || '';
            const cat = stStagePick(ev, ['category', 'type']) || '';
            const inten = stStagePick(ev, ['intensity']) || '';
            const tname = stStagePick(ev, ['typeName', 'type_name']) || '';
            const tags = Array.isArray(ev.tags) ? ev.tags.slice(0, 3) : [];
            let label = stStageVal(base);
            if (cat) label += ' · ' + stStageVal(cat);
            // title 属性（hover 提示）挂 intensity + typeName
            const tip = [inten, tname].filter(function (s) { return s; }).join(' ');
            const chip = '<span class="st-stage-chip"' + (tip ? ' title="' + escapeHtml(tip) + '"' : '') + '>' + label + '</span>';
            // tags 前 3 个，# 前缀
            const tagsHtml = tags.length
              ? tags.map(function (t) { return '<span class="st-stage-tag">#' + stStageVal(t) + '</span>'; }).join('')
              : '';
            return chip + tagsHtml;
          }).join('');
          h += '<div style="margin-top:4px">' + chips + (cards.length > 12 ? ' <span class="st-stage-empty">+' + (cards.length - 12) + '…</span>' : '') + '</div>';
        }
      });
      // G16: 事件卡编辑（enabled/cooldownTurns 写接口）后端暂缺 → 跳过编辑并注明
      h += '<div class="st-stage-empty" style="margin-top:6px">事件卡编辑（enabled / cooldownTurns）暂只读，需后端写接口后支持</div>';
      out += stStageSec('🃏 事件卡包', packs.length + ' 包', h);
    }
  } catch (e) {
    out += stStageSec('🃏 事件卡包', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>');
  }
  // 3) 最近事件（实际响应: {lastEvent:{turn,packageId,cardId,title,prompt,createdAt} | null}）
  try {
    const le = await stStageFetch('/last-event');
    const ev = le.lastEvent || le.event;
    if (!ev) {
      out += stStageSec('📌 最近事件', '', '<div class="st-stage-empty">尚未触发事件卡</div>');
    } else {
      const name = stStagePick(ev, ['title', 'name', 'id']);
      const kind = stStagePick(ev, ['kind', 'type']);
      const summary = stStagePick(ev, ['prompt', 'summary', 'tell', 'text']) || '';
      let h = '<div class="st-stage-row"><b>' + stStageVal(name) + '</b>';
      if (kind) h += '<span class="st-stage-chip">' + stStageVal(kind) + '</span>';
      const ts = stStagePick(ev, ['createdAt', 'triggeredAt', 'triggered_at', 'time']);
      if (ts) h += '<span>' + stStageVal(ts) + '</span>';
      h += '</div>';
      if (summary) h += '<div class="st-stage-ev">' + stStageVal(summary) + '</div>';
      out += stStageSec('📌 最近事件', '', h);
    }
  } catch (e) {
    out += stStageSec('📌 最近事件', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>');
  }
  // 4) 角色状态 + 归档（实际响应: {actorStates:{actors:{characterId: ActorStateEntry}, archive, traitPools}}）
  try {
    const as = await stStageFetch('/actor-states');
    const ast = as.actorStates || as;
    stStageLastActorStates = ast;
    // [自动罗盘] 导演台罗盘行（compass 随 actor-states 返回）
    try {
      const cp = ast.compass || {};
      const ai = (cp.authorIntent || '').trim();
      const cf = (cp.currentFocus || '').trim();
      if (ai || cf) {
        let rh = '';
        if (ai) rh += '<span>【全书承诺】' + stStageVal(ai) + '</span>';
        if (cf) rh += '<span>【近期目标】' + stStageVal(cf) + '</span>';
        out += stStageSec('🧭 罗盘', '自动', rh);
      }
    } catch (_) {}
    const actorsMap = ast.actors || {};
    const actors = (typeof actorsMap === 'object' && !Array.isArray(actorsMap))
      ? Object.keys(actorsMap).map(function (k) {
          const v = actorsMap[k] || {};
          v.characterId = v.characterId || k;
          v.character_id = v.character_id || k;
          return v;
        })
      : (Array.isArray(actorsMap) ? actorsMap : []);
    let h = '';
    if (!actors.length) {
      h = '<div class="st-stage-empty">暂无角色登记 —— 剧情回合中 LLM 输出【状态更新】后自动登记</div>';
    } else {
      actors.forEach(function (a) {
        const cid = stStagePick(a, ['characterId', 'character_id', 'id']);
        const name = stStagePick(a, ['name', 'displayName']) || cid || '未知角色';
        const traits = stStagePick(a, ['traits']);
        const tags = stStagePick(a, ['tags']);
        const notes = stStagePick(a, ['notes', 'note']);
        const fields = stStagePick(a, ['fields']);
        h += '<div class="st-stage-row"><b>' + stStageVal(name) + '</b>' +
          (cid && cid !== name ? '<span>' + stStageVal(cid) + '</span>' : '');
        if (Array.isArray(traits) && traits.length) h += '<span>特质：' + traits.map(function (t) {
          const tn = stStagePick(t, ['name', 'traitId', 'id']);
          return tn ? escapeHtml(tn) : stStageVal(t);
        }).join('、') + '</span>';
        else if (typeof traits === 'string' && traits) h += '<span>特质：' + escapeHtml(traits) + '</span>';
        h += ' <button type="button" class="st-stage-edit-btn" data-stage-edit="' + encodeURIComponent(cid || '') + '" title="手动调整属性">✏️ 编辑</button>' +
          '</div>';
        h += '<div class="st-stage-edit-form" id="st-stage-edit-' + encodeURIComponent(cid || '') + '" hidden></div>';
        if (fields && typeof fields === 'object') {
          const fkeys = Object.keys(fields);
          if (fkeys.length) {
            h += '<div style="margin-top:3px">' + fkeys.slice(0, 8).map(function (fk) {
              const fv = fields[fk] || {};
              let val = fv.value;
              if (val && typeof val === 'object') val = JSON.stringify(val);
              return '<span class="st-stage-chip" title="' + stStageVal(fk) + '">' + stStageVal(fk) + (val != null ? '=' + stStageVal(val) : '') + '</span>';
            }).join('') + '</div>';
          }
        }
        if (Array.isArray(tags) && tags.length) {
          h += '<div style="margin-top:3px">' + tags.slice(0, 10).map(function (t) {
            return '<span class="st-stage-chip">' + stStageVal(t) + '</span>';
          }).join('') + '</div>';
        }
        if (notes) h += '<div class="st-stage-ev" style="margin-top:4px">' + stStageVal(notes) + '</div>';
      });
    }
    h += '<div class="st-stage-acts">' +
      '<button type="button" data-stage-act="archive" data-sid="' + encodeURIComponent(tavernSession.sessionId) + '"' + (actors.length ? '' : ' disabled title="暂无角色可归档"') + '>📦 归档全部角色</button>' +
      '<button type="button" data-stage-act="restore" data-sid="' + encodeURIComponent(tavernSession.sessionId) + '">♻️ 恢复最近归档</button>' +
      '</div>';
    out += stStageSec('🎭 角色状态', actors.length + ' 人', h);
  } catch (e) {
    out += stStageSec('🎭 角色状态', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>');
  }
  // [吞噬 Front Porch AI pockets.dart] 口袋与衣物（per-character 只读展示，P1-B À la carte 独立开关）
  try {
    const sid = tavernSession.sessionId;
    const pk = await stStageFetch('/pockets');
    const pockets = pk.pockets || pk || {};
    const pids = Object.keys(pockets);
    // pocketsEnabled 单独开关（默认开，Own switch. Does not need the Realism Engine.）
    let pocketsEnabled = true;
    try { const pe = await stStageFetch('/pockets-enabled'); pocketsEnabled = pe.pocketsEnabled !== false; } catch (_) {}
    let evExtract = true;
    try { const ee = await stStageFetch('/event-extract'); evExtract = ee.eventExtract !== false; } catch (_) {}
    let ph = '<div class="st-stage-acts"><label style="display:inline-flex;align-items:center;gap:8px;cursor:pointer"><input type="checkbox" id="st-pockets-enabled" ' + (pocketsEnabled ? 'checked' : '') + ' /> 口袋与衣物（独立开关，提示词注入）</label><span style="margin-left:8px;font-size:11px;color:var(--text-dim,#8b93a7)">关时仅隐藏提示词注入，数据保留</span></div>';
    ph += '<div class="st-stage-acts"><label style="display:inline-flex;align-items:center;gap:8px;cursor:pointer"><input type="checkbox" id="st-event-extract" ' + (evExtract ? 'checked' : '') + ' /> ⚡ 全自动事件提取（回合末后台 LLM 写口袋/承诺/成长/羁绊）</label></div>';
    if (!pids.length) {
      ph += '<div class="st-stage-empty">暂无口袋记录 —— 角色获得/穿戴物品后此处展示</div>';
    } else {
      pids.forEach(function (cid) {
        const p = pockets[cid] || {};
        const worn = (p.worn || []).map(function (it) { return (it && it.name) ? (it.name + (it.state ? '（' + it.state + '）' : '')) : String(it); }).join('、');
        const carrying = (p.carrying || []).map(function (it) { return (it && it.name) ? (it.name + (it.state ? '（' + it.state + '）' : '')) : String(it); }).join('、');
        const setAside = (p.setAside || p.set_aside || []).map(function (e) { const it = e.item || e; return (it && it.name) ? (it.name + (it.state ? '（' + it.state + '）' : '') + (e.clothing ? ' [衣物]' : '')) : String(e); }).join('、');
        ph += '<div class="st-stage-row"><b>' + stStageVal(cid) + '</b></div>';
        if (worn) ph += '<div class="st-stage-ev">👗 身穿：' + stStageVal(worn) + '</div>';
        if (carrying) ph += '<div class="st-stage-ev">🎒 携带：' + stStageVal(carrying) + '</div>';
        if (setAside) ph += '<div class="st-stage-ev" style="color:var(--text-dim,#8b93a7)">🪑 暂存：' + stStageVal(setAside) + '（衣物次日清晨过期）</div>';
        if (!worn && !carrying && !setAside) ph += '<div class="st-stage-ev" style="color:var(--text-dim,#8b93a7)">（空）</div>';
      });
    }
    if (!pocketsEnabled) ph += '<div class="st-stage-ev" style="color:var(--text-dim,#8b93a7)">（口袋提示词注入已关闭，LLM 不会看到随身物品；数据仍保留）</div>';
    out += stStageSec('🎒 口袋与衣物' + (pocketsEnabled ? '' : ' · 已关闭'), pids.length ? pids.length + ' 人' : '0 人', ph);
  } catch (e) {
    out += stStageSec('🎭 角色状态', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>');
  }
  // [P2-A 吞噬 Front Porch AI needs_simulation.rs] Needs 六维只读+标注（0-100，urgent≤35/critical≤20）
  try {
    const nk = await stStageFetch('/needs');
    const needsMap = nk.needs || nk || {};
    const nids = Object.keys(needsMap);
    let nh = '';
    if (!nids.length) nh = '<div class="st-stage-empty">暂无 Needs 记录 —— 需求随回合自衰减</div>';
    else nids.forEach(function (cid) {
      const n = needsMap[cid] || {};
      const vec = n.vector || n || {};
      const pend = n.pendingCatastrophe || n.pending_catastrophe || '';
      const chips = ['hunger','energy','social','fun','hygiene','comfort'].map(function (k) {
        const v = vec[k]; if (v == null) return '';
        const cls = v <= 20 ? 'color:#ff6b6b' : v <= 35 ? 'color:#ffb86b' : '';
        return '<span class="st-stage-chip" style="' + cls + '">' + k + '=' + escapeHtml(v) + '</span>';
      }).filter(Boolean).join('');
      nh += '<div class="st-stage-row"><b>' + stStageVal(cid) + '</b> ' + chips + '</div>';
      if (pend) nh += '<div class="st-stage-ev" style="color:#ff6b6b">⚠️ ' + stStageVal(pend) + '</div>';
    });
    out += stStageSec('🧠 Needs 六维', nids.length ? nids.length + ' 人' : '0 人', nh + '<div class="st-stage-acts"><button type="button" data-stage-act="needs-tick" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">⏳ 推进一回合衰减</button></div>');
  } catch (e) { out += stStageSec('🧠 Needs 六维', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>'); }
  // [P2-B/P3-A 吞噬 Front Porch AI growth-rings + world] Growth / Climate
  try {
    const [gk, wc] = await Promise.all([stStageFetch('/growth').catch(function () { return { growth: { rings: [] } }; }), stStageFetch('/world-climate').catch(function () { return { worldClimate: {} }; })]);
    const rings = (gk.growth && gk.growth.rings) ? gk.growth.rings : (Array.isArray(gk.rings) ? gk.rings : []);
    const climate = wc.worldClimate || wc.world_climate || wc || {};
    const atmo = climate.atmosphere || 'breathable';
    const grav = climate.gravity || 'earth';
    const tb = climate.temp_band || climate.tempBand || 'auto';
    let gh = '';
    if (!rings.length) gh = '<div class="st-stage-empty">暂无年轮 —— 事件触发后自动登记</div>';
    else rings.forEach(function (r) {
      gh += '<div class="st-stage-row"><b>' + stStageVal(r.character || r.characterId || '') + '</b><span>' + stStageVal(r.triggerEvent || r.trigger_event || r.event || '') + '</span><span class="st-stage-chip">' + (function(s){s=Number(s||0);return s>=0.8?'established':s>=0.35?'developing':'fragile';})(r.strength) + ' ' + stStageVal((r.strength != null ? Number(r.strength).toFixed(2) : '')) + '</span>' + (r.faded ? '<span class="st-stage-chip">faded</span>' : '') + '</div>';
    });
    out += stStageSec('🌱 成长年轮', rings.length + ' 枚', gh);
    out += stStageSec('🌍 世界气候', atmo + ' · ' + grav + ' · ' + tb, '<div class="st-stage-row"><b>atmosphere</b><span>' + stStageVal(atmo) + '</span><b>gravity</b><span>' + stStageVal(grav) + '</span><b>temp_band</b><span>' + stStageVal(tb) + '</span></div><div class="st-stage-acts"><button type="button" data-stage-act="climate-edit" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">✏️ 编辑气候</button></div>');
  } catch (e) { out += stStageSec('🌱 成长年轮 / 🌍 世界气候', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>'); }
  // [P4 吞噬 Front Porch AI chaos / milestones / objectives / dreams]
  try {
    const [ch, ms, objs, dreamRes, eps] = await Promise.all([
      stStageFetch('/chaos').catch(function () { return { chaos: {} }; }),
      stStageFetch('/milestones').catch(function () { return { milestones: [] }; }),
      stStageFetch('/objectives').catch(function () { return { objectives: [] }; }),
      stStageFetch('/dreams').catch(function () { return { dream: {}, episodes: { crumbs: [] } }; }),
      stStageFetch('/episodes').catch(function () { return { episodes: { crumbs: [] } }; }),
    ]);
    const chaos = ch.chaos || ch || {};
    const miles = ms.milestones || ms || [];
    const objectives = objs.objectives || objs || [];
    const dream = (dreamRes.dream || dreamRes || {});
    const dreamText = dream.last_dream || dream.lastDream || dream.lastDreamText || '';
    const episodes = (eps.episodes && eps.episodes.crumbs) ? eps.episodes.crumbs : (dreamRes.episodes && dreamRes.episodes.crumbs) ? dreamRes.episodes.crumbs : [];
    let chh = '<div class="st-stage-row"><b>pressure</b><span>' + stStageVal(chaos.pressure != null ? chaos.pressure : 0) + '/100</span><span>' + (chaos.enabled ? 'enabled' : 'off') + '</span>' + (chaos.pendingInjection || chaos.pending_injection ? '<span class="st-stage-chip">pending</span>' : '') + '</div>';
    if (chaos.pendingInjection || chaos.pending_injection) chh += '<div class="st-stage-ev">' + stStageVal(chaos.pendingInjection || chaos.pending_injection) + '</div>';
    chh += '<div class="st-stage-acts"><button type="button" data-stage-act="chaos-toggle" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">' + (chaos.enabled ? '关闭 Chaos' : '开启 Chaos') + '</button><button type="button" data-stage-act="chaos-tick" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">🎲 推进压力</button></div>';
    out += stStageSec('🎲 Chaos / Chance Time', chaos.enabled ? 'pressure ' + (chaos.pressure||0) : 'off', chh);
    let msh = '';
    if (!miles.length) msh = '<div class="st-stage-empty">暂无里程碑</div>';
    else miles.forEach(function (m) { msh += '<div class="st-stage-row"><b>' + stStageVal(m.character) + '</b><span>' + stStageVal(m.label || m.kind) + '</span><span class="st-stage-chip">' + stStageVal(m.kind) + ' tier ' + stStageVal(m.tier) + '</span></div>'; });
    out += stStageSec('🏅 里程碑', miles.length + ' 个', msh);
    let obh = '';
    if (!objectives.length) obh = '<div class="st-stage-empty">暂无目标 —— POST /objectives 创建</div>';
    else objectives.forEach(function (o) {
      const done = (o.tasks||[]).filter(function(x){return x.completed;}).length;
      const total = (o.tasks||[]).length || 1;
      const pct = Math.round(done/total*100);
      const stage = pct>=100?'achieved':pct>=75?'nearly there':pct>=50?'halfway there':pct>=25?'gaining ground':'just beginning';
      obh += '<div class="st-stage-row"><b>' + stStageVal(o.title) + '</b><span>' + stStageVal(o.status) + ' · ' + stStageVal(o.owner) + '</span><span class="st-stage-chip">' + stage + '</span></div>';
      (o.tasks||[]).forEach(function (t) { obh += '<div class="st-stage-ev">' + (t.completed ? '✅' : '⬜') + ' ' + stStageVal(t.title) + '</div>'; });
    });
    obh += '<div class="st-stage-acts"><button type="button" data-stage-act="obj-new" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">＋ 新目标</button></div>';
    out += stStageSec('🎯 目标', objectives.length + ' 个', obh);
    const ambs = dream.ambitions || [];
    let drh = '';
    if (dreamText) drh += '<div class="st-stage-ev">💤 昨夜之梦：' + stStageVal(dreamText) + '</div>';
    if (!episodes.length && !dreamText && !ambs.length) drh = '<div class="st-stage-empty">暂无夜梦/碎屑 —— 跨夜后生成梦境，episode 记录日常</div>';
    else {
      episodes.slice(-3).forEach(function (e) { drh += '<div class="st-stage-ev">[' + stStageVal(e.kind) + '] ' + stStageVal(e.content) + '</div>'; });
      ambs.forEach(function (a) {
        const linked = objectives.filter(function(o){return o.owner===a.character && o.status!=='abandoned';});
        const pct = linked.length ? Math.round(linked.reduce(function(s,o){const d=(o.tasks||[]).filter(function(x){return x.completed;}).length;const tt=(o.tasks||[]).length||1;return s+d/tt*100;},0)/linked.length) : 0;
        const stage = pct>=100?'achieved':pct>=75?'nearly there':pct>=50?'halfway there':pct>=25?'gaining ground':'just beginning';
        drh += '<div class="st-stage-ev">野心：' + stStageVal(a.character) + ' — ' + stStageVal(a.text) + ' <span class="st-stage-chip">' + stage + '</span></div>';
      });
    }
    drh += '<div class="st-stage-acts"><button type="button" data-stage-act="episode-add" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">＋ 记录碎屑</button></div>';
    out += stStageSec('💤 夜梦 / 日常碎屑', (episodes.length||0) + ' 碎屑', drh);
    // Journal 卡片
    try {
      const jr = await stStageFetch('/journals').catch(function(){return {journals:[]};});
      let jcards = jr.journals || jr.journal_cards || jr.cards || [];
      if (!Array.isArray(jcards)) jcards = [];
      let jh = '';
      if (!jcards.length) jh = '<div class="st-stage-empty">暂无 Journal 卡片 —— POST /journals 新建（热卡常驻，冷卡召回）</div>';
      else jcards.slice(-10).forEach(function(c){
        jh += '<div class="st-stage-row"><b>' + stStageVal(c.content ? c.content.slice(0,24) : c.id) + '</b><span class="st-stage-chip">heat ' + stStageVal(c.heat != null ? Number(c.heat).toFixed(2) : '') + '</span>' + (c.pinned ? '<span class="st-stage-chip">📌 pinned</span>' : '') + (c.emotion_label ? '<span class="st-stage-chip">' + stStageVal(c.emotion_label) + '</span>' : '') + '<button type="button" class="sm" data-jpin="' + stStageVal(c.id||'') + '">' + (c.pinned?'unpin':'pin') + '</button><button type="button" class="ghost sm" data-jdel="' + stStageVal(c.id||'') + '">删</button></div>';
        if (c.content) jh += '<div class="st-stage-ev">' + stStageVal(c.content) + '</div>';
      });
      jh += '<div class="st-stage-acts"><button type="button" data-stage-act="journal-new" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">＋ 新建 Journal</button></div>';
      out += stStageSec('📓 Journal 卡片', jcards.length + ' 张', jh);
    } catch(e){ out += '<div class="st-stage-err">Journal 读取失败：' + escapeHtml(e.message||e) + '</div>'; }
    // 羁绊活数值
    try {
      const rr = await stStageFetch('/relationships').catch(function(){return {relationships:{}};});
      const rels = rr.relationships || rr || {};
      const rids = Object.keys(rels);
      let rh = '';
      if (!rids.length) rh = '<div class="st-stage-empty">暂无羁绊 —— PUT /relationships 写入 bond/trust</div>';
      else rids.forEach(function(cid){
        const b = rels[cid]||{};
        rh += '<div class="st-stage-row"><b>' + stStageVal(cid) + '</b><span>bond ' + stStageVal(b.score!=null?b.score:0) + '</span><span>trust ' + stStageVal(b.trust!=null?b.trust:0) + '</span>' + (b.fixation?'<span class="st-stage-chip">'+stStageVal(b.fixation)+'</span>':'') + (b.spatial_stance?'<span class="st-stage-chip">'+stStageVal(b.spatial_stance)+'</span>':'') + '</div>';
      });
      rh += '<div class="st-stage-acts"><button type="button" data-stage-act="rel-tick" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">⏳ 羁绊衰减</button></div>';
      out += stStageSec('💞 羁绊', rids.length + ' 人', rh);
    } catch(e){ out += '<div class="st-stage-err">羁绊读取失败：' + escapeHtml(e.message||e) + '</div>'; }
    // Our Story 时间线聚合
    try {
      const sl = await stStageFetch('/storyline').catch(function(){return {storyline:[]};});
      const items = sl.storyline || sl.items || [];
      let sh = '';
      if (!items.length) sh = '<div class="st-stage-empty">暂无时间线 —— 里程碑/Journal/年轮/目标完成后聚合</div>';
      else items.slice(-20).forEach(function(it){
        sh += '<div class="st-stage-row"><span class="st-stage-chip">' + stStageVal(it.kind||'') + '</span><span>' + stStageVal(it.text||'') + '</span><span class="st-stage-chip">turn ' + stStageVal(it.turn!=null?it.turn:'') + '</span>' + (it.receipts && it.receipts.length ? '<span class="st-stage-chip">🧾' + it.receipts.length + '</span>' : '') + '</div>';
      });
      out += stStageSec('📜 Our Story', items.length + ' 条', sh);
    } catch(e){ out += '<div class="st-stage-err">时间线读取失败：' + escapeHtml(e.message||e) + '</div>'; }
    // 心情/在场
    try {
      const [moodRes, presRes] = await Promise.all([
        stStageFetch('/mood').catch(function(){return {mood:{}};}),
        stStageFetch('/presence').catch(function(){return {presence:{}};}),
      ]);
      const mood = moodRes.mood || {};
      const pres = presRes.presence || {};
      const pids2 = Object.keys(pres);
      let mh = '';
      if (mood.summary) mh += '<div class="st-stage-ev">😶 ' + stStageVal(mood.summary) + '</div>';
      else mh += '<div class="st-stage-empty">心情平稳 —— needs/时间/天气无显著倾斜</div>';
      if (pids2.length) pids2.forEach(function(cid){
        const pr = pres[cid]||{};
        mh += '<div class="st-stage-row"><b>' + stStageVal(cid) + '</b><span>' + stStageVal(pr.occupation||'') + ' ' + stStageVal(pr.hours||'') + '</span></div>';
      });
      mh += '<div class="st-stage-acts"><button type="button" data-stage-act="presence-edit" data-sid="' + encodeURIComponent((tavernSession && tavernSession.sessionId) || '') + '">✏️ 在场设置</button></div>';
      out += stStageSec('😶 心情 / 🏢 在场', (mood.offset||0) + ' · ' + pids2.length + ' 人', mh);
    } catch(e){ out += '<div class="st-stage-err">心情读取失败：' + escapeHtml(e.message||e) + '</div>'; }
    // 世界书定时 sticky/cooldown pill
    try {
      const tw = await stStageFetch('/timed-world-info').catch(function(){return {sticky:[],cooldown:[]};});
      const st2 = tw.sticky || [];
      const cd2 = tw.cooldown || [];
      let th = '';
      if (!st2.length && !cd2.length) th = '<div class="st-stage-empty">无长效世界书 —— sticky/cooldown 条目触发后常驻多条</div>';
      else {
        st2.forEach(function(e){ th += '<div class="st-stage-row"><span class="st-stage-chip">sticky 剩' + stStageVal(e.remaining) + '条</span><span>' + stStageVal(e.key||'') + '</span></div>'; });
        cd2.forEach(function(e){ th += '<div class="st-stage-row"><span class="st-stage-chip">cooldown 剩' + stStageVal(e.remaining) + '条</span><span>' + stStageVal(e.key||'') + '</span></div>'; });
      }
      out += stStageSec('📖 世界书定时', st2.length + ' 长效', th);
    } catch(e){ out += '<div class="st-stage-err">定时读取失败：' + escapeHtml(e.message||e) + '</div>'; }
    // 承诺债务
    try {
      const pr = await stStageFetch('/promises').catch(function(){return {promises:[]};});
      const plist = pr.promises || pr || [];
      const arr = Array.isArray(plist) ? plist : [];
      const open = arr.filter(function(p){ return (p.status||'open')==='open'; });
      let ph2 = '';
      if (!open.length) ph2 = '<div class="st-stage-empty">暂无未竟承诺</div>';
      else open.slice(-5).forEach(function(p){
        ph2 += '<div class="st-stage-row"><b>' + stStageVal(p.character||'') + '</b><span>' + stStageVal(p.text||'') + '</span><span class="st-stage-chip">' + stStageVal(p.party||'') + '</span></div>';
      });
      ph2 += '<div class="st-stage-acts"><button type="button" data-stage-act="promise-new" data-sid="' + encodeURIComponent((stCurrentSession() || {}).sid || '') + '">＋ 新承诺</button></div>';
      out += stStageSec('🤝 承诺', open.length + ' 未竟', ph2);
    } catch(e){ out += '<div class="st-stage-err">承诺读取失败：' + escapeHtml(e.message||e) + '</div>'; }
  } catch (e) { out += stStageSec('🎲 / 🏅 / 🎯 / 💤 P4', '', '<div class="st-stage-err">读取失败：' + escapeHtml(e.message || e) + '</div>'); }
  body.innerHTML = out;
  // event_extract toggle
  (function () {
    const eb = document.getElementById('st-event-extract');
    if (eb) eb.onchange = async function () {
      const v = eb.checked;
      eb.disabled = true;
      try {
        await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/event-extract', { method: 'PUT', body: JSON.stringify({ eventExtract: v }) });
        stStatus(v ? '全自动事件提取已开启' : '全自动事件提取已关闭');
        await stStageRender();
      } catch (e) { stStatus('切换失败：' + (e.message || e)); eb.checked = !v; } finally { eb.disabled = false; }
    };
  })();
  // pockets_enabled toggle (must wire after innerHTML — element lives inside out)
  (function () {
    const cb = document.getElementById('st-pockets-enabled');
    if (!cb) return;
    cb.onchange = async function () {
      const v = cb.checked;
      cb.disabled = true;
      try {
        await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/pockets-enabled', { method: 'PUT', body: JSON.stringify({ pocketsEnabled: v }) });
        stStatus(v ? '口袋提示词注入已开启' : '口袋提示词注入已关闭（数据保留）');
        await stStageRender();
      } catch (e) { stStatus('切换失败：' + (e.message || e)); cb.checked = !v; } finally { cb.disabled = false; }
    };
  })();
  body.querySelectorAll('[data-jpin]').forEach(function (btn) {
    btn.onclick = async function () {
      const cid2 = btn.getAttribute('data-jpin');
      const sid2 = (tavernSession && tavernSession.sessionId) || '';
      if (!cid2 || !sid2) return;
      try {
        const cur = btn.textContent.trim() === 'pin';
        await stApi('/sessions/' + encodeURIComponent(sid2) + '/journals/' + encodeURIComponent(cid2) + '/pin', { method: 'POST', body: JSON.stringify({ pinned: cur }) });
        await stStageRender();
      } catch (e) { stStatus('pin失败：' + (e.message || e)); }
    };
  });
  body.querySelectorAll('[data-jdel]').forEach(function (btn) {
    btn.onclick = async function () {
      const cid2 = btn.getAttribute('data-jdel');
      const sid2 = (tavernSession && tavernSession.sessionId) || '';
      if (!cid2 || !sid2) return;
      if (!confirm('删除该 Journal 卡？')) return;
      try {
        await stApi('/sessions/' + encodeURIComponent(sid2) + '/journals/' + encodeURIComponent(cid2), { method: 'DELETE' });
        await stStageRender();
      } catch (e) { stStatus('删除失败：' + (e.message || e)); }
    };
  });
  body.querySelectorAll('[data-stage-act]').forEach(function (btn) {
    btn.onclick = function () {
      const act = btn.getAttribute('data-stage-act');
      const sid = btn.getAttribute('data-sid');
      // G16: 编辑态切换在本地处理（不走进度/禁用逻辑），其余走通用动作
      if (act === 'director-edit') { stStageToggleEdit(); return; }
      stStageAct(act, sid);
    };
  });
  // 角色属性编辑：点击「✏️ 编辑」展开/收起表单
  body.querySelectorAll('[data-stage-edit]').forEach(function (btn) {
    btn.onclick = function () {
      const cid = decodeURIComponent(btn.getAttribute('data-stage-edit') || '');
      const formEl = document.getElementById('st-stage-edit-' + encodeURIComponent(cid));
      if (!formEl) return;
      if (!formEl.getAttribute('hidden')) { formEl.setAttribute('hidden', ''); return; }
      // 从当前 actorStates 快照取该角色 fields，按 valueType 渲染输入控件
      const ast = stStageLastActorStates || {};
      const actorsMap = ast.actors || {};
      const actor = actorsMap[cid] || {};
      const fields = (actor.fields && typeof actor.fields === 'object') ? actor.fields : {};
      const fkeys = Object.keys(fields);
      let rows = '';
      if (!fkeys.length) {
        rows = '<div class="st-stage-empty">该角色暂无字段，可直接新增（下方输入字段名+值）</div>';
        rows += '<div class="st-stage-form"><div class="st-f-row"><label>字段名</label><input type="text" class="st-f-name" placeholder="如 服装" /></div>' +
          '<div class="st-f-row"><label>字段值</label><input type="text" class="st-f-val" placeholder="如 碎花裙" /></div>' +
          '<div class="st-f-btns"><button type="button" class="st-f-save">保存</button><button type="button" class="st-f-cancel">取消</button></div></div>';
      } else {
        rows += '<div class="st-stage-form">';
        fkeys.forEach(function (fk) {
          const fv = fields[fk] || {};
          const vt = fv.valueType || (typeof fv.value === 'number' ? 'number' : (typeof fv.value === 'boolean' ? 'bool' : 'string'));
          let cur = fv.value;
          if (cur && typeof cur === 'object') cur = JSON.stringify(cur);
          if (vt === 'number') {
            const lo = fv.min != null ? fv.min : '';
            const hi = fv.max != null ? fv.max : '';
            rows += '<div class="st-f-row"><label>' + stStageVal(fk) + '</label>' +
              '<input type="number" step="any" class="st-f-val" data-fk="' + stStageVal(fk) + '" data-vt="number" value="' + stStageVal(cur != null ? cur : '') + '"' +
              (lo !== '' ? ' min="' + escapeHtml(lo) + '"' : '') + (hi !== '' ? ' max="' + escapeHtml(hi) + '"' : '') + ' /></div>';
          } else if (vt === 'bool') {
            rows += '<div class="st-f-row"><label>' + stStageVal(fk) + '</label>' +
              '<select class="st-f-val" data-fk="' + stStageVal(fk) + '" data-vt="bool">' +
              '<option value="true"' + (cur === true || cur === 'true' ? ' selected' : '') + '>是</option>' +
              '<option value="false"' + (cur === false || cur === 'false' ? ' selected' : '') + '>否</option>' +
              '</select></div>';
          } else {
            rows += '<div class="st-f-row"><label>' + stStageVal(fk) + '</label>' +
              '<input type="text" class="st-f-val" data-fk="' + stStageVal(fk) + '" data-vt="string" value="' + stStageVal(cur != null ? cur : '') + '" /></div>';
          }
        });
        rows += '<div class="st-f-btns"><button type="button" class="st-f-save">保存</button><button type="button" class="st-f-cancel">取消</button></div></div>';
      }
      formEl.innerHTML = rows;
      formEl.removeAttribute('hidden');
      const saveBtn = formEl.querySelector('.st-f-save');
      const cancelBtn = formEl.querySelector('.st-f-cancel');
      if (cancelBtn) cancelBtn.onclick = function () { formEl.setAttribute('hidden', ''); };
      if (saveBtn) saveBtn.onclick = async function () {
        // 收集字段
        const newFields = {};
        const dynamicName = formEl.querySelector('.st-f-name');
        const dynamicVal = formEl.querySelector('.st-f-val');
        if (dynamicName && dynamicName.value.trim()) {
          const dv = dynamicVal ? dynamicVal.value.trim() : '';
          newFields[dynamicName.value.trim()] = (dynamicVal && dynamicVal.getAttribute('data-vt') === 'number') ? Number(dv) : dv;
        } else {
          formEl.querySelectorAll('.st-f-val').forEach(function (inp) {
            const fk = inp.getAttribute('data-fk');
            const vt = inp.getAttribute('data-vt');
            if (!fk) return;
            if (vt === 'number') { newFields[fk] = Number(inp.value); }
            else if (vt === 'bool') { newFields[fk] = inp.value === 'true'; }
            else { newFields[fk] = inp.value; }
          });
        }
        saveBtn.disabled = true; saveBtn.textContent = '保存中…';
        try {
          await stApi('/sessions/' + ((stCurrentSession() || {}).sid || '') + '/actor-states', {
            method: 'PUT',
            body: JSON.stringify({ characterId: cid, fields: newFields })
          });
          stStatus('已更新 ' + name + ' 的 ' + Object.keys(newFields).length + ' 个属性');
          formEl.setAttribute('hidden', '');
          await stStageRender();
        } catch (e) {
          stStatus('保存失败：' + (e.message || e));
        } finally {
          saveBtn.disabled = false; saveBtn.textContent = '保存';
        }
      };
    };
  });
  // G16: 导演策略编辑表单提交 / 取消
  const dcForm = body.querySelector('.st-stage-form');
  if (dcForm) {
    dcForm.addEventListener('submit', function (ev) {
      ev.preventDefault();
      stStageEditSave(dcForm).catch(function () {});
    });
    const cancel = dcForm.querySelector('[data-stage-edit-cancel]');
    if (cancel) {
      cancel.onclick = function () {
        stStageEditing = false;
        stStageSd = null;
        stStageRender().catch(function () {});
      };
    }
  }
}

