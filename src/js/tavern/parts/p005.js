function stSetComposerBusy(busy) {
  try {
    if ($('st-stop')) $('st-stop').classList.toggle('hidden', !busy);
    if ($('st-send')) $('st-send').disabled = !!busy;
    if ($('st-continue')) $('st-continue').disabled = !!busy;
    if ($('st-retry')) $('st-retry').disabled = !!busy;
  } catch (_) {}
}

function stScrollToLastMsgTop() {
  window.requestAnimationFrame(function () {
    window.requestAnimationFrame(function () {
      const el = $('st-messages');
      if (!el) return;
      const bubbles = el.querySelectorAll(
        '.st-bubble:not(.st-user):not(.st-role-user):not(.st-bubble-user)'
      );
      const lastA = bubbles.length ? bubbles[bubbles.length - 1] : null;
      if (!lastA) return;
      window.__stProgrammaticScroll = true;
      // #st-messages 全局 scroll-behavior:smooth 会把赋值变成动画、被后续
      // delta 渲染打断——临时禁用做瞬时定位。
      const prev = el.style.scrollBehavior;
      el.style.scrollBehavior = 'auto';
      el.scrollTop = Math.max(0, lastA.offsetTop - el.offsetTop - 12);
      el.style.scrollBehavior = prev;
      window.setTimeout(function () { window.__stProgrammaticScroll = false; }, 50);
    });
  });
}

function stShowLlmIndicator() {
  const el = $('st-llm-indicator');
  if (!el) return;
  el.classList.remove('hidden', 'error');
}

function stHideLlmIndicator() {
  const el = $('st-llm-indicator');
  if (!el) return;
  el.classList.add('hidden');
  el.classList.remove('error');
}

function stErrorLlmIndicator() {
  const el = $('st-llm-indicator');
  if (!el) return;
  el.classList.remove('hidden');
  el.classList.add('error');
}

function stContinue() {
  if (!tavernSession || tavernSession.packMissing || tavernStreaming) return;
  stSend('', { continue: true });
}

async function stRetry() {
  if (!tavernSession || tavernSession.packMissing || tavernStreaming) return;
  const msgs = Array.isArray(tavernSession.messages) ? tavernSession.messages.slice() : [];
  if (!msgs.length) {
    stStatus('还没有可重试的回合');
    return;
  }
  let userText = '';
  let cut = msgs.length;
  const last = msgs[msgs.length - 1];
  if (last && last.role !== 'user') {
    // … user, assistant  → drop both, resend user
    let ui = -1;
    for (let i = msgs.length - 2; i >= 0; i--) {
      if (msgs[i] && msgs[i].role === 'user') { ui = i; break; }
    }
    if (ui < 0) {
      stStatus('找不到上一轮用户发言');
      return;
    }
    userText = String(msgs[ui].content || '');
    const wasContinue = msgs[ui].kind === 'continue' || !userText.trim() || userText === '（续写）';
    cut = ui;
    if (wasContinue) {
      // restore without the pair, then continue
      try {
        stSetComposerBusy(true);
        const next = Object.assign({}, tavernSession, { messages: msgs.slice(0, cut) });
        tavernSession = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId), {
          method: 'PUT', body: JSON.stringify(next),
        });
        stRenderMessages({ forceScroll: true, quiet: true });
        stRenderOptions([]);
        stSetComposerBusy(false);
        stSend('', { continue: true });
      } catch (e) {
        stSetComposerBusy(false);
        stStatus('重试失败：' + ((e && e.message) || e));
      }
      return;
    }
  } else if (last && last.role === 'user') {
    userText = String(last.content || '');
    cut = msgs.length - 1;
    if (last.kind === 'continue' || !userText.trim() || userText === '（续写）') {
      try {
        stSetComposerBusy(true);
        const next = Object.assign({}, tavernSession, { messages: msgs.slice(0, cut) });
        tavernSession = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId), {
          method: 'PUT', body: JSON.stringify(next),
        });
        stRenderMessages({ forceScroll: true, quiet: true });
        stSetComposerBusy(false);
        stSend('', { continue: true });
      } catch (e) {
        stSetComposerBusy(false);
        stStatus('重试失败：' + ((e && e.message) || e));
      }
      return;
    }
  } else {
    stStatus('没有可重试的内容');
    return;
  }
  try {
    stSetComposerBusy(true);
    const next = Object.assign({}, tavernSession, { messages: msgs.slice(0, cut) });
    tavernSession = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId), {
      method: 'PUT', body: JSON.stringify(next),
    });
    stRenderMessages({ forceScroll: true, quiet: true });
    stRenderOptions([]);
    stSetComposerBusy(false);
    await stSend(userText);
  } catch (e) {
    stSetComposerBusy(false);
    stStatus('重试失败：' + ((e && e.message) || e));
  }
}

function stStop() {
  if (window.__stController) { try { window.__stController.abort(); } catch (_) {} }
  const rid = tavernRunId || (tavernSession && tavernSession.activeRunId) || 'force-unlock';
  if (tavernSession && tavernSession.sessionId) {
    stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/stop', {
      method: 'POST', body: JSON.stringify({ runId: rid })
    }).catch(() => {});
  }
  tavernStreaming = false;
  tavernRunId = null;
  const stMsgsBusyStop = $('st-messages');
  if (stMsgsBusyStop) stMsgsBusyStop.removeAttribute('aria-busy');
  clearStStreamPaint();
  stSetComposerBusy(false);
}

let stMediaRec = null;

let stRecChunks = [];

let stRecMime = 'audio/webm';

function stVoiceInputEnabled() {
  return localStorage.getItem('stVoiceInput') === '1';
}

function stSyncRecBtn() {
  const btn = $('st-asr-btn');
  if (!btn) return;
  const rec = stMediaRec && stMediaRec.state === 'recording';
  btn.classList.toggle('st-recording', rec);
  btn.classList.toggle('is-on', !rec && stVoiceInputEnabled());
  btn.disabled = !stVoiceInputEnabled() && !rec;
  btn.title = rec ? '录音中：点击停止' : (stVoiceInputEnabled() ? '点击开始录音（转写后直发）' : '语音输入已关闭，先点旁边 🎙 开关');
  const lab = btn.querySelector('.btn-lab');
  if (lab) lab.textContent = rec ? '停止' : '语音';
}

async function stToggleRecording() {
  if (stMediaRec && stMediaRec.state === 'recording') {
    stMediaRec.stop();
    return;
  }
  if (!stVoiceInputEnabled()) {
    stStatus('🎙 语音输入未开启：点亮旁边「语音双工」开关再试');
    return;
  }
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
    stStatus('📴 此环境不支持麦克风录音');
    return;
  }
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stRecChunks = [];
    const mime = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4', '']
      .find((m) => !m || (window.MediaRecorder && MediaRecorder.isTypeSupported(m))) || '';
    const mr = mime ? new MediaRecorder(stream, { mimeType: mime }) : new MediaRecorder(stream);
    stMediaRec = mr;
    stRecMime = mime || mr.mimeType || 'audio/webm';
    mr.ondataavailable = (e) => { if (e.data && e.data.size) stRecChunks.push(e.data); };
    mr.onstop = () => {
      stream.getTracks().forEach((t) => t.stop());
      stMediaRec = null;
      stSyncRecBtn();
      const blob = new Blob(stRecChunks, { type: stRecMime });
      stRecChunks = [];
      if (!blob.size) { stStatus('🎤 未录到声音'); return; }
      stAsrSend(blob).catch((err) => stStatus('语音转写失败：' + ((err && err.message) || err)));
    };
    mr.onerror = () => {
      stream.getTracks().forEach((t) => t.stop());
      stMediaRec = null;
      stSyncRecBtn();
      stStatus('🎤 录音出错');
    };
    mr.start();
    stStatus('🎤 录音中…再来点一下停止');
    stSyncRecBtn();
  } catch (e) {
    stMediaRec = null;
    stSyncRecBtn();
    stStatus('🚫 麦克风权限被拒，无法录音');
  }
}

async function stAsrSend(blob) {
  const token = localStorage.getItem('kaleido_token') || '';
  const base = localStorage.getItem('kaleido_api_base') || '';
  const fd = new FormData();
  const ext = (stRecMime.indexOf('mp4') >= 0) ? 'm4a' : 'webm';
  fd.append('audio', blob, 'kaleido_capture.' + ext);
  stStatus('⏳ 语音转写中（首次约需几十秒加载引擎）…');
  const res = await fetch(base + '/api/v1/kaleido-tools/asr', {
    method: 'POST',
    headers: token ? { 'Authorization': 'Bearer ' + token } : {},
    body: fd,
  });
  if (!res.ok) {
    let msg = 'HTTP ' + res.status;
    try { const j = await res.json(); if (j && j.error) msg = j.error; } catch (_) {}
    throw new Error(msg);
  }
  const j = await res.json();
  const text = (j && typeof j.text === 'string') ? j.text.trim() : '';
  if (!text) throw new Error('转写为空');
  // 语音双工：转写结果直发（自动朗读开关开启时，回合结束即自动朗读回复）
  stStatus('🎤 已听懂：' + (text.length > 22 ? text.slice(0, 22) + '…' : text));
  stSend(text);
}

let stImmChromeBound = false;

let stImmTapStart = null;

function stSetImmChromeVisible(on) {
  const root = document.documentElement;
  if (root.getAttribute('data-immersive') !== '1') {
    root.classList.remove('imm-top-hidden');
    root.classList.remove('imm-chrome-hidden');
    return;
  }
  // streaming: force chrome so 停止 is reachable
  if (tavernStreaming) {
    root.classList.remove('imm-top-hidden');
    root.classList.remove('imm-chrome-hidden');
    return;
  }
  const layout = $('st-layout');
  if (layout && layout.classList.contains('st-side-open')) {
    root.classList.remove('imm-top-hidden');
    root.classList.remove('imm-chrome-hidden');
    return;
  }
  // S8.29: scrolling hides top bar AND bottom dock together for full-screen
  // reading; returning to top (or tap) restores both. Wand lives in dock.
  const wantHidden = !on;
  const isHidden = root.classList.contains('imm-chrome-hidden');
  if (wantHidden === isHidden) return;
  root.classList.toggle('imm-chrome-hidden', wantHidden);
  root.classList.remove('imm-top-hidden');
  // showing top shrinks message pane — if already near end, keep tail visible
  if (!wantHidden && isHidden) {
    try { stKeepImmTailVisible(); } catch (_) {}
  }
}

function stKeepImmTailVisible() {
  // retained no-op-ish helper for option growth: only nudge if already near end
  const msg = $('st-messages');
  if (!msg) return;
  if (document.documentElement.getAttribute('data-immersive') !== '1') return;
  if (document.documentElement.classList.contains('imm-chrome-hidden')) return;
  const gap = msg.scrollHeight - msg.scrollTop - msg.clientHeight;
  if (gap > 80) return;
  requestAnimationFrame(function () {
    try { msg.scrollTop = msg.scrollHeight; } catch (_) {}
  });
}

function stSyncImmChromeFromScroll() {
  // S8.29: scrolling the story hides top bar AND bottom dock for full-screen
  // reading; wand returns with the dock when back at top.
  const msg = $('st-messages');
  if (!msg) return;
  const root = document.documentElement;
  if (root.getAttribute('data-immersive') !== '1') return;
  const y = msg.scrollTop;
  root.classList.toggle('imm-chrome-hidden', y > 24);
  root.classList.remove('imm-top-hidden');
}

function stShowImmChrome() {
  stSetImmChromeVisible(true);
}

function stArmImmChromeHide() {
  // S8.29: hidden by scroll via imm-chrome-hidden; nothing to arm here.
  document.documentElement.classList.remove('imm-top-hidden');
}

function stToggleImmChromeFromTap() {
  if (document.documentElement.getAttribute('data-immersive') !== '1') return;
  if (tavernStreaming) {
    stSetImmChromeVisible(true);
    return;
  }
  const hidden = document.documentElement.classList.contains('imm-chrome-hidden');
  stSetImmChromeVisible(hidden); // if hidden → show; if shown → hide
}

function stImmTapTargetOk(t) {
  if (!t || !t.closest) return false;
  // don't toggle when interacting with controls
  if (t.closest('button, a, input, textarea, select, label, .st-option-chip, .st-history-fold, .composer-actions, .st-composer-tools, .imm-bar')) {
    return false;
  }
  return true;
}

function stImmTapInCenterBand(clientY, msgEl) {
  if (!msgEl) return false;
  const r = msgEl.getBoundingClientRect();
  if (r.height < 8) return false;
  const y = (clientY - r.top) / r.height;
  // middle band of the stage (not top/bottom chrome edges)
  return y >= 0.22 && y <= 0.78;
}

function stBindImmChrome() {
  const msg = $('st-messages');
  if (msg && !msg._immChromeBound) {
    msg.addEventListener('pointerdown', function (e) {
      if (e.button != null && e.button !== 0) return;
      stImmTapStart = {
        x: e.clientX,
        y: e.clientY,
        t: Date.now(),
        ok: stImmTapTargetOk(e.target),
      };
    }, { passive: true });
    msg.addEventListener('pointerup', function (e) {
      const s = stImmTapStart;
      stImmTapStart = null;
      if (!s || !s.ok) return;
      if (e.button != null && e.button !== 0) return;
      const dx = Math.abs((e.clientX || 0) - s.x);
      const dy = Math.abs((e.clientY || 0) - s.y);
      // treat as scroll/drag, not tap
      if (dx > 12 || dy > 12) return;
      if (Date.now() - s.t > 650) return;
      if (!stImmTapInCenterBand(e.clientY, msg)) return;
      stToggleImmChromeFromTap();
    }, { passive: true });
    msg.addEventListener('pointercancel', function () { stImmTapStart = null; }, { passive: true });
    msg._immChromeBound = true;
  }
  // default hidden when binding in immersive
  try { stArmImmChromeHide(); } catch (_) {}
  if (!stImmChromeBound) {
    stImmChromeBound = true;
  }
}

function stMountWizard() {
  const wizView = $('st-view-wizard');
  if (!wizView) return;
  const active = document.querySelector('.tab-panel:not(.hidden)');
  const activeId = active ? active.id : '';
  let host = null;
  if (activeId === 'tab-tavern') {
    host = document.querySelector('#tab-tavern .st-main') || document.querySelector('#tab-tavern');
  } else {
    host = document.querySelector('#tab-packs .st-packs-page') || document.querySelector('#tab-packs');
  }
  if (host && wizView.parentElement !== host) host.appendChild(wizView);
}

function stOpenWizard(playable, source) {
  // R1: 记录进入来源，供向导取消/剧场退出按来源返回
  stNavFrom = (source === 'story-entry' || source === 'packs-detail') ? source : '';
  stMountWizard();
  const wiz = $('st-wizard');
  if (wiz) wiz.classList.remove('hidden');
  const wizView = $('st-view-wizard');
  if (wizView) wizView.classList.remove('hidden');
  // 隐藏档案馆的其他视图
  const listview = $('st-packs-listview');
  const packDetail = $('st-view-pack');
  if (listview) listview.classList.add('hidden');
  if (packDetail) packDetail.classList.add('hidden');
  // 故事馆视图也同步
  const entry = $('st-view-entry');
  const play = $('st-view-play');
  if (entry) entry.classList.add('hidden');
  if (play) play.classList.add('hidden');
  if (playable) $('st-wizard-playable').value = playable;
  if (tavernPack && tavernPack.id) {
    const w = $('st-wizard-pack');
    if (w) w.value = tavernPack.id;
  }
  stWizardToggleRole();
  const msg = $('st-wizard-msg'); if (msg) msg.textContent = '';
}

function stWizardToggleRole() {
  const role = $('st-wizard-role').value;
  $('st-wizard-isekai').classList.toggle('hidden', role !== 'isekai');
  const packId = $('st-wizard-pack').value;
  const pack = tavernPacks.find(p => p.id === packId);
  const vessel = $('st-wizard-vessel');
  if (!vessel || vessel.tagName !== 'SELECT') return;
  const prev = vessel.value;
  const need = (role === 'supporting' || role === 'protagonist');
  vessel.title = '';
  vessel.innerHTML = '';
  const mkOpt = (label, val) => { const o = document.createElement('option'); o.value = val; o.textContent = label; return o; };
  vessel.appendChild(mkOpt(need ? '（请选择要附体的角色）' : '不附身', ''));
  const chars = (pack && Array.isArray(pack.characters)) ? pack.characters : [];
  const shown = chars.filter((c) => {
    const n = String(c.name || '').trim();
    const r = String(c.role || '').toLowerCase();
    if (r.includes('narrator') || n === '旁白') return false;
    return !!(c.id && n);
  });
  for (const c of shown) vessel.appendChild(mkOpt(c.name + '（' + c.id + '）', c.id));
  if (!shown.length) {
    vessel.appendChild(mkOpt('（本包暂无角色卡）', ''));
    vessel.title = '当前包没有可附体角色，请先在角色卡页导入/确认';
    vessel.value = '';
    return;
  }
  vessel.value = ([...vessel.options].some(o => o.value === prev) ? prev : (need ? shown[0].id : ''));
}

async function stCreateSession() {
  const packId = $('st-wizard-pack').value;
  if (!packId) { $('st-wizard-msg').textContent = '请选择 Pack'; return; }
  const playable = $('st-wizard-playable').value;
  // R5: 从作者区/分析页进入时带上 workId，让 U13 的 create 罗盘自动挂载生效
  // （anWorkId() 回退 'default' 时视为无项目，保持原行为）
  const wId = (typeof anWorkId === 'function') ? anWorkId() : '';
  const req = {
    packId,
    playable,
    playMode: $('st-wizard-mode').value,
    userTier: $('st-wizard-tier').value,
    adultConfirmed: !!$('st-wizard-adult').checked,
    workId: (wId && wId !== 'default') ? wId : undefined,
  };
  if (playable === 'P3') {
    const entry = {
      entryRole: $('st-wizard-role').value,
      metaKnowledge: $('st-wizard-meta').value,
      rewriteIntensity: $('st-wizard-rewrite').value,
    };
    const vessel = ($('st-wizard-vessel').value || '').trim();
    if (vessel) entry.vesselCharacterId = vessel;
    if (entry.entryRole === 'isekai') {
      entry.isekai = {};
      const fields = ['name','appearance','cheat','origin'];
      for (const f of fields) entry.isekai[f] = ($('st-wizard-isekai-' + f).value || '').trim();
    }
    req.entry = entry;
  }
  try {
    const s = await stApi('/sessions', { method: 'POST', body: JSON.stringify(req) });
    tavernSession = s;
    $('st-wizard').classList.add('hidden');
    await stLoadSession(s.sessionId);
    $('st-wizard-msg').textContent = '';
  } catch (e) {
    $('st-wizard-msg').textContent = '创建失败：' + e.message;
  }
}

export async function stSwipe(divEl, dir) {
  if (!divEl || !tavernSession) return;
  const mid = divEl.getAttribute('data-mid');
  if (!mid) return;
  const msgs = tavernSession.messages || [];
  const idx = msgs.findIndex(function (m) { return String(m.id || '') === mid; });
  if (idx < 0 || msgs[idx].role === 'user') return;
  const m = msgs[idx];
  if (!m._swipes) m._swipes = [String(m.content || '')];
  if (typeof m._swipeIdx !== 'number') m._swipeIdx = 0;
  // [Swipe 后端持久化] 首次加载时把后端 swipes 映射到 _swipes
  if (m.swipes && m.swipes.length && m._swipes.length <= 1) {
    m.swipes.forEach(function(s){ if (m._swipes.indexOf(s) < 0) m._swipes.push(s); });
  }
  const n = m._swipes.length;
  // 右箭头且已是最后一条 → 请求新备选（reroll 变体）
  if (dir > 0 && m._swipeIdx >= n - 1) {
    if (tavernStreaming) { stStatus('流式进行中，稍候再试'); return; }
    try {
      stStatus('生成备选回复…');
      const prev = String(m.content || '');
      const fresh = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/reroll', { method: 'POST', body: '{}' });
      // reroll 返回 {ok, lastUserMessage, turn: <turn数>}——新回复需重拉会话取最后一条 assistant
      let freshText = '';
      if (fresh && fresh.ok) {
        const s2 = await stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId));
        const arr = (s2 && Array.isArray(s2.messages)) ? s2.messages : [];
        for (let i = arr.length - 1; i >= 0; i--) {
          if (arr[i] && arr[i].role === 'assistant' && String(arr[i].content || '').trim()) {
            freshText = String(arr[i].content).trim(); break;
          }
        }
      }
      if (freshText && freshText !== prev) {
        m._swipes.push(freshText);
        m._swipeIdx = m._swipes.length - 1;
      } else {
        stStatus('新备选与当前一致或获取失败');
        return;
      }
    } catch (e) {
      stStatus('备选失败：' + ((e && e.message) || e));
      return;
    }
  } else {
    let ni = m._swipeIdx + dir;
    if (ni < 0) ni = 0;
    if (ni >= n) return;
    m._swipeIdx = ni;
    try { stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/messages/' + encodeURIComponent(mid) + '/swipe', { method: 'PUT', body: JSON.stringify({ index: ni }) }).catch(function(){}); } catch (e) {}
  }
  // 切正文（只换 body 文本，保留角色/角标/程序卡）
  const bodyEl = divEl.querySelector('.bubble-body');
  if (bodyEl && typeof fillBubbleBody === 'function') {
    fillBubbleBody(bodyEl, m._swipes[m._swipeIdx], { speakerMode: true });
  } else if (bodyEl) {
    bodyEl.textContent = m._swipes[m._swipeIdx];
  }
  const cnt = divEl.querySelector('.st-swipe-cnt');
  if (cnt) cnt.textContent = (m._swipeIdx + 1) + '/' + m._swipes.length;
  const sel = stStatus; if (sel) sel('备选 ' + (m._swipeIdx + 1) + '/' + m._swipes.length);
}

function stSwipePicker(divEl) {
  if (!divEl || !tavernSession) return;
  const mid = divEl.getAttribute('data-mid');
  if (!mid) return;
  const msgs = tavernSession.messages || [];
  const idx = msgs.findIndex(function (m) { return String(m.id || '') === mid; });
  if (idx < 0) return;
  const m = msgs[idx];
  const swipes = (m._swipes && m._swipes.length) ? m._swipes : [String(m.content || '')];
  const cur = (typeof m._swipeIdx === 'number') ? m._swipeIdx : 0;

  // 复用 .st-modal 弹窗体系
  const modal = document.createElement('div');
  modal.className = 'st-modal';
  modal.id = 'st-swipe-picker';
  const card = document.createElement('div');
  card.className = 'st-modal-card';
  const head = document.createElement('div');
  head.className = 'st-modal-head';
  const title = document.createElement('div');
  title.className = 'st-modal-title'; title.textContent = '备选回复 (' + swipes.length + ')';
  const close = document.createElement('button');
  close.type = 'button'; close.className = 'st-modal-close'; close.setAttribute('aria-label', '关闭');
  close.innerHTML = '&#10005;';
  head.appendChild(title); head.appendChild(close);
  const body = document.createElement('div');
  body.className = 'st-modal-body st-swipe-picker-body';
  swipes.forEach(function (text, i) {
    const item = document.createElement('button');
    item.type = 'button';
    item.className = 'st-swipe-pick' + (i === cur ? ' current' : '');
    const num = document.createElement('span');
    num.className = 'st-swipe-pick-num'; num.textContent = (i + 1) + '/' + swipes.length;
    const txt = document.createElement('span');
    txt.className = 'st-swipe-pick-txt';
    txt.textContent = String(text || '').replace(/\s+/g, ' ').slice(0, 140);
    item.appendChild(num); item.appendChild(txt);
    item.onclick = function (e) {
      e.stopPropagation();
      m._swipeIdx = i;
      // 更新正文
      const bodyEl = divEl.querySelector('.bubble-body');
      if (bodyEl && typeof fillBubbleBody === 'function') {
        fillBubbleBody(bodyEl, m._swipes[m._swipeIdx], { speakerMode: true });
      } else if (bodyEl) {
        bodyEl.textContent = m._swipes[m._swipeIdx];
      }
      const cntEl = divEl.querySelector('.st-swipe-cnt');
      if (cntEl) cntEl.textContent = (m._swipeIdx + 1) + '/' + m._swipes.length;
      closeModal();
      const sel2 = stStatus; if (sel2) sel2('已选备选 ' + (m._swipeIdx + 1) + '/' + m._swipes.length);
    };
    body.appendChild(item);
  });
  card.appendChild(head); card.appendChild(body);
  modal.appendChild(card);
  document.body.appendChild(modal);

  function closeModal() {
    if (modal.parentNode) modal.parentNode.removeChild(modal);
    document.removeEventListener('keydown', onKey);
  }
  function onKey(e) {
    if (e.key === 'Escape') closeModal();
  }
  close.onclick = function (e) { e.stopPropagation(); closeModal(); };
  modal.addEventListener('click', function (e) { if (e.target === modal) closeModal(); });
  document.addEventListener('keydown', onKey);
}

async function stEditMessage(divEl) {
  if (!divEl || !tavernSession || tavernStreaming) return;
  const mid = divEl.getAttribute('data-mid');
  if (!mid) return;
  const msgs = tavernSession.messages || [];
  const idx = msgs.findIndex(function (m) { return String(m.id || '') === mid; });
  if (idx < 0) return;
  const m = msgs[idx];
  const bodyEl = divEl.querySelector('.bubble-body');
  const oldText = (bodyEl && bodyEl.textContent) || String(m.content || '');
  const fresh = await showPrompt('编辑消息内容：', { value: oldText });
  if (fresh === null) return;
  if (!String(fresh).trim()) { stStatus('内容不能为空'); return; }
  stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/messages/' + encodeURIComponent(mid), {
    method: 'PUT', body: JSON.stringify({ content: String(fresh) })
  }).then(function (s) {
    tavernSession = s;
    stRenderMessages({ forceScroll: true, quiet: true });
    stStatus('消息已编辑');
  }).catch(function (e) { stStatus('编辑失败：' + ((e && e.message) || e)); });
}

async function stDeleteMessage(divEl) {
  if (!divEl || !tavernSession || tavernStreaming) return;
  const mid = divEl.getAttribute('data-mid');
  if (!mid) return;
  if (!await showConfirm('删除这条消息？')) return;
  stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/messages/' + encodeURIComponent(mid), {
    method: 'DELETE'
  }).then(function (s) {
    tavernSession = s;
    stRenderMessages({ forceScroll: true, quiet: true });
    stStatus('消息已删除');
  }).catch(function (e) { stStatus('删除失败：' + ((e && e.message) || e)); });
}

function stPartialEdit(divEl) {
  if (!divEl || !tavernSession || tavernStreaming) return;
  const mid = divEl.getAttribute('data-mid');
  if (!mid) return;
  const msgs = tavernSession.messages || [];
  const m = msgs.find(function (x) { return String(x.id || '') === mid; });
  if (!m) return;
  const bodyEl = divEl.querySelector('.bubble-body');
  const oldText = (bodyEl && bodyEl.textContent) || String(m.content || '');
  // 弹层
  const overlay = document.createElement('div');
  overlay.className = 'st-modal-overlay';
  overlay.innerHTML =
    '<div class="st-modal st-partial-modal">' +
    '<div class="st-modal-head">部分编辑消息<span class="st-modal-x" title="关闭">✕</span></div>' +
    '<div class="st-partial-hint">修改后保存；仅本消息正文更新，其余消息不变。</div>' +
    '<textarea class="st-partial-text" rows="8" spellcheck="false"></textarea>' +
    '<div class="st-modal-foot"><button type="button" class="ghost st-partial-cancel">取消</button>' +
    '<button type="button" class="st-partial-save">保存</button></div>' +
    '</div>';
  document.body.appendChild(overlay);
  const ta = overlay.querySelector('.st-partial-text');
  ta.value = oldText;
  overlay.querySelector('.st-modal-x').onclick = function () { overlay.remove(); };
  overlay.querySelector('.st-partial-cancel').onclick = function () { overlay.remove(); };
  overlay.addEventListener('click', function (e) { if (e.target === overlay) overlay.remove(); });
  overlay.querySelector('.st-partial-save').onclick = function () {
    const fresh = ta.value;
    if (!String(fresh).trim()) { stStatus('内容不能为空'); return; }
    stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/messages/' + encodeURIComponent(mid), {
      method: 'PUT', body: JSON.stringify({ content: String(fresh) })
    }).then(function (s) {
      tavernSession = s;
      overlay.remove();
      stRenderMessages({ forceScroll: true, quiet: true });
      stStatus('消息已更新');
    }).catch(function (e) { stStatus('保存失败：' + ((e && e.message) || e)); });
  };
  ta.focus();
  ta.setSelectionRange(0, 0);
}

