/**
 * shelf_foliate_reader.js — 主 UI 书架阅读器替换为 Foliate 分页阅读
 *
 * 背景：主 UI 的原阅读器 td() 把整本书塞进 <pre> 全文 dump，
 * 无分页/进度/批注。独立书架 /web/bookshelf/ 已是完整的 Foliate
 * 分页阅读器（同一数据源 GET /novels/{slug}/content），本脚本把
 * 主 UI 的「阅读」改道打开内嵌 iframe 版的独立书架阅读器。
 *
 * 方式：monkey-patch window.td（打开阅读）/ window.Vh（关闭阅读），
 * 零改动压缩 app.js。iframe src = /web/bookshelf/#/reader/{slug}。
 *
 * 依赖：api_token_capture.js 的 localStorage:museai_token
 */
(function () {
  'use strict';

  // ── 覆盖层样式（注入 style，避免动压缩 styles.css）──
  function injectCss() {
    if (document.getElementById('shelf-foliate-css')) return;
    var st = document.createElement('style');
    st.id = 'shelf-foliate-css';
    st.textContent = [
      '.shelf-foliate-wrap{position:absolute;inset:0;z-index:var(--z-dropdown, 1000);background:var(--surface-1, #0c0d12);display:flex;flex-direction:column}',
      '.shelf-foliate-wrap.hidden{display:none}',
      '.shelf-foliate-toolbar{display:flex;align-items:center;gap:var(--sp-2, 8px);padding:var(--sp-2, 8px) var(--sp-3, 12px);border-bottom:1px solid var(--border, rgba(127,127,127,.25))}',
      '.shelf-foliate-title{font-weight:600;font-size:var(--fs-md, 1rem);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:45%}',
      '.shelf-foliate-toolbar .ml-auto{margin-left:auto}',
      '.shelf-foliate-frame{flex:1;border:none;width:100%;background:#fff}'
    ].join('\n');
    document.head.appendChild(st);
  }

  var wrap = null;
  var frame = null;
  var titleEl = null;
  var currentSlug = '';

  function el(id) {
    return document.getElementById(id);
  }

  // ── 打开 Foliate iframe 阅读 ──────────────
  function openFoliate(slug, titleText) {
    currentSlug = slug;
    if (!wrap) wrap = el('shelf-foliate-wrap');
    if (!frame) frame = el('shelf-foliate-frame');
    if (!titleEl) titleEl = el('shelf-foliate-title');
    if (!wrap || !frame) return false;

    if (titleEl) titleEl.textContent = titleText || slug || '';
    // 依次清空旧内容再换新 slug（避免 iframe 内旧书残留）
    frame.src = 'about:blank';
    var target = '/web/bookshelf/#/reader/' + encodeURIComponent(slug);
    // 延迟赋值确保 about:blank 生效
    setTimeout(function () {
      frame.src = target;
    }, 80);

    wrap.classList.remove('hidden');
    // 隐藏原 pre reader（若被误开）
    var old = el('novel-reader');
    if (old) old.classList.add('hidden');
    // 原标题异步加载，稍后跟随刷新
    setTimeout(function () {
      var rt2 = el('reader-title');
      if (rt2 && rt2.textContent && rt2.textContent !== '加载中…') {
        if (titleEl) titleEl.textContent = rt2.textContent;
      }
    }, 1200);
    return true;
  }

  function closeFoliate() {
    if (!wrap) wrap = el('shelf-foliate-wrap');
    if (wrap) {
      wrap.classList.add('hidden');
      var f = el('shelf-foliate-frame');
      if (f) f.src = 'about:blank';
    }
    currentSlug = '';
  }

  // ── 事件拦截：td 在 app.js 模块内部，无法直接 monkey-patch ──
  // 在捕获阶段拦截「阅读」按钮 / 书卡点击，打开 Foliate 覆盖层；
  // 同时保留原 td() 副作用（设置 Ji 供「进故事馆/导出」用）——
  // 原点击仍会执行（不阻止默认），我们只负责显示覆盖层。
  var activeSlug = '';
  function interceptReaderClicks(e) {
    var target = e.target;
    var readBtn = target && target.closest ? target.closest('.shelf-read') : null;
    var card = target && target.closest ? target.closest('.shelf-card') : null;
    if (!readBtn && !card) return;
    // 延迟到原 onclick 的 td() 执行完再开覆盖层（td 会同步设 reader-title + Ji）
    var slug = card ? card.getAttribute('data-slug') || '' : '';
    if (!slug && readBtn) {
      // 从 onclck 解析 P5AiModelToggle 样式路径不可行——优先卡 data-slug
      var card2 = readBtn.closest('.shelf-card');
      if (card2) slug = card2.getAttribute('data-slug') || '';
    }
    if (!slug) return;
    activeSlug = slug;
    var rt = document.getElementById('reader-title');
    setTimeout(function () {
      openFoliate(slug, (rt && rt.textContent && rt.textContent !== '加载中…') ? rt.textContent : slug);
    }, 60);
  }
  document.addEventListener('click', interceptReaderClicks, true);

  // ── 绑定覆盖层按钮 ────────────────────────
  function bind() {
    var back = el('shelf-foliate-back');
    if (back && !back.getAttribute('data-bound')) {
      back.setAttribute('data-bound', '1');
      back.addEventListener('click', function () {
        closeFoliate();
        var old = el('novel-reader');
        if (old) old.classList.add('hidden');
      });
    }
    var pack = el('shelf-foliate-pack');
    if (pack && !pack.getAttribute('data-bound')) {
      pack.setAttribute('data-bound', '1');
      pack.addEventListener('click', function () {
        // 复用原「进故事馆」按钮的点击处理（内部依赖 Ji/当前 slug）
        var orig = el('reader-to-pack');
        if (orig && typeof orig.click === 'function') orig.click();
      });
    }
    var exp = el('shelf-foliate-export');
    if (exp && !exp.getAttribute('data-bound')) {
      exp.setAttribute('data-bound', '1');
      exp.addEventListener('click', function () {
        var orig = el('reader-export');
        if (orig && typeof orig.click === 'function') orig.click();
      });
    }
    // Esc 关闭
    if (!document.documentElement.getAttribute('data-shelf-foliate-esc')) {
      document.documentElement.setAttribute('data-shelf-foliate-esc', '1');
      document.addEventListener('keydown', function (e) {
        if (e.key === 'Escape') {
          var w = el('shelf-foliate-wrap');
          if (w && !w.classList.contains('hidden')) {
            closeFoliate();
            var old = el('novel-reader');
            if (old) old.classList.add('hidden');
          }
        }
      });
    }
  }

  // ── 轮询绑定（app.js 绑定在 DOMContentLoaded，确保覆盖层按钮后于脚本注入）──
  var timer = null;
  function schedule() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(function () {
      injectCss();
      bind();
      schedule();
    }, 800);
  }

  injectCss();
  schedule();
})();