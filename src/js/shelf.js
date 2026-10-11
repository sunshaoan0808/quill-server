/**
 * src/js/shelf.js — 书架（Bookshelf）域 as a real ES module (P1-3 S2.21).
 *
 * Extracted verbatim from src/js/tavern.js (the former _tavern-packmgmt.js
 * shelf section, tavern.js lines 4355-5061 + the two shelf lets at 3982/3984):
 *   shelfStatus / shelfApi / loadShelfChatSessions / loadShelfSchedule /
 *   shelfPublishChat / shelfSaveSchedule / shelfRunScheduleNow / loadBookshelf /
 *   renderBookshelfGrid / openShelfReader / closeShelfReader / shelfPromoteToPack,
 *   the distil-world job cluster (SHELF_DISTIL_JOBS_KEY / _shelfWatchers /
 *   shelf*Distil* / bindShelfDistilActions / buildDistilReportHtml /
 *   stCloseDistilReport / showDistilReport / finishDistil* / watchDistilJob /
 *   shelfSyncDistilJobs / shelfDistilWorld) and shelfExport / stDecodeTextFile /
 *   shelfImportFile.
 *
 * Boundary decisions:
 * - shelfNovels/shelfActiveSlug/_shelfWatchers are module state here now;
 *   tavern.js initTavernUI's reader-to-pack/reader-export handlers read them
 *   through the shelfActiveNovel() accessor.
 * - loadBookshelf is re-exported by tavern.js (tabs.js and the converted[]
 *   line in vite.config.mjs import it from there) — zero consumer edits.
 * - stDecodeTextFile is exported back to tavern.js (stImportNovel consumes it).
 * - escapeHtml is a local copy (keyboard.js / S2.10 precedent).
 * - The tavern.js ⇄ shelf.js import cycle is safe: tavern bindings are only
 *   referenced inside function bodies (runtime live bindings), never at
 *   module top level.
 */
import { $ } from './dom.js';
import { api } from './api.js';
import { apiBase } from './api_shell.js';
import { readSSE } from './jobs.js';
import { switchTab } from './tabs_bridge.js';
import { stApi, stStatus, stTurnLabel, stLoadPacks, stShowPack } from './tavern.js';
import mammoth from 'mammoth/mammoth.browser.js';

/** local copy (tavern.js keeps its own; keyboard.js precedent) */
function escapeHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/* ===================== canonical shelf state (moved from tavern.js) ===================== */
let shelfNovels = [];
let shelfActiveSlug = null;

function shelfStatus(msg) {
  const el = $('shelf-status');
  if (el) el.textContent = msg || '';
}

function shelfApi(path, opts = {}) {
  return api('/api/v1/crawler' + path, opts);
}

async function loadShelfChatSessions() {
  const sel = $('shelf-chat-session');
  if (!sel) return;
  sel.innerHTML = '<option value="">加载中…</option>';
  try {
    const data = await stApi('/sessions');
    const list = (data && data.sessions) || [];
    sel.innerHTML = '';
    if (!list.length) {
      const o = document.createElement('option');
      o.value = ''; o.textContent = '（暂无故事馆会话）';
      sel.appendChild(o);
      return;
    }
    for (const s of list) {
      const o = document.createElement('option');
      o.value = s.sessionId;
      const t = s.title || s.sessionId;
      o.textContent = t + ' · ' + stTurnLabel(s.turn || 0);
      sel.appendChild(o);
    }
  } catch (e) {
    sel.innerHTML = '';
    const o = document.createElement('option');
    o.value = ''; o.textContent = '加载失败：' + (e.message || e);
    sel.appendChild(o);
  }
}

async function loadShelfSchedule() {
  try {
    const data = await shelfApi('/chat-to-shelf/schedule');
    const sch = (data && data.schedule) || {};
    if ($('shelf-sched-enabled')) $('shelf-sched-enabled').checked = !!sch.enabled;
    if ($('shelf-sched-hours')) $('shelf-sched-hours').value = sch.intervalHours || 24;
    if ($('shelf-sched-turns')) $('shelf-sched-turns').value = sch.minTurns || 3;
    if ($('shelf-sched-topack')) $('shelf-sched-topack').checked = sch.toPack !== false;
    const meta = $('shelf-sched-meta');
    if (meta) {
      const last = sch.lastRunAt ? ('上次 ' + String(sch.lastRunAt).slice(0, 19).replace('T', ' ')) : '尚未运行';
      const lr = sch.lastResult || {};
      meta.textContent = (sch.enabled ? '定时开 · ' : '定时关 · ') + last +
        (lr.publishedCount != null ? (' · 上架 ' + lr.publishedCount + ' / 跳过 ' + (lr.skipped || 0)) : '');
    }
  } catch (e) {
    const meta = $('shelf-sched-meta');
    if (meta) meta.textContent = '定时配置读取失败：' + (e.message || e);
  }
}

async function shelfPublishChat() {
  const sid = ($('shelf-chat-session') && $('shelf-chat-session').value) || '';
  if (!sid) { shelfStatus('请选择故事馆会话'); return; }
  const title = (($('shelf-chat-title') && $('shelf-chat-title').value) || '').trim();
  const toPack = !($('shelf-chat-topack') && !$('shelf-chat-topack').checked);
  shelfStatus('正在整理并上架…');
  try {
    const body = { source: 'tavern', sessionId: sid, toPack: toPack, force: true };
    if (title) body.title = title;
    const data = await shelfApi('/chat-to-shelf', { method: 'POST', body: JSON.stringify(body) });
    if (!data || data.ok === false) throw new Error((data && data.error) || '失败');
    shelfStatus((data.skipped ? '未变化：' : '已上架：') + (data.title || '') +
      (data.chapterCount ? (' · ' + data.chapterCount + ' 章') : '') +
      (data.packId ? (' · Pack ' + data.packId) : ''));
    await loadBookshelf();
    if (data.packId && typeof stLoadPacks === 'function') {
      try { await stLoadPacks(); } catch (_) {}
    }
  } catch (e) {
    shelfStatus('上架失败：' + (e.message || e));
  }
}

async function shelfSaveSchedule() {
  const body = {
    enabled: !!( $('shelf-sched-enabled') && $('shelf-sched-enabled').checked ),
    intervalHours: Math.max(1, parseInt(($('shelf-sched-hours') && $('shelf-sched-hours').value) || '24', 10) || 24),
    minTurns: Math.max(1, parseInt(($('shelf-sched-turns') && $('shelf-sched-turns').value) || '3', 10) || 3),
    toPack: !($('shelf-sched-topack') && !$('shelf-sched-topack').checked),
    source: 'tavern',
  };
  try {
    const data = await shelfApi('/chat-to-shelf/schedule', { method: 'PUT', body: JSON.stringify(body) });
    if (!data || data.ok === false) throw new Error((data && data.error) || '保存失败');
    shelfStatus(body.enabled ? ('定时已开启：每 ' + body.intervalHours + ' 小时 · 最少 ' + body.minTurns + ' 回合') : '定时已关闭');
    await loadShelfSchedule();
  } catch (e) {
    shelfStatus('保存定时失败：' + (e.message || e));
  }
}

async function shelfRunScheduleNow() {
  shelfStatus('正在执行定时整理…');
  try {
    const data = await shelfApi('/chat-to-shelf/run-due', { method: 'POST', body: '{}' });
    if (!data || data.ok === false) throw new Error((data && data.error) || '失败');
    if (data.ran === false) {
      shelfStatus('未运行：' + (data.reason || '定时未启用（先勾选并保存）'));
    } else {
      shelfStatus('本轮上架 ' + (data.publishedCount || 0) + ' · 跳过 ' + (data.skipped || 0) +
        ((data.errors && data.errors.length) ? (' · 错误 ' + data.errors.length) : ''));
    }
    await loadBookshelf();
    await loadShelfSchedule();
  } catch (e) {
    shelfStatus('执行失败：' + (e.message || e));
  }
}

async function loadBookshelf() {
  const grid = $('bookshelf-grid');
  if (!grid) return;
  grid.innerHTML = '<p class="muted">加载中…</p>';
  try {
    const data = await shelfApi('/novels');
    shelfNovels = (data && data.novels) || [];
    // API uses snake_case; normalize once so cards never show a false 0 章.
    shelfNovels = shelfNovels.map((n) => ({ ...n, chapterCount: n.chapterCount ?? n.chapter_count ?? 0, hasCover: n.hasCover ?? n.has_cover ?? false }));
    renderBookshelfGrid();
    shelfStatus(shelfNovels.length ? ('共 ' + shelfNovels.length + ' 部') : '书架为空 — 可导入 TXT/MD/DOCX');
    loadShelfChatSessions().catch(() => {});
    loadShelfSchedule().catch(() => {});
    // 恢复进行中/刚完成的转换任务订阅（刷新后 localStorage 记录仍在）
    if (typeof shelfSyncDistilJobs === 'function') shelfSyncDistilJobs().catch(() => {});
  } catch (e) {
    grid.innerHTML = '<p class="err">加载失败：' + escapeHtml(e.message || String(e)) + '</p>';
    shelfStatus('加载失败');
  }
}

function renderBookshelfGrid() {
  const grid = $('bookshelf-grid');
  if (!grid) return;
  if (!shelfNovels.length) {
    grid.innerHTML = '<div class="st-empty"><span>书架上空空如也</span><span class="action">导入 TXT/MD/DOCX 后可阅读，并一键进故事馆</span></div>';
    return;
  }
  grid.innerHTML = '';
  // Keep the grid scrollable on mobile; the parent tab must not clip long shelves.
  grid.style.overflow = 'visible';
  for (const n of shelfNovels) {
    const card = document.createElement('div');
    card.className = 'shelf-card';
    card.dataset.slug = n.slug;
    card.dataset.testid = 'shelf-card';
    card.setAttribute('data-book-slug', n.slug);
    const cover = n.hasCover
      ? '<img class="shelf-cover" src="' + escapeHtml(apiBase() + '/api/v1/crawler/novels/' + encodeURIComponent(n.slug) + '/cover') + '" alt="" loading="lazy" />'
      : '<div class="shelf-cover shelf-cover-empty">📖</div>';
    card.innerHTML =
      cover +
      '<div class="shelf-card-body">' +
        '<div class="shelf-title">' + escapeHtml(n.title || n.slug) + '</div>' +
        '<div class="shelf-meta muted sm">' + (n.chapterCount || 0) + ' 章' + (n.hasCover ? ' · 有封面' : '') + '</div>' +
        '<div class="shelf-actions row gap-sm wrap">' +
          '<button type="button" class="sm shelf-read" data-testid="shelf-read" data-action="read" data-book-slug="' + escapeHtml(n.slug) + '" aria-label="阅读' + escapeHtml(n.title || n.slug) + '">阅读</button>' +
          '<button type="button" class="sm shelf-play" data-testid="shelf-convert" data-action="convert" data-book-slug="' + escapeHtml(n.slug) + '" aria-label="开始转换' + escapeHtml(n.title || n.slug) + '">开始转换</button>' +
          '<button type="button" class="ghost sm shelf-export" data-testid="shelf-export" data-action="export" data-book-slug="' + escapeHtml(n.slug) + '" aria-label="导出' + escapeHtml(n.title || n.slug) + '">导出</button>' +
        '</div>' +
        '<div class="shelf-distil-progress" data-slug="' + escapeHtml(n.slug) + '" hidden></div>' +
      '</div>';
    card.querySelector('.shelf-read').onclick = (ev) => { ev.stopPropagation(); openShelfReader(n.slug); };
    card.querySelector('.shelf-play').onclick = (ev) => { ev.stopPropagation(); shelfDistilWorld(n.slug, n.title); };
    card.querySelector('.shelf-export').onclick = (ev) => { ev.stopPropagation(); shelfExport(n.slug, n.title); };
    card.onclick = () => openShelfReader(n.slug);
    grid.appendChild(card);
    if (typeof shelfRenderDistilProgress === 'function') shelfRenderDistilProgress(n.slug);
  }
}

async function openShelfReader(slug) {
  shelfActiveSlug = slug;
  const overlay = $('novel-reader');
  const titleEl = $('reader-title');
  const bodyEl = $('reader-content');
  if (!overlay || !bodyEl) return;
  overlay.classList.remove('hidden');
  if (titleEl) titleEl.textContent = '加载中…';
  bodyEl.textContent = '…';
  try {
    const data = await shelfApi('/novels/' + encodeURIComponent(slug) + '/content');
    if (titleEl) titleEl.textContent = data.title || slug;
    bodyEl.textContent = data.content || '';
  } catch (e) {
    if (titleEl) titleEl.textContent = slug;
    bodyEl.textContent = '读取失败：' + (e.message || e);
  }
}

function closeShelfReader() {
  const overlay = $('novel-reader');
  if (overlay) overlay.classList.add('hidden');
}

async function shelfPromoteToPack(slug, title) {
  shelfStatus('正在生成故事馆 Pack…');
  try {
    const data = await shelfApi('/novels/' + encodeURIComponent(slug) + '/to-pack', {
      method: 'POST',
      body: JSON.stringify({}),
    });
    const packId = data.packId;
    if (!packId) throw new Error(data.error || '未返回 packId');
    shelfStatus((data.existed ? '已有 Pack：' : '已生成 Pack：') + (data.title || title || packId));
    if (typeof switchTab === 'function') switchTab('tavern');
    if (typeof stLoadPacks === 'function') await stLoadPacks();
    if (typeof stShowPack === 'function') {
      try { await stShowPack(packId); } catch (_) {}
    }
    if (typeof stStatus === 'function') {
      stStatus('书架 → 故事馆：' + (data.title || title || packId) + ' · 可点「用此包开玩」');
    }
  } catch (e) {
    shelfStatus('进故事馆失败：' + (e.message || e));
  }
}

const SHELF_DISTIL_JOBS_KEY = 'shelfDistilJobs';

const _shelfWatchers = new Set();

// 轮询兜底的连续失败上限（20 次 × 3s ≈ 60s）：超限终止监听并判失败，
// 防止服务重启/网络长时间中断后每 3s 永久重试、UI 永远卡「运行中」。
const SHELF_DISTIL_POLL_FAIL_LIMIT = 20;

function shelfReadDistilJobs() {
  try {
    const raw = localStorage.getItem(SHELF_DISTIL_JOBS_KEY);
    const obj = raw ? JSON.parse(raw) : {};
    return (obj && typeof obj === 'object') ? obj : {};
  } catch (_) { return {}; }
}

function shelfWriteDistilJobs(obj) {
  try { localStorage.setItem(SHELF_DISTIL_JOBS_KEY, JSON.stringify(obj || {})); } catch (_) {}
}

function shelfDistilJobFor(slug) {
  return shelfReadDistilJobs()[slug] || null;
}

function shelfDropDistilJob(slug) {
  const obj = shelfReadDistilJobs();
  if (obj[slug]) {
    delete obj[slug];
    shelfWriteDistilJobs(obj);
  }
  shelfRenderDistilProgress(slug);
}

function shelfDistilActive(status) {
  return status === 'queued' || status === 'running' || status === 'pending';
}

function shelfRenderDistilProgress(slug) {
  const slots = document.querySelectorAll('.shelf-distil-progress');
  for (const slot of slots) {
    const s = slot.getAttribute('data-slug');
    if (slug && s !== slug) continue;
    const job = shelfDistilJobFor(s);
    const card = slot.closest('.shelf-card');
    const btn = card && card.querySelector('.shelf-play');
    if (!job) {
      slot.classList.add('hidden');
      slot.innerHTML = '';
      if (btn) { btn.disabled = false; btn.classList.remove('disabled'); btn.textContent = '开始转换'; }
      continue;
    }
    slot.classList.remove('hidden');
    const pct = Math.max(0, Math.min(100, Math.round((Number(job.progress) || 0) * 100)));
    const activeNow = shelfDistilActive(job.status);
    const stage = job.status === 'succeeded' ? '完成'
      : job.status === 'failed' || job.status === 'cancelled' ? '失败'
        : job.message || '转换中';
    slot.innerHTML =
      '<div class="shelf-distil-progress-bar"><div class="bar" style="width:' + pct + '%"></div></div>' +
      '<div class="shelf-distil-progress-stage">' + escapeHtml(stage) + ' · ' + pct + '%</div>' +
      shelfDistilActionHtml(job) +
      (job.status === 'succeeded' && job.report
        ? '<div class="shelf-distil-report-actions"><button type="button" class="shelf-distil-report-btn">📋 查看蒸馏报告</button></div>'
        : '');
    const rbtn = slot.querySelector('.shelf-distil-report-btn');
    if (rbtn) {
      rbtn.onclick = (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        showDistilReport(job.report, job.title || s);
      };
    }
    bindShelfDistilActions(slot, job);
    if (btn) {
      btn.disabled = !!activeNow;
      btn.classList.toggle('disabled', !!activeNow);
      btn.textContent = activeNow ? '转换中…' : job.status === 'succeeded' ? '转换完成' : '开始转换';
    }
  }
}

function shelfDistilPaused(job) {
  return job && (job.status === 'running' || job.status === 'queued')
    && job.message && job.message.indexOf('已暂停') >= 0;
}

function shelfDistilActionHtml(job) {
  const jid = job && job.jobId;
  if (!jid) return '';
  let html = '<div class="shelf-distil-actions">';
  if (shelfDistilPaused(job)) {
    html += '<button type="button" class="shelf-distil-ctl" data-action="resume">▶ 继续</button>';
    html += '<button type="button" class="shelf-distil-ctl" data-action="cancel">⏹ 取消</button>';
  } else if (shelfDistilActive(job.status)) {
    html += '<button type="button" class="shelf-distil-ctl" data-action="pause">⏸ 暂停</button>';
    html += '<button type="button" class="shelf-distil-ctl" data-action="cancel">⏹ 取消</button>';
  } else if (job.status === 'failed' || job.status === 'cancelled' || job.status === 'error') {
    html += '<button type="button" class="shelf-distil-ctl" data-action="retry">↻ 重试</button>';
  }
  html += '</div>';
  return html;
}

function bindShelfDistilActions(slot, job) {
  const jid = job && job.jobId;
  if (!jid) return;
  const actions = slot.querySelectorAll('.shelf-distil-ctl');
  for (const a of actions) {
    a.onclick = async (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      const action = a.getAttribute('data-action');
      if (!action) return;
      a.disabled = true;
      try {
        await api('/api/v1/jobs/' + encodeURIComponent(jid) + '/' + action, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({}),
        });
        shelfStatus(action === 'pause' ? '⏸ 已请求暂停，当前阶段完成后生效…'
          : action === 'cancel' ? '⏹ 已请求取消…'
            : action === 'retry' ? '↻ 已重新启动，从断点续跑…'
              : '▶ 已请求继续…');
        // 立即刷新一次进度，随后由 SSE/轮询驱动
        shelfRenderDistilProgress(slot.getAttribute('data-slug'));
      } catch (e) {
        a.disabled = false;
        shelfStatus((action === 'pause' ? '暂停失败：' : action === 'cancel' ? '取消失败：' : action === 'retry' ? '重试失败：' : '继续失败：') + ((e && (e.message || e.statusText)) || e));
      }
    };
  }
}

function buildDistilReportHtml(report) {
  const esc = (v) => escapeHtml(v == null ? '' : v);
  const blank = '<div class="st-distil-report-empty">—</div>';
  const list = (items) => {
    if (!items || !items.length) return blank;
    return '<ul class="st-distil-report-list">' + items.map((x) => '<li>' + x + '</li>').join('') + '</ul>';
  };
  let out = '';

  const chars = (report && report.characters) || [];
  out += '<h4 class="st-distil-report-sec">角色（' + chars.length + '）</h4>';
  out += chars.length
    ? '<ul class="st-distil-report-list">' + chars.map((c) => '<li><span class="st-distil-report-name">' + esc(c.name) + '</span>' + (c.role ? ' <span class="st-distil-report-meta">· ' + esc(c.role) + '</span>' : '') + '</li>').join('') + '</ul>'
    : blank;

  const lore = (report && report.lore) || [];
  out += '<h4 class="st-distil-report-sec">世界书（' + lore.length + '）</h4>';
  out += list(lore.map((v) => esc(v.title)));

  const beats = (report && report.beats) || {};
  out += '<h4 class="st-distil-report-sec">节拍与出口</h4>';
  out += '<div class="st-distil-report-stat">' +
    '<span>节点 ' + (Number(beats.node_count) || 0) + '</span>' +
    '<span>节拍 ' + (Number(beats.beat_count) || 0) + '</span>' +
    '<span>出口 ' + (Number(report.exits) || 0) + '</span>' +
    '</div>';

  const wl = (report && report.worldline) || [];
  out += '<h4 class="st-distil-report-sec">世界线（' + wl.length + '）</h4>';
  out += list(wl.map((v) => esc(v.title)));

  const eps = (report && report.event_packages) || [];
  out += '<h4 class="st-distil-report-sec">事件包（' + eps.length + '）</h4>';
  out += list(eps.map((p) => esc(p.name)));

  const templates = (report && report.actor_templates) || [];
  out += '<h4 class="st-distil-report-sec">演员模板（' + templates.length + '）</h4>';
  out += templates.length
    ? '<ul class="st-distil-report-list">' + templates.map((t) => '<li><span class="st-distil-report-name">' + esc(t.name) + '</span> <span class="st-distil-report-meta">· ' + (Number(t.field_count) || 0) + ' 字段</span></li>').join('') + '</ul>'
    : blank;

  const style = report && report.narrative_style;
  out += '<h4 class="st-distil-report-sec">文风</h4>';
  out += style ? '<div class="st-distil-report-style">' + esc(String(style).slice(0, 200)) + '</div>' : blank;

  const checks = (report && report.rule_checks) || [];
  out += '<h4 class="st-distil-report-sec">规则检定（' + checks.length + '）</h4>';
  out += checks.length
    ? '<ul class="st-distil-report-list">' + checks.map((c) => {
        const label = c.label || c.id || '检定';
        return '<li><span class="st-distil-report-name">' + esc(label) + '</span>' + (c.dice ? ' <span class="st-distil-report-meta">· ' + esc(c.dice) + '</span>' : '') + '</li>';
      }).join('') + '</ul>'
    : blank;

  return out;
}

function stCloseDistilReport() {
  const m = $('st-distil-report-modal');
  if (m) m.classList.add('hidden');
}

function showDistilReport(report, doneTitle) {
  let m = $('st-distil-report-modal');
  if (!m) {
    m = document.createElement('div');
    m.id = 'st-distil-report-modal';
    m.className = 'st-modal hidden';
    m.setAttribute('role', 'dialog');
    m.setAttribute('aria-modal', 'true');
    m.setAttribute('aria-label', '蒸馏报告');
    m.innerHTML =
      '<div class="st-modal-card st-distil-report-card">' +
      '<div class="st-modal-head">' +
      '<span class="st-modal-title st-distil-report-title"></span>' +
      '<button type="button" class="ghost st-modal-close" data-st-distil-close aria-label="关闭">✕</button>' +
      '</div>' +
      '<div class="st-modal-body st-distil-report-body"></div>' +
      '<div class="st-modal-foot">' +
      '<button type="button" class="primary" data-st-distil-close>关闭</button>' +
      '</div>' +
      '</div>';
    document.body.appendChild(m);
    m.addEventListener('click', (e) => {
      if (e.target === m || (e.target.closest && e.target.closest('[data-st-distil-close]'))) stCloseDistilReport();
    });
  }
  const head = m.querySelector('.st-distil-report-title');
  if (head) head.textContent = '蒸馏报告' + (doneTitle ? ' · ' + doneTitle : '');
  const body = m.querySelector('.st-distil-report-body');
  if (body) body.innerHTML = buildDistilReportHtml(report || {});
  m.classList.remove('hidden');
}

async function finishDistilSuccess(slug, title, result, opts) {
  const jump = !(opts && opts.jump === false);
  let r = result || null;
  if (!r) {
    const job = shelfDistilJobFor(slug);
    if (job) {
      try {
        const data = await api('/api/v1/jobs/' + encodeURIComponent(job.jobId));
        if (data) r = data.result || null;
      } catch (_) {}
    }
  }
  const packId = (r && r.packId) || '';
  const doneTitle = (r && r.title) ? r.title : (title || slug);
  const report = (r && r.report) || null;
  if (report) {
    const obj = shelfReadDistilJobs();
    const prev = obj[slug] || {};
    obj[slug] = { jobId: prev.jobId || '', title: prev.title || title || slug, status: 'succeeded', progress: 1, report };
    shelfWriteDistilJobs(obj);
  } else {
    shelfDropDistilJob(slug);
  }
  let msg = '✅ 转换完成：' + doneTitle;
  if (r) {
    const bits = [];
    if (r.character_count != null) bits.push('角色 ' + r.character_count);
    if (r.beat_count != null) bits.push('节拍 ' + r.beat_count);
    if (r.lore_count != null) bits.push('传说 ' + r.lore_count);
    if (r.worldline_count != null) bits.push('世界线 ' + r.worldline_count);
    if (bits.length) msg += ' · ' + bits.join(' / ');
  }
  if (report) msg += ' · 📋 蒸馏报告';
  shelfStatus(msg);
  shelfRenderDistilProgress(slug);
  if (packId && typeof stLoadPacks === 'function') {
    try { await stLoadPacks(); } catch (_) {}
  }
  if (packId && jump && typeof switchTab === 'function') switchTab('tavern');
  if (packId && typeof stShowPack === 'function') {
    try { await stShowPack(packId); } catch (_) {}
  }
  if (typeof stStatus === 'function') {
    stStatus('🔮 转换完成，可点「用此包开玩」：' + (doneTitle || packId));
  }
}

function finishDistilFail(slug, title, msg) {
  shelfDropDistilJob(slug);
  shelfStatus('LLM 转换失败：' + (msg || title || slug));
}

function watchDistilJob(jobId, opts) {
  if (!jobId || _shelfWatchers.has(jobId)) return Promise.resolve();
  _shelfWatchers.add(jobId);
  const slug = (opts && opts.slug) || '';
  const title = (opts && opts.title) || slug;
  // 任务到达终态（成功/失败/放弃监听）前保持 _shelfWatchers 占位：
  // 轮询器是 setInterval 异步跑的，占位若在启动后立即释放，sync/重进页面会叠加出多个 watcher。
  let settled = false;
  return (async () => {
    const completeFrom = async (st, body) => {
      settled = true;
      _shelfWatchers.delete(jobId);
      if (st === 'succeeded') {
        await finishDistilSuccess(slug, title, (body && body.result) || null);
      } else {
        finishDistilFail(slug, title, (body && (body.error || body.message)) || '');
      }
    };
    let streamed = false;
    try {
      for await (const ev of readSSE('/api/v1/jobs/' + encodeURIComponent(jobId) + '/stream')) {
        const j = ev && ev.json;
        if (!j) continue;
        streamed = true;
        const et = j.eventType || j.event_type || j.type || '';
        const st = String(j.status || '').toLowerCase();
        if (et === 'done' || et === 'error' || et === 'success'
            || st === 'succeeded' || st === 'failed' || st === 'cancelled' || st === 'error') {
          await completeFrom((et === 'error' || st === 'failed' || st === 'cancelled' || st === 'error') ? 'failed' : 'succeeded', j);
          return;
        }
        const p = (typeof j.progress === 'number') ? j.progress : null;
        const msg = j.message || j.progressMessage || '';
        const obj = shelfReadDistilJobs();
        if (obj[slug] && obj[slug].jobId === jobId) {
          obj[slug].status = 'running';
          if (p != null) obj[slug].progress = p;
          if (j.message) { obj[slug].message = j.message; }
          shelfWriteDistilJobs(obj);
        }
        shelfRenderDistilProgress(slug);
        if (msg) shelfStatus('⏳ ' + msg + (p != null ? ' · ' + Math.round(p * 100) + '%' : ''));
      }
    } catch (_e) {
      // SSE 异常 → 下方轮询兜底（若终态已在 SSE 事件里处理则不再起轮询）
    }
    if (settled) return;
    if (!streamed) { /* SSE 未连上 → 轮询 */ }
    // Fallback: 每 3s 轮询 GET /api/v1/jobs/{jobId}
    // 终止条件：404/410（任务已消失，如服务重启）立即判失败；
    // 其余错误连续失败超上限（SHELF_DISTIL_POLL_FAIL_LIMIT）也终止 —— 不再永久重试卡「运行中」。
    let pollFails = 0;
    const timer = setInterval(async () => {
      try {
        const data = await api('/api/v1/jobs/' + encodeURIComponent(jobId));
        pollFails = 0; // 只要有一次成功就重置连续失败计数
        const st = String((data && data.status) || '').toLowerCase();
        const p = (data && typeof data.progress === 'number') ? data.progress : null;
        const msg = (data && (data.progressMessage || data.error || '')) || '';
        if (st === 'succeeded') { clearInterval(timer); await completeFrom('succeeded', data); return; }
        if (st === 'failed' || st === 'cancelled' || st === 'error') { clearInterval(timer); await completeFrom('failed', data); return; }
        const obj = shelfReadDistilJobs();
        if (obj[slug] && obj[slug].jobId === jobId) {
          obj[slug].status = 'running';
          if (p != null) obj[slug].progress = p;
          if (msg) obj[slug].message = msg;
          shelfWriteDistilJobs(obj);
        }
        shelfRenderDistilProgress(slug);
        if (msg) shelfStatus('⏳ ' + msg + (p != null ? ' · ' + Math.round(p * 100) + '%' : ''));
      } catch (e) {
        pollFails += 1;
        const status = e && e.status;
        if (status === 404 || status === 410) {
          // 任务已不存在（服务重启/记录被清理）→ 立即判失败并上屏，不再重试
          clearInterval(timer);
          await completeFrom('failed', { error: '任务已不存在（可能服务已重启）' });
          return;
        }
        if (pollFails >= SHELF_DISTIL_POLL_FAIL_LIMIT) {
          clearInterval(timer);
          await completeFrom('failed', { error: '连续 ' + pollFails + ' 次轮询失败，已停止监听：' + ((e && (e.message || e.statusText)) || '网络异常') });
          return;
        }
        // 其余失败继续重试（计入连续失败，超限即终止）
      }
    }, 3000);
  })().finally(() => { if (settled) _shelfWatchers.delete(jobId); });
}

async function shelfSyncDistilJobs() {
  const jobs = shelfReadDistilJobs();
  for (const slug of Object.keys(jobs)) {
    const job = jobs[slug];
    if (!job || !job.jobId) continue;
    let data = null;
    try {
      data = await api('/api/v1/jobs/' + encodeURIComponent(job.jobId));
    } catch (_) { /* offline → 保留记录，下次再试 */ }
    if (!data) { shelfRenderDistilProgress(slug); continue; }
    const st = String((data.status) || '').toLowerCase();
    if (st === 'succeeded') {
      await finishDistilSuccess(slug, job.title || slug, data.result || null, { jump: false });
    } else if (st === 'failed' || st === 'error' || st === 'cancelled') {
      finishDistilFail(slug, job.title || slug, data.error || st);
    } else {
      shelfRenderDistilProgress(slug);
      watchDistilJob(job.jobId, { slug, title: job.title }).catch(() => {});
    }
  }
}

async function shelfDistilWorld(slug, title) {
  const existing = shelfDistilJobFor(slug);
  if (existing && shelfDistilActive(existing.status)) {
    shelfStatus('该作品已有转换任务在运行，进度见卡片…');
    watchDistilJob(existing.jobId, { slug, title: title || existing.title }).catch(() => {});
    return;
  }
  shelfStatus('🔮 正在提交转换：角色蒸馏 → 世界树/节拍/出口/世界线 → 文风…');
  try {
    const data = await shelfApi('/novels/' + encodeURIComponent(slug) + '/distil-world', {
      method: 'POST',
      body: JSON.stringify({}),
    });
    const jobId = data.jobId || data.runId || data.id || data.run_id;
    if (data.error && !jobId) throw new Error(data.error || '提交转换失败');
    if (!jobId) throw new Error((data && data.error) || '未返回 jobId');
    const obj = shelfReadDistilJobs();
    obj[slug] = {
      jobId,
      title: title || slug,
      startedAt: new Date().toISOString(),
      status: 'queued',
      progress: 0.01,
      message: '已进入后台转换',
    };
    shelfWriteDistilJobs(obj);
    shelfRenderDistilProgress(slug);
    shelfStatus('⏳ 已进入后台转换（角色蒸馏→世界线→文风），可继续浏览，进度见卡片…');
    watchDistilJob(jobId, { slug, title: title || slug }).catch(() => {});
  } catch (e) {
    // 409：同作品已有转换任务 → 恢复其订阅
    if (e && e.status === 409 && e.body && e.body.jobId) {
      const jid = e.body.jobId;
      const obj = shelfReadDistilJobs();
      const prev = obj[slug];
      if (!prev || prev.jobId !== jid) {
        obj[slug] = { jobId: jid, title: title || slug, startedAt: new Date().toISOString(), status: 'running', progress: 0.02, message: '转换已在进行' };
        shelfWriteDistilJobs(obj);
      }
      shelfRenderDistilProgress(slug);
      shelfStatus('该作品已有转换任务在后台运行，进度见卡片…');
      watchDistilJob(jid, { slug, title: title || slug }).catch(() => {});
    } else {
      shelfStatus('LLM 转换失败：' + ((e && e.message) || e));
      shelfRenderDistilProgress(slug);
    }
  }
}

async function shelfExport(slug, title) {
  try {
    const url = apiBase() + '/api/v1/crawler/novels/' + encodeURIComponent(slug) + '/export';
    const a = document.createElement('a');
    a.href = url;
    a.download = (title || slug) + '.md';
    a.rel = 'noopener';
    document.body.appendChild(a);
    a.click();
    a.remove();
    shelfStatus('已开始导出：' + (title || slug));
  } catch (e) {
    shelfStatus('导出失败：' + (e.message || e));
  }
}

async function stDecodeTextFile(file) {
  const buf = await new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result);
    r.onerror = () => reject(new Error('读取失败'));
    r.readAsArrayBuffer(file);
  });
  const bytes = new Uint8Array(buf);
  // 1) 严格 UTF-8 解码（失败=非 UTF-8 编码）
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch (_) { /* not utf-8, fall through */ }
  // 2) 回退 GB18030（GBK/GB2312 超集，可全覆盖）
  try {
    return new TextDecoder('gb18030').decode(bytes);
  } catch (_) { /* last resort lossy utf-8 */ }
  return new TextDecoder('utf-8').decode(bytes);
}

async function shelfImportFile(file) {
  if (!file) return;
  shelfStatus('导入中：' + file.name + '…');
  const isDocx = typeof file.name === 'string' && /\.docx$/i.test(file.name);
  let text;
  if (isDocx) {
    shelfStatus('解析 DOCX：' + file.name + '…');
    const buf = await new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => resolve(r.result);
      r.onerror = () => reject(new Error('读取失败'));
      r.readAsArrayBuffer(file);
    });
    const res = await mammoth.extractRawText({ arrayBuffer: buf });
    text = res && (res.value || '');
    if (!text || !text.trim()) { text = ''; shelfStatus('未从 DOCX 解析到文本'); return; }
  } else {
    text = await stDecodeTextFile(file);
  }
  const title = file.name.replace(/\.[^.]+$/, '');
  const data = await shelfApi('/novels', {
    method: 'POST',
    body: JSON.stringify({ text, title, toPack: true }),
  });
  if (!data || data.ok === false) throw new Error((data && data.error) || '导入失败');
  await loadBookshelf();
  shelfStatus('已上架「' + (data.title || title) + '」' +
    (data.chapterCount ? (' · ' + data.chapterCount + ' 章') : '') +
    (data.packId ? (' · Pack ' + data.packId) : ''));
  if (data.packId && typeof stLoadPacks === 'function') {
    try { await stLoadPacks(); } catch (_) {}
  }
  return data;
}

/* ===== exports consumed by tavern.js (initTavernUI wiring / stImportNovel); =====
   loadBookshelf is re-exported by tavern.js for tabs.js + the converted[] line */
export {
  loadBookshelf, shelfPublishChat, shelfSaveSchedule, shelfRunScheduleNow,
  shelfImportFile, shelfStatus, closeShelfReader, shelfPromoteToPack, shelfExport,
  stDecodeTextFile,
};

/** initTavernUI reader-to-pack / reader-export handlers read the active novel */
export function shelfActiveNovel() {
  if (!shelfActiveSlug) return null;
  const n = shelfNovels.find((x) => x.slug === shelfActiveSlug);
  return { slug: shelfActiveSlug, title: n && n.title };
}
