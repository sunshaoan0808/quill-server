async function stLoadWorldline() {
  const wraps = Array.from(document.querySelectorAll('.st-worldline'));
  if (!wraps.length) return;
  if (!tavernSession) {
    for (const w of wraps) w.innerHTML = stEmpty('先打开会话', '选择会话后可见世界线');
    return;
  }
  for (const w of wraps) w.innerHTML = stSkeleton(1);
  try {
    const worldlineData = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/worldline');
    const lines = (worldlineData && worldlineData.lines) || [];
    const totalSaves = lines.reduce((n, l) => n + ((l.saves || []).length), 0);
    for (const w of wraps) {
      w.innerHTML = '';
      if (!lines.length || !totalSaves) {
        w.innerHTML = stEmpty('暂无存档点', '在会话中保存进度后，这里会出现世界线分支');
        continue;
      }
      const ordered = lines.slice().sort((a, b) => (!!a.forkFromSaveId) - (!!b.forkFromSaveId));
      for (const line of ordered) w.appendChild(stRenderWorldlineLine(line, worldlineData));
    }
  } catch (e) {
    for (const w of wraps) w.innerHTML = '<div class="muted sm">世界线加载失败</div>';
    console.warn(e);
  }
}

async function stExportPackZip() {
  if (!tavernPack || !tavernPack.id) { stStatus('先选择 Pack'); return; }
  try {
    const headers = {};
    if (__authToken()) { headers.Authorization = 'Bearer ' + __authToken(); headers['X-Mobile-Token'] = __authToken(); }
    const res = await fetch(apiBase() + '/api/v1/story-tavern/packs/' + encodeURIComponent(tavernPack.id) + '/export.zip', { headers, cache: 'no-store' });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const blob = await res.blob();
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = (tavernPack.id || 'pack') + '.zip';
    document.body.appendChild(a); a.click(); a.remove();
    stStatus('已导出 ' + a.download);
  } catch (e) { stStatus('导出失败：' + e.message); }
}

function stFileToBase64(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => {
      const s = String(r.result || '');
      const i = s.indexOf(',');
      resolve(i >= 0 ? s.slice(i + 1) : s);
    };
    r.onerror = () => reject(new Error('读取失败'));
    r.readAsDataURL(file);
  });
}

async function stImportPackZip(file) {
  const b64 = await stFileToBase64(file);
  const saved = await stApi('/packs/import', { method: 'POST', body: JSON.stringify({ zipBase64: b64 }) });
  await stLoadPacks();
  stStatus('已导入 Pack：' + (saved.title || saved.id));
  return saved;
}

function stPackChars(pack) {
  return (pack && Array.isArray(pack.characters)) ? pack.characters : [];
}

function stCharNameOf(id, pack) {
  const raw = String(id || '').trim();
  if (!raw) return '';
  const chars = stPackChars(pack);
  const hit = chars.find((c) => c && c.id === raw);
  if (hit && hit.name && String(hit.name).trim()) return String(hit.name).trim();
  // soft match by suffix (legacy random ids)
  const soft = chars.find((c) => c && c.id && (raw.endsWith(c.id) || c.id.endsWith(raw)));
  if (soft && soft.name && String(soft.name).trim()) return String(soft.name).trim();
  // hide narrator/player technical labels
  if (/narrator/i.test(raw)) return '旁白';
  if (/player|reader/i.test(raw)) return '读者';
  // last resort: short id, not full uuid-ish
  if (raw.length > 18) return raw.slice(0, 10) + '…';
  return raw;
}

function stIsPlayableCastId(id, pack) {
  const c = stPackChars(pack).find((x) => x && x.id === id);
  if (!c) return false;
  const role = String(c.role || '').toLowerCase();
  const name = String(c.name || '').trim();
  if (role.includes('narrator') || role.includes('player')) return false;
  if (name === '旁白' || name === '读者' || name === '玩家') return false;
  // junk auto names
  if (/^(露出|眼角|换鞋|随口|轻声|低头)/.test(name)) return false;
  if (name.length < 2 || name.length > 8) return false;
  return true;
}

const stFullPackRetried = new Set();

async function stEnsureFullPack(packId) {
  if (!packId) return null;
  let pack = tavernPacks.find((p) => p.id === packId) || (tavernPack && tavernPack.id === packId ? tavernPack : null);
  if (pack && Array.isArray(pack.characters) && pack.characters.length) {
    tavernPack = pack;
    return pack;
  }
  const fetchFull = async () => {
    const full = await stApi('/packs/' + encodeURIComponent(packId));
    const idx = tavernPacks.findIndex((p) => p.id === packId);
    if (idx >= 0) tavernPacks[idx] = { ...tavernPacks[idx], ...full };
    else tavernPacks.push(full);
    tavernPack = full;
    return full;
  };
  try {
    const full = await fetchFull();
    stFullPackRetried.delete(packId);
    return full;
  } catch (e) {
    console.warn('stEnsureFullPack', e);
    // pack may be transiently unavailable (just imported / dir race): retry once
    // so wand focus/vessel lists don't stay thin. Never loop on a hard 404.
    if (!stFullPackRetried.has(packId)) {
      stFullPackRetried.add(packId);
      try {
        return await fetchFull();
      } catch (e2) {
        console.warn('stEnsureFullPack retry failed', e2);
      }
    }
    return pack || null;
  }
}

function stRenderFocusBar() {
  const bar = $('st-focus-bar');
  const chips = $('st-focus-chips');
  if (!bar || !chips) return;
  if (!tavernSession) { bar.classList.add('hidden'); return; }
  bar.classList.remove('hidden');
  const rotBtn = $('st-rot-toggle');
  if (rotBtn) {
    const rotOn = tavernSession.speakerRotation !== false;
    rotBtn.classList.toggle('active', rotOn);
    rotBtn.setAttribute('aria-pressed', rotOn ? 'true' : 'false');
  }
  const vBtn = $('st-vessel-toggle');
  if (vBtn) {
    const vcur = (tavernSession.entry && tavernSession.entry.vesselCharacterId)
      || (tavernSession.player && tavernSession.player.controlCharacterId) || '';
    vBtn.classList.toggle('active', !!vcur);
    vBtn.setAttribute('aria-pressed', !!vcur ? 'true' : 'false');
  }
  const pack = tavernPacks.find(p => p.id === tavernSession.packId) || tavernPack;
  let present = (tavernSession.presentCharacterIds || []).slice();
  // Drop deleted/junk ids that no longer exist on pack; keep order
  if (stPackChars(pack).length) {
    const known = new Set(stPackChars(pack).map((c) => c.id));
    const cleaned = present.filter((id) => known.has(id) && stIsPlayableCastId(id, pack));
    if (cleaned.length) present = cleaned;
    else {
      // fall back to pack cast if session list is all junk
      present = stPackChars(pack).filter((c) => stIsPlayableCastId(c.id, pack)).map((c) => c.id);
    }
  }
  const focus = tavernSession.focusCharacterId || '';
  chips.innerHTML = '';
  if (!present.length) {
    chips.innerHTML = '<span class="muted sm">暂无在场角色，可在下方选择容器角色继续</span>';
    return;
  }
  for (const id of present) {
    const btn = document.createElement('button');
    btn.type = 'button';
    const isFocus = id === focus;
    btn.className = 'st-focus-chip' + (isFocus ? ' active' : '');
    const label = stCharNameOf(id, pack);
    btn.textContent = isFocus ? (label + ' · 焦点') : label;
    btn.title = label + (isFocus ? '（当前焦点）' : '（点击设为焦点）');
    btn.dataset.characterId = id;
    btn.onclick = () => stSetFocus(id);
    chips.appendChild(btn);
  }
}

async function stSetFocus(characterId) {
  if (!tavernSession || tavernStreaming) return;
  try {
    const body = { characterId };
    if ($('st-speaker-rot')) body.speakerRotation = !!$('st-speaker-rot').checked;
    const s = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/focus', {
      method: 'POST', body: JSON.stringify(body)
    });
    tavernSession = s;
    stRenderFocusBar();
  stFillVesselSelect();
    // Mask 面具（Omate 对齐）：焦点即身份——切换后同步刷新立绘/角色背景
    try { stRenderSprite(); } catch (_) {}
    try { if (window.stRefreshImmerseBg) stRefreshImmerseBg(); } catch (_) {}
    const _fp = tavernPacks.find(p => p.id === tavernSession.packId) || tavernPack;
    stStatus(`${tavernSession.title || '故事馆'} · 焦点 ${stCharNameOf(tavernSession.focusCharacterId, _fp) || '-'} · ${stTurnLabel(tavernSession.turn || 0)}`);
  } catch (e) { stStatus('切换焦点失败：' + e.message); }
}

const _rotBtn = $('st-rot-toggle');

function stFillVesselSelect() {
  const picker = $('st-vessel-picker');
  if (!picker || !tavernSession) return;
  const pack = tavernPacks.find(p => p.id === tavernSession.packId) || tavernPack;
  let chars = stPackChars(pack).filter((c) => {
    const role = String(c.role || '').toLowerCase();
    const n = String(c.name || '').trim();
    if (role.includes('narrator')) return false;
    if (n === '旁白') return false;
    return !!(c.id && n);
  });
  const cur = (tavernSession.entry && tavernSession.entry.vesselCharacterId)
    || (tavernSession.player && tavernSession.player.controlCharacterId)
    || '';
  picker.innerHTML = '';
  const title = document.createElement('div');
  title.className = 'st-vessel-picker-title';
  title.textContent = chars.length ? '选择容器角色' : (pack ? '（本包无可用角色）' : '（加载人物中…）');
  picker.appendChild(title);
  const mkOpt = (id, label) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'st-vessel-opt' + (id === cur ? ' active' : '');
    b.textContent = label;
    b.onclick = () => stRebindVessel(id);
    return b;
  };
  picker.appendChild(mkOpt('', '不附身（旁白视角）'));
  for (const c of chars) picker.appendChild(mkOpt(c.id, stCharNameOf(c.id, pack)));
}

async function stRebindVessel(vesselCharacterId) {
  if (!tavernSession || tavernStreaming) return;
  try {
    const s = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/rebind-vessel', {
      method: 'POST', body: JSON.stringify({ vesselCharacterId: vesselCharacterId || null })
    });
    tavernSession = s;
    stRenderMessages();
    stRenderFocusBar();
    stFillVesselSelect();
    const _vp = tavernPacks.find(p => p.id === s.packId) || tavernPack;
    stStatus('已换壳 · ' + (stCharNameOf(vesselCharacterId || '', _vp) || '不附身'));
    const _m = document.getElementById('st-wand-menu');
    const _b = document.getElementById('st-wand-btn');
    if (_m) _m.classList.add('hidden');
    if (_b) _b.setAttribute('aria-expanded', 'false');
  } catch (e) { stStatus('换壳失败：' + e.message); }
}

const vToggle = $('st-vessel-toggle');

let stNodeEditId = null;

function stParseExitsText(text) {
  const exits = [];
  const lines = String(text || '').split(/\n+/);
  let i = 0;
  for (const line of lines) {
    const t = line.trim();
    if (!t) continue;
    const parts = t.split('|');
    const when = (parts[0] || '').trim();
    const next = (parts[1] || '').trim();
    if (!when || !next) continue;
    i += 1;
    exits.push({ id: 'e' + i, when, next });
  }
  return exits;
}

function stExitsToText(exits) {
  return (exits || []).map(e => (e.when || '') + '|' + (e.next || '')).join('\n');
}

function stRenderNodes() {
  const panel = $('st-node-panel');
  const list = $('st-node-list');
  if (!panel || !list) return;
  if (!tavernPack) { panel.classList.add('hidden'); return; }
  panel.classList.remove('hidden');
  const nodes = tavernPack.nodes || [];
  list.innerHTML = '';
  if (!nodes.length) {
    list.innerHTML = '<div class="muted sm">暂无节点，可添加</div>';
    return;
  }
  for (const n of nodes) {
    const el = document.createElement('div');
    el.className = 'item' + (stNodeEditId === n.id ? ' active' : '');
    const exits = (n.exit || []).map(e => e.next).filter(Boolean).join('→') || '（无出口）';
    el.innerHTML = '<span class="t"></span><small></small><div class="ex"></div>';
    el.querySelector('.t').textContent = (n.title || n.id) + ' · ' + (n.id || '');
    el.querySelector('small').textContent = '章 ' + (n.chapterId || '?');
    el.querySelector('.ex').textContent = '→ ' + exits;
    el.onclick = () => stOpenNodeEditor(n.id);
    list.appendChild(el);
  }
}

function stOpenNodeEditor(nodeId) {
  if (!tavernPack) return;
  stNodeEditId = nodeId || null;
  $('st-node-editor').classList.remove('hidden');
  $('st-node-msg').textContent = '';
  const n = (tavernPack.nodes || []).find(x => x.id === nodeId);
  if (!n) {
    // new
    const ch0 = (tavernPack.chapters && tavernPack.chapters[0] && tavernPack.chapters[0].id) || 'ch01';
    const nid = 'n' + Date.now().toString().slice(-4);
    $('st-node-id').value = nid;
    $('st-node-id').disabled = false;
    $('st-node-chapter').value = ch0;
    $('st-node-title').value = '新节点';
    $('st-node-entry').value = '';
    $('st-node-summary').value = '';
    $('st-node-exits').value = '';
    stNodeEditId = null;
  } else {
    $('st-node-id').value = n.id || '';
    $('st-node-id').disabled = true;
    $('st-node-chapter').value = n.chapterId || '';
    $('st-node-title').value = n.title || '';
    $('st-node-entry').value = n.entry || '';
    $('st-node-summary').value = n.summary || '';
    $('st-node-exits').value = stExitsToText(n.exit || []);
  }
  stRenderNodes();
}

async function stSaveNode() {
  if (!tavernPack) return;
  const id = ($('st-node-id').value || '').trim();
  const chapterId = ($('st-node-chapter').value || '').trim();
  const title = ($('st-node-title').value || '').trim();
  if (!id || !chapterId || !title) {
    $('st-node-msg').textContent = 'ID/章节/标题必填';
    return;
  }
  const node = {
    id,
    chapterId,
    title,
    entry: ($('st-node-entry').value || '').trim(),
    summary: ($('st-node-summary').value || '').trim(),
    exit: stParseExitsText($('st-node-exits').value),
    lockedBeats: [],
    allowedDivergence: 'branch',
    presentCharacters: (tavernPack.characters || []).map(c => c.id).slice(0, 4),
  };
  const nodes = Array.isArray(tavernPack.nodes) ? tavernPack.nodes.slice() : [];
  const idx = nodes.findIndex(n => n.id === id);
  if (idx >= 0) nodes[idx] = { ...nodes[idx], ...node };
  else nodes.push(node);
  // keep chapter.nodeIds in sync lightly
  const chapters = (tavernPack.chapters || []).map(ch => {
    const c = { ...ch, nodeIds: Array.isArray(ch.nodeIds) ? ch.nodeIds.slice() : [] };
    if (c.id === chapterId && !c.nodeIds.includes(id)) c.nodeIds.push(id);
    return c;
  });
  tavernPack = { ...tavernPack, nodes, chapters, updatedAt: new Date().toISOString() };
  try {
    const payload = JSON.parse(JSON.stringify(tavernPack));
    delete payload.uploadChapters;
    const saved = await stApi('/packs', { method: 'POST', body: JSON.stringify(payload) });
    tavernPack = saved;
    stNodeEditId = id;
    $('st-node-id').disabled = true;
    $('st-node-msg').textContent = '已保存';
    stRenderNodes();
    stStatus('节点已保存 · ' + id);
  } catch (e) {
    $('st-node-msg').textContent = '保存失败：' + e.message;
  }
}

async function stDeleteNode() {
  if (!tavernPack) return;
  const id = ($('st-node-id').value || '').trim();
  if (!id) return;
  if (!await showConfirm('删除节点 ' + id + '？')) return;
  const nodes = (tavernPack.nodes || []).filter(n => n.id !== id);
  const chapters = (tavernPack.chapters || []).map(ch => ({
    ...ch,
    nodeIds: (ch.nodeIds || []).filter(nid => nid !== id),
  }));
  // scrub exits pointing to deleted
  for (const n of nodes) {
    n.exit = (n.exit || []).filter(e => e.next !== id);
  }
  tavernPack = { ...tavernPack, nodes, chapters, updatedAt: new Date().toISOString() };
  try {
    const payload = JSON.parse(JSON.stringify(tavernPack));
    delete payload.uploadChapters;
    const saved = await stApi('/packs', { method: 'POST', body: JSON.stringify(payload) });
    tavernPack = saved;
    $('st-node-editor').classList.add('hidden');
    stNodeEditId = null;
    stRenderNodes();
    stStatus('节点已删除 · ' + id);
  } catch (e) {
    $('st-node-msg').textContent = '删除失败：' + e.message;
  }
}

const ST_CHAR_ROLE_MAP = {
  protagonist: '主角', main: '主角', lead: '主角',
  antagonist: '反派', villain: '反派',
  supporting: '配角', side: '配角', secondary: '配角',
  narrator: '旁白',
  player: '玩家', reader: '玩家',
  npc: 'NPC', extra: '龙套',
};

const ST_CHAR_FIELDS = [
  { key: 'personality',        label: '性格',       icon: 'personality' },
  { key: 'speechStyle',        label: '说话风格',   icon: 'speechStyle' },
  { key: 'motivation',         label: '动机',       icon: 'motivation' },
  { key: 'relationships',      label: '关系',       icon: 'relationships',  array: true },
  { key: 'mentalModels',       label: '心智模型',   icon: 'mentalModels',   array: true },
  { key: 'decisionHeuristics', label: '决策启发式', icon: 'decisionHeuristics', array: true },
  { key: 'beliefs',            label: '信念',       icon: 'beliefs',        array: true },
];

const ST_CHAR_ICONS_SVG = {
  personality:      '<circle cx="12" cy="12" r="10"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><line x1="9" y1="9" x2="9.01" y2="9"/><line x1="15" y1="9" x2="15.01" y2="9"/>',
  speechStyle:      '<path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z"/>',
  motivation:       '<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="6"/><circle cx="12" cy="12" r="2"/>',
  relationships:    '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
  mentalModels:     '<path d="M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20Z"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
  decisionHeuristics: '<path d="m16 16-4-4-4 4"/><path d="M12 12v9"/><path d="m8 8 4 4 4-4"/>',
  beliefs:          '<path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z"/>',
  chevron:          '<path d="m9 18 6-6-6-6"/>',
};

function stCharIconSvg(name, size) {
  var paths = ST_CHAR_ICONS_SVG[name] || ST_CHAR_ICONS_SVG.personality;
  var s = size || 12;
  return '<svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="' + s + '" height="' + s + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' + paths + '</svg>';
}

function stCharRoleLabel(role) {
  var r = String(role || '').trim().toLowerCase();
  return ST_CHAR_ROLE_MAP[r] || r || '角色';
}

function stCharFilterPack(chars) {
  if (!Array.isArray(chars)) return [];
  var junkRe = /^(露出|眼角|换鞋|随口|轻声|低头|抬起|转身|伸手|走过去|看向|听见|突然)/;
  return chars.filter(function (c) {
    if (!c) return false;
    var role = String(c.role || '').toLowerCase();
    var n = String(c.name || '').trim();
    if (role.includes('narrator') || role.includes('player')) return false;
    if (n === '旁白' || n === '读者' || n === '玩家') return false;
    if (!n || n.length < 2 || n.length > 12) return false;
    if (junkRe.test(n)) return false;
    if (/[的了着在把被]/.test(n)) return false;
    return true;
  });
}

function stCharRenderCard(ch) {
  var name = escapeHtml(String(ch.name || '').trim() || '???');
  var role = stCharRoleLabel(ch.role);
  var initial = name.charAt(0);

  // Avatar
  var avatarHtml;
  if (ch.avatar && String(ch.avatar).trim()) {
    avatarHtml = '<div class="st-char-avatar"><img src="' + escapeHtml(ch.avatar) + '" alt="' + name + '" loading="lazy" /></div>';
  } else {
    avatarHtml = '<div class="st-char-avatar">' + initial + '</div>';
  }

  // Build field rows
  var fieldsHtml = '';
  for (var fi = 0; fi < ST_CHAR_FIELDS.length; fi++) {
    var f = ST_CHAR_FIELDS[fi];
    var val = ch[f.key];
    if (val === undefined || val === null || val === '') continue;
    if (Array.isArray(val) && val.length === 0) continue;

    fieldsHtml += '<div class="st-char-field">';
    fieldsHtml += '<div class="st-char-field-label">' + stCharIconSvg(f.icon) + ' ' + f.label + '</div>';

    if (f.array && Array.isArray(val)) {
      var items = val.slice(0, 3);
      var hasMore = val.length > 3;
      fieldsHtml += '<div class="st-char-field-list">';
      for (var ai = 0; ai < items.length; ai++) {
        fieldsHtml += '<div class="st-char-field-item">' + escapeHtml(String(items[ai])) + '</div>';
      }
      if (hasMore) {
        fieldsHtml += '<div class="st-char-field-item st-char-collapsed" style="display:none">';
        for (var bi = 3; bi < val.length; bi++) {
          fieldsHtml += escapeHtml(String(val[bi])) + (bi < val.length - 1 ? '<br>' : '');
        }
        fieldsHtml += '</div>';
        fieldsHtml += '<button type="button" class="st-char-collapse-btn" data-char-toggle="collapsed" data-count="' + (val.length - 3) + '">' + stCharIconSvg('chevron', 10) + ' +' + (val.length - 3) + ' 更多</button>';
      }
      fieldsHtml += '</div>';
    } else {
      fieldsHtml += '<div class="st-char-field-text">' + escapeHtml(String(val)) + '</div>';
    }
    fieldsHtml += '</div>';
  }

  // Evidence refs (collapsible)
  var refs = ch.evidenceRefs;
  if (Array.isArray(refs) && refs.length > 0) {
    fieldsHtml += '<div class="st-char-evidence">';
    fieldsHtml += '<button type="button" class="st-char-evidence-toggle">' + stCharIconSvg('chevron', 10) + ' 证据出处 (' + refs.length + ')</button>';
    fieldsHtml += '<div class="st-char-evidence-body">';
    for (var ri = 0; ri < refs.length; ri++) {
      fieldsHtml += '<span class="st-char-evidence-tag">' + escapeHtml(String(refs[ri])) + '</span> ';
    }
    fieldsHtml += '</div></div>';
  }

  // SoulLink 档案（archive: {fields, personality, worldview, family, relationships, memory}）
  var arch = ch.archive;
  if (arch && typeof arch === 'object') {
    fieldsHtml += '<div class="st-char-archive">';
    fieldsHtml += '<div class="st-char-archive-title">' + stCharIconSvg('personality', 11) + ' 角色档案</div>';
    // 标量字段
    var archF = arch.fields || {};
    var scalarPairs = [['name', '姓名'], ['age', '年龄'], ['gender', '性别'], ['occupation', '职业']];
    var scalarHtml = '';
    for (var si = 0; si < scalarPairs.length; si++) {
      var sv = archF[scalarPairs[si][0]];
      if (sv === undefined || sv === null || sv === '') continue;
      scalarHtml += '<span class="st-archive-scalar">' + scalarPairs[si][1] + ': ' + escapeHtml(String(sv)) + '</span>';
    }
    if (scalarHtml) fieldsHtml += '<div class="st-archive-scalars">' + scalarHtml + '</div>';
    // 分节
    var archSections = [['personality', '性格'], ['worldview', '世界观'], ['family', '家庭'], ['relationships', '关系'], ['memory', '记忆']];
    for (var secI = 0; secI < archSections.length; secI++) {
      var secKey = archSections[secI][0];
      var secLabel = archSections[secI][1];
      var entries = Array.isArray(arch[secKey]) ? arch[secKey] : [];
      if (!entries.length) continue;
      fieldsHtml += '<div class="st-archive-section"><div class="st-archive-section-label">' + secLabel + '</div>';
      for (var eI = 0; eI < entries.length; eI++) {
        var eC = entries[eI] && entries[eI].content !== undefined ? entries[eI].content : String(entries[eI]);
        if (eC === undefined || eC === null || eC === '') continue;
        fieldsHtml += '<div class="st-archive-item">' + escapeHtml(String(eC)) + '</div>';
      }
      fieldsHtml += '</div>';
    }
    // 操作按钮
    fieldsHtml += '<div class="st-archive-actions">' +
      '<button type="button" class="st-archive-btn" data-arch-action="analyze" data-char="' + escapeHtml(ch.id) + '">' + stCharIconSvg('motivation', 10) + ' 分析</button>' +
      '<button type="button" class="st-archive-btn" data-arch-action="refine" data-char="' + escapeHtml(ch.id) + '">' + stCharIconSvg('beliefs', 10) + ' 精编</button>' +
      '</div>';
    fieldsHtml += '</div>';
  }

  // If no fields at all, show minimal card
  var bodyContent = fieldsHtml || '<div class="st-char-empty" style="padding:var(--sp-1h) 0;opacity:.6;font-size:var(--fs-xs)">暂无蒸馏数据</div>';

  return '<div class="st-char-card">' +
    '<div class="st-char-head">' + avatarHtml +
      '<div class="st-char-info"><div class="st-char-name">' + name + '</div>' +
      '<div class="st-char-role">' + escapeHtml(role) + '</div></div></div>' +
    '<div class="st-char-body">' + bodyContent + '</div></div>';
}

function stRenderCharSummary() {
  var container = $('st-char-list');
  if (!container) return;
  var pack = tavernPack; // IIFE 共享变量（_state-part.js let 声明），非 window 全局
  var chars = stCharFilterPack(pack && pack.characters);

  if (!chars.length) {
    container.innerHTML = '<div class="st-char-empty">' +
      '<svg aria-hidden="true" xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.128a4 4 0 0 1 0 7.744"/></svg>' +
      '<span>暂无角色数据</span></div>';
    return;
  }

  var html = '';
  for (var i = 0; i < chars.length; i++) {
    html += stCharRenderCard(chars[i]);
  }
  container.innerHTML = html;

  // Bind collapse/expand toggles
  container.querySelectorAll('.st-char-collapse-btn').forEach(function (btn) {
    btn.onclick = function () {
      var target = btn.parentElement.querySelector('.st-char-collapsed');
      if (!target) return;
      var expanded = target.style.display !== 'none';
      target.style.display = expanded ? 'none' : '';
      btn.classList.toggle('expanded', !expanded);
      var cnt = btn.getAttribute('data-count') || '?';
      btn.innerHTML = stCharIconSvg('chevron', 10) + ' ' + (expanded ? ('+' + cnt + ' 更多') : '收起');
    };
  });

  // Bind evidence toggles
  container.querySelectorAll('.st-char-evidence-toggle').forEach(function (btn) {
    btn.onclick = function () {
      var body = btn.nextElementSibling;
      if (!body) return;
      var showing = body.classList.toggle('show');
      btn.classList.toggle('expanded', showing);
      var svgHtml = stCharIconSvg('chevron', 10);
      var tag = btn.closest('.st-char-evidence');
      var count = tag ? (tag.querySelectorAll('.st-char-evidence-tag').length) : 0;
      btn.innerHTML = svgHtml + ' 证据出处 (' + count + ')' + (showing ? ' ▾' : '');
    };
  });

  // Bind SoulLink 档案 分析/精编 buttons
  container.querySelectorAll('.st-archive-btn').forEach(function (btn) {
    btn.onclick = function () {
      var action = btn.getAttribute('data-arch-action');
      var charId = btn.getAttribute('data-char');
      if (!action || !charId || !tavernPack || !tavernPack.id || typeof stApi !== 'function') return;
      var packId = tavernPack.id;
      var body = { characterId: charId };
      // analyze 需要近期对话：优先当前 URL 会话（hash 里的 session id 最可靠；
      // sessionId 变量可能是旧 partner-session，不能信）
      if (action === 'analyze') {
        var sid = ((location.hash.match(/session\/([^/?#]+)/) || [])[1] || '') ||
          (typeof sessionId === 'string' ? sessionId : '');
        if (sid) body.sessionId = sid;
      }
      btn.disabled = true;
      var old = btn.innerHTML;
      btn.innerHTML = (action === 'analyze' ? '分析中…' : '精编中…');
      stApi('/packs/' + encodeURIComponent(packId) + '/archive/' + action, {
        method: 'POST',
        body: JSON.stringify(body),
      }).then(function (res) {
        if (res && res.changes && res.changes.length) {
          stStatus('档案' + (action === 'analyze' ? '分析' : '精编') + '完成：' + res.changes.length + ' 处变更');
        } else if (res && res.ok) {
          stStatus('档案' + (action === 'analyze' ? '分析' : '精编') + '完成：无变更');
        } else {
          stStatus('档案' + (action === 'analyze' ? '分析' : '精编') + '失败');
        }
        return stRefreshCharSummary();
      }).catch(function () {
        stStatus('档案' + (action === 'analyze' ? '分析' : '精编') + '请求失败');
      }).finally(function () {
        btn.disabled = false;
        btn.innerHTML = old;
      });
    };
  });

}

async function stRefreshCharSummary() {
  // 精简版 pack 的 characters 只有 id/name（列表接口），蒸馏字段(personality 等)缺失时
  // 重拉 full pack 再渲染（stEnsureFullPack 的缓存分支会命中精简版，绕过它直接走 full）
  var pack = tavernPack;
  var chars = stCharFilterPack(pack && pack.characters);
  var needFull = chars.length > 0 && !chars.some(function (c) {
    return c.personality || c.motivation || c.beliefs || (c.relationships && c.relationships.length) || (c.mentalModels && c.mentalModels.length);
  });
  if (needFull && pack && pack.id && typeof stApi === 'function') {
    try {
      var full = await stApi('/packs/' + encodeURIComponent(pack.id));
      if (full && Array.isArray(full.characters)) tavernPack = full;
    } catch (_) { /* keep existing */ }
  }
  stRenderCharSummary();
}

function stImmerseBgUrl() {
  const msgs = (tavernSession && tavernSession.messages) || [];
  // 回溯最近一条有发言人前缀的 assistant 消息
  let name = null;
  for (let i = msgs.length - 1; i >= 0; i--) {
    const m = msgs[i];
    if (m && m.role && m.role !== 'user') {
      const s = stSpeakerNameOf(m);
      if (s) { name = s; break; }
    }
  }
  const pack = tavernPack;
  if (!pack || !Array.isArray(pack.characters)) return null;
  // 有明确发言者：优先其 avatar
  if (name) {
    const cid = stCharIdOf(name);
    const ch = pack.characters.find(function (c) { return c && String(c.id) === String(cid); });
    if (ch) {
      const av = (ch.avatar && String(ch.avatar).trim()) ? String(ch.avatar) : null;
      if (av) return av;
      try {
        const u = stSpriteOf(name);
        if (u) return u;
      } catch (_) {}
    }
  }
  // 叙述体/无发言者：fallback 到在场角色中第一个有 avatar 的（保持背景稳定）
  // 优先 presentCharacterIds，其次 cast 顺序
  const present = (tavernSession && tavernSession.presentCharacterIds) || [];
  const ordered = (present.length ? present : []).concat(pack.characters.map(function (c) { return c.id; }));
  for (let i = 0; i < ordered.length; i++) {
    const cid2 = String(ordered[i]);
    const ch2 = pack.characters.find(function (c) { return c && String(c.id) === cid2; });
    if (ch2 && ch2.avatar && String(ch2.avatar).trim()) return String(ch2.avatar);
  }
  // 最后兜底：cast 顺序第一个有 avatar 的角色
  for (let j = 0; j < pack.characters.length; j++) {
    const c3 = pack.characters[j];
    if (c3 && c3.avatar && String(c3.avatar).trim()) return String(c3.avatar);
  }
  return null;
}

