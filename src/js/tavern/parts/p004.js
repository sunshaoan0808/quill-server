function stCloseVisualModal() {
  const m = $('st-visual-modal');
  if (m) m.classList.add('hidden');
}

async function stGenVisual() {
  const btn = $('st-visual-gen');
  if (!btn || btn.disabled) return;
  btn.disabled = true;
  const old = btn.textContent;
  btn.textContent = '生成中…';
  try {
    const sid = tavernSession && tavernSession.sessionId;
    if (!sid) { stStatus('无会话'); return; }
    const data = await stApi('/sessions/' + encodeURIComponent(sid) + '/assistant', {
      method: 'POST', body: JSON.stringify({ message: '请生成当前剧情的可视化面板：地图、线索图谱、线索板、装备栏（用【面板】JSON 块输出，kind 用 markdown/svg/html）' })
    });
    // 服务端已把【面板】回写 session.panels，客户端直接刷新
    if (tavernSession) {
      try {
        const s = await stApi('/sessions/' + encodeURIComponent(sid));
        if (s && s.panels) tavernSession.panels = s.panels;
      } catch (_) {}
    }
    stRenderPanels($('st-visual-body'));
    stRenderPanels();
    if (data && data.reply && data.reply.trim()) stStatus(data.reply.trim().slice(0, 80));
  } catch (e) {
    stStatus('可视化生成失败：' + (e && e.message || e));
  } finally {
    btn.disabled = false;
    btn.textContent = old;
  }
}

const ST_ASSIST_KEY_PREFIX = 'kaleido_assist_';

const ST_ASSIST_MAX = 200;

function stAssistHistory(sid) {
  try {
    const raw = localStorage.getItem(ST_ASSIST_KEY_PREFIX + String(sid || ''));
    if (!raw) return [];
    const arr = JSON.parse(raw);
    return Array.isArray(arr) ? arr.slice(-ST_ASSIST_MAX) : [];
  } catch (_) { return []; }
}

function stAssistSave(sid, history) {
  try {
    const arr = (history || []).slice(-ST_ASSIST_MAX);
    localStorage.setItem(ST_ASSIST_KEY_PREFIX + String(sid || ''), JSON.stringify(arr));
  } catch (_) {}
}

function stRenderAssist(history) {
  const body = $('st-assist-body');
  if (!body) return;
  body.innerHTML = '';
  const msgs = (history && history.length) ? history : null;
  if (!msgs) {
    const empty = document.createElement('div');
    empty.className = 'st-assist-msg agent';
    empty.textContent = '问助手：当前剧情状态？线索梳理？（多轮记忆已生效）';
    body.appendChild(empty);
  } else {
    for (const m of msgs) {
      const div = document.createElement('div');
      div.className = 'st-assist-msg ' + (m.role === 'user' ? 'user' : 'agent');
      div.textContent = m.content;
      body.appendChild(div);
    }
  }
  body.scrollTop = body.scrollHeight;
}

function stFocusAssistInput() {
  const inp = $('st-assist-input');
  if (!inp) return;
  inp.focus();
  try { inp.scrollIntoView({ behavior: 'smooth', block: 'nearest' }); } catch (_) {}
}

function stOpenAssistModal() {
  const m = $('st-assist-modal');
  if (!m) return;
  m.classList.remove('hidden');
  const storyMode = __curTab() === 'story' || __curTab() === 'adventure' || __curTab() === 'chat';
  // story/冒险/跑团/对话无 tavern 会话：reroll/rewind 是剧场专属，隐藏工具行
  const toolsRow = document.querySelector('.st-assist-tools');
  if (toolsRow) toolsRow.style.display = storyMode ? 'none' : '';
  const sid = storyMode
    ? (__curTab() === 'chat' ? (__chatState().sessionId || '') : (__storyState().storySessionId || ''))
    : (tavernSession && tavernSession.sessionId);
  stRenderAssist(stAssistHistory(sid));
  stFocusAssistInput();
}

function stCloseAssistModal() {
  const m = $('st-assist-modal');
  if (m) m.classList.add('hidden');
  const inp = $('st-assist-input');
  if (inp) inp.value = '';
}

function storyWbIds() {
  const ids = [];
  const wbSel = __curTab() === 'chat' ? $('chat-wb') : (__curTab() === 'adventure' ? $('adv-wb') : $('story-wb'));
  if (wbSel && wbSel.value) ids.push(wbSel.value);
  const ccSel = __curTab() === 'chat' ? $('chat-cc') : (__curTab() === 'adventure' ? $('adv-cc') : $('story-cc'));
  if (ccSel && ccSel.value && typeof __chatState().partner !== 'undefined' && __chatState().partner.characterCards) {
    const cc = __chatState().partner.characterCards.find((c) => c.id === ccSel.value);
    if (cc && cc.worldBookId) ids.push(cc.worldBookId);
  }
  return ids.filter((v, i, a) => a.indexOf(v) === i);
}

async function stSendAssist() {
  const inp = $('st-assist-input');
  const btn = $('st-assist-send');
  if (!inp) return;
  const text = inp.value.trim();
  if (!text) return;
  const storyMode = __curTab() === 'story' || __curTab() === 'adventure' || __curTab() === 'chat';
  const sid = storyMode
    ? (__curTab() === 'chat' ? (__chatState().sessionId || '') : (__storyState().storySessionId || ''))
    : (tavernSession && tavernSession.sessionId);
  inp.value = '';
  if (btn) { btn.disabled = true; btn.textContent = '…'; }
  const history = stAssistHistory(sid);
  history.push({ role: 'user', content: text });
  stAssistSave(sid, history);
  stRenderAssist(history);
  try {
    if (!sid) throw new Error('当前无会话');
    // 带助手对话历史（剔除刚 push 的最后一条 user——那是本次 message）
    const hist = history.slice(0, -1).map((m) => ({ role: m.role, content: String(m.content || '') }));
    // story/冒险/跑团/对话：上下文来自前端本地消息；tavern：服务端会话注入剧情上下文
    const ctxMessages = __curTab() === 'chat'
      ? (__chatState().messages || [])
      : (__storyState().storyMessages || []);
    const data = storyMode
      ? await api('/api/v1/story/assistant', {
          method: 'POST', body: JSON.stringify({
            message: text,
            history: hist,
            title: '',
            kind: __curTab() === 'chat' ? 'chat' : 'story',
            worldBookIds: storyWbIds(),
            messages: ctxMessages.slice(-10).map((m) => ({ role: m.role, content: String(m.content || '') }))
          })
        })
      : await stApi('/sessions/' + encodeURIComponent(sid) + '/assistant', {
          method: 'POST', body: JSON.stringify({ message: text, history: hist })
        });
    const reply = (data && data.reply)
      ? data.reply
      : ('（无回复：' + ((data && data.error) || '未知错误') + '）');
    history.push({ role: 'agent', content: reply });
  } catch (e) {
    history.push({ role: 'agent', content: '请求失败：' + (e && e.message || e) });
  }
  stAssistSave(sid, history);
  stRenderAssist(history);
  if (btn) { btn.disabled = false; btn.textContent = '发送'; }
  stFocusAssistInput();
}

async function stRerollLast() {
  const sid = tavernSession && tavernSession.sessionId;
  if (!sid) { stStatus('当前无会话'); return; }
  const btn = $('st-assist-reroll');
  if (btn) btn.disabled = true;
  try {
    const data = await stApi('/sessions/' + encodeURIComponent(sid) + '/reroll', { method: 'POST', body: '{}' });
    const text = (data && data.lastUserMessage) ? String(data.lastUserMessage) : '';
    stCloseAssistModal();
    if (text && typeof stSend === 'function') {
      await stSend(text);
      stStatus('已重生成上一条回复');
    } else {
      stStatus('已回退 1 回合（无上一条用户消息可重发）');
    }
  } catch (e) {
    stStatus('重生成失败：' + ((e && e.message) || e));
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function stRewindOne() {
  const sid = tavernSession && tavernSession.sessionId;
  if (!sid) { stStatus('当前无会话'); return; }
  const btn = $('st-assist-rewind');
  if (btn) btn.disabled = true;
  try {
    await stApi('/sessions/' + encodeURIComponent(sid) + '/rewind', { method: 'POST', body: JSON.stringify({ steps: 1 }) });
    await stLoadSession(sid);
    stStatus('已回退 1 回合');
  } catch (e) {
    stStatus('回退失败：' + ((e && e.message) || e));
  } finally {
    if (btn) btn.disabled = false;
  }
}

function stFetch(path, opts = {}) {
  const token = localStorage.getItem('kaleido_token') || '';
  const base = localStorage.getItem('kaleido_api_base') || '';
  return fetch(base + path, {
    method: opts.method || 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { 'Authorization': 'Bearer ' + token } : {})
    },
    body: opts.body
  });
}

async function stGenerateImage() {
  if (!tavernSession) { stStatus('无会话'); return; }
  const msgs = tavernSession.messages || [];
  let scene = '';
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant' && String(msgs[i].content || '').trim()) {
      scene = String(msgs[i].content).trim().slice(0, 120);
      break;
    }
  }
  if (!scene) scene = '当前场景';
  const chSel = $('st-image-channel');
  const channel = chSel ? (chSel.dataset.value || 'uniapi') : 'uniapi';
  stStatus('生图中…');
  const res = await stFetch('/api/v1/kaleido-tools/image', {
    body: JSON.stringify({ prompt: '动漫电影感插画，' + scene, channel })
  });
  if (!res.ok) throw new Error('HTTP ' + res.status);
  const data = await res.json();
  if (data && (data.url || data.b64)) {
    stShowImage(data.url || ('data:image/jpeg;base64,' + data.b64));
    stStatus('插图已生成（' + (data.channel || '') + '）');
  } else {
    throw new Error((data && data.error) || '生图无返回');
  }
}

function stSpriteHintsOf(name) {
  const msgs = (tavernSession && tavernSession.messages) || [];
  const pool = [];
  for (let i = 0; i < msgs.length; i++) {
    const c = String((msgs[i] && msgs[i].content) || '');
    let from = 0, idx;
    while ((idx = c.indexOf(name, from)) >= 0) {
      pool.push(c.slice(idx, idx + 90));
      from = idx + name.length;
    }
  }
  let look = '';
  const lookRe = /(肩膀|肌肉|头发|眼睛|个子|身形|穿着|外套|运动服|衬衫|眼镜|短发|长发|身高|清瘦|结实|少年感|西装|校服)/;
  for (const seg of pool) {
    const m = seg.match(lookRe);
    if (m) { look = seg.replace(/\s+/g, ' ').trim().slice(0, 70); break; }
  }
  return { look };
}

async function stGenerateSprite() {
  const sid = tavernSession && tavernSession.sessionId;
  if (!sid) { stStatus('无会话'); return; }
  // 目标角色：最后发言人优先，回退换壳/在场第一个
  let roleName = '';
  const msgs = (tavernSession.messages) || [];
  for (let i = msgs.length - 1; i >= 0; i--) {
    const m = msgs[i];
    if (m && m.role && m.role !== 'user') { roleName = stSpeakerNameOf(m); if (roleName) break; }
  }
  if (!roleName) {
    const vcid = (tavernSession.entry && tavernSession.entry.vesselCharacterId)
      || (tavernSession.player && tavernSession.player.controlCharacterId) || '';
    const present = (tavernSession.presentCharacterIds || [])[0] || '';
    const fallback = vcid || present;
    const pack = tavernPack;
    const ch = fallback && pack && pack.characters
      ? pack.characters.find((c) => c && String(c.id) === String(fallback)) : null;
    roleName = (ch && ch.name) || '';
    if (!roleName) { stStatus('无可生成目标角色'); return; }
  }
  const cid = stCharIdOf(roleName);
  const pack = tavernPack;
  const ch = pack && pack.characters ? pack.characters.find((c) => c && String(c.id) === String(cid)) : null;
  if (!ch) { stStatus('pack 中无此角色：' + roleName); return; }
  const emotion = stEmotionOf({ content: roleName + '：→' }) || '平静';
  const chSel = $('st-image-channel');
  const channel = chSel ? (chSel.dataset.value || 'uniapi') : 'uniapi';
  stStatus('生成立绘中（' + roleName + '·' + emotion + '），约 10-30s…');
  // P2-1c: 性别/形象——角色卡 gender/appearance 为主（手动蒸馏/蒸馏补全写入），
  // 原文只兜底形象片段；风格不再硬编码「美少女」：卡判女性才美少女，否则中性动漫质感
  const hints = stSpriteHintsOf(roleName);
  const cardGender = (ch.gender && String(ch.gender).trim() && String(ch.gender) !== '未知')
    ? String(ch.gender).trim() : '';
  const cardLook = (ch.appearance && String(ch.appearance).trim() && String(ch.appearance) !== '未知')
    ? String(ch.appearance).trim() : '';
  const lookStr = (cardLook || hints.look) ? '，形象：' + (cardLook || hints.look) : '';
  const isFemale = /女/.test(cardGender);
  const style = isFemale ? '美少女游戏立绘质感' : '写实漫画质感';
  const prompt = '动漫风格半身立绘，' + style + '，竖构图，角色：' + roleName
    + (cardGender ? '，' + cardGender : '') + lookStr
    + '，表情：' + emotion + '，干净纯色背景，人物居中';
  const res = await stFetch('/api/v1/kaleido-tools/image', {
    body: JSON.stringify({ prompt: prompt, channel })
  });
  if (!res.ok) throw new Error('HTTP ' + res.status);
  const data = await res.json();
  const url = (data && (data.url || data.b64)) ? (data.url || ('data:image/jpeg;base64,' + data.b64)) : null;
  if (!url) throw new Error((data && data.error) || '生成立绘无返回');
  // 写回 pack：GET 全量 → 改 expressions[emotion] → POST upsert
  const full = await stApi('/packs/' + encodeURIComponent(pack.id));
  if (!full || !Array.isArray(full.characters)) throw new Error('读 pack 失败');
  const target = full.characters.find((c) => c && String(c.id) === String(cid));
  if (!target) throw new Error('pack 角色缺失');
  if (!target.expressions || typeof target.expressions !== 'object') target.expressions = {};
  target.expressions[emotion] = url;
  if (!target.avatar) target.avatar = url;
  const saved = await stApi('/packs', { method: 'POST', body: JSON.stringify(full) });
  // 同步本地 tavernPack 与全局缓存，再刷立绘
  if (saved && tavernPacks) {
    const idx = tavernPacks.findIndex((p) => p && p.id === pack.id);
    if (idx >= 0) tavernPacks[idx] = saved; else tavernPacks.push(saved);
  }
  tavernPack = saved || full;
  stShowImage(url);
  try { stRenderSprite(); } catch (_) {}
  stStatus('立绘已生成并写入 pack（' + roleName + '·' + emotion + '）');
}

function stShowImage(url) {
  let view = $('st-image-view');
  if (!view) {
    view = document.createElement('div');
    view.id = 'st-image-view';
    view.className = 'st-image-view';
    view.innerHTML = '<img alt="生成插图"><button type="button" class="st-image-close" aria-label="关闭">✕</button>';
    view.addEventListener('click', () => view.remove());
    document.body.appendChild(view);
  }
  view.querySelector('img').src = url;
  view.style.display = 'flex';
}

function stCharNameById(id) {
  if (!id) return '';
  const chars = (tavernPack && Array.isArray(tavernPack.characters)) ? tavernPack.characters : [];
  for (let i = 0; i < chars.length; i++) {
    if (String(chars[i].id || '') === String(id)) return (chars[i].name || '').trim();
  }
  return '';
}

function stRenderSprite() {
  const box = $('st-sprite');
  if (!box) return;
  const msgs = (tavernSession && tavernSession.messages) || [];

  // —— Step 1: 确定当前发言者 charId（回溯最近一条有发言人前缀的 assistant 消息）——
  let speakingCharId = '';
  let speakingName = '';
  for (let i = msgs.length - 1; i >= 0; i--) {
    const m = msgs[i];
    if (m && m.role && m.role !== 'user') {
      const speaker = stSpeakerNameOf(m);
      if (!speaker) continue;
      const u = stSpriteOf(speaker);
      if (u) { speakingCharId = stCharIdOf(speaker); speakingName = speaker; break; }
    }
  }

  // —— Step 2: 收集在场角色 ID 列表（presentCharacterIds 优先，无则降级单角色）——
  let characterIds = [];
  const present = (tavernSession && tavernSession.presentCharacterIds) || [];
  if (present.length > 1) {
    // 多角色：遍历 present，过滤掉有立绘的角色
    for (let j = 0; j < present.length; j++) {
      const cid = present[j];
      const name = stCharNameById(cid) || cid;
      const url = stSpriteOf(name);
      if (url) characterIds.push({ id: cid, name: name, url: url });
    }
  }

  // —— Step 3: 降级到单角色（无 presentCharacterIds 或多角色无立绘）——
  if (characterIds.length === 0) {
    // 复用旧逻辑：回溯最后一条有立绘的角色
    let url = null;
    let label = speakingName;
    if (!url && speakingName) url = stSpriteOf(speakingName);
    // 再尝试回溯
    if (!url) {
      for (let i = msgs.length - 1; i >= 0; i--) {
        const m = msgs[i];
        if (m && m.role && m.role !== 'user') {
          const s = stSpeakerNameOf(m);
          if (!s) continue;
          const u = stSpriteOf(s);
          if (u) { url = u; label = s; break; }
        }
      }
    }
    if (!url) {
      box.classList.add('hidden');
      return;
    }
    // 单图降级
    box.classList.remove('hidden');
    box.classList.add('is-single');
    box.setAttribute('aria-label', label ? (label + ' 立绘') : '角色立绘');
    // 清空旧内容（防旧结构残留）
    box.innerHTML = '';
    const slot = document.createElement('div');
    slot.className = 'st-sprite-slot';
    const img = document.createElement('img');
    img.className = 'st-sprite-img';
    img.src = url;
    img.alt = (label || '角色') + ' 立绘';
    img.addEventListener('click', function () { stShowImage(img.src); });
    slot.appendChild(img);
    box.appendChild(slot);
    return;
  }

  // —— Step 4: 多角色阵列渲染 ——
  box.classList.remove('is-single');
  box.classList.remove('hidden');
  box.setAttribute('aria-label', speakingName ? (speakingName + ' 等角色立绘') : '角色立绘');

  // 清除旧结构残留（旧版直接插 img，现统一为 .st-sprite-slot）
  Array.from(box.children).forEach(function (ch) {
    if (!ch.classList || !ch.classList.contains('st-sprite-slot')) ch.remove();
  });

  // 构建新 DOM（diff 更新：保留已有的 slot，增删差额）
  const existingSlots = box.querySelectorAll('.st-sprite-slot');
  const existingCount = existingSlots.length;
  const targetCount = characterIds.length;

  // 增加 slot
  for (let k = existingCount; k < targetCount; k++) {
    const slot = document.createElement('div');
    slot.className = 'st-sprite-slot';
    const img = document.createElement('img');
    img.className = 'st-sprite-img';
    img.alt = '角色立绘';
    img.addEventListener('click', function () { stShowImage(img.src); });
    slot.appendChild(img);
    const label = document.createElement('span');
    label.className = 'st-sprite-label';
    slot.appendChild(label);
    box.appendChild(slot);
  }
  // 删减多余 slot
  while (box.children.length > targetCount) {
    box.removeChild(box.lastChild);
  }

  // 更新每个 slot
  const slots = box.querySelectorAll('.st-sprite-slot');
  for (let k = 0; k < targetCount; k++) {
    const entry = characterIds[k];
    const slot = slots[k];
    const img = slot.querySelector('img');
    const labelEl = slot.querySelector('.st-sprite-label');

    // 发言状态 class
    const isSpeaking = entry.id && speakingCharId && String(entry.id) === String(speakingCharId);
    slot.classList.toggle('st-speaking', !!isSpeaking);
    slot.classList.toggle('st-idle', !isSpeaking);

    // 图片 src
    if (img && img.getAttribute('src') !== entry.url) img.setAttribute('src', entry.url);
    if (img) img.alt = (entry.name || '角色') + ' 立绘';
    // 名字标签
    if (labelEl) labelEl.textContent = entry.name || '';
  }
}

let stTtsAudio = null, stTtsUrl = '';

function stWriterQuality() {
  const btn = document.getElementById('st-writer-quality');
  if (btn && btn.dataset && btn.dataset.value) return btn.dataset.value;
  try { const v = localStorage.getItem('st-writer-quality'); if (v) return v; } catch (_) {}
  return 'lite';
}

function stTtsSync() {
  const r = $('st-tts-btn'), p = $('st-tts-pause'), s = $('st-tts-stop');
  const has = !!stTtsAudio;
  if (r) r.classList.toggle('hidden', has);
  if (s) s.classList.toggle('hidden', !has);
  if (p) {
    p.classList.toggle('hidden', !has);
    const lab = p.querySelector('.btn-lab');
    if (lab) lab.textContent = (has && stTtsAudio.paused) ? '继续' : '暂停';
  }
}

function stTtsPauseToggle() {
  if (!stTtsAudio) return;
  if (stTtsAudio.paused) { stTtsAudio.play().catch(() => {}); stStatus('🔊 播放中'); }
  else { stTtsAudio.pause(); stStatus('⏸ 已暂停'); }
  stTtsSync();
}

function stTtsStop() {
  if (!stTtsAudio) return;
  try { stTtsAudio.pause(); } catch (_) {}
  stTtsAudio = null;
  if (stTtsUrl) { URL.revokeObjectURL(stTtsUrl); stTtsUrl = ''; }
  stStatus('');
  stTtsSync();
}

const ST_VOICE_POOL = [
  'zh-CN-XiaoxiaoNeural', // 女·晓晓
  'zh-CN-YunxiNeural',    // 男·云希
  'zh-CN-YunyangNeural',  // 男·云扬
  'zh-CN-XiaoyiNeural',   // 女·晓伊
  'zh-CN-liaoning-XiaobeiNeural', // 东北女·晓北
];

function stVoiceOf(speaker) {
  if (!speaker) return 'zh-CN-XiaoxiaoNeural';
  // pack.characters[].voice 优先（含摘要字段透传）
  const packs = (typeof tavernPacks !== 'undefined' && Array.isArray(tavernPacks)) ? tavernPacks : [];
  for (const pk of packs) {
    const chars = (pk && Array.isArray(pk.characters)) ? pk.characters : [];
    for (const c of chars) {
      if (c && String(c.name || '').trim() === speaker && c.voice && String(c.voice).trim()) {
        return String(c.voice).trim();
      }
    }
  }
  // hash(roleName) 稳定选池（同角色同音色）
  let h = 0;
  for (let i = 0; i < speaker.length; i++) h = (h * 31 + speaker.charCodeAt(i)) >>> 0;
  return ST_VOICE_POOL[h % ST_VOICE_POOL.length];
}

const ST_EMO_RATE = {
  '愤怒': '+25%', '恐惧': '+15%', '惊讶': '+20%', '厌恶': '+10%',
  '疲惫': '-15%', '悲伤': '-20%', '心动': '-10%', '温柔': '-10%', '平静': '+0%',
};

function stRateOf(speaker) {
  // 从最后一条消息解析发言人情绪（复用 actorStates）
  if (!speaker || !tavernSession || !tavernSession.actorStates) return '';
  const actors = (tavernSession.actorStates.actors) || {};
  const cid = stCharIdOfLocal(speaker);
  const ent = (cid && actors[cid]) ? actors[cid] : actors[speaker];
  if (!ent || !ent.fields) return '';
  const emo = ent.fields.emotion;
  const v = (emo && typeof emo === 'object' && 'value' in emo) ? emo.value : emo;
  return (v && ST_EMO_RATE[v]) ? ST_EMO_RATE[v] : '';
}

function stCharIdOfLocal(name) {
  if (!name) return '';
  const chars = (typeof tavernPacks !== 'undefined' && Array.isArray(tavernPacks)) ? tavernPacks.flatMap(pk => (pk && Array.isArray(pk.characters)) ? pk.characters : []) : [];
  for (const c of chars) if (c && String(c.name || '').trim() === name) return c.id;
  return name;
}

async function stSpeak() {
  if (!tavernSession) { stStatus('无会话'); return; }
  const msgs = tavernSession.messages || [];
  let text = '';
  let speaker = '';
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant' && String(msgs[i].content || '').trim()) {
      text = String(msgs[i].content).trim().slice(0, 500);
      const c = String(msgs[i].content);
      const hit = c.match(/^[【\[]?([^：:\n]{1,12})[】\]]?[：:]/);
      speaker = hit ? hit[1].trim() : '';
      break;
    }
  }
  if (!text) { stStatus('没有可朗读的剧情'); return; }
  if (stTtsAudio) { try { stTtsAudio.pause(); } catch (_) {} stTtsAudio = null; if (stTtsUrl) URL.revokeObjectURL(stTtsUrl); stTtsUrl = ''; }
  const voice = stVoiceOf(speaker);
  const rate = stRateOf(speaker);
  stStatus(speaker ? ('🔊 朗读 ' + speaker + (rate ? '（' + rate + '）' : '') + '…') : '朗读中…');
  const res = await stFetch('/api/v1/kaleido-tools/tts', {
    body: JSON.stringify({ text, voice, rate })
  });
  if (!res.ok) throw new Error('HTTP ' + res.status);
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = new Audio(url);
  stTtsAudio = a; stTtsUrl = url;
  a.onended = () => {
    if (stTtsAudio === a) { stTtsAudio = null; stTtsUrl = ''; }
    stStatus('');
    URL.revokeObjectURL(url);
    stTtsSync();
  };
  a.onerror = () => {
    if (stTtsAudio === a) { stTtsAudio = null; stTtsUrl = ''; }
    stStatus('播放失败');
    URL.revokeObjectURL(url);
    stTtsSync();
  };
  a.play().catch(() => {});
  stStatus('🔊 播放中');
  stTtsSync();
}

async function stSend(text, sendOpts) {
  sendOpts = sendOpts || {};
  const rawIn = (text == null) ? '' : String(text);
  const isContinue = !!sendOpts.continue;
  // continue may be empty; normal send needs non-empty trim
  if ((!isContinue && !rawIn.trim()) || !tavernSession || tavernSession.packMissing || tavernStreaming) return;
  // 吸收自梨园 assistant-gateway：// 开头 → 剧情助手弹窗（独立会话，绝不代写剧情、不混入主线消息）
  if (!isContinue && rawIn.trim().startsWith('//')) {
    const q = rawIn.trim().slice(2).trim();
    stOpenAssistModal();
    const inp = $('st-assist-input');
    if (inp) inp.value = q;
    if (window.stFocusAssistInput) stFocusAssistInput();
    else if (inp) inp.focus();
    if ($('st-input')) $('st-input').value = '';
    return;
  }
  const payload = isContinue ? rawIn : rawIn.trim();
  if ($('st-input')) $('st-input').value = '';
  stSetComposerBusy(true);
  tavernStreaming = true;
  document.documentElement.setAttribute('data-streaming', '1');
  // S9.21: 流式期间公告读屏 (aria-busy)
  const stMsgsBusy = $('st-messages');
  if (stMsgsBusy) stMsgsBusy.setAttribute('aria-busy', 'true');
  try { stSetImmChromeVisible(true); } catch (_) {}
  stShowLlmIndicator();
  stTavernUserScrolled = false;
  tavernSession.messages = tavernSession.messages || [];
  const userMsg = {
    role: 'user',
    content: isContinue ? (payload || '（续写）') : payload,
    id: uid('u'),
    options: [],
  };
  if (isContinue) userMsg.kind = 'continue';
  tavernSession.messages.push(userMsg);
  const agentMsg = { role: 'assistant', content: '', id: uid('a'), options: [] };
  tavernSession.messages.push(agentMsg);
  stRenderMessages({ forceScroll: false });
  // S8.31: 发送后滚到新消息开头（顶部），生成中保持开头，用户从开头下滑阅读
  try { stScrollToLastMsgTop(); } catch (_) {}
  stRenderOptions([]);
  let controller = null;
  let llmHadError = false;
  try {
    let start;
    const stTurnBody = () => JSON.stringify({ message: isContinue ? '' : payload, quality: stWriterQuality() });
    try {
      start = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/turn', { method: 'POST', body: stTurnBody() });
    } catch (e) {
      // Stuck previous turn: auto-stop then retry once
      const msg = String(e.message || e || '');
      if (/turn in progress|409|Conflict/i.test(msg) || e.status === 409) {
        try {
          const rid = (e.body && e.body.activeRunId) || tavernRunId || (tavernSession && tavernSession.activeRunId) || '';
          await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/stop', {
            method: 'POST', body: JSON.stringify({ runId: rid || 'force-unlock' })
          });
        } catch (_) {}
        start = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/turn', { method: 'POST', body: stTurnBody() });
      } else {
        throw e;
      }
    }
    tavernRunId = start.runId;
    controller = new AbortController();
    window.__stController = controller;
    const headers = { Accept: 'text/event-stream' };
    if (__authToken()) { headers.Authorization = 'Bearer ' + __authToken(); headers['X-Mobile-Token'] = __authToken(); }
    const res = await fetch(apiBase() + '/api/v1/story-tavern/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/stream?runId=' + encodeURIComponent(tavernRunId), { headers, cache: 'no-store', signal: controller.signal });
    if (!res.ok) { agentMsg.content = '流式错误 HTTP ' + res.status; stRenderMessages({ forceScroll: true }); return; }
    const reader = res.body.getReader(); const decoder = new TextDecoder('utf-8'); let buf = '';
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      let idx; while ((idx = buf.indexOf('\n')) >= 0) {
        let line = buf.slice(0, idx); buf = buf.slice(idx + 1);
        if (line.endsWith('\r')) line = line.slice(0, -1);
        if (!line || line.startsWith(':')) continue;
        let data = line.startsWith('data:') ? line.slice(5).trimStart() : line;
        let obj; try { obj = JSON.parse(data); } catch (_) { continue; }
        if (obj.runId && obj.runId !== tavernRunId) continue;
        if (obj.type === 'delta' && obj.delta) {
          if (agentMsg._thinkingOnly) {
            agentMsg.content = '';
            agentMsg._thinkingOnly = false;
          }
          agentMsg.content += obj.delta;
          scheduleStStreamPaint();
        } else if (obj.type === 'thinking_delta' && obj.delta) {
          // 内心独白（Omate 对齐）：累积 thinking 内容供渲染折叠区块；
          // 同时保留原「思考中…」占位提示避免 UI 看起来卡死。
          if (!agentMsg._monologue) agentMsg._monologue = '';
          agentMsg._monologue += obj.delta;
          if (!agentMsg.content || agentMsg._thinkingOnly) {
            agentMsg._thinkingOnly = true;
            const tip = '（思考中…）';
            if (!agentMsg.content) agentMsg.content = tip;
            scheduleStStreamPaint();
          }
        } else if (obj.type === 'done') {
          // Finalize typewriter: mark stream-final for full paragraph formatting
          const stEl = $('st-messages');
          if (stEl) {
            stEl.querySelectorAll('.bubble.is-streaming .bubble-body').forEach(function (b) {
              b.setAttribute('data-stream-final', '1');
              b.removeAttribute('data-stream-base');
            });
            stEl.querySelectorAll('.bubble.is-streaming').forEach(function (b) {
              b.classList.remove('is-streaming');
            });
          }
          break;
        } else if (obj.type === 'error') {
          if (!agentMsg.content || agentMsg._thinkingOnly) {
            agentMsg.content = '请求失败：' + (obj.message || '');
            agentMsg._thinkingOnly = false;
          }
          stRenderMessages({ forceScroll: true });
          break;
        }
      }
    }
  } catch (e) {
    const raw = String((e && e.message) || e || '');
    let tip = raw;
    if (/Failed to fetch|NetworkError|Load failed|network/i.test(raw)) {
      // 断线自愈：不立即报死——后端 worker 仍在跑，结果最终会写入 session。
      // 轮询 session 直到 run 结束，恢复完整内容（切页面/网络抖动不丢输出）。
      const sid = tavernSession && tavernSession.sessionId;
      const hadPartial = !!(agentMsg.content && !agentMsg._thinkingOnly && agentMsg.content !== '（思考中…）');
      let recovered = false;
      try {
        for (let attempt = 0; attempt < 36; attempt++) {
          await new Promise((r) => setTimeout(r, 2500));
          const fresh = await stApi('/sessions/' + encodeURIComponent(sid));
          if (!fresh || !Array.isArray(fresh.messages)) break;
          const runDone = !fresh.activeRunId || fresh.activeRunId !== tavernRunId;
          const lastA = fresh.messages[fresh.messages.length - 1];
          const lastHasContent = lastA && lastA.role === 'assistant' && String(lastA.content || '').trim().length > 0;
          if (lastHasContent && runDone) {
            agentMsg.content = String(lastA.content || '');
            if (lastA.reasoning) agentMsg._monologue = lastA.reasoning;
            agentMsg._thinkingOnly = false;
            recovered = true;
            break;
          }
          if (runDone) break; // run 结束但无内容 = 真失败
        }
      } catch (_) {}
      if (recovered) {
        tip = '网络波动，已自动恢复完整内容';
      } else {
        // 未恢复：区分「后端真失败」与「后端仍在生成（上游慢/池忙）」
        let stillRunning = false;
        try {
          const chk = await stApi('/sessions/' + encodeURIComponent(sid));
          stillRunning = !!(chk && chk.activeRunId === tavernRunId);
        } catch (_) {}
        if (stillRunning) {
          tip = hadPartial
            ? '生成中（上游较慢）：已恢复部分内容，稍后刷新可查看完整结果'
            : '生成中（上游较慢）：断线已重连，稍后刷新可查看结果';
        } else {
          tip = hadPartial
            ? '网络波动：仅恢复部分内容，可点「重试」重新生成'
            : '生成失败：上游繁忙或网络断开，可点「重试」重新生成';
          llmHadError = true;
        }
        agentMsg.content = agentMsg.content || ('错误：' + tip);
      }
      stStatus(tip);
    } else if (/turn in progress/i.test(raw)) {
      tip = '上一回合未结束：已尝试解锁，请再发一次';
      agentMsg.content = agentMsg.content || ('错误：' + tip);
      stStatus(tip);
      llmHadError = true;
    } else {
      agentMsg.content = agentMsg.content || ('错误：' + tip);
      stStatus(tip);
      llmHadError = true;
    }
    stRenderMessages({ forceScroll: true });
  } finally {
    tavernStreaming = false;
    const stMsgsBusyEnd = $('st-messages');
    if (stMsgsBusyEnd) stMsgsBusyEnd.removeAttribute('aria-busy');
    // S8.31: LLM 指示器——正常结束消失；出错变红 3s 后消失
    if (llmHadError) {
      stErrorLlmIndicator();
      window.setTimeout(function () { stHideLlmIndicator(); }, 3000);
    } else {
      stHideLlmIndicator();
    }
    clearStStreamPaint();
    stSetComposerBusy(false);
    try {
      const fresh = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId));
      tavernSession = fresh;
      // 内心独白：fresh 重拉覆盖消息对象，把流式累积的 monologue 粘回最后一条 assistant
      if (agentMsg._monologue && fresh && Array.isArray(fresh.messages)) {
        const mArr = fresh.messages;
        for (let i = mArr.length - 1; i >= 0; i--) {
          if (mArr[i] && mArr[i].role === 'assistant') {
            mArr[i]._monologue = agentMsg._monologue;
            break;
          }
        }
      }
      stRenderMessages({ quiet: true });
      // S8.30: 输出完成滚到新消息开头（用户从开头下滑阅读），不再停在底部
      try { stScrollToLastMsgTop(); } catch (_) {}
      // belt: full rebuild must not leave stream chrome
      clearStStreamPaint();
      stRenderOptions();
      stShowImmChrome(); /* auto-show chrome when options appear */
      stRenderFocusBar();
      stRenderRecallBar();
      // P3 自动朗读：开关开启且无用户手动播放中时，朗读最新一条 assistant 消息
      if (localStorage.getItem('stAutoTts') === '1' && !stTtsAudio && !tavernStreaming) {
        stSpeak().catch(() => {});
      }
      stStatus(`${tavernSession.title || '故事馆'} · ${PLAYABLE_LABELS[tavernSession.playable] || ''} · ${PLAY_MODE_LABELS[tavernSession.playMode] || tavernSession.playMode || ''} · node ${tavernSession.nodeId || '?'} · resume ${tavernSession.resumeNodeId || '-'} · ${stTurnLabel(tavernSession.turn || 0)}`);
      stSyncModeToggle();
      // best-effort BGE re-rank against the user turn we just sent
      stRefreshRecallSemantic().catch(() => {});
    } catch (_) {}
  }
}

