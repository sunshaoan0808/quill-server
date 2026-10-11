/**
 * AI 管理增强 — 独立 JS（不压缩 app.js 的 monkey-patch）
 *
 * 功能：
 * 1. 模型卡片紧凑化（重写 app.js 的宽卡片样式）
 * 2. 「设为默认」按钮：点击 PATCH provider.default_model_id 切换默认模型
 * 3. 默认模型置顶 + 星标 + 高亮边框，一眼区分生效/未生效
 *
 * 依赖 api_token_capture.js 捕获的 localStorage:museai_token
 */
(function () {
  'use strict';

  function token() {
    try { return localStorage.getItem('museai_token') || ''; } catch (e) { return ''; }
  }
  function apiBase() {
    try {
      var b = localStorage.getItem('kaleido_api_base');
      if (b) return b.replace(/\/$/, '');
    } catch (e) {}
    return '';
  }
  // 只显示「设为默认」，不重复渲染—直接操作列表
  var DEFAULT_BTN_NOTE = '设为默认';

  // ── 1. 紧凑卡片样式覆盖 ──────────────────────────────────
  function injectCss() {
    var style = document.createElement('style');
    style.setAttribute('data-aiadm-lite', '1');
    style.textContent = [
      '#aiadm-models-list .settings.card { min-width: 190px !important; flex: 1 1 210px !important; padding: 8px 10px !important; margin: 0 !important; }',
      '#aiadm-models-list .settings.card .row.between > b { font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 120px; }',
      '#aiadm-models-list .settings.card .muted.sm { font-size: 11px; line-height: 1.5; }',
      '#aiadm-models-list .settings.card .muted.sm:nth-child(3) { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }',
      '#aiadm-models-list .settings.card .row.gap-sm { margin-top: 6px !important; }',
      '#aiadm-models-list .settings.card .row.gap-sm button { padding: 2px 8px; font-size: 11px; }',
      // 默认模型高亮
      '#aiadm-models-list .settings.card.model-active { border-color: var(--accent, #4f8cff) !important; box-shadow: 0 0 0 1px var(--accent, #4f8cff); background: color-mix(in srgb, var(--accent, #4f8cff) 8%, transparent); }',
      '#aiadm-models-list .settings.card .model-badge { display: inline-block; background: var(--accent, #4f8cff); color: #fff; font-size: 10px; font-weight: 700; border-radius: 3px; padding: 0 5px; line-height: 16px; margin-left: 6px; vertical-align: 1px; }',
      '#aiadm-models-list .settings.card .row.between { align-items: center; }',
      '#aiadm-models-list .settings.card .row.between .right { display: flex; align-items: center; gap: 4px; }',
      // ── 供应商列表紧凑 + 状态区分 ──
      '#aiadm-providers > .row.wrap.gap-sm { gap: 8px; }',
      '#aiadm-providers .settings.card { min-width: 210px !important; flex: 1 1 230px !important; padding: 10px 12px !important; margin: 0 !important; position: relative; }',
      '#aiadm-providers .settings.card .row.between > b { font-size: 14px; }',
      '#aiadm-providers .settings.card .muted.sm { font-size: 11px; line-height: 1.6; }',
      '#aiadm-providers .settings.card .row.gap-sm { margin-top: 8px !important; }',
      '#aiadm-providers .settings.card .row.gap-sm button { padding: 3px 10px; font-size: 11px; }',
      // 供应商 base_url 单行截断
      '#aiadm-providers .settings.card .muted.sm[style*="word-break"] { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }',
      // active（启用）供应商：金色高亮边框 + 顶部色条
      '#aiadm-providers .settings.card.prov-active { border-color: var(--accent, #4f8cff) !important; box-shadow: 0 0 0 1px var(--accent, #4f8cff); }',
      '#aiadm-providers .settings.card.prov-active::before { content: ""; position: absolute; top: 0; left: 0; right: 0; height: 3px; background: var(--accent, #4f8cff); border-radius: 4px 4px 0 0; }',
      // 非 active：灰边 + 降饱和
      '#aiadm-providers .settings.card.prov-idle { border-color: rgba(127,127,127,.35) !important; opacity: .85; }',
      '#aiadm-providers .settings.card.prov-idle::before { content: ""; position: absolute; top: 0; left: 0; right: 0; height: 3px; background: rgba(127,127,127,.25); border-radius: 4px 4px 0 0; }',
      // 状态徽章：启用/未启用
      '#aiadm-providers .settings.card .prov-badge { display: inline-block; font-size: 10px; font-weight: 700; border-radius: 3px; padding: 0 6px; line-height: 16px; margin-left: 6px; vertical-align: 1px; }',
      '#aiadm-providers .settings.card.prov-active .prov-badge { background: var(--accent, #4f8cff); color: #fff; }',
      '#aiadm-providers .settings.card.prov-idle .prov-badge { background: rgba(127,127,127,.18); color: #999; border: 1px solid rgba(127,127,127,.3); }',
      '#aiadm-providers .settings.card .row.between { align-items: center; }'
    ].join('\n');
    document.head.appendChild(style);
  }

  // ── 2. 工具函数 ──────────────────────────────────────────
  function getProviders() {
    return fetch(apiBase() + '/api/v1/ai/providers', {
      headers: token() ? { 'Authorization': 'Bearer ' + token() } : {}
    }).then(function (r) { return r.json(); }).then(function (d) {
      return (d && (d.providers || d.items)) || (Array.isArray(d) ? d : []);
    });
  }

  function setDefaultModel(providerId, modelId) {
    return fetch(apiBase() + '/api/v1/ai/providers/' + encodeURIComponent(providerId), {
      method: 'PATCH',
      headers: {
        'Content-Type': 'application/json',
        ...(token() ? { 'Authorization': 'Bearer ' + token() } : {})
      },
      body: JSON.stringify({ default_model_id: modelId })
    }).then(function (r) {
      if (!r.ok) throw new Error('HTTP ' + r.status);
      return r.json();
    });
  }

  // ── 3. 供应商卡增强：active 高亮 + 状态徽章 ──────────────
  function enhanceProviders() {
    var wrap = document.querySelector('#aiadm-providers > .row.wrap.gap-sm');
    if (!wrap) return;
    if (wrap.getAttribute('data-enh') === '1') return;
    var cards = wrap.querySelectorAll(':scope > .settings.card');
    if (!cards.length) return;
    for (var i = 0; i < cards.length; i++) {
      var card = cards[i];
      var isActive = !!(card.querySelector('.badge-active') || card.hasAttribute('card-active'));
      card.classList.add(isActive ? 'prov-active' : 'prov-idle');
      // 状态徽章插到标题 <b> 后
      var b = card.querySelector('.row.between > b');
      if (b && !card.querySelector('.prov-badge')) {
        var badge = document.createElement('span');
        badge.className = 'prov-badge';
        badge.textContent = isActive ? '● 当前启用' : '○ 未启用';
        b.appendChild(badge);
      }
      // 「设为当前」按钮加个强调样式
      var act = card.querySelector('[data-activate]');
      if (act) {
        act.classList.add('sm', 'ghost');
        act.style.borderColor = 'var(--accent, #4f8cff)';
        act.style.color = 'var(--accent, #4f8cff)';
      }
    }
    wrap.setAttribute('data-enh', '1');
  }

  // ── 4. 页面挂点：wrap P5AiModels 记录当前 provider id ──
  var currentProviderId = '';
  var origP5AiModels = null;
  function wrapP5AiModels() {
    if (window.P5AiModels && !window.P5AiModels.__wrapped) {
      origP5AiModels = window.P5AiModels;
      var wrapped = function (pid, name) {
        currentProviderId = pid || '';
        // 重置增强标记（新 provider 的模型列表需要重新处理）
        var list = document.getElementById('aiadm-models-list');
        if (list) list.setAttribute('data-enhanced', '0');
        // P5AiModels 是异步渲染,调用后延迟增强
        setTimeout(enhance, 1200);
        return origP5AiModels.call(this, pid, name);
      };
      wrapped.__wrapped = true;
      window.P5AiModels = wrapped;
    }
  }

  // ── 5. 增强每个模型卡 ────────────────────────────────────
  // 轮询容器，找到 app.js 渲染的模型卡，追加「设为默认」+ 默认高亮
  function enhance() {
    var list = document.getElementById('aiadm-models-list');
    if (!list) return;
    if (!currentProviderId) {
      // 兜底：从 providers 列表第一个「模型」按钮捞
      var btn = document.querySelector('#aiadm-providers button[onclick*="P5AiModels"]');
      if (btn) {
        var m = (btn.getAttribute('onclick') || '').match(/P5AiModels\('([^']+)'/);
        if (m) currentProviderId = m[1];
        else return;
      } else return;
    }
    if (list.getAttribute('data-enhanced') === '1') return;

    getProviders().then(function (providers) {
      var pid = currentProviderId;
      var prov = null;
      for (var i = 0; i < providers.length; i++) {
        if (providers[i].id === pid) { prov = providers[i]; break; }
      }
      if (!prov) return;
      var defaultModelId = (prov.default_model_id || '').toString();

      var cards = list.querySelectorAll('.settings.card');
      for (var c = 0; c < cards.length; c++) {
        var card = cards[c];
        // 跳过已处理的
        if (card.getAttribute('data-enh') === '1') continue;
        card.setAttribute('data-enh', '1');

        // 解析 model id：卡片上的「设为默认」需要 model 的 id
        // app.js 渲染时行内 onclick=P5AiModelToggle('<id>',...) 包含 model id
        var modelId = '';
        var toggler = card.querySelector('button[onclick*="P5AiModelToggle"]');
        if (toggler) {
          var mo = (toggler.getAttribute('onclick') || '').match(/P5AiModelToggle\('([^']+)'/);
          if (mo) modelId = mo[1];
        }
        if (!modelId) continue;

        var isDefault = (modelId === defaultModelId);

        // 标题行右侧加星标/默认徽章
        var titleRow = card.querySelector('.row.between');
        if (titleRow) {
          // 徽章插到 <b> 后面
          var b = titleRow.querySelector('b');
          if (b) {
            var badge = b.parentNode.querySelector('.model-badge');
            if (!badge) {
              badge = document.createElement('span');
              badge.className = 'model-badge';
              badge.textContent = isDefault ? '★ 默认' : '☆';
              b.parentNode.insertBefore(badge, b.nextSibling);
            } else {
              badge.textContent = isDefault ? '★ 默认' : '☆';
            }
          }
        }

        // 默认模型高亮 + 置顶
        if (isDefault) {
          card.classList.add('model-active');
          card.parentNode.insertBefore(card, card.parentNode.firstChild);
        }

        // 非默认卡加「设为默认」按钮
        if (!isDefault) {
          var actions = card.querySelector('.row.gap-sm');
          if (actions && !card.querySelector('[data-set-default]')) {
            var btn = document.createElement('button');
            btn.type = 'button';
            btn.className = 'sm ghost';
            btn.setAttribute('data-set-default', '1');
            btn.textContent = DEFAULT_BTN_NOTE;
            // 闭包陷阱修复：var 在 for 循环共享同一变量，
            // 必须用 IIFE 把 modelId 独立捕获到每次迭代
            (function (mid, providerId) {
              btn.onclick = function (e) {
                e.stopPropagation();
                var self = this;
                self.disabled = true;
                self.textContent = '设置中…';
                setDefaultModel(providerId, mid).then(function () {
                  // 成功后重新加载模型列表
                  if (window.P5AiModels) window.P5AiModels(providerId);
                  else location.reload();
                }).catch(function (err) {
                  alert('设置默认模型失败: ' + err.message);
                  self.disabled = false;
                  self.textContent = DEFAULT_BTN_NOTE;
                });
              };
            })(modelId, pid);
            actions.appendChild(btn);
          }
        }

        // 所有卡加「测试」按钮
        var actions = card.querySelector('.row.gap-sm');
        if (actions && !card.querySelector('[data-test-model]')) {
          var tbtn = document.createElement('button');
          tbtn.type = 'button';
          tbtn.className = 'sm ghost';
          tbtn.setAttribute('data-test-model', '1');
          tbtn.textContent = '测试';
          // 闭包陷阱修复：IIFE 捕获本次迭代卡片对应的上游模型名
          // （/api/v1/llm/test 的 body.model 需要上游模型名，onclick 里的
          //   modelId 是库内记录 id，直接传上游不认识）
          (function (mname) {
            tbtn.onclick = function (e) {
              e.stopPropagation();
              var self = this;
              self.disabled = true;
              self.textContent = '测试中…';
              testModel(mname).then(function (res) {
                self.disabled = false;
                self.textContent = '测试';
                var st = document.getElementById('aiadm-model-test-status');
                if (res.ok) {
                  var sample = res.sample || res.content || '';
                  self.style.color = 'var(--ok, #3dd68c)';
                  self.style.borderColor = 'var(--ok, #3dd68c)';
                  if (st) { st.textContent = '✓ ' + res.latencyMs + 'ms · ' + sample; st.style.color = 'var(--ok, #3dd68c)'; }
                  showTestModal('测试通过 ✓', (sample || '（空回复）') + '（' + res.latencyMs + 'ms）', true);
                } else {
                  self.style.color = 'var(--err, #e06c6c)';
                  self.style.borderColor = 'var(--err, #e06c6c)';
                  if (st) { st.textContent = '✗ ' + (res.error || '未知错误'); st.style.color = 'var(--err, #e06c6c)'; }
                  showTestModal('测试失败 ✗', res.error || '未知错误', false);
                }
              }).catch(function (err) {
                self.disabled = false;
                self.textContent = '测试';
                showTestModal('测试失败 ✗', err.message, false);
              });
            };
          })(cardUpstreamName(card));
          actions.appendChild(tbtn);
        }
      }
      list.setAttribute('data-enhanced', '1');
    }).catch(function () {});
  }

  // ── 6. 模型测试 ---------------------------------------------
  // 服务端不存在 POST /api/v1/ai/models/{id}/test（必然 404）；
  // 连通性测试统一走 POST /api/v1/llm/test，body 传 { model: 上游模型名 }，
  // 与 app.js 设置页测试按钮（{model: llmModel||void 0}）同一约定。
  // model 留空时序列化为 {} ，服务端回落到自身配置的默认模型。
  function testModel(modelName) {
    return fetch(apiBase() + '/api/v1/llm/test', {
      method: 'POST',
      headers: Object.assign(
        { 'Content-Type': 'application/json' },
        token() ? { 'Authorization': 'Bearer ' + token() } : {}
      ),
      body: JSON.stringify({ model: modelName || undefined })
    }).then(function (r) {
      return r.json().then(function (d) {
        if (!r.ok && !d) throw new Error('HTTP ' + r.status);
        return d;
      });
    });
  }

  // 从 app.js 渲染的模型卡提取上游模型名：第二行 muted.sm 形如
  // "gpt-4o · ctx 128000"（app.js: qt(c.model_id)+" · ctx "+context_window）。
  // 卡片 onclick 里的 id 是 ai_models 记录 id，不能当上游模型名用。
  // 提取不到时返回 ''，由服务端回落默认模型。
  function cardUpstreamName(card) {
    var divs = card.querySelectorAll('.muted.sm');
    for (var i = 0; i < divs.length; i++) {
      var t = (divs[i].textContent || '').trim();
      var k = t.indexOf(' · ctx ');
      if (k > 0) return t.slice(0, k).trim();
    }
    return '';
  }

  // 弹窗展示测试结果（复用页内 overlay，避免依赖 app.js 的弹层）
  function showTestModal(title, body, isOk) {
    var ov = document.getElementById('aiadm-test-overlay');
    if (!ov) {
      ov = document.createElement('div');
      ov.id = 'aiadm-test-overlay';
      ov.style.cssText = 'position:fixed;inset:0;background:rgba(0,0,0,.55);z-index:99999;display:flex;align-items:center;justify-content:center;';
      ov.addEventListener('click', function (e) { if (e.target === ov) ov.remove(); });
      document.body.appendChild(ov);
    }
    var box = document.createElement('div');
    box.style.cssText = 'background:var(--bg,#1b1e27);border:1px solid ' + (isOk ? 'var(--ok,#3dd68c)' : 'var(--err,#e06c6c)') + ';border-radius:10px;padding:18px 22px;max-width:460px;width:90%;box-shadow:0 8px 30px rgba(0,0,0,.4);';
    var close = document.createElement('button');
    close.textContent = '✕ 关闭';
    close.style.cssText = 'margin-top:12px;padding:6px 16px;border-radius:6px;border:1px solid rgba(127,127,127,.4);background:transparent;color:inherit;cursor:pointer;';
    close.addEventListener('click', function () { ov.remove(); });
    box.innerHTML = '<div style="display:flex;align-items:center;gap:8px;margin-bottom:10px"><b style="font-size:15px;color:' + (isOk ? 'var(--ok,#3dd68c)' : 'var(--err,#e06c6c)') + '">' + title + '</b></div><div style="font-size:13px;line-height:1.7;word-break:break-all;white-space:pre-wrap">' + String(body).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;') + '</div>';
    box.appendChild(close);
    ov.appendChild(box);
  }

  // ── 4. 页面挂点：P5AiModels 打开时重新增强 ──────────────
  // app.js 的 P5AiModels 函数会把 provider id 传给「模型」按钮的 onclick，
  // 但我们拿不到它的内部渲染回调,所以用轮询增强 + 返回按钮记录 provider id。
  function patchBackButton() {
    var back = document.getElementById('aiadm-models-back');
    if (back && !back.hasAttribute('data-provider-id')) {
      // 尝试从 provider 列表的「模型」按钮 onclick 里捞当前 pid
      var btn = document.querySelector('#aiadm-providers button[onclick*="P5AiModels"]');
      if (btn) {
        var m = (btn.getAttribute('onclick') || '').match(/P5AiModels\('([^']+)'/);
        if (m) back.setAttribute('data-provider-id', m[1]);
      }
    }
  }

  // 供应商增强入口（列表渲染由 P5AiLoad 触发, 轮询兜底）
  function scheduleProviders() {
    enhanceProviders();
  }

  // app.js 渲染完成是异步的,这里用 MutationObserver + 轮询双保险
  var enhanceTimer = null;
  function schedule() {
    if (enhanceTimer) clearTimeout(enhanceTimer);
    enhanceTimer = setTimeout(function () {
      patchBackButton();
      scheduleProviders();
      enhance();
      schedule();
    }, 800);
  }

  // tab 切到 aiadmin 时立即跑一次
  var tabSwitchHandler = function (e) {
    var target = e.target && e.target.closest ? e.target.closest('[data-tab="aiadmin"]') : null;
    if (target) {
      var plist = document.getElementById('aiadm-models-list');
      if (plist) plist.setAttribute('data-enhanced', '0');
      var pwrap = document.querySelector('#aiadm-providers > .row.wrap.gap-sm');
      if (pwrap) pwrap.setAttribute('data-enh', '0');
      schedule();
    }
  };

  function init() {
    injectCss();
    // 等 app.js 暴露 P5AiModels 后 wrap
    var wrapTimer = setInterval(function () {
      wrapP5AiModels();
      if (window.P5AiModels && window.P5AiModels.__wrapped) clearInterval(wrapTimer);
    }, 500);
    document.addEventListener('click', tabSwitchHandler, true);
    schedule();
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();