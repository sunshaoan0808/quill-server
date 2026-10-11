function stCosine(a, b) {
  if (!a || !b || !a.length || a.length !== b.length) return 0;
  let dot = 0, na = 0, nb = 0;
  for (let i = 0; i < a.length; i++) {
    const x = a[i], y = b[i];
    dot += x * y; na += x * x; nb += y * y;
  }
  const d = Math.sqrt(na) * Math.sqrt(nb);
  return d ? dot / d : 0;
}

function stTokenOverlap(query, text) {
  const q = String(query || '').toLowerCase().replace(/[^\u4e00-\u9fff\w]+/g, ' ').split(/\s+/).filter((t) => t.length > 1);
  const t = String(text || '').toLowerCase();
  if (!q.length) return 0;
  let hit = 0;
  for (const tok of q) if (t.indexOf(tok) >= 0) hit++;
  return hit / q.length;
}

const ST_EMOTION_EMOJI = {
  '平静': '', '开心': '😊', '愤怒': '😠', '悲伤': '😢', '害羞': '😳',
  '惊讶': '😲', '恐惧': '😨', '厌恶': '😒', '疲惫': '😪', '心动': '💗',
};

const ST_SPEAKER_RE = /^[【\[]?([^：:\n]{1,12})[】\]]?[：:]/;

function stSpeakerNameOf(m) {
  const c = (m && m.content) ? String(m.content) : '';
  const hit = c.match(ST_SPEAKER_RE);
  return hit ? hit[1].trim() : '';
}

function stCharIdOf(name) {
  if (!name) return '';
  const chars = (tavernPack && Array.isArray(tavernPack.characters)) ? tavernPack.characters : [];
  for (let i = 0; i < chars.length; i++) {
    if (String(chars[i].name || '').trim() === name) return chars[i].id;
  }
  return name;
}

function stEmotionOf(m) {
  if (!m || !tavernSession || !tavernSession.actorStates) return '';
  const actors = tavernSession.actorStates.actors || {};
  const cid = stCharIdOf(stSpeakerNameOf(m));
  if (cid) {
    const ent = actors[cid];
    if (ent && ent.fields) {
      const emo = ent.fields.emotion;
      const v = (emo && typeof emo === 'object' && 'value' in emo) ? emo.value : emo;
      if (v !== undefined && v !== null && String(v).trim()) return String(v).trim();
    }
  }
  // 兜底：无发言人前缀时，若仅一个角色有情绪字段则用之（单角色对话）
  let fallback = '';
  for (const k in actors) {
    const ent = actors[k];
    if (!ent || !ent.fields) continue;
    const emo = ent.fields.emotion;
    const v = (emo && typeof emo === 'object' && 'value' in emo) ? emo.value : emo;
    if (v === undefined || v === null || !String(v).trim()) continue;
    if (fallback && fallback !== String(v).trim()) return ''; // 多角色都带情绪且名字对不上 → 静默
    fallback = String(v).trim();
  }
  return fallback;
}

function stEmotionEmojiOf(m) {
  return ST_EMOTION_EMOJI[stEmotionOf(m)] || '';
}

function stSpriteOf(roleName) {
  if (!roleName) return null;
  const pack = tavernPack;
  if (!pack || !Array.isArray(pack.characters)) return null;
  const cid = stCharIdOf(roleName);
  const ch = pack.characters.find((c) => c && String(c.id) === String(cid));
  if (!ch) return null;
  const emo = stEmotionOf({ content: roleName + '：→' }); // 以发言人名为传递伪消息
  const exp = (ch.expressions && typeof ch.expressions === 'object') ? ch.expressions : {};
  if (emo && exp[emo]) return String(exp[emo]);
  return (ch.avatar && String(ch.avatar).trim()) ? String(ch.avatar) : null;
}

function stLastUserText() {
  const msgs = (tavernSession && tavernSession.messages) || [];
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i] && msgs[i].role === 'user' && (msgs[i].content || '').trim()) return msgs[i].content.trim();
  }
  return '';
}

function stScoreEvents(events, queryText, queryEmb) {
  const charIds = new Set((tavernSession && tavernSession.presentCharacterIds) || []);
  const curNode = (tavernSession && tavernSession.nodeId) || '';
  const turn = (tavernSession && tavernSession.turn) || 0;
  return (events || []).map((e) => {
    let score = 0;
    if (e.nodeId && e.nodeId === curNode) score += 3;
    for (const a of (e.actors || [])) if (charIds.has(a)) score += 2;
    if ((e.turn || 0) >= turn - 3) score += 1;
    let semantic = 0;
    let mode = 'token';
    if (queryEmb && e.embedding && e.embedding.length) {
      semantic = stCosine(e.embedding, queryEmb);
      mode = 'bge';
      score = score * 0.6 + semantic * 3 * 0.4;
    } else if (queryText) {
      semantic = stTokenOverlap(queryText, (e.kind || '') + ' ' + (e.summary || ''));
      score = score * 0.7 + semantic * 2;
      mode = 'token';
    }
    return { e, score, semantic, mode };
  }).sort((a, b) => b.score - a.score);
}

function stRenderRecallBar(opts) {
  const bar = $('st-recall-bar');
  const list = $('st-recall-list');
  const meta = $('st-recall-meta');
  if (!bar || !list) return;
  // 语义记忆召回条仅「播放视图」显示;进入页/列表页保持隐藏(移动端沉浸播放由 CSS 隐藏)
  const play = $('st-view-play');
  const playing = !!(play && !play.classList.contains('hidden'));
  if (!playing || !tavernSession) {
    bar.classList.add('hidden');
    return;
  }
  const events = (((tavernSession.memoryL2 || tavernSession.memory_l2) || {}).events) || [];
  const withEmb = events.filter((e) => e.embedding && e.embedding.length).length;
  if (!events.length) {
    bar.classList.remove('hidden');
    list.innerHTML = '<div class="st-recall-empty muted sm">尚无 L2 事件 — 多聊几轮后会写入语义缓存</div>';
    if (meta) meta.textContent = '0 事件';
    return;
  }
  bar.classList.remove('hidden');
  const queryText = (opts && opts.queryText) || stLastUserText();
  const queryEmb = opts && opts.queryEmb;
  const ranked = stScoreEvents(events, queryText, queryEmb).slice(0, 6);
  if (meta) {
    meta.textContent =
      events.length + ' 事件 · ' + withEmb + ' 已嵌入' +
      (queryEmb ? ' · BGE 重排' : (queryText ? ' · 词面/结构' : ' · 结构排序')) +
      (queryText ? ' · q「' + String(queryText).slice(0, 18) + (queryText.length > 18 ? '…' : '') + '」' : '');
  }
  list.innerHTML = ranked.map((row, i) => {
    const e = row.e;
    const actors = (e.actors || []).slice(0, 3).join(', ');
    const badge = row.mode === 'bge'
      ? ('cos ' + row.semantic.toFixed(2))
      : (row.mode === 'token' ? ('tok ' + row.semantic.toFixed(2)) : ('sc ' + row.score.toFixed(1)));
    return (
      '<div class="st-recall-item" title="' + String(e.summary || '').replace(/"/g, '&quot;') + '">' +
        '<span class="st-recall-rank">#' + (i + 1) + '</span>' +
        '<span class="st-recall-body">' +
          '<span class="st-recall-kind">' + (e.kind || 'event') + '</span>' +
          '<span class="st-recall-sum">' + String(e.summary || '').slice(0, 72) + (String(e.summary || '').length > 72 ? '…' : '') + '</span>' +
          '<span class="st-recall-sub muted sm">t' + (e.turn || '?') + (actors ? ' · ' + actors : '') + (e.nodeId ? ' · ' + e.nodeId : '') + '</span>' +
        '</span>' +
        '<span class="st-recall-score">' + badge + '</span>' +
      '</div>'
    );
  }).join('');
}

async function stRefreshRecallSemantic() {
  if (!tavernSession) return;
  const q = stLastUserText();
  if (!q) {
    stRenderRecallBar();
    if ($('st-recall-meta')) $('st-recall-meta').textContent = ((tavernSession.memoryL2 || {}).events || []).length + ' 事件 · 无用户句可查询';
    return;
  }
  if ($('st-recall-refresh')) $('st-recall-refresh').disabled = true;
  try {
    const data = await api('/api/v1/embeddings', {
      method: 'POST',
      body: JSON.stringify({ input: q, model: 'BAAI/bge-small-zh-v1.5' }),
    });
    const emb = (((data || {}).data || [])[0] || {}).embedding || [];
    stRenderRecallBar({ queryText: q, queryEmb: emb.length ? emb : null });
  } catch (e) {
    stRenderRecallBar({ queryText: q });
    if ($('st-recall-meta')) {
      const cur = $('st-recall-meta').textContent || '';
      $('st-recall-meta').textContent = cur + ' · embed失败 ' + (e.message || e);
    }
  } finally {
    if ($('st-recall-refresh')) $('st-recall-refresh').disabled = false;
  }
}

function stRenderMessages(opts) {
  const el = $('st-messages'); if (!el) return;
  opts = opts || {};
  const list = (tavernSession && tavernSession.messages) || [];
  if (!tavernSession || !list.length) {
    el.innerHTML = stEmpty('没有出现对话', tavernSession ? '正在准备开场白…若仍为空可点下方输入或重进会话' : '选择剧本包与玩法，开始一场新的叙事');
    el.scrollTop = 0;
    return;
  }

  function roleClassOf(m) {
    return m.role === 'user' ? 'user' : (m.role === 'narrator' ? 'narrator' : 'agent');
  }
  function roleLabelOf(m) {
    return m.role === 'user' ? '你' : (m.role === 'narrator' ? '旁白' : '故事');
  }
  function bodyOf(m) {
    // Always strip option protocol from bubbles (stream + final). Chips own the choices.
    if (m && (m.kind === 'continue' || (m.role === 'user' && !(String(m.content || '').trim())))) {
      return (m.content && String(m.content).trim()) ? m.content : '（续写）';
    }
    const raw = (m.role === 'user') ? (m.content || '') : stripChoicesBlock(m.content || '');
    // 流式阶段程序卡原文先不显示（闪烁防抖）：最终保存后 program 字段渲染 iframe
    if (opts.stream && m.role !== 'user') {
      return String(raw).replace(/【程序】[\s\S]*?【\/程序】/g, '').trim() || raw;
    }
    const body = applyStRegexScripts(raw, m.role);
    // 纯询问回合（【询问】停笔卡）：无正文但有选项 → 占位卡文本（吸收自梨园 ask_director）
    if (!String(body).trim() && m.role !== 'user' && Array.isArray(m.options) && m.options.length) {
      return '（请选择后续走向）';
    }
    return body;
  }
  function extraClassOf(m) {
    if (m && (m.kind === 'continue' || (m.role === 'user' && !(String(m.content || '').trim())))) return 'is-continue';
    return '';
  }

  // S8.25: fold older than last ST_VISIBLE_TURNS 对话 (user-started rounds)
  const fold = stMessageFoldPlan(list);
  const startIdx = (!stHistoryExpanded && fold.foldUntil > 0) ? fold.foldUntil : 0;

  const streamTail = !!(opts.stream && list.length && list[list.length - 1] && list[list.length - 1].role !== 'user');
  if (streamTail) {
    const last = list[list.length - 1];
    const mid = last.id || ('st-idx-' + (list.length - 1));
    let node = el.querySelector('.bubble[data-mid="' + cssEscape(mid) + '"]');
    if (!node) {
      // append missing bubbles only within visible window (don't re-inflate folded history mid-stream)
      const existing = new Set();
      el.querySelectorAll('.bubble[data-mid]').forEach(function (n) {
        existing.add(n.getAttribute('data-mid'));
      });
      if (el.querySelector('.st-empty')) el.innerHTML = '';
      stEnsureFoldBanner(el, fold);
      for (let i = startIdx; i < list.length; i++) {
        const m = list[i];
        const id = m.id || ('st-idx-' + i);
        if (existing.has(id)) continue;
        const isStream = id === mid;
        const div = buildBubbleEl({
          id: id,
          roleClass: 'st-bubble ' + roleClassOf(m),
          roleLabel: roleLabelOf(m),
          body: bodyOf(m),
          enter: !isStream,
          streaming: isStream,
          extraClass: extraClassOf(m),
          program: m.program || null,
          emotionEmoji: stEmotionEmojiOf(m),
          swipeSupport: m.role !== 'user',
          swipeCount: (m._swipes && m._swipes.length) || 1,
          swipeIdx: (typeof m._swipeIdx === 'number') ? m._swipeIdx : 0,
          ts: m.createdAt || m.ts || '',
          tokens: (m.tokens || (m.usage && m.usage.total_tokens)) || 0,
          monologue: (m.role !== 'user' && (m._monologue || m.reasoning)) ? (m._monologue || m.reasoning) : null,
        });
        el.appendChild(div);
      }
      node = el.querySelector('.bubble[data-mid="' + cssEscape(mid) + '"]');
    }
    if (node) {
      const body = node.querySelector('.bubble-body');
      const text = bodyOf(last);
      if (body) {
        // Typewriter: only append new chars in stream mode instead of full replace
        if (opts.stream && body.getAttribute('data-stream-base') && !body.hasAttribute('data-stream-final')) {
          const cur = body.textContent || '';
          // Only append if text is longer (growing)
          if (text.length > cur.length) {
            const diff = text.slice(cur.length);
            // Append as text node to preserve existing node structure
            body.appendChild(document.createTextNode(diff));
          } else if (text.length < cur.length) {
            // Text shrank (mid-stream correction) — replace fully
            body.textContent = text;
          }
        } else {
          fillBubbleBody(body, text);
          if (opts.stream) body.setAttribute('data-stream-base', '1');
        }
      } else {
        const span = document.createElement('span');
        span.className = 'bubble-body';
        fillBubbleBody(span, text);
        if (opts.stream) span.setAttribute('data-stream-base', '1');
        node.appendChild(span);
      }
      node.classList.add('is-streaming');
      // S8.31: 流式期间不自动跟随滚动——视口停在开头（用户从开头下滑阅读）；
      // 用户手动滚动不受影响。移除原 nearBottom 跟随。
      // P2-1 立绘层：流式每帧同步当前焦点角色立绘（无情绪/无立绘静默降级）
      try { stRenderSprite(); } catch (_) {}
      return;
    }
  }

  const stick = el.scrollHeight - el.scrollTop - el.clientHeight < 96;
  const prevTop = el.scrollTop;
  el.innerHTML = '';
  stEnsureFoldBanner(el, fold);
  for (let i = startIdx; i < list.length; i++) {
    const m = list[i];
    const id = m.id || ('st-idx-' + i);
    const isLastStream = !!(opts.stream && i === list.length - 1 && m.role !== 'user');
    el.appendChild(buildBubbleEl({
      id: id,
      roleClass: 'st-bubble ' + roleClassOf(m),
      roleLabel: roleLabelOf(m),
      body: bodyOf(m),
      enter: !isLastStream && !opts.quiet,
      streaming: isLastStream,
      extraClass: extraClassOf(m),
      program: m.program || null,
      emotionEmoji: stEmotionEmojiOf(m),
      swipeSupport: m.role !== 'user',
      swipeCount: (m._swipes && m._swipes.length) || 1,
      swipeIdx: (typeof m._swipeIdx === 'number') ? m._swipeIdx : 0,
      ts: m.createdAt || m.ts || '',
      tokens: (m.tokens || (m.usage && m.usage.total_tokens)) || 0,
      monologue: (m.role !== 'user' && (m._monologue || m.reasoning)) ? (m._monologue || m.reasoning) : null,
    }));
  }
  if (opts.restoreScroll) {
    window.__stProgrammaticScroll = true;
    stRestoreReadPos(el);
    window.setTimeout(function () { window.__stProgrammaticScroll = false; }, 50);
  } else if (stick || opts.forceScroll) {
    window.__stProgrammaticScroll = true;
    el.scrollTop = el.scrollHeight;
    window.setTimeout(function () { window.__stProgrammaticScroll = false; }, 50);
  } else {
    el.scrollTop = prevTop;
  }
  // Programmatic scrolls (forceScroll/stick/restoreScroll) must NOT hide the
  // chrome: hiding on load leaves no input box and stray taps hit option
  // buttons. Only user-initiated scroll events hide the chrome.
  if (!opts.forceScroll && !opts.stream && !opts.restoreScroll) {
    try { stSyncImmChromeFromScroll(); } catch (_) {}
  }
  try { stBindReadPosSaver(el); } catch (_) {}
  // P2-1 立绘层：消息区重渲染后同步当前焦点角色立绘（无立绘/无情绪静默降级）
  try { stRenderSprite(); } catch (_) {}
}

function stMessageFoldPlan(list) {
  const turnStarts = [];
  for (let i = 0; i < list.length; i++) {
    if (i === 0 || (list[i] && list[i].role === 'user')) turnStarts.push(i);
  }
  const n = turnStarts.length;
  if (n <= ST_VISIBLE_TURNS) {
    return { foldUntil: 0, hiddenTurns: 0, hiddenMsgs: 0, totalTurns: n };
  }
  const foldUntil = turnStarts[n - ST_VISIBLE_TURNS];
  return {
    foldUntil: foldUntil,
    hiddenTurns: n - ST_VISIBLE_TURNS,
    hiddenMsgs: foldUntil,
    totalTurns: n,
  };
}

function stEnsureFoldBanner(el, fold) {
  if (!el || !fold) return;
  const existing = el.querySelector('.st-history-fold');
  if (stHistoryExpanded) {
    if (fold.hiddenTurns > 0) {
      // show collapse control at top
      const bar = existing || document.createElement('button');
      bar.type = 'button';
      bar.className = 'st-history-fold st-history-fold-collapse';
      bar.textContent = '收起较早对话（只留最近 ' + ST_VISIBLE_TURNS + ' 轮）';
      bar.onclick = function (e) {
        e.preventDefault();
        setStHistoryExpanded(false);
        stRenderMessages({ restoreScroll: false, forceScroll: false, quiet: true });
        // after collapse, jump to bottom of visible window
        const box = $('st-messages');
        if (box) box.scrollTop = box.scrollHeight;
        try { stSyncImmChromeFromScroll(true); } catch (_) {}
      };
      if (!existing) el.insertBefore(bar, el.firstChild);
    } else if (existing) {
      existing.remove();
    }
    return;
  }
  if (fold.foldUntil <= 0) {
    if (existing) existing.remove();
    return;
  }
  const bar = existing || document.createElement('button');
  bar.type = 'button';
  bar.className = 'st-history-fold';
  bar.textContent = '较早对话已折叠 · ' + fold.hiddenTurns + ' 轮 / ' + fold.hiddenMsgs + ' 条 · 点击展开';
  bar.onclick = function (e) {
    e.preventDefault();
    setStHistoryExpanded(true);
    const box = $('st-messages');
    const keep = box ? box.scrollHeight - box.scrollTop : 0;
    stRenderMessages({ quiet: true });
    // keep viewport anchored near where user was (bottom of previous visible set)
    if (box) {
      box.scrollTop = Math.max(0, box.scrollHeight - keep);
    }
    try { stSyncImmChromeFromScroll(true); } catch (_) {}
  };
  if (!existing) el.insertBefore(bar, el.firstChild);
  else if (bar.parentElement !== el) el.insertBefore(bar, el.firstChild);
}

function stReadPosKey(sid) {
  return ST_READPOS_PREFIX + String(sid || '');
}

function stSaveReadPos() {
  try {
    if (!tavernSession || !tavernSession.sessionId) return;
    const el = $('st-messages');
    if (!el) return;
    const max = Math.max(0, el.scrollHeight - el.clientHeight);
    const gap = el.scrollHeight - el.scrollTop - el.clientHeight;
    const nearBot = gap <= 96;
    const payload = {
      top: Math.round(el.scrollTop),
      ratio: max > 0 ? el.scrollTop / max : 1,
      nearBot: nearBot,
      at: Date.now(),
    };
    localStorage.setItem(stReadPosKey(tavernSession.sessionId), JSON.stringify(payload));
  } catch (_) {}
}

function stRestoreReadPos(el) {
  el = el || $('st-messages');
  if (!el || !tavernSession || !tavernSession.sessionId) return;
  let raw = null;
  try { raw = localStorage.getItem(stReadPosKey(tavernSession.sessionId)); } catch (_) {}
  const apply = function () {
    try {
      if (!raw) {
        // no history → last-read default = end (not top)
        el.scrollTop = el.scrollHeight;
      } else {
        const o = JSON.parse(raw);
        const max = Math.max(0, el.scrollHeight - el.clientHeight);
        if (o && o.nearBot) {
          el.scrollTop = el.scrollHeight;
        } else if (o && typeof o.ratio === 'number' && max > 0) {
          el.scrollTop = Math.min(max, Math.max(0, o.ratio * max));
        } else if (o && typeof o.top === 'number') {
          el.scrollTop = Math.min(max, Math.max(0, o.top));
        } else {
          el.scrollTop = el.scrollHeight;
        }
      }
    } catch (_) {
      el.scrollTop = el.scrollHeight;
    }
    // Do NOT hide chrome after restoring position: that leaves no input box
    // and stray taps hit option buttons. Show chrome so the composer is
    // reachable on entry; scroll-hide only kicks in on user scroll.
    try { stShowImmChrome(); } catch (_) {}
  };
  // layout after fold/render
  requestAnimationFrame(function () { requestAnimationFrame(apply); });
}

function stBindReadPosSaver(el) {
  if (!el || el._stReadPosBound) return;
  let t = 0;
  // Mark user-initiated scrolls (touch drag / wheel / keyboard). Programmatic
  // scrolls (restore/forceScroll/render jump) do NOT mark → chrome stays
  // visible so the input box and options are reachable on entry.
  const markUser = function () { window.__stUserScrolling = true; };
  el.addEventListener('pointerdown', markUser, { passive: true });
  el.addEventListener('touchstart', markUser, { passive: true });
  el.addEventListener('wheel', markUser, { passive: true });
  el.addEventListener('keydown', markUser, { passive: true });
  el.addEventListener('scroll', function () {
    // S8.28: live scroll drives top-bar hide/show — only for user scrolls.
    if (window.__stUserScrolling) {
      try { stSyncImmChromeFromScroll(); } catch (_) {}
      window.__stUserScrolling = false;
    }
    // S8.31: 记录用户真实滚动（程序化滚动有 __stProgrammaticScroll 标志）
    if (!window.__stProgrammaticScroll) {
      stTavernUserScrolled = true;
    }
    if (t) return;
    t = window.setTimeout(function () {
      t = 0;
      stSaveReadPos();
    }, 180);
  }, { passive: true });
  el._stReadPosBound = true;
}

let stStreamRaf = 0;

function scheduleStStreamPaint() {
  if (stStreamRaf) return;
  stStreamRaf = requestAnimationFrame(function () {
    stStreamRaf = 0;
    if (!tavernStreaming) return; // S8.11e: drop late frames after stop/finally
    stRenderMessages({ stream: true });
    // S8.31: 生成中视口保持在文本开头（用户未手动滚动时）；内容增长后校正
    if (!stTavernUserScrolled) {
      try { stScrollToLastMsgTop(); } catch (_) {}
    }
    // live-extract chips once 【选项】 appears in the streaming tail
    try {
      const msgs = (tavernSession && tavernSession.messages) || [];
      const last = msgs.length ? msgs[msgs.length - 1] : null;
      if (last && last.role !== 'user') {
        const live = resolveMessageOptions(last);
        if (live.length) stRenderOptions(live);
      }
    } catch (_) {}
  });
}

function clearStStreamPaint() {
  if (stStreamRaf) {
    try { cancelAnimationFrame(stStreamRaf); } catch (_) {}
    stStreamRaf = 0;
  }
  const stEl = $('st-messages');
  if (stEl) stEl.querySelectorAll('.bubble.is-streaming').forEach(function (n) { n.classList.remove('is-streaming'); });
  document.documentElement.removeAttribute('data-streaming');
}

function stRenderOptions(opts) {
  const el = $('st-options');
  if (!el) return;
  el.innerHTML = '';
  let source = Array.isArray(opts) ? opts : null;
  if (!source || !source.length) {
    const msgs = (tavernSession && tavernSession.messages) || [];
    let last = null;
    for (let i = msgs.length - 1; i >= 0; i--) {
      if (msgs[i] && msgs[i].role !== 'user') { last = msgs[i]; break; }
    }
    source = resolveMessageOptions(last);
  }
  source = (source || []).map(String).map((s) => s.trim()).filter(Boolean);
  if (!source.length) {
    el.classList.add('is-empty');
    return;
  }
  el.classList.remove('is-empty');
  // ensure play view options row visible
  el.hidden = false;
  el.style.display = '';
  for (const text of source) {
    const chip = document.createElement('button');
    chip.type = 'button';
    chip.className = 'st-option-chip';
    chip.textContent = text;
    chip.onclick = () => {
      if ($('st-input')) $('st-input').value = text;
      stSend(text);
    };
    el.appendChild(chip);
  }
  // S8.23: options can grow dock — keep last lines above it
  try { stKeepImmTailVisible(); } catch (_) {}
}

let stActivePanelName = '';

function stRenderPanels(target) {
  if (!target) {
    const p = $('st-panels');
    if (p) p.innerHTML = '';
    return;
  }
  const el = target;
  if (!el) return;
  el.innerHTML = '';
  const panels = (tavernSession && Array.isArray(tavernSession.panels)) ? tavernSession.panels : [];
  if (!panels.length) {
    el.innerHTML = (target && target.id === 'st-visual-body')
      ? '<div class="st-panels-empty">还没有可视化面板。点「让助手生成可视化」，或在剧情中让模型输出【面板】块。</div>'
      : '<div class="st-panels-empty">AI 可在剧情中生成可视化面板（地图/装备栏/线索板），出现后自动显示于此</div>';
    return;
  }
  if (!stActivePanelName || !panels.some(p => p.name === stActivePanelName)) {
    stActivePanelName = panels[0].name;
  }
  const tabs = document.createElement('div');
  tabs.className = 'st-panels-tabs';
  for (const p of panels) {
    const t = document.createElement('button');
    t.type = 'button';
    t.className = 'st-panel-tab' + (p.name === stActivePanelName ? ' active' : '');
    t.textContent = p.name;
    t.onclick = () => { stActivePanelName = p.name; stRenderPanels(target); };
    tabs.appendChild(t);
  }
  el.appendChild(tabs);
  const cur = panels.find(p => p.name === stActivePanelName) || panels[0];
  const body = document.createElement('div');
  body.className = 'st-panel-body';
  if (cur.kind === 'eventbook') {
    // 事件书（Omate 对齐）：剧情链状态追踪——解锁/完成/条件
    const wrap = document.createElement('div');
    wrap.className = 'st-eventbook';
    let events = [];
    try {
      const parsed = JSON.parse(cur.content);
      events = Array.isArray(parsed) ? parsed : (Array.isArray(parsed.events) ? parsed.events : []);
    } catch (_) {
      // 非 JSON 降级：按行渲染为纯文本事件列表
      events = String(cur.content).split('\n').filter(function (l) { return l.trim(); }).map(function (l) {
        return { title: l.replace(/^[-*]\s*/, '').replace(/^\[[ xX]\]\s*/, '').trim(), done: /^\[[xX]\]/.test(l.trim()) };
      });
    }
    if (!events.length) {
      wrap.innerHTML = '<div class="st-panels-empty">事件书为空——让助手生成事件链，或剧情中输出【事件书】块。</div>';
    } else {
      const list = document.createElement('ol');
      list.className = 'st-eventbook-list';
      for (const ev of events) {
        const li = document.createElement('li');
        li.className = 'st-eventbook-item' + (ev.done ? ' done' : '');
        const mark = document.createElement('span');
        mark.className = 'st-eventbook-mark';
        mark.textContent = ev.done ? '✓' : '○';
        const info = document.createElement('div');
        info.className = 'st-eventbook-info';
        const title = document.createElement('div');
        title.className = 'st-eventbook-title';
        title.textContent = ev.title || '（未命名事件）';
        info.appendChild(title);
        if (ev.desc) {
          const desc = document.createElement('div');
          desc.className = 'st-eventbook-desc';
          desc.textContent = ev.desc;
          info.appendChild(desc);
        }
        if (ev.cond && !ev.done) {
          const cond = document.createElement('div');
          cond.className = 'st-eventbook-cond';
          cond.textContent = '条件：' + ev.cond;
          info.appendChild(cond);
        }
        li.appendChild(mark); li.appendChild(info);
        list.appendChild(li);
      }
      wrap.appendChild(list);
    }
    body.appendChild(wrap);
  } else if (cur.kind === 'svg') {
    const wrap = document.createElement('div');
    wrap.className = 'st-panel-svg';
    // 清洗加固 (audit P1#8)：双/单引号 on* 处理器 + xlink:href + javascript: href 全剥
    const safe = String(cur.content)
      .replace(/<script[\s\S]*?<\/script>/gi, '')
      .replace(/\son\w+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]*)/gi, '')
      .replace(/\sxlink:href\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]*)/gi, '')
      .replace(/\shref\s*=\s*"javascript:[^"]*"/gi, '')
      .replace(/\shref\s*=\s*'javascript:[^']*'/gi, '');
    wrap.innerHTML = safe;
    body.appendChild(wrap);
  } else if (cur.kind === 'html') {
    const frame = document.createElement('iframe');
    // audit P0#1：去掉 allow-same-origin —— 同源会使沙箱内脚本读取父页 token/调用同源 API
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.setAttribute('title', cur.name);
    frame.srcdoc = String(cur.content);
    body.appendChild(frame);
  } else {
    body.textContent = cur.content;
  }
  el.appendChild(body);
}

function stOpenVisualModal() {
  const m = $('st-visual-modal');
  if (!m) return;
  m.classList.remove('hidden');
  stRenderPanels($('st-visual-body'));
}

