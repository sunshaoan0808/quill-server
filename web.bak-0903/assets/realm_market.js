/**
 * RisuRealm 角色卡市场 — 独立 JS（不依赖压缩的 app.js）
 *
 * 依赖后端代理端点（见 museai-server/src/realm_api.rs）：
 * - GET  /api/v1/realm/search?q=&page=&nsfw=&sort=
 * - GET  /api/v1/realm/resource/{hash}?w=300   （封面图缩略，经 auth middleware，支持 ?token=）
 * - POST /api/v1/realm/import/{id}             （一步导入角色卡 + 世界书）
 * - POST /api/v1/realm/translate               （批量翻译 name/desc/tags → 中文）
 *
 * 交互：
 * - 搜索框 + NSFW 开关 + 排序 + 分页
 * - 标签/类型/世界书筛选（客户端过滤当前页）
 * - 自动翻译开关（名称+描述 → 中文，走后端 LLM）
 * - 卡片网格：封面缩略图 + 名称 + 标签 + 作者 + 导入按钮
 * - 导入成功 toast 提示
 */
(function () {
  'use strict';

  var page = 0;
  var loading = false;
  var currentCards = [];   // 当前页原始卡片
  var filteredCards = [];  // 筛选后的卡片
  var transCache = {};     // id -> {name, desc} 翻译缓存

  function $(id) { return document.getElementById(id); }

  function apiBase() {
    try {
      var b = localStorage.getItem('museai_api_base');
      if (b) return b.replace(/\/$/, '');
    } catch (e) {}
    return '';
  }

  function getToken() {
    try { return localStorage.getItem('museai_token') || ''; } catch (e) { return ''; }
  }

  function toast(msg, isErr) {
    var el = $('realm-msg');
    if (el) { el.textContent = msg; el.style.color = isErr ? 'var(--err, #f87171)' : ''; }
  }

  function esc(s) {
    if (s === undefined || s === null) return '';
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  async function api(path, opts) {
    opts = opts || {};
    var headers = Object.assign({ 'Content-Type': 'application/json' }, opts.headers || {});
    var token = getToken();
    if (token) headers['Authorization'] = 'Bearer ' + token;
    var res = await fetch(apiBase() + path, Object.assign({}, opts, { headers: headers }));
    if (!res.ok) {
      var msg = res.status + ' ' + res.statusText;
      try { var e = await res.json(); msg = e.error || msg; } catch (err) {}
      throw new Error(msg);
    }
    var ct = res.headers.get('content-type') || '';
    if (ct.indexOf('application/json') >= 0) return res.json();
    return res;
  }

  // ── 筛选状态 ──────────────────────────────────────────────
  function currentFilter() {
    return {
      tag: ($('realm-filter-tag') && $('realm-filter-tag').value) || '',
      type: ($('realm-filter-type') && $('realm-filter-type').value) || '',
      wb: ($('realm-filter-wb') && $('realm-filter-wb').value) || ''
    };
  }

  function cardMatches(card, f) {
    if (f.tag) {
      var tags = card.tags || [];
      if (tags.indexOf(f.tag) < 0) return false;
    }
    if (f.type) {
      var ct = (card.type || '').toLowerCase();
      var cv = f.type.toLowerCase();
      if (cv === 'sfw') { if (ct === 'nsfw') return false; }
      else if (cv === 'nsfw') { if (ct !== 'nsfw') return false; }
      else if (ct !== cv) return false;
    }
    if (f.wb) {
      var hasWb = !!(card.haslore || card.hasAsset || card.hasEmotion || card.hasWorldBook);
      if (f.wb === 'yes' && !hasWb) return false;
      if (f.wb === 'no' && hasWb) return false;
    }
    return true;
  }

  function applyFilters() {
    var f = currentFilter();
    filteredCards = currentCards.filter(function (c) { return cardMatches(c, f); });
    renderGrid(filteredCards);
    var info = $('realm-page-info');
    if (info) info.textContent = '第 ' + (page + 1) + ' 页 · 筛选 ' + filteredCards.length + '/' + currentCards.length;
    // 自动翻译开启时，对筛选结果做翻译
    if (isTranslateOn()) translateCards(filteredCards);
  }

  // ── 翻译 ──────────────────────────────────────────────────
  function isTranslateOn() {
    var cb = $('realm-translate');
    return cb ? cb.checked : false;
  }

  function looksForeign(s) {
    if (!s) return false;
    return /[\u3040-\u30ff\uac00-\ud7af\u0400-\u04ff]/.test(s);
  }

  var translating = false;
  async function translateCards(cards) {
    if (!cards || !cards.length || translating) return;
    // 收集未缓存且含外语的文本
    var need = [];
    var needIdx = [];
    for (var i = 0; i < cards.length; i++) {
      var c = cards[i];
      if (!c || !c.id) continue;
      if (transCache[c.id]) continue;
      var text = (c.name || '') + ' || ' + (c.desc || '').slice(0, 200);
      if (looksForeign(text)) {
        need.push(text);
        needIdx.push({ id: c.id, card: c });
      }
    }
    if (!need.length) return;

    translating = true;
    toast('翻译中…（' + need.length + ' 张）');
    try {
      var data = await api('/api/v1/realm/translate', {
        method: 'POST',
        body: JSON.stringify({ texts: need.slice(0, 40) })
      });
      var results = data.results || [];
      for (var j = 0; j < results.length; j++) {
        var entry = needIdx[j];
        if (!entry) continue;
        var parts = String(results[j]).split(' || ');
        transCache[entry.id] = {
          name: parts[0] || entry.card.name,
          desc: parts.length > 1 ? parts.slice(1).join(' || ') : entry.card.desc
        };
      }
      renderGrid(filteredCards);
      toast('翻译完成 ✓');
    } catch (e) {
      toast('翻译失败: ' + e.message, true);
    } finally {
      translating = false;
    }
  }

  function displayName(c) {
    var t = transCache[c.id];
    return (t && t.name) ? t.name : c.name;
  }
  function displayDesc(c) {
    var t = transCache[c.id];
    if (t && t.desc) return t.desc;
    return c.desc || '';
  }

  // ── 卡片渲染 ──────────────────────────────────────────────
  function cardHtml(c) {
    var token = getToken();
    var imgUrl = c.imageUrl
      ? (apiBase() + c.imageUrl + '?w=300&token=' + encodeURIComponent(token))
      : '';
    var tags = (c.tags || []).slice(0, 4).map(function (t) { return '<span class="realm-tag">' + esc(t) + '</span>'; }).join('');
    var creator = c.creatorName || c.authorname || c.creator || '';
    var meta = [];
    if (c.hot) meta.push('❤ ' + c.hot);
    if (c.download) meta.push('↓ ' + esc(c.download));
    if (c.type === 'nsfw') meta.push('🔞');
    var wbFlags = [];
    if (c.haslore) wbFlags.push('📚世界书');
    if (c.hasAsset) wbFlags.push('🧩资源');
    if (c.hasEmotion) wbFlags.push('😊表情');
    var wbStr = wbFlags.length ? '<div class="realm-meta">' + wbFlags.join(' ') + '</div>' : '';
    return (
      '<div class="realm-card">' +
        '<div class="realm-cover">' +
          (imgUrl
            ? '<img loading="lazy" decoding="async" src="' + esc(imgUrl) + '" alt="' + esc(displayName(c)) + '" onerror="this.style.display=\'none\'" />'
            : '<div class="realm-cover-none">🎭</div>') +
        '</div>' +
        '<div class="realm-card-body">' +
          '<div class="realm-name" title="' + esc(displayName(c)) + '">' + esc(displayName(c)) + '</div>' +
          '<div class="realm-desc">' + esc(displayDesc(c).slice(0, 90)) + '</div>' +
          '<div class="realm-tags">' + tags + '</div>' +
          '<div class="realm-meta">' + meta.join(' · ') + '</div>' +
          wbStr +
          '<div class="realm-actions">' +
            '<span class="realm-creator" title="' + esc(creator) + '">' + esc(creator || '匿名') + '</span>' +
            '<button type="button" class="ghost sm realm-import" data-id="' + esc(c.id) + '">导入</button>' +
          '</div>' +
        '</div>' +
      '</div>'
    );
  }

  function renderGrid(cards) {
    var grid = $('realm-grid');
    if (!grid) return;
    grid.innerHTML = '';
    if (!cards || !cards.length) {
      grid.setAttribute('data-empty', '无结果，换个关键词试试');
      return;
    }
    grid.removeAttribute('data-empty');
    for (var i = 0; i < cards.length; i++) {
      var wrap = document.createElement('div');
      wrap.innerHTML = cardHtml(cards[i]);
      grid.appendChild(wrap.firstChild);
    }
    var btns = grid.querySelectorAll('.realm-import');
    for (var j = 0; j < btns.length; j++) {
      btns[j].onclick = function () { importCard(this.getAttribute('data-id')); };
    }
  }

  // ── 搜索 ──────────────────────────────────────────────────
  async function search() {
    if (loading) return;
    loading = true;
    var btn = $('realm-search-btn');
    if (btn) btn.disabled = true;
    toast('搜索中…');
    var q = ($('realm-q') && $('realm-q').value.trim()) || '';
    var nsfw = $('realm-nsfw') ? $('realm-nsfw').checked : false;
    var sort = ($('realm-sort') && $('realm-sort').value) || 'recommended';
    try {
      var data = await api('/api/v1/realm/search?q=' + encodeURIComponent(q) +
        '&page=' + page + '&nsfw=' + nsfw + '&sort=' + encodeURIComponent(sort));
      currentCards = data.cards || [];
      updateTagFilter(currentCards);
      applyFilters();
      toast('找到 ' + currentCards.length + ' 张角色卡');
    } catch (e) {
      toast('搜索失败: ' + e.message, true);
    } finally {
      loading = false;
      if (btn) btn.disabled = false;
    }
  }

  // 收集当前页所有标签，填充筛选下拉
  function updateTagFilter(cards) {
    var sel = $('realm-filter-tag');
    if (!sel) return;
    var tags = {};
    cards.forEach(function (c) {
      (c.tags || []).forEach(function (t) { tags[t] = (tags[t] || 0) + 1; });
    });
    var sorted = Object.keys(tags).sort(function (a, b) { return tags[b] - tags[a]; });
    var cur = sel.value;
    sel.innerHTML = '<option value="">全部标签</option>';
    sorted.slice(0, 30).forEach(function (t) {
      var opt = document.createElement('option');
      opt.value = t;
      opt.textContent = t + ' (' + tags[t] + ')';
      sel.appendChild(opt);
    });
    sel.value = cur;
  }

  // ── 导入 ──────────────────────────────────────────────────
  async function importCard(id) {
    if (!id) return;
    if (!confirm('确认从 RisuRealm 导入这张角色卡？（自动提取设定 + 世界书）')) return;
    toast('导入中…');
    try {
      var data = await api('/api/v1/realm/import/' + encodeURIComponent(id), { method: 'POST' });
      var item = data.item || {};
      var wb = data.worldBook ? ' · 世界书 ✓' : '';
      toast('导入成功: ' + (item.name || '角色卡') + wb + (data.loreEntryCount ? ' · ' + data.loreEntryCount + ' 条设定' : ''));
    } catch (e) {
      toast('导入失败: ' + e.message, true);
    }
  }

  // ── 初始化 ────────────────────────────────────────────────
  function scrollToRealm() {
    var grid = $('realm-grid');
    if (grid) {
      setTimeout(function () {
        grid.scrollIntoView({ behavior: 'smooth', block: 'start' });
        if (!grid.querySelector('.realm-card')) {
          var btn = $('realm-search-btn');
          if (btn) btn.click();
        }
      }, 300);
    }
  }

  function init() {
    var searchBtn = $('realm-search-btn');
    if (searchBtn) searchBtn.onclick = function () { page = 0; search(); };
    var q = $('realm-q');
    if (q) {
      q.addEventListener('keydown', function (ev) {
        if (ev.key === 'Enter') { page = 0; search(); }
      });
    }
    var nsfw = $('realm-nsfw');
    if (nsfw) nsfw.onchange = function () { page = 0; search(); };
    var sort = $('realm-sort');
    if (sort) sort.onchange = function () { page = 0; search(); };
    var prev = $('realm-prev');
    if (prev) prev.onclick = function () { if (page > 0) { page--; search(); } };
    var next = $('realm-next');
    if (next) next.onclick = function () { page++; search(); };

    // 筛选
    ['realm-filter-tag', 'realm-filter-type', 'realm-filter-wb'].forEach(function (id) {
      var el = $(id);
      if (el) el.onchange = applyFilters;
    });

    // 翻译开关
    var tr = $('realm-translate');
    if (tr) tr.onchange = function () {
      if (tr.checked) translateCards(filteredCards);
      else renderGrid(filteredCards);
    };

    // 监听「更多 → 🎭 角色卡市场」入口
    document.querySelectorAll('[data-focus="realm"]').forEach(function (btn) {
      btn.addEventListener('click', scrollToRealm);
    });
    if (window.location.hash === '#realm') {
      scrollToRealm();
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
