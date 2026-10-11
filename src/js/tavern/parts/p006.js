function stBookmarkKey() {
  return 'st_bookmarks_v1';
}

function stLoadBookmarks() {
  try {
    const raw = localStorage.getItem(stBookmarkKey());
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr) ? arr : [];
  } catch (_) { return []; }
}

function stSaveBookmarks(arr) {
  try { localStorage.setItem(stBookmarkKey(), JSON.stringify(arr.slice(-200))); } catch (_) {}
}

function stToggleBookmark(divEl) {
  if (!divEl || !tavernSession) return;
  const mid = divEl.getAttribute('data-mid');
  if (!mid) return;
  const msgs = tavernSession.messages || [];
  const m = msgs.find(function (x) { return String(x.id || '') === mid; });
  if (!m) return;
  const bodyEl = divEl.querySelector('.bubble-body');
  const text = ((bodyEl && bodyEl.textContent) || String(m.content || '')).trim().slice(0, 120);
  const list = stLoadBookmarks();
  const idx = list.findIndex(function (b) { return b.mid === mid && b.sessionId === tavernSession.sessionId; });
  if (idx >= 0) {
    list.splice(idx, 1);
    stSaveBookmarks(list);
    stStatus('已取消收藏');
  } else {
    list.unshift({
      mid: mid,
      sessionId: tavernSession.sessionId,
      title: tavernSession.title || '会话',
      role: m.role || '',
      text: text,
      ts: Date.now()
    });
    stSaveBookmarks(list);
    stStatus('已收藏消息 ⭐');
  }
  if (typeof stRenderBookmarks === 'function') stRenderBookmarks();
}

function stBmEsc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function stRenderBookmarks() {
  var host = $('st-bookmarks');
  if (!host) return;
  var list = stLoadBookmarks();
  if (!list.length) {
    host.innerHTML = '<div class="st-bm-empty">暂无书签——消息操作菜单 ⭐ 收藏重点剧情。</div>';
    return;
  }
  var html = '<div class="st-bm-head">书签 · ' + list.length + '</div><div class="st-bm-list">';
  for (var i = 0; i < list.length; i++) {
    var b = list[i];
    var roleTag = b.role === 'user' ? '你' : 'AI';
    html += '<div class="st-bm-item" data-bm="' + i + '">' +
      '<div class="st-bm-role">' + roleTag + '</div>' +
      '<div class="st-bm-body">' +
      '<div class="st-bm-text">' + stBmEsc(b.text || '') + '</div>' +
      '<div class="st-bm-meta">' + stBmEsc(b.title || '') + ' · ' + new Date(b.ts || Date.now()).toLocaleString() + '</div>' +
      '</div>' +
      '<button type="button" class="st-bm-del" title="删除书签">✕</button>' +
      '</div>';
  }
  html += '</div>';
  host.innerHTML = html;
  // 事件绑定
  var items = host.querySelectorAll('.st-bm-item');
  Array.prototype.forEach.call(items, function (it) {
    it.querySelector('.st-bm-del').onclick = function (e) {
      e.stopPropagation();
      var idx = parseInt(it.getAttribute('data-bm'), 10);
      var arr = stLoadBookmarks();
      arr.splice(idx, 1);
      stSaveBookmarks(arr);
      stRenderBookmarks();
    };
  });
}

function stApplyChatWidth(pct) {
  const msgs = $('st-messages');
  if (!msgs) return;
  const stage = msgs.closest('.stage-messages') || msgs.parentElement;
  if (!stage) return;
  const p = Math.max(40, Math.min(100, pct || 100));
  // 宽度作用于剧场视图容器（st-view-play），而非消息区父链
  const host = $('st-view-play');
  if (host) {
    host.style.maxWidth = (p === 100) ? '' : p + '%';
    host.style.margin = (p === 100) ? '' : '0 auto';
  }
  const val = $('st-chat-width-val');
  if (val) val.textContent = p + '%';
  const slider = $('st-chat-width');
  if (slider) slider.value = String(p);
  try { localStorage.setItem('stChatWidth', String(p)); } catch (_) {}
}

function stApplyBubbleStyle(style) {
  const stage = $('st-view-play') || document.documentElement;
  stage.setAttribute('data-msg-style', style || 'bubble');
  try { localStorage.setItem('stBubbleStyle', style || 'bubble'); } catch (_) {}
  const seg = $('st-bubble-style');
  if (seg) {
    const btns = seg.querySelectorAll('.st-seg-btn');
    Array.prototype.forEach.call(btns, function (b) {
      b.classList.toggle('active', b.getAttribute('data-style') === (style || 'bubble'));
    });
  }
}

function stWireDisplaySettings() {
  // 宽度滑杆
  const slider = $('st-chat-width');
  if (slider) {
    const savedW = parseInt(localStorage.getItem('stChatWidth') || '100', 10);
    stApplyChatWidth(isNaN(savedW) ? 100 : savedW);
    slider.oninput = function () { stApplyChatWidth(parseInt(slider.value, 10)); };
  }
  // 气泡风格
  const seg = $('st-bubble-style');
  if (seg) {
    const savedStyle = localStorage.getItem('stBubbleStyle') || 'bubble';
    stApplyBubbleStyle(savedStyle);
    const btns = seg.querySelectorAll('.st-seg-btn');
    Array.prototype.forEach.call(btns, function (b) {
      b.onclick = function () { stApplyBubbleStyle(b.getAttribute('data-style')); };
    });
  }
  // 时间戳开关
  const metaBtn = $('st-msg-meta');
  if (metaBtn) {
    const syncMeta = function () {
      const on = localStorage.getItem('stMsgMeta') === '1';
      metaBtn.dataset.on = on ? '1' : '0';
      metaBtn.classList.toggle('is-on', on);
    };
    metaBtn.onclick = function () {
      const on = localStorage.getItem('stMsgMeta') === '1';
      localStorage.setItem('stMsgMeta', on ? '0' : '1');
      syncMeta();
      stRenderMessages({ quiet: true });
      stStatus(on ? '消息元信息已关闭' : '消息元信息已开启（时间戳 + token）');
    };
    syncMeta();
  }
}

/* ===== S2.21: shelf/bookshelf 域（原 4355-5061 行，shelfStatus…shelfImportFile）
   已逐字抽取至 src/js/shelf.js，本文件改为 import 引用（见该文件头注释）===== */

const _origSwitchTabShelf = typeof switchTab === 'function' ? switchTab : null;

const stAsrBtn = $('st-asr-btn');

const stAsrToggle = $('st-asr-toggle');

const stAutoBtn = $('st-tts-auto');

const stImgSelBtn = $('st-image-channel');

const stImgSelOpts = document.querySelectorAll('#st-image-channel-list .st-select-opt');

const stQualitySelBtn = $('st-writer-quality');

const stTierSelBtn = $('st-content-tier');

const stPovBtn = $('st-narr-pov');

function stMakeId(prefix) { return prefix + '-' + Math.random().toString(36).slice(2, 9); }

function stSlugify(s) { return String(s).toLowerCase().replace(/[^a-z0-9\u4e00-\u9fa5]+/g, '-').replace(/^-+|-+$/g, '').slice(0,40) || 'pack'; }

function stSplitChapters(text) {
  // 仅匹配行首独立标题：## 第N章 / 行首第N章 / Chapter N
  // （排除正文内嵌缩进段落如「　　第一章」）
  const re = /(?:^|\n)(?:#{1,3}\s*第\s*[一二三四五六七八九十零百千万\d]+\s*[章节]|第\s*[一二三四五六七八九十零百千万\d]+\s*[章节]|Chapter\s+\d+|CHAPTER\s+\d+)[\s\t]*[:：]?\s*(.+?)?(?=\n|$)/gm;
  const matches = []; let m;
  while ((m = re.exec(text)) !== null) {
    const idx = m.index + (m[0].startsWith('\n') ? 1 : 0);
    const titleRaw = (m[0].replace(/^\s*\n?/, '') || '').replace(/^第\s*/, '第');
    const title = m[1] ? (m[1].slice(0,120).trim() || titleRaw) : titleRaw;
    matches.push({ idx, raw: m[0], title });
  }
  if (matches.length < 2) {
    // Fallback word window: split every ~2500 chars
    const window = 2400;
    const parts = []; let pos = 0;
    while (pos < text.length) {
      let breakAt = Math.min(pos + window, text.length);
      if (breakAt < text.length) {
        let nearest = text.lastIndexOf('\n', breakAt);
        if (nearest > pos + window * 0.5) breakAt = nearest;
      }
      parts.push({ idx: pos, title: '第' + (parts.length + 1) + '章', raw: '' });
      pos = breakAt;
    }
    parts.forEach((p, i) => { p.end = (parts[i + 1] ? parts[i + 1].idx : text.length); });
    return parts.map(p => ({ ...p, content: text.slice(p.idx, p.end).trim() }));
  }
  const chapters = [];
  matches.forEach((match, i) => {
    const end = (matches[i + 1] ? matches[i + 1].idx : text.length);
    chapters.push({ idx: match.idx, title: match.title, content: text.slice(match.idx, end).trim() });
  });
  return chapters;
}

function stBuildPackFromNovel(title, chapters) {
  const packId = 'pack-' + Date.now();
  const now = new Date().toISOString();
  const chars = [
    { id: 'c-' + stMakeId('n'), name: '旁白', role: 'narrator', personality: '旁白', speechStyle: '' },
    { id: 'c-' + stMakeId('p'), name: '读者', role: 'player', personality: '你自己', speechStyle: '' }
  ];
  // Scan for potential named characters via simple pattern (filter narrative junk)
  const seen = new Set(['旁白', '读者', '玩家', 'narrator']);
  const junkRe = /^(露出|眼角|换鞋|随口|轻声|低头|抬起|转身|伸手|走过去|看向|听见|突然|只是|已经|然后|因为|所以)/;
  const nameRe = /([^\s，。！？、；：""''（）\(\)]{2,4})(?:说|道|问|答|喊|叫)/g;
  for (const ch of chapters) {
    let mm; while ((mm = nameRe.exec(ch.content)) !== null) {
      const n = String(mm[1] || '').trim();
      if (!n || seen.has(n)) continue;
      if (n.length < 2 || n.length > 4) continue;
      if (junkRe.test(n)) continue;
      if (/[的了着在把被会就还也都很]/.test(n)) continue;
      if (!/^[\u4e00-\u9fff·]+$/.test(n)) continue;
      if (seen.size >= 8) break;
      seen.add(n);
      chars.push({ id: 'c-' + stMakeId('c'), name: n, role: 'supporting', personality: '', speechStyle: '' });
    }
  }

  const storyChapters = [];
  const nodes = [];
  chapters.forEach((ch, i) => {
    const chId = 'ch' + String(i + 1).padStart(2, '0');
    const nodeId = 'n' + (i + 1);
    storyChapters.push({ id: chId, title: ch.title, order: i + 1, goals: [], nodeIds: [nodeId], bodyPath: 'chapters/' + chId + '.md' });
    const exits = [];
    if (i + 1 < chapters.length) exits.push({ id: 'e' + (i + 1), when: '继续', next: 'n' + (i + 2) });
    nodes.push({ id: nodeId, chapterId: chId, title: ch.title, entry: '本章开始', exit: exits, lockedBeats: [], allowedDivergence: 'branch', presentCharacters: chars.slice(2).map(c => c.id), summary: (ch.content || '').slice(0, 400) });
  });

  return {
    id: packId,
    title: title || ('导入：' + now.slice(0, 10)),
    source: { type: 'novel', refs: [] },
    characters: chars,
    worldBookIds: [],
    chapters: storyChapters,
    nodes: nodes,
    loreEntries: [],
    defaultMode: 'mainline',
    maxTier: 'standard',
    language: 'zh',
    createdAt: now,
    updatedAt: now,
    uploadChapters: chapters,
  };
}

async function stImportNovel(file, title) {
  const text = await stDecodeTextFile(file);
  const chapters = stSplitChapters(text);
  if (chapters.length < 1) throw new Error('未识别到章节');
  const pack = stBuildPackFromNovel(title || file.name.replace(/\.[^.]+$/, ''), chapters);
  // First create pack, then write chapter bodies
  const saved = await stApi('/packs', { method: 'POST', body: JSON.stringify(pack) });
  for (const ch of pack.uploadChapters) {
    const chId = saved.chapters.find(c => c.title === ch.title)?.id;
    if (chId) {
      const rel = 'chapters/' + chId + '.md';
      await stApi('/packs/' + encodeURIComponent(saved.id) + '/chapters/' + encodeURIComponent(rel), { method: 'PUT', body: JSON.stringify({ content: ch.content }) });
    }
  }
  return saved;
}

async function stCreateEmptyPack() {
  const title = ($('st-pack-title').value || '').trim();
  if (!title) { showToast('请输入标题', 'warning'); return; }
  const packId = 'pack-' + stSlugify(title) + '-' + Date.now().toString().slice(-4);
  const now = new Date().toISOString();
  const pack = {
    id: packId, title,
    source: { type: 'manual', refs: [] },
    characters: [{ id: 'c-player', name: '玩家', role: 'player', personality: '', speechStyle: '' }],
    worldBookIds: [],
    chapters: [
      { id: 'ch01', title: '第一章', order: 1, goals: ['开场'], nodeIds: ['n1'], bodyPath: 'chapters/ch01.md' },
      { id: 'ch02', title: '第二章', order: 2, goals: ['推进'], nodeIds: ['n2'], bodyPath: 'chapters/ch02.md' }
    ],
    nodes: [
      { id: 'n1', chapterId: 'ch01', title: '开局', entry: '故事从这里开始', exit: [{ id: 'e1', when: '继续', next: 'n2' }], lockedBeats: [], allowedDivergence: 'branch', presentCharacters: [], summary: '' },
      { id: 'n2', chapterId: 'ch02', title: '推进', entry: '情节推进', exit: [], lockedBeats: [], allowedDivergence: 'branch', presentCharacters: [], summary: '' }
    ],
    loreEntries: [],
    defaultMode: 'mainline',
    maxTier: 'standard',
    language: 'zh',
    createdAt: now,
    updatedAt: now,
  };
  await stApi('/packs', { method: 'POST', body: JSON.stringify(pack) });
  // Write empty chapter bodies
  await stApi('/packs/' + packId + '/chapters/' + encodeURIComponent('chapters/ch01.md'), { method: 'PUT', body: JSON.stringify({ content: '（在此粘贴第一章正文）' }) });
  await stApi('/packs/' + packId + '/chapters/' + encodeURIComponent('chapters/ch02.md'), { method: 'PUT', body: JSON.stringify({ content: '（在此粘贴第二章正文）' }) });
  return packId;
}

async function stDeletePack(id) {
  if (!await showConfirm('删除 Pack？引用它的会话将变为只读。')) return;
  await stApi('/packs/' + encodeURIComponent(id), { method: 'DELETE' });
  await stLoadPacks();
  stStatus('Pack 已删除');
}

function stShowChapter(packId, chId) {
  const p = tavernPacks.find(x => x.id === packId); if (!p) return;
  const ch = p.chapters.find(x => x.id === chId); if (!ch) return;
  tavernPack = p;
  stRenderLore();
  $('st-chapter-view').classList.remove('hidden');
  $('st-chapter-view').dataset.chapterId = chId;
  const pre = $('st-chapter-view').querySelector('pre');
  pre.textContent = '章节：' + ch.title + '\n节点：' + (ch.nodeIds || []).join('、') + '\n正文加载中…';
  stApi('/packs/' + encodeURIComponent(packId) + '/chapters/' + encodeURIComponent(ch.bodyPath))
    .then(body => {
      pre.textContent = (body.content || '').slice(0, 800);
    })
    .catch(e => { pre.textContent = '读取失败：' + e.message; });
}

let stLoreEditIdx = -1;

function stEnsureLoreArray(pack) {
  if (!pack.loreEntries || !Array.isArray(pack.loreEntries)) pack.loreEntries = [];
  return pack.loreEntries;
}

function stRenderLore() {
  const panel = $('st-lore-panel');
  const list = $('st-lore-list');
  if (!panel || !list) return;
  if (!tavernPack) { panel.classList.add('hidden'); return; }
  panel.classList.remove('hidden');
  const entries = stEnsureLoreArray(tavernPack);
  list.innerHTML = '';
  if (!entries.length) {
    list.innerHTML = '<div class="muted sm">暂无 lore，可添加永久条或章范围条</div>';
    return;
  }
  entries.forEach((e, i) => {
    const el = document.createElement('div');
    el.className = 'item' + (stLoreEditIdx === i ? ' active' : '');
    const title = e.title || e.id || ('条目' + (i + 1));
    let meta = (e.permanent ? '永久' : (e.chapterRange || '无范围')) + (e.nodeIds && e.nodeIds.length ? ' · nodes ' + e.nodeIds.join(',') : '');
    const trig = [];
    if (e.triggerPresent && e.triggerPresent.length) trig.push('在场:' + e.triggerPresent.join(','));
    if (e.triggerItem && e.triggerItem.length) trig.push('持有:' + e.triggerItem.join(','));
    if (e.triggerBondMin && Object.keys(e.triggerBondMin).length) trig.push('羁绊:' + Object.entries(e.triggerBondMin).map(([k,v]) => k + '≥' + v).join(','));
    if (trig.length) meta += ' · 🪝' + trig.join('|');
    el.innerHTML = '<span class="t"></span><small></small>';
    el.querySelector('.t').textContent = title;
    el.querySelector('small').textContent = meta;
    el.onclick = () => stOpenLoreEditor(i);
    list.appendChild(el);
  });
}

function stOpenLoreEditor(idx) {
  stLoreEditIdx = idx;
  $('st-lore-editor').classList.remove('hidden');
  const e = idx >= 0 ? stEnsureLoreArray(tavernPack)[idx] : { title: '', text: '', chapterRange: '', permanent: true, nodeIds: [] };
  $('st-lore-title').value = e.title || '';
  $('st-lore-text').value = e.text || e.content || '';
  $('st-lore-range').value = e.chapterRange || '';
  $('st-lore-perm').checked = !!e.permanent || !e.chapterRange;
  $('st-lore-trigger-present').value = (e.triggerPresent || []).join(',');
  $('st-lore-trigger-item').value = (e.triggerItem || []).join(',');
  $('st-lore-trigger-bond').value = Object.entries(e.triggerBondMin || {}).map(([k,v]) => k + ':' + v).join(',');
  stRenderLore();
}

async function stSaveLore() {
  if (!tavernPack) return;
  const entries = stEnsureLoreArray(tavernPack);
  const parseList = (s) => String(s || '').split(/[,，]/).map((x) => x.trim()).filter(Boolean);
  const parseBond = (s) => { const o = {}; String(s || '').split(/[,，]/).forEach((p) => { const [k, v] = p.split(':').map((x) => (x || '').trim()); if (k && v !== undefined && v !== '' && Number.isFinite(Number(v))) o[k] = Number(v); }); return o; };
  const entry = {
    id: (stLoreEditIdx >= 0 && entries[stLoreEditIdx].id) || ('lore-' + Date.now()),
    title: ($('st-lore-title').value || '').trim() || '未命名',
    text: ($('st-lore-text').value || '').trim(),
    chapterRange: ($('st-lore-range').value || '').trim(),
    permanent: !!$('st-lore-perm').checked,
    nodeIds: (stLoreEditIdx >= 0 && entries[stLoreEditIdx].nodeIds) || [],
    triggerPresent: parseList($('st-lore-trigger-present').value),
    triggerItem: parseList($('st-lore-trigger-item').value),
    triggerBondMin: parseBond($('st-lore-trigger-bond').value),
  };
  if (!entry.triggerPresent.length) delete entry.triggerPresent;
  if (!entry.triggerItem.length) delete entry.triggerItem;
  if (!Object.keys(entry.triggerBondMin).length) delete entry.triggerBondMin;
  if (stLoreEditIdx >= 0) entries[stLoreEditIdx] = entry; else entries.push(entry);
  tavernPack.loreEntries = entries;
  tavernPack.updatedAt = new Date().toISOString();
  // strip UI-only if any
  const payload = JSON.parse(JSON.stringify(tavernPack));
  delete payload.uploadChapters;
  await stApi('/packs', { method: 'POST', body: JSON.stringify(payload) });
  $('st-lore-editor').classList.add('hidden');
  stLoreEditIdx = -1;
  await stLoadPacks();
  const fresh = tavernPacks.find(p => p.id === payload.id);
  if (fresh) { tavernPack = fresh; }
  stRenderLore();
  stStatus('Lore 已保存 · ' + entry.title);
}

async function stDeleteLore() {
  if (!tavernPack || stLoreEditIdx < 0) return;
  if (!await showConfirm('删除该 lore 条目？')) return;
  const entries = stEnsureLoreArray(tavernPack);
  entries.splice(stLoreEditIdx, 1);
  tavernPack.loreEntries = entries;
  tavernPack.updatedAt = new Date().toISOString();
  const payload = JSON.parse(JSON.stringify(tavernPack));
  delete payload.uploadChapters;
  await stApi('/packs', { method: 'POST', body: JSON.stringify(payload) });
  $('st-lore-editor').classList.add('hidden');
  stLoreEditIdx = -1;
  await stLoadPacks();
  const fresh = tavernPacks.find(p => p.id === payload.id);
  if (fresh) tavernPack = fresh;
  stRenderLore();
}

function stSyncModeToggle() {
  const box = $('st-mode-toggle');
  if (!box) return;
  if (!tavernSession || tavernSession.packMissing) { box.classList.add('hidden'); return; }
  box.classList.remove('hidden');
  const mode = (tavernSession.playMode || 'mainline').toLowerCase();
  box.querySelectorAll('.st-mode-btn').forEach(btn => {
    btn.classList.toggle('active', (btn.dataset.mode || '') === mode);
  });
}

async function stSetPlayMode(mode) {
  if (!tavernSession || tavernStreaming) return;
  // 支线：始终打开节点选择（总结整本 + 重要节点 + 支线开场）
  if (mode === 'side') {
    await stOpenSidePanel();
    return;
  }
  if ((tavernSession.playMode || '').toLowerCase() === mode) return;
  try {
    const s = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/mode', {
      method: 'POST', body: JSON.stringify({ playMode: mode })
    });
    tavernSession = s;
    stCloseSidePanel();
    stRenderMessages();
    stRenderOptions();
    stSyncModeToggle();
    const sideLabel = tavernSession.sideBranchLabel ? (' · 支线「' + tavernSession.sideBranchLabel + '」') : '';
    stStatus(`${tavernSession.title || '故事馆'} · ${PLAYABLE_LABELS[tavernSession.playable] || ''} · ${PLAY_MODE_LABELS[tavernSession.playMode] || tavernSession.playMode || ''}${sideLabel} · tier ${tavernSession.contentTier || ''} · ${stTurnLabel(tavernSession.turn || 0)}`);
  } catch (e) {
    stStatus('模式切换失败：' + e.message);
  }
}

function stCloseSidePanel() {
  const p = $('st-side-panel');
  if (p) p.classList.add('hidden');
}

async function stOpenSidePanel() {
  if (!tavernSession || tavernSession.packMissing) return;
  const panel = $('st-side-panel');
  const list = $('st-side-node-list');
  const sumEl = $('st-side-novel-summary');
  const meta = $('st-side-panel-meta');
  if (!panel || !list) return;
  // S8.29: in immersive theater, #st-side-panel lives inside the #tab-tavern
  // shell (z=0 stacking context) which is painted *behind* #st-view-play
  // (messages/composer tree). Reparent it into #st-view-play so the overlay
  // actually sits above the story text and receives taps, not the messages.
  if (document.documentElement.getAttribute('data-immersive') === '1') {
    const host = $('st-view-play');
    if (host && panel.parentElement !== host) {
      host.appendChild(panel);
      panel.classList.add('st-side-float');
    }
  }
  panel.classList.remove('hidden');
  list.innerHTML = (typeof stSkeleton === 'function') ? stSkeleton(3) : '加载中…';
  if (sumEl) sumEl.textContent = '正在总结整本小说并选取重要节点…';
  try {
    const cat = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/side-branches');
    if (sumEl) sumEl.textContent = cat.novelSummary || '（暂无整本摘要）';
    if (meta) {
      meta.textContent = (cat.packTitle || '') + ' · ' + ((cat.nodes || []).length) + ' 个关键节点'
        + (cat.resumeNodeId ? (' · 回主线锚点 ' + cat.resumeNodeId) : '');
    }
    list.innerHTML = '';
    const nodes = cat.nodes || [];
    if (!nodes.length) {
      list.innerHTML = (typeof stEmpty === 'function') ? stEmpty('没有可用节点', '请先完善剧本包章节') : '没有可用节点';
      return;
    }
    for (const n of nodes) {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'st-side-node-card';
      const reason = document.createElement('span');
      reason.className = 'reason';
      reason.textContent = n.reason || '关键节点';
      const t = document.createElement('span');
      t.className = 't';
      t.textContent = (n.chapterTitle ? (n.chapterTitle + ' · ') : '') + (n.title || n.id);
      const d = document.createElement('span');
      d.className = 'd';
      d.textContent = (n.summary || n.entry || '').slice(0, 160) || n.id;
      btn.appendChild(reason);
      btn.appendChild(t);
      btn.appendChild(d);
      btn.onclick = () => stEnterSideBranch(n.id);
      list.appendChild(btn);
    }
  } catch (e) {
    list.innerHTML = '';
    stStatus('支线目录加载失败：' + e.message);
    if (sumEl) sumEl.textContent = '加载失败：' + e.message;
  }
}

async function stEnterSideBranch(nodeId) {
  if (!tavernSession || !nodeId || tavernStreaming) return;
  try {
    stStatus('进入支线…', { silent: true });
    const s = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/side-branches/enter', {
      method: 'POST', body: JSON.stringify({ nodeId })
    });
    tavernSession = s;
    stCloseSidePanel();
    stRenderMessages({ forceScroll: true });
    stRenderOptions();
    stRenderFocusBar();
    stSyncModeToggle();
    const sideLabel = tavernSession.sideBranchLabel ? ('「' + tavernSession.sideBranchLabel + '」') : nodeId;
    stStatus((tavernSession.title || '故事馆') + ' · 支线 ' + sideLabel + ' · 已写入支线开场白');
  } catch (e) {
    stStatus('进入支线失败：' + e.message);
  }
}

async function stLoadSaves() {
  const lists = Array.from(document.querySelectorAll('.st-save-list'));
  if (!lists.length) return;
  for (const list of lists) {
    list.innerHTML = '';
    if (!tavernSession) {
      list.innerHTML = stEmpty('先打开会话', '选择会话后可见存档');
      await stLoadWorldline();
      return;
    }
  }
  for (const list of lists) list.innerHTML = stSkeleton(2);
  try {
    const data = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/saves');
    const saves = data.saves || [];
    for (const list of lists) {
      list.innerHTML = '';
      if (!saves.length) {
        list.innerHTML = stEmpty('暂无存档', '点击保存记录当前进度');
        continue;
      }
      for (const s of saves) {
        const el = document.createElement('div');
        el.className = 'item';
        el.innerHTML = '<span class="t"></span><span class="d"></span><div class="row-actions"></div>';
        el.querySelector('.t').textContent = s.label || s.saveId;
        el.querySelector('.d').textContent = stTurnLabel(s.turn || 0) + ' · ' + (s.nodeId || '?') + ' · ' + (PLAY_MODE_LABELS[s.playMode] || s.playMode || '');
        const actions = el.querySelector('.row-actions');
        const btnR = document.createElement('button');
        btnR.type = 'button'; btnR.className = 'sm'; btnR.textContent = '回档';
        btnR.onclick = (ev) => { ev.stopPropagation(); stRestoreSave(s.saveId); };
        const btnF = document.createElement('button');
        btnF.type = 'button'; btnF.className = 'sm'; btnF.textContent = '分叉新会话';
        btnF.title = '从该存档开新分支（旧会话不动）';
        btnF.onclick = (ev) => { ev.stopPropagation(); stForkSave(s.saveId); };
        const btnD = document.createElement('button');
        btnD.type = 'button'; btnD.className = 'ghost sm'; btnD.textContent = '删';
        btnD.onclick = (ev) => { ev.stopPropagation(); stDeleteSave(s.saveId); };
        actions.appendChild(btnR); actions.appendChild(btnF); actions.appendChild(btnD);
        list.appendChild(el);
      }
    }
  } catch (e) {
    for (const list of lists) list.innerHTML = '<div class="muted sm">加载失败</div>';
    console.warn(e);
  }
  await stLoadWorldline();
}

async function stCreateSave() {
  if (!tavernSession) {
    showToast('还没有进行中的会话，请先在故事馆或档案馆开始剧本', 'warning');
    return;
  }
  const label = await showPrompt('存档名称（可空）', { value: '第' + (tavernSession.turn || 0) + '回合' }) || '';
  try {
    await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/saves', {
      method: 'POST', body: JSON.stringify({ label: label.trim() || undefined })
    });
    await stLoadSaves();
    showToast('已存档', 'success');
  } catch (e) { showToast('存档失败：' + e.message, 'error'); }
}

async function stRestoreSave(saveId) {
  if (!tavernSession || !saveId) return;
  if (!await showConfirm('回档会覆盖当前会话进度，确认？')) return;
  try {
    const s = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/saves/' + encodeURIComponent(saveId) + '/restore', { method: 'POST', body: '{}' });
    tavernSession = s;
    stRenderMessages();
    stRenderOptions();
    stSyncModeToggle();
    await stLoadSessions();
    await stLoadSaves();
    stStatus(`${tavernSession.title || '故事馆'} · 已回档 · ${stTurnLabel(tavernSession.turn || 0)} · ${tavernSession.nodeId || ''}`);
  } catch (e) { stStatus('回档失败：' + e.message); }
}

async function stForkSave(saveId) {
  if (!tavernSession || !saveId) return;
  const label = await showPrompt('新分支名称（可空）', { value: '' }) || '';
  try {
    const r = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/saves/' + encodeURIComponent(saveId) + '/fork', {
      method: 'POST', body: JSON.stringify({ label: label.trim() || undefined })
    });
    const ns = r && r.session;
    await stLoadSessions();
    await stLoadSaves();
    if (ns && ns.sessionId) {
      showToast('已分叉到新会话', 'success');
      stStatus('新分支会话 ' + ns.sessionId + ' · ' + stTurnLabel(ns.turn || 0));
    } else showToast('已分叉', 'success');
  } catch (e) { stStatus('分叉失败：' + e.message); }
}

async function stDeleteSave(saveId) {
  if (!tavernSession || !saveId) return;
  if (!await showConfirm('删除该存档？')) return;
  try {
    await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/saves/' + encodeURIComponent(saveId), { method: 'DELETE' });
    await stLoadSaves();
  } catch (e) { stStatus('删除失败：' + e.message); }
}

function stWorldlineShortId(id) {
  const s = String(id || '');
  return s.length > 18 ? s.slice(0, 12) + '…' : s;
}

async function stWorldlineNodeClick(s, currentSaveId) {
  if (!s || !s.saveId || s.saveId === currentSaveId) return;
  const label = s.label || s.saveId;
  if (!await showConfirm('回档到「' + label + '」turn ' + (s.turn || 0) + ' 会覆盖当前进度，确认？')) return;
  stRestoreSave(s.saveId);
}

function stRenderWorldlineLine(line, data) {
  const currentWorldlineId = data && data.currentWorldlineId;
  const currentSaveId = data && data.currentSaveId;
  const isCurrent = line.id === currentWorldlineId;
  const el = document.createElement('div');
  el.className = 'wl-line' + (isCurrent ? ' current' : '');
  const head = document.createElement('div');
  head.className = 'wl-line-head';
  const idEl = document.createElement('span');
  idEl.className = 'wl-line-id';
  idEl.textContent = String(line.id || '');
  const tag = document.createElement('span');
  tag.className = 'wl-tag' + (isCurrent ? ' current' : '');
  tag.textContent = line.forkFromSaveId
    ? '分支 · ← fork 自 ' + stWorldlineShortId(line.forkFromSaveId)
    : '主线';
  head.appendChild(idEl);
  head.appendChild(tag);
  el.appendChild(head);
  const flow = document.createElement('div');
  flow.className = 'wl-flow';
  const saves = (line.saves || []).slice().sort((a, b) => (a.turn || 0) - (b.turn || 0));
  for (const s of saves) {
    const node = document.createElement('button');
    node.type = 'button';
    let cls = 'wl-node';
    if (s.saveId === currentSaveId) cls += ' current';
    if (isCurrent) cls += ' active';
    node.className = cls;
    const label = document.createElement('span');
    label.className = 'wl-label';
    label.textContent = s.label || s.saveId;
    const turn = document.createElement('span');
    turn.className = 'wl-turn';
    turn.textContent = stTurnLabel(s.turn || 0);
    node.appendChild(label);
    node.appendChild(turn);
    node.title = (s.label || s.saveId) + ' · ' + stTurnLabel(s.turn || 0) + ' · ' + (s.nodeId || '');
    node.onclick = () => stWorldlineNodeClick(s, currentSaveId);
    flow.appendChild(node);
  }
  el.appendChild(flow);
  return el;
}

