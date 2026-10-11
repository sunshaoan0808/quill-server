/**
 * SillyTavern 预设管理 — 独立 JS，不依赖压缩的 app.js
 *
 * 功能：
 * 1. 预设列表/上传/删除（tab-st 面板）
 * 2. chat-setup 预设下拉选择器
 * 3. monkey-patch fetch：向 /api/mobile/chat/start 和 /api/v1/chat/start 注入 presetId
 */
(function () {
  'use strict';

  // ── 工具函数 ──────────────────────────────────────────────
  const $ = (id) => document.getElementById(id);
  let presets = [];
  let selectedDetailId = null;

  function apiBase() {
    try {
      const b = localStorage.getItem('kaleido_api_base');
      if (b) return b.replace(/\/$/, '');
    } catch {}
    return '';
  }

  function getToken() {
    try { return localStorage.getItem('museai_token') || ''; } catch { return ''; }
  }

  async function api(path, opts = {}) {
    const url = apiBase() + path;
    const headers = Object.assign({ 'Content-Type': 'application/json' }, opts.headers || {});
    const token = getToken();
    if (token) headers['Authorization'] = 'Bearer ' + token;
    const res = await fetch(url, Object.assign({}, opts, { headers }));
    if (!res.ok) {
      let msg = res.status + ' ' + res.statusText;
      try { const e = await res.json(); msg = e.error || e.message || msg; } catch {}
      const err = new Error(msg);
      err.status = res.status;
      throw err;
    }
    const ct = res.headers.get('content-type') || '';
    if (ct.includes('application/json')) return res.json();
    return res.text();
  }

  function toast(msg, isErr) {
    const el = $('st-preset-msg');
    if (el) { el.textContent = msg; el.style.color = isErr ? 'var(--accent-2, #e55)' : ''; }
    if (typeof window.toast === 'function') window.toast(msg, isErr);
  }

  function fmtSize(n) {
    if (n < 1024) return n + 'B';
    if (n < 1048576) return (n / 1024).toFixed(1) + 'KB';
    return (n / 1048576).toFixed(1) + 'MB';
  }

  // ── 预设列表 ──────────────────────────────────────────────
  function renderList() {
    const list = $('st-preset-list');
    if (!list) return;
    list.innerHTML = '';
    if (!presets.length) {
      list.setAttribute('data-empty', '暂无预设，上传 JSON 开始');
      return;
    }
    list.removeAttribute('data-empty');
    for (const p of presets) {
      const item = document.createElement('div');
      item.className = 'item' + (selectedDetailId === p.id ? ' active' : '');
      const t = document.createElement('span');
      t.className = 't';
      t.textContent = p.name || p.id;
      const d = document.createElement('span');
      d.className = 'd';
      d.textContent = `${p.enabledCount || 0}/${p.blockCount || 0} 块 · ${fmtSize(p.fileSize || 0)}`;
      item.appendChild(t);
      item.appendChild(d);
      item.onclick = () => showDetail(p.id);
      list.appendChild(item);
    }
  }

  function renderChatSelect() {
    const sel = $('chat-preset');
    if (!sel) return;
    const current = sel.value;
    sel.innerHTML = '<option value="">（默认模式）</option>';
    for (const p of presets) {
      const opt = document.createElement('option');
      opt.value = p.id;
      opt.textContent = p.name || p.id;
      sel.appendChild(opt);
    }
    if (current) sel.value = current;
  }

  async function loadPresets() {
    try {
      const data = await api('/api/v1/kaleido-tools/presets');
      // The backend currently returns the catalog as a JSON array. Keep
      // accepting the older object-wrapped shape for compatibility.
      presets = Array.isArray(data) ? data : (Array.isArray(data && data.presets) ? data.presets : []);
      renderList();
      renderChatSelect();
      toast(`已加载 ${presets.length} 个预设`);
    } catch (e) {
      // Older servers do not expose this optional endpoint. Treat that as an
      // empty catalog, not an application error or a retry loop.
      if (e && e.status === 404) {
        presets = [];
        renderList();
        renderChatSelect();
        toast('暂无预设，可导入 JSON');
        return;
      }
      toast('加载预设失败: ' + e.message, true);
    }
  }

  // ── 预设详情 ──────────────────────────────────────────────
  async function showDetail(id) {
    selectedDetailId = id;
    renderList();
    const detail = $('st-preset-detail');
    const nameEl = $('st-preset-detail-name');
    const metaEl = $('st-preset-detail-meta');
    const blocksEl = $('st-preset-blocks');
    if (!detail) return;

    const meta = presets.find(p => p.id === id);
    if (!meta) { detail.classList.add('hidden'); return; }
    detail.classList.remove('hidden');
    nameEl.textContent = meta.name || meta.id;
    metaEl.textContent = `${meta.enabledCount || 0}/${meta.blockCount || 0} 块启用 · ${fmtSize(meta.fileSize || 0)} · ${meta.description || '无描述'}`;

    // 加载块列表
    blocksEl.innerHTML = '<span class="muted sm">加载中…</span>';
    try {
      // The current backend exposes list/upsert only; details are included in the list item when available.
      const raw = presets.find(p => p.id === id) || {};
      const prompts = Array.isArray(raw.prompts) ? raw.prompts : [];
      blocksEl.innerHTML = '';
      for (let i = 0; i < prompts.length; i++) {
        const b = prompts[i];
        const row = document.createElement('div');
        row.className = 'preset-block-row' + (b.disabled ? ' is-off' : '');
        const label = document.createElement('label');
        label.className = 'preset-block-label';
        const cb = document.createElement('input');
        cb.type = 'checkbox';
        cb.checked = !b.disabled;
        cb.disabled = true; // 只读展示（块开关需回写 JSON，MVP 先只读）
        const name = document.createElement('span');
        name.textContent = b.name || b.identifier || `块 #${i + 1}`;
        const badge = document.createElement('span');
        badge.className = 'preset-block-badge';
        const tags = [];
        if (b.role) tags.push(b.role);
        if (b.injection_position != null) tags.push('pos=' + b.injection_position);
        if (b.injection_depth && b.injection_depth > 0) tags.push('depth=' + b.injection_depth);
        badge.textContent = tags.join(' · ');
        label.appendChild(cb);
        label.appendChild(name);
        if (tags.length) label.appendChild(badge);
        row.appendChild(label);
        blocksEl.appendChild(row);
      }
      if (!prompts.length) blocksEl.innerHTML = '<span class="muted sm">无 prompt 块</span>';
    } catch (e) {
      blocksEl.innerHTML = '<span class="muted sm">加载块失败: ' + e.message + '</span>';
    }
  }

  // ── 上传 ──────────────────────────────────────────────────
  async function uploadPreset() {
    const fileInput = $('st-preset-file');
    if (!fileInput || !fileInput.files || !fileInput.files[0]) {
      toast('请先选择 JSON 文件');
      return;
    }
    const file = fileInput.files[0];
    toast('上传中…');
    try {
      const text = await file.text();
      // 验证是合法 JSON
      JSON.parse(text);
      const result = await api('/api/v1/kaleido-tools/presets', {
        method: 'POST',
        body: text,
        headers: { 'Content-Type': 'application/json' },
      });
      toast(`上传成功 · ${result.enabled_count || 0}/${result.block_count || 0} 块启用 · ${fmtSize(result.file_size || 0)}`);
      fileInput.value = '';
      await loadPresets();
      if (result.id) showDetail(result.id);
    } catch (e) {
      toast('上传失败: ' + e.message, true);
    }
  }

  // ── 删除 ──────────────────────────────────────────────────
  async function deletePreset() {
    if (!selectedDetailId) return;
    if (!confirm('确定删除预设「' + selectedDetailId + '」？')) return;
    toast('当前后端暂不支持删除预设', true);
  }

  // ── fetch monkey-patch：注入 presetId ─────────────────────
  function installFetchPatch() {
    const origFetch = window.fetch;
    window.fetch = function (input, init) {
      try {
        let url = typeof input === 'string' ? input : (input && input.url) || '';
        const isChatStart = url.includes('/api/mobile/chat/start') || url.includes('/api/v1/chat/start');
        if (isChatStart && init && init.body) {
          const sel = $('chat-preset');
          const presetId = sel ? sel.value : (localStorage.getItem('museai_st_preset_id') || '');
          if (presetId) {
            const body = typeof init.body === 'string' ? JSON.parse(init.body) : init.body;
            if (body && typeof body === 'object' && !body.presetId) {
              body.presetId = presetId;
              init.body = JSON.stringify(body);
            }
          }
        }
      } catch (e) {
        console.warn('[st_presets] fetch patch error:', e);
      }
      return origFetch.apply(this, arguments);
    };
  }

  // ── 持久化选中预设 ────────────────────────────────────────
  function persistSelection() {
    const sel = $('chat-preset');
    if (!sel) return;
    sel.addEventListener('change', () => {
      try { localStorage.setItem('museai_st_preset_id', sel.value); } catch {}
      const p = presets.find(x => x.id === sel.value);
      const hint = $('partner-hint');
      if (hint && sel.value) {
        const orig = hint.getAttribute('data-orig') || hint.textContent;
        if (!hint.getAttribute('data-orig')) hint.setAttribute('data-orig', orig);
        hint.textContent = `ST 预设: ${p ? p.name : sel.value}`;
      } else if (hint && hint.getAttribute('data-orig')) {
        hint.textContent = hint.getAttribute('data-orig');
      }
    });
    // 恢复
    try {
      const saved = localStorage.getItem('museai_st_preset_id');
      if (saved) sel.value = saved;
    } catch {}
  }

  // ── 初始化 ────────────────────────────────────────────────
  function init() {
    const uploadBtn = $('st-preset-upload');
    const refreshBtn = $('st-preset-refresh');
    const delBtn = $('st-preset-delete');
    if (uploadBtn) uploadBtn.onclick = uploadPreset;
    if (refreshBtn) refreshBtn.onclick = loadPresets;
    if (delBtn) delBtn.onclick = deletePreset;

    installFetchPatch();
    persistSelection();
    loadPresets();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
