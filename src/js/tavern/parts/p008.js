function stRefreshImmerseBg() {
  const stage = document.getElementById('st-view-play');
  if (!stage) return;
  try {
    const url = stImmerseBgUrl();
    if (url) {
      stage.classList.add('has-char-bg');
      stage.style.setProperty('--char-bg-image', 'url("' + String(url).replace(/"/g, '\\"') + '")');
    } else {
      stage.classList.remove('has-char-bg');
      stage.style.removeProperty('--char-bg-image');
    }
  } catch (_) {
    stage.classList.remove('has-char-bg');
    stage.style.removeProperty('--char-bg-image');
  }
}

/* ================= initTavernUI: former top-level DOM/window wiring ================= */
export function initTavernUI() {
// ---- WIRE:_tavern-core.js ----
try {
  Object.defineProperty(window, 'stNavFrom', {
    get: () => stNavFrom,
    set: (v) => { stNavFrom = v; },
    configurable: true,
  });
} catch (_) {}

// 弹层是否真的可见（未被宿主 tab 面板隐藏）
// ---- WIRE:_tavern-core.js ----
window.stGoBack = stGoBack;
// ---- WIRE:_tavern-core.js ----
window.addEventListener('popstate', () => {
  if (stGoBack(true)) {
    __setSuppressHashWrite(true);
    setTimeout(() => { __setSuppressHashWrite(false); }, 50);
  }
});
// ---- WIRE:_tavern-core.js ----
if (stStageBtn) stStageBtn.onclick = stStageOpen;
// ---- WIRE:_tavern-core.js ----
try { window.stStatus = stStatus; } catch (_) {}
// ---- WIRE:_tavern-session.js ----
if ($('st-recall-refresh')) {
  $('st-recall-refresh').onclick = (e) => {
    e.preventDefault();
    stRefreshRecallSemantic();
  };
}
// ---- WIRE:_tavern-session.js ----
try { if (window.stRefreshImmerseBg) stRefreshImmerseBg(); } catch (_) {}

// 剧情助手工具按钮：重生成 / 回退
// ---- WIRE:_tavern-session.js ----
if ($('st-assist-reroll')) $('st-assist-reroll').onclick = (e) => { e.preventDefault(); stRerollLast(); }
// ---- WIRE:_tavern-session.js ----
if ($('st-assist-rewind')) $('st-assist-rewind').onclick = (e) => { e.preventDefault(); stRewindOne(); }
// ---- WIRE:_tavern-session.js ----
try {
window.stOpenAssistModal = stOpenAssistModal;
window.stFocusAssistInput = stFocusAssistInput;
} catch (_) {}
// ---- WIRE:_tavern-send.js ----
function bindAdultConfirm() { const btn = $('st-adult-ok'); if (!btn || btn.dataset.bound === '1') return; btn.dataset.bound = '1'; btn.onclick = async () => { btn.disabled = true; try { await setAdultOk(); $('st-adult-banner').classList.add('hidden'); const l = $('st-layout'); if (l) l.classList.remove('st-gated'); await stRefresh(); } catch (e) { if (typeof showToast === 'function') showToast('确认失败：' + (e.message || e), 'error'); } finally { btn.disabled = false; } }; }
bindAdultConfirm();
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bindAdultConfirm); else setTimeout(bindAdultConfirm, 0);
// ---- WIRE:_tavern-send.js ----
if ($('st-new-session')) { $('st-new-session').onclick = () => stOpenWizard('P3', 'story-entry'); }
// ---- WIRE:_tavern-send.js ----
if ($('st-drawer-new-session')) { $('st-drawer-new-session').onclick = () => stOpenWizard('P3', 'story-entry'); }

// ─── Bookshelf ↔ Story Tavern bridge ───────────────────────────────────────
// ---- WIRE:_tavern-send.js ----
window.stSwipe = stSwipe;
// ---- WIRE:_tavern-send.js ----
window.stSwipePicker = stSwipePicker;
// ---- WIRE:_tavern-send.js ----
window.stEditMessage = stEditMessage;
// ---- WIRE:_tavern-send.js ----
window.stDeleteMessage = stDeleteMessage;
// ---- WIRE:_tavern-send.js ----
window.stPartialEdit = stPartialEdit;
// ---- WIRE:_tavern-send.js ----
window.stToggleBookmark = stToggleBookmark;
// ---- WIRE:_tavern-send.js ----
window.stRenderBookmarks = stRenderBookmarks;
// ---- WIRE:_tavern-send.js ----
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', stWireDisplaySettings);
} else {
  stWireDisplaySettings();
}
// ---- WIRE:_tavern-send.js ----
window.stApplyChatWidth = stApplyChatWidth;
// ---- WIRE:_tavern-send.js ----
window.stApplyBubbleStyle = stApplyBubbleStyle;
// ---- WIRE:_tavern-packmgmt.js ----
if ($('shelf-refresh')) $('shelf-refresh').onclick = () => loadBookshelf();
// ---- WIRE:_tavern-packmgmt.js ----
if ($('shelf-chat-publish')) $('shelf-chat-publish').onclick = () => shelfPublishChat();
// ---- WIRE:_tavern-packmgmt.js ----
if ($('shelf-sched-save')) $('shelf-sched-save').onclick = () => shelfSaveSchedule();
// ---- WIRE:_tavern-packmgmt.js ----
if ($('shelf-sched-run')) $('shelf-sched-run').onclick = () => shelfRunScheduleNow();
// ---- WIRE:_tavern-packmgmt.js ----
if ($('shelf-import-file')) {
  $('shelf-import-file').onchange = async (e) => {
    const file = e.target.files && e.target.files[0];
    if (!file) return;
    try {
      await shelfImportFile(file);
    } catch (err) {
      shelfStatus('导入失败：' + (err.message || err));
    } finally {
      e.target.value = '';
    }
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('reader-back')) $('reader-back').onclick = closeShelfReader;
// ---- WIRE:_tavern-packmgmt.js ----
if ($('reader-to-pack')) {
  $('reader-to-pack').onclick = () => {
    const n = shelfActiveNovel();
    if (!n) return;
    shelfPromoteToPack(n.slug, n.title);
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('reader-export')) {
  $('reader-export').onclick = () => {
    const n = shelfActiveNovel();
    if (!n) return;
    shelfExport(n.slug, n.title);
  };
}

// load when switching to bookshelf tab
// ---- WIRE:_tavern-packmgmt.js ----
document.querySelectorAll('.tab[data-tab="bookshelf"], [data-tab="bookshelf"]').forEach((btn) => {
  btn.addEventListener('click', () => { setTimeout(loadBookshelf, 0); });
});
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-pack-demo')) {
  $('st-pack-demo').onclick = async () => {
    try {
      const before = (tavernPacks || []).some((p) => p.id === 'demo-rain-alley');
      const pack = await stApi('/packs/demo', { method: 'POST' });
      await stLoadPacks();
      // jump to 故事馆 so user sees the pack
      if (typeof switchTab === 'function') switchTab('tavern');
      if (typeof closeToolsSheet === 'function') closeToolsSheet();
      const title = (pack && pack.title) || '雨巷来客';
      stStatus(before
        ? ('新手引导：演示包「' + title + '」已在库中，可直接开玩')
        : ('新手引导：已安装演示包「' + title + '」· 左侧 Pack 库可见'));
      // highlight / open pack detail if helper exists
      if (pack && pack.id && typeof stShowPack === 'function') {
        try { await stShowPack(pack.id); } catch (_) {}
      }
    } catch (e) {
      stStatus('新手引导失败：' + e.message);
    }
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('home-demo-card')) {
  $('home-demo-card').onclick = async () => {
    try {
      const before = (tavernPacks || []).some((p) => p.id === 'demo-rain-alley');
      const pack = await stApi('/packs/demo', { method: 'POST' });
      await stLoadPacks();
      if (typeof switchTab === 'function') switchTab('tavern');
      if (typeof closeToolsSheet === 'function') closeToolsSheet();
      const title = (pack && pack.title) || '雨巷来客';
      stStatus(before
        ? ('演示包「' + title + '」已在库中 · 选择玩法开玩')
        : ('已安装演示包「' + title + '」· 选择玩法开玩'));
      if (pack && pack.id && typeof stShowPack === 'function') {
        try { await stShowPack(pack.id); } catch (_) {}
      }
    } catch (e) {
      stStatus('一键开始失败：' + (typeof friendlyError === 'function' ? friendlyError(e) : e.message));
    }
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('home-demo-start')) {
  $('home-demo-start').onclick = async () => {
    try {
      const before = (tavernPacks || []).some((p) => p.id === 'demo-rain-alley');
      if (!before) {
        await stApi('/packs/demo', { method: 'POST' });
        await stLoadPacks();
      }
      if (typeof switchTab === 'function') switchTab('tavern');
      if (typeof closeToolsSheet === 'function') closeToolsSheet();
    } catch (e) {
      stStatus('开始失败：' + (typeof friendlyError === 'function' ? friendlyError(e) : e.message));
    }
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('home-continue-btn')) {
  $('home-continue-btn').addEventListener('click', () => {
    const has = (tavernSessions && tavernSessions.length) || 0;
    if (!has) {
      if (typeof switchTab === 'function') switchTab('chat');
    }
  });
}
// ---- WIRE:_tavern-packmgmt.js ----
document.querySelectorAll('#st-entry-cards .st-entry-card').forEach((card) => { card.onclick = () => stOpenWizard(card.dataset.playable, 'story-entry'); });
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-pack-detail-play')) {
  $('st-pack-detail-play').onclick = () => {
    if (!tavernPack || !tavernPack.id) { stStatus('请先选择 Pack'); return; }
    // 在档案馆内直接打开创建向导（wizard 已在档案馆 DOM 里）
    const listview = $('st-packs-listview');
    const packDetail = $('st-view-pack');
    if (listview) listview.classList.add('hidden');
    if (packDetail) packDetail.classList.add('hidden');
    stOpenWizard('P1', 'packs-detail');
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-pack-detail-back')) {
  $('st-pack-detail-back').onclick = () => {
    const listview = $('st-packs-listview');
    const packDetail = $('st-view-pack');
    if (listview) listview.classList.remove('hidden');
    if (packDetail) packDetail.classList.add('hidden');
    stStatus('档案馆 — 选择剧本包');
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-pack-detail-compass')) {
  $('st-pack-detail-compass').onclick = () => {
    if (typeof switchTab === 'function') switchTab('works');
    setTimeout(() => {
      if (typeof switchAzView === 'function') switchAzView('dual-agent');
      if (typeof showToast === 'function') showToast('世界线/罗盘工具在作者区双 Agent / 关系图中');
    }, 80);
  };
}
// ---- WIRE:_tavern-packmgmt.js ----
$('st-wizard-create').onclick = stCreateSession;
// ---- WIRE:_tavern-packmgmt.js ----
$('st-wizard-cancel').onclick = () => stCancelWizard(false);
// ---- WIRE:_tavern-packmgmt.js ----
$('st-wizard-role').onchange = stWizardToggleRole;
// ---- WIRE:_tavern-packmgmt.js ----
$('st-wizard-pack').onchange = stWizardToggleRole;
// ---- WIRE:_tavern-packmgmt.js ----
$('st-composer').onsubmit = (e) => { e.preventDefault(); stSend($('st-input').value); }
// ---- WIRE:_tavern-packmgmt.js ----
$('st-input').addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); stSend($('st-input').value); } });
// ---- WIRE:_tavern-packmgmt.js ----
$('st-stop').onclick = stStop;
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-wand-btn')) {
  $('st-wand-btn').onclick = (e) => {
    e.preventDefault();
    const menu = $('st-wand-menu');
    const btn = $('st-wand-btn');
    if (!menu) return;
    const open = menu.classList.toggle('hidden');
    btn.setAttribute('aria-expanded', open ? 'false' : 'true');
    if (!open) {
      // 菜单高度按视口自适应:底贴 composer 顶向上,不能超出屏幕顶,内容可滚动
      // 62 = 沉浸顶栏(~52px)+安全边距, 否则菜单顶部会被顶栏盖住导致hittest失败
      const comp = $('st-composer');
      if (comp) {
        const top = Math.round(comp.getBoundingClientRect().top);
        menu.style.maxHeight = Math.max(160, top - 62) + 'px';
      }
      // 默认滚到最底:让"操作区+生图通道"这些常用动作立即可见
      requestAnimationFrame(() => { menu.scrollTop = menu.scrollHeight; });
    }
  };
}
// 点按钮后收起;select 选择完成(change)后收起,避免下拉展开即收起
// ---- WIRE:_tavern-packmgmt.js ----
document.querySelectorAll('#st-wand-menu button').forEach((el) => {
  el.addEventListener('click', () => {
    if (el.id === 'st-vessel-toggle' || el.id === 'st-rot-toggle' || el.closest('#st-vessel-picker') || el.closest('.st-select-wrap')) return;
    const menu = $('st-wand-menu');
    const btn = $('st-wand-btn');
    if (menu && btn) { menu.classList.add('hidden'); btn.setAttribute('aria-expanded', 'false'); }
  });
});
// ---- WIRE:_tavern-packmgmt.js ----
document.querySelectorAll('#st-wand-menu select').forEach((el) => {
  el.addEventListener('change', () => {
    const menu = $('st-wand-menu');
    const btn = $('st-wand-btn');
    if (menu && btn) { menu.classList.add('hidden'); btn.setAttribute('aria-expanded', 'false'); }
  });
});
// ---- WIRE:_tavern-packmgmt.js ----
document.addEventListener('click', (e) => {
  const menu = $('st-wand-menu');
  const btn = $('st-wand-btn');
  if (!menu || menu.classList.contains('hidden')) return;
  if (btn && btn.contains(e.target)) return;
  if (menu.contains(e.target)) return;
  menu.classList.add('hidden');
  if (btn) btn.setAttribute('aria-expanded', 'false');
});
// ---- WIRE:_tavern-packmgmt.js ----
['st-magic-assist', 'st-magic-assist-btn'].forEach((mid) => {
  const btn = document.getElementById(mid);
  if (!btn) return;
  btn.onclick = (e) => {
    e.preventDefault();
    if (typeof stOpenAssistModal === 'function') stOpenAssistModal();
  };
});
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-visual-btn')) $('st-visual-btn').onclick = (e) => { e.preventDefault(); stOpenVisualModal(); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-visual-close')) $('st-visual-close').onclick = (e) => { e.preventDefault(); stCloseVisualModal(); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-visual-gen')) $('st-visual-gen').onclick = (e) => { e.preventDefault(); stGenVisual(); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-assist-close')) $('st-assist-close').onclick = (e) => { e.preventDefault(); stCloseAssistModal(); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-assist-send')) $('st-assist-send').onclick = (e) => { e.preventDefault(); stSendAssist(); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-assist-input')) $('st-assist-input').addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); stSendAssist(); } });
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-continue')) $('st-continue').onclick = (e) => { e.preventDefault(); stContinue(); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-retry')) $('st-retry').onclick = (e) => { e.preventDefault(); stRetry().catch((err) => stStatus('重试失败：' + ((err && err.message) || err))); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-image-btn')) $('st-image-btn').onclick = (e) => { e.preventDefault(); stGenerateImage().catch((err) => stStatus('生图失败：' + ((err && err.message) || err))); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-sprite-btn')) $('st-sprite-btn').onclick = (e) => { e.preventDefault(); stGenerateSprite().catch((err) => stStatus('生成立绘失败：' + ((err && err.message) || err))); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-tts-btn')) $('st-tts-btn').onclick = (e) => { e.preventDefault(); stSpeak().catch((err) => stStatus('朗读失败：' + ((err && err.message) || err))); }
// ---- WIRE:_tavern-packmgmt.js ----
if (stAsrBtn) stAsrBtn.onclick = (e) => { e.preventDefault(); stToggleRecording().catch(() => {}); stSyncRecBtn(); }
// ---- WIRE:_tavern-packmgmt.js ----
if (stAsrToggle) {
  const syncAsrToggle = () => {
    const on = localStorage.getItem('stVoiceInput') === '1';
    stAsrToggle.dataset.on = on ? '1' : '0';
    stAsrToggle.classList.toggle('is-on', on);
    stAsrToggle.title = on ? '语音双工：开（点击关闭）' : '语音双工：关（点击开启）';
  };
  stAsrToggle.onclick = (e) => {
    e.preventDefault();
    const on = localStorage.getItem('stVoiceInput') === '1';
    localStorage.setItem('stVoiceInput', on ? '0' : '1');
    syncAsrToggle();
    if (typeof stSyncRecBtn === 'function') stSyncRecBtn();
    stStatus(on ? '🎙 语音双工已关闭' : '🎙 语音双工已开启（说话即发送，回合尾自动朗读）');
  };
  syncAsrToggle();
  if (typeof stSyncRecBtn === 'function') stSyncRecBtn();
}
// P3 自动朗读开关：点击切换 + 初始化读取 localStorage
// ---- WIRE:_tavern-packmgmt.js ----
if (stAutoBtn) {
  const syncAutoBtn = () => {
    const on = localStorage.getItem('stAutoTts') === '1';
    stAutoBtn.dataset.on = on ? '1' : '0';
    stAutoBtn.classList.toggle('is-on', on);
    stAutoBtn.title = on ? '自动朗读：开（点击关闭）' : '自动朗读：关（点击开启）';
  };
  stAutoBtn.onclick = (e) => {
    e.preventDefault();
    const on = localStorage.getItem('stAutoTts') === '1';
    localStorage.setItem('stAutoTts', on ? '0' : '1');
    syncAutoBtn();
    stStatus(on ? '自动朗读已关闭' : '🔊 自动朗读已开启（回合结束自动朗读）');
  };
  syncAutoBtn();
}
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-tts-pause')) $('st-tts-pause').onclick = (e) => { e.preventDefault(); stTtsPauseToggle(); }
// ---- WIRE:_tavern-packmgmt.js ----
if ($('st-tts-stop')) $('st-tts-stop').onclick = (e) => { e.preventDefault(); stTtsStop(); }
// ---- WIRE:_tavern-packmgmt.js ----
if (stImgSelBtn) stImgSelBtn.onclick = () => {
  const list = $('st-image-channel-list');
  if (!list) return;
  const opening = list.classList.contains('hidden');
  list.classList.toggle('hidden', !opening);
  stImgSelBtn.setAttribute('aria-expanded', opening ? 'true' : 'false');
  if (opening) {
    list.querySelectorAll('.st-select-opt').forEach((o) =>
      o.classList.toggle('active', o.dataset.channel === stImgSelBtn.dataset.value));
    // 弹窗脱离菜单 overflow 裁剪:fixed 定位到视口,优先向下,空间不足向上,限制高度可滚
    const wr = stImgSelBtn.closest('.st-select-wrap') || stImgSelBtn;
    const r = wr.getBoundingClientRect();
    const listH = list.offsetHeight || 160;
    let top = Math.round(r.bottom + 4);
    if (top + listH > innerHeight - 8) top = Math.max(8, Math.round(r.top - listH - 4));
    list.style.position = 'fixed';
    list.style.left = Math.round(r.left) + 'px';
    list.style.top = top + 'px';
    list.style.right = 'auto';
    list.style.width = Math.round(r.width) + 'px';
    list.style.maxHeight = Math.max(120, innerHeight - top - 8) + 'px';
    list.style.overflowY = 'auto';
    list.style.zIndex = '150';
  } else {
    list.style.position = '';
    list.style.left = '';
    list.style.top = '';
    list.style.right = '';
    list.style.width = '';
    list.style.maxHeight = '';
    list.style.overflowY = '';
    list.style.zIndex = '';
  }
}
// ---- WIRE:_tavern-packmgmt.js ----
stImgSelOpts.forEach((o) => {
  o.onclick = (e) => { e.preventDefault(); e.stopPropagation();
    const btn = $('st-image-channel');
    if (btn) { btn.dataset.value = o.dataset.channel; btn.textContent = o.textContent; btn.setAttribute('aria-expanded', 'false'); }
    const list = $('st-image-channel-list');
    if (list) list.classList.add('hidden');
    list.querySelectorAll('.st-select-opt').forEach((x) => x.classList.toggle('active', x === o));
  };
});
// ---- WIRE:_tavern-packmgmt.js ----
if (stQualitySelBtn) {
  const stQualityList = $('st-writer-quality-list');
  const stQualityOpts = stQualityList ? stQualityList.querySelectorAll('.st-select-opt') : [];
  const updateQuality = () => {
    const titles = { lite: '轻量', standard: '标准', heavy: '深度' };
    const v = stQualitySelBtn.dataset.value;
    try { localStorage.setItem('st-writer-quality', v || 'lite'); } catch (_) {}
    if (stQualitySelBtn.querySelector('.btn-lab')) stQualitySelBtn.querySelector('.btn-lab').textContent = titles[v] || '轻量';
  };
  stQualitySelBtn.addEventListener('click', () => {
    if (!stQualityList) return;
    const open = !stQualityList.classList.contains('hidden');
    stQualityList.classList.toggle('hidden', open);
    stQualitySelBtn.setAttribute('aria-expanded', String(!open));
  });
  stQualityOpts.forEach(o => {
    o.addEventListener('click', () => {
      stQualitySelBtn.dataset.value = o.dataset.quality;
      updateQuality();
      if (stQualityList) stQualityList.classList.add('hidden');
      stQualitySelBtn.setAttribute('aria-expanded', 'false');
      stStatus('档位：' + o.textContent.trim());
    });
  });
  try {
    const saved = localStorage.getItem('st-writer-quality');
    if (saved) { stQualitySelBtn.dataset.value = saved; updateQuality(); }
  } catch (_) {}
}

// 内容档位（st-content-tier）：会话级 contentTier 中段切换
// 后端：POST /api/v1/story-tavern/sessions/{id}/tier（显式端点，放宽需 adultConfirmed）
// ---- WIRE:_tavern-packmgmt.js ----
if (stTierSelBtn) {
  const stTierList = $('st-content-tier-list');
  const stTierOpts = stTierList ? stTierList.querySelectorAll('.st-select-opt') : [];
  const TIER_TITLES = { safe: '安全', standard: '标准', open: '开放' };
  const updateTierBtn = () => {
    const v = stTierSelBtn.dataset.value || 'standard';
    const lab = stTierSelBtn.querySelector('.btn-lab');
    if (lab) lab.textContent = TIER_TITLES[v] || '标准';
  };
  const syncTierFromSession = () => {
    if (!tavernSession) return;
    const cur = (tavernSession.contentTier || '').toLowerCase();
    if (TIER_TITLES[cur]) {
      stTierSelBtn.dataset.value = cur;
      updateTierBtn();
    }
  };
  // 供 stLoadSession 跨文件调用：进入会话时刷新按钮档位
  window.stSyncTierFromSession = syncTierFromSession;
  stTierSelBtn.addEventListener('click', () => {
    if (!stTierList) return;
    const open = !stTierList.classList.contains('hidden');
    stTierList.classList.toggle('hidden', open);
    stTierSelBtn.setAttribute('aria-expanded', String(!open));
  });
  stTierOpts.forEach(o => {
    o.addEventListener('click', async () => {
      const want = o.dataset.tier;
      stTierSelBtn.dataset.value = want;
      updateTierBtn();
      if (stTierList) stTierList.classList.add('hidden');
      stTierSelBtn.setAttribute('aria-expanded', 'false');
      // 需要当前会话
      if (!tavernSession || !tavernSession.sessionId) {
        stStatus('内容档位：请先进入一个会话');
        return;
      }
      const sid = tavernSession.sessionId;
      // 放宽到 open 需要成年确认；未确认时先走确认
      if (want === 'open' && !adultOk()) {
        const ok = window.confirm('「开放」档包含成人内容。\n确认你已成年并自担风险？');
        if (!ok) {
          // 还原为会话当前档位
          syncTierFromSession();
          stStatus('已取消：未确认成年，内容档位保持不变');
          return;
        }
        await setAdultOk();
      }
      try {
        const r = await stApi('/sessions/' + encodeURIComponent(sid) + '/tier', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ contentTier: want, adultConfirmed: !!adultOk() }),
        });
        if (r && r.sessionId) tavernSession = r;
        syncTierFromSession();
        stStatus('内容档位已切换为「' + TIER_TITLES[stTierSelBtn.dataset.value] + '」');
      } catch (e) {
        stTierSelBtn.dataset.value = (tavernSession && tavernSession.contentTier) || 'standard';
        updateTierBtn();
        stStatus('内容档位切换失败：' + (e && e.message ? e.message : '未知错误'));
      }
    });
  });
  syncTierFromSession();
}

// 叙述视角（st-narr-pov）：会话级 /config pov=first|third 覆盖蒸馏文风人称
// 后端：POST /sessions/{id}/assistant {message:"/config pov=first"} → flags 持久化 → 注入 system prompt
// ---- WIRE:_tavern-packmgmt.js ----
if (stPovBtn) {
  const stPovList = $('st-narr-pov-list');
  const stPovOpts = stPovList ? stPovList.querySelectorAll('.st-select-opt') : [];
  const POV_TITLES = { '': '默认（跟随作品）', first: '第一人称（我）', third: '第三人称（他/她）' };
  const POV_LABELS = { '': '默认', first: '第一人称', third: '第三人称' };
  const updatePovBtn = () => {
    const v = stPovBtn.dataset.value || '';
    const lab = stPovBtn.querySelector('.btn-lab');
    if (lab) lab.textContent = POV_LABELS[v] || '默认';
  };
  stPovBtn.addEventListener('click', () => {
    if (!stPovList) return;
    const open = !stPovList.classList.contains('hidden');
    stPovList.classList.toggle('hidden', open);
    stPovBtn.setAttribute('aria-expanded', String(!open));
  });
  stPovOpts.forEach(o => {
    o.addEventListener('click', async () => {
      const want = o.dataset.pov || '';
      if (!tavernSession || !tavernSession.sessionId) {
        stStatus('叙述视角：请先进入一个会话');
        return;
      }
      const sid = tavernSession.sessionId;
      stPovBtn.dataset.value = want;
      updatePovBtn();
      if (stPovList) stPovList.classList.add('hidden');
      stPovBtn.setAttribute('aria-expanded', 'false');
      try {
        const cmd = want ? ('/config pov=' + want) : '/config pov=default';
        const r = await stApi('/sessions/' + encodeURIComponent(sid) + '/assistant', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ message: cmd }),
        });
        stStatus('叙述视角已设为「' + POV_LABELS[want] + '」');
      } catch (e) {
        stPovBtn.dataset.value = '';
        updatePovBtn();
        stStatus('叙述视角设置失败：' + (e && e.message ? e.message : '未知错误'));
      }
    });
  });
  updatePovBtn();
}

// S8.17: bind imm chrome listeners early (safe if elements missing)
// ---- WIRE:_tavern-packmgmt.js ----
try { stBindImmChrome(); } catch (_) {}


// ----- ST-4 Pack production / novel import helpers -----
// ---- WIRE:_tavern-lore.js ----
if ($('st-novel-file')) {
  $('st-novel-file').onchange = async (e) => {
    const file = e.target.files[0]; if (!file) return;
    const progress = $('st-import-progress');
    progress.classList.remove('hidden');
    try {
      const saved = await stImportNovel(file, file.name.replace(/\.[^.]+$/, ''));
      await stLoadPacks();
      stStatus('已导入 Pack：' + saved.title + '，共' + (saved.chapters || []).length + '章');
      e.target.value = '';
    } catch (err) {
      stStatus('导入失败：' + err.message);
    } finally {
      progress.classList.add('hidden');
    }
  };
}
// ---- WIRE:_tavern-lore.js ----
if ($('st-pack-create')) $('st-pack-create').onclick = () => $('st-pack-editor').classList.remove('hidden');
// ---- WIRE:_tavern-lore.js ----
if ($('st-side-expand-all')) {
  $('st-side-expand-all').onclick = () => {
    const heads = document.querySelectorAll('#st-pack-list .st-pack-group-head, #st-session-list .st-session-group-head');
    const anyClosed = Array.from(heads).some(h => !h.classList.contains('open'));
    heads.forEach((h) => {
      const open = anyClosed; // expand all if any is closed, else collapse all
      h.classList.toggle('open', open);
      const g = h.parentElement;
      if (g) g.classList.toggle('open', open);
      h.setAttribute('aria-expanded', open ? 'true' : 'false');
    });
    const btn = $('st-side-expand-all');
    if (btn) btn.textContent = anyClosed ? '全部收起' : '全部展开';
  };
}
// ---- WIRE:_tavern-lore.js ----
if ($('st-pack-cancel')) $('st-pack-cancel').onclick = () => $('st-pack-editor').classList.add('hidden');
// ---- WIRE:_tavern-lore.js ----
if ($('st-pack-save')) {
  $('st-pack-save').onclick = async () => {
    await stCreateEmptyPack();
    $('st-pack-editor').classList.add('hidden');
    $('st-pack-title').value = '';
    await stLoadPacks();
    stStatus('空 Pack 创建成功');
  };
}
// ---- WIRE:_tavern-lore.js ----
if ($('st-chapter-edit')) {
  $('st-chapter-edit').onclick = () => {
    if (!tavernPack || !tavernPack.chapters || !tavernPack.chapters[0]) return;
    // toggle an inline editor
    const view = $('st-chapter-view');
    const pre = view.querySelector('pre');
    const editing = view.dataset.editing === '1';
    view.dataset.editing = editing ? '' : '1';
    if (editing) {
      // save
      const chId = view.dataset.chapterId;
      const content = (view.querySelector('textarea')?.value || '').trim();
      stApi('/packs/' + encodeURIComponent(tavernPack.id) + '/chapters/' + encodeURIComponent('chapters/' + chId + '.md'), { method: 'PUT', body: JSON.stringify({ content }) })
        .then(() => { stShowChapter(tavernPack.id, chId); stStatus('章节已保存'); })
        .catch(err => stStatus('保存失败：' + err.message));
    } else {
      // turn pre into textarea
      const txt = document.createElement('textarea');
      txt.rows = 10; txt.style.flex = '1';
      const oldText = pre.textContent; pre.innerHTML = ''; pre.appendChild(txt); txt.value = oldText;
    }
  };
}
// ---- WIRE:_tavern-side.js ----
if ($('st-lore-add')) $('st-lore-add').onclick = () => stOpenLoreEditor(-1);
// ---- WIRE:_tavern-side.js ----
if ($('st-lore-save')) $('st-lore-save').onclick = () => stSaveLore().catch(e => stStatus('Lore 保存失败：' + e.message));
// ---- WIRE:_tavern-side.js ----
if ($('st-lore-cancel')) $('st-lore-cancel').onclick = () => { $('st-lore-editor').classList.add('hidden'); stLoreEditIdx = -1; stRenderLore(); }
// ---- WIRE:_tavern-side.js ----
if ($('st-lore-del')) $('st-lore-del').onclick = () => stDeleteLore().catch(e => stStatus('Lore 删除失败：' + e.message));
// ---- WIRE:_tavern-side.js ----
if ($('st-mode-mainline')) $('st-mode-mainline').onclick = () => stSetPlayMode('mainline');
// ---- WIRE:_tavern-side.js ----
if ($('st-mode-side')) $('st-mode-side').onclick = () => stSetPlayMode('side');
// ---- WIRE:_tavern-side.js ----
if ($('st-mode-free')) $('st-mode-free').onclick = () => stSetPlayMode('free');
// ---- WIRE:_tavern-side.js ----
if ($('st-side-panel-close')) $('st-side-panel-close').onclick = () => stCloseSidePanel();
// ---- WIRE:_tavern-side.js ----
if ($('st-save-create')) $('st-save-create').onclick = () => stCreateSave();
// ---- WIRE:_tavern-side.js ----
if ($('st-drawer-save-create')) $('st-drawer-save-create').onclick = () => stCreateSave();
if ($('st-share-create')) $('st-share-create').onclick = () => stCreateShare();
if ($('st-share-revoke')) $('st-share-revoke').onclick = () => stRevokeShare();
if ($('st-drawer-share-create')) $('st-drawer-share-create').onclick = () => stCreateShare();
if ($('st-drawer-share-revoke')) $('st-drawer-share-revoke').onclick = () => stRevokeShare();
// ---- WIRE:_tavern-side.js ----
if ($('st-pack-export')) $('st-pack-export').onclick = () => stExportPackZip();
// ---- WIRE:_tavern-side.js ----
if ($('st-zip-file')) {
  $('st-zip-file').onchange = async (e) => {
    const file = e.target.files && e.target.files[0]; if (!file) return;
    try { await stImportPackZip(file); e.target.value = ''; }
    catch (err) { stStatus('ZIP 导入失败：' + err.message); }
  };
}
// ---- WIRE:_tavern-side.js ----
if (_rotBtn) _rotBtn.onclick = () => {
  if (!tavernSession) return;
  const next = !_rotBtn.classList.contains('active');
  _rotBtn.classList.toggle('active', next);
  _rotBtn.setAttribute('aria-pressed', next ? 'true' : 'false');
  stApi('/sessions/' + encodeURIComponent(tavernSession.sessionId) + '/focus', {
    method: 'POST', body: JSON.stringify({ speakerRotation: next, characterId: tavernSession.focusCharacterId || undefined })
  }).then(s => { tavernSession = s; stRenderFocusBar(); stFillVesselSelect(); }).catch(e => stStatus('轮流设置失败：' + e.message));
}
// ---- WIRE:_tavern-side.js ----
if (vToggle) vToggle.onclick = () => {
  if (!tavernSession) return;
  const picker = $('st-vessel-picker');
  if (!picker) return;
  const opening = picker.classList.contains('hidden');
  picker.classList.toggle('hidden', !opening);
  if (opening) stFillVesselSelect();
}
// ---- WIRE:_tavern-side.js ----
if ($('st-node-add')) $('st-node-add').onclick = () => stOpenNodeEditor(null);
// ---- WIRE:_tavern-side.js ----
if ($('st-node-save')) $('st-node-save').onclick = () => stSaveNode();
// ---- WIRE:_tavern-side.js ----
if ($('st-node-cancel')) $('st-node-cancel').onclick = () => { $('st-node-editor').classList.add('hidden'); stNodeEditId = null; stRenderNodes(); }
// ---- WIRE:_tavern-side.js ----
if ($('st-node-del')) $('st-node-del').onclick = () => stDeleteNode();
// ---- WIRE:_tavern-bg-immerse.js ----
window.stRefreshImmerseBg = stRefreshImmerseBg;
// ---- WIRE:_tavern-bg-immerse.js ----
window.stImmerseBgUrl = stImmerseBgUrl;
}

/* ===== exports consumed by remaining closure parts (Mechanism Y: converted[] import line) ===== */
export { stApi, stStatus, stGoBack, stSwitchView, stDisplayTitle, stHasOpenOverlay, stBindImmChrome,
         stRefresh, stLoadPacks, stLoadSessions, stLoadSaves, stLoadSession, stRefreshCharSummary,
         stRenderContinueCard, renderHomeRecent, loadBookshelf };
/* ===== S2.21: also consumed by src/js/shelf.js（书架域经 tavern.js⇄shelf.js 循环引用，仅函数体内使用） ===== */
export { stTurnLabel, stShowPack };
