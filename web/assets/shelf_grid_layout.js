/**
 * shelf_grid_layout.js — 书架双形态布局
 *
 * 需求：无封面的书占大卡片空间太浪费 —— 无封面 = 紧凑一行，
 *       有封面 = 大卡片网格。
 *
 * 方式：纯 CSS 注入（styles.css 是压缩文件，零改动）。
 *  - .shelf-card 含 .shelf-cover-empty（无封面）→ grid-column:1/-1
 *    横跨整行，flex row 紧凑排列：小书标 + 标题 + 章数 + 操作按钮。
 *  - 有封面（img.shelf-cover）→ 保持现有大卡片网格。
 * 依赖 :has() 选择器（现代浏览器均支持）。
 */
(function () {
  'use strict';
  if (document.getElementById('shelf-grid-layout-css')) return;

  var st = document.createElement('style');
  st.id = 'shelf-grid-layout-css';
  st.textContent = [
    /* 无封面书：横跨整行，紧凑一行 */
    '.bookshelf-grid .shelf-card:has(> .shelf-cover-empty) {',
    '  grid-column: 1 / -1;',
    '  flex-direction: row;',
    '  align-items: center;',
    '  gap: var(--sp-2h, 10px);',
    '  padding: var(--sp-2h, 10px) var(--sp-3, 12px);',
    '  min-height: 2.75rem;',
    '}',
    /* 小书标占位 */
    '.bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-cover-empty {',
    '  width: 2.25rem; height: 2.25rem;',
    '  font-size: var(--fs-md, 1rem);',
    '  background: var(--surface-2, rgba(255,255,255,.06));',
    '  border-radius: var(--radius, 8px);',
    '  flex-shrink: 0;',
    '}',
    /* body 行内排布 */
    '.bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-card-body {',
    '  flex-direction: row;',
    '  align-items: center;',
    '  gap: var(--sp-2h, 10px);',
    '  padding: 0;',
    '  flex: 1;',
    '  min-width: 0;',
    '}',
    /* 标题单行省略，弹性占位 */
    '.bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-title {',
    '  flex: 1;',
    '  min-width: 0;',
    '  -webkit-line-clamp: 1;',
    '  white-space: nowrap;',
    '}',
    /* 章节数右对齐 */
    '.bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-meta {',
    '  flex-shrink: 0;',
    '  min-width: 4rem;',
    '  text-align: right;',
    '}',
    /* 操作按钮靠右 */
    '.bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-actions {',
    '  flex-shrink: 0;',
    '  margin-left: auto;',
    '}',
    /* 蒸馏进度条行内无需显示 */
    '.bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-distil-progress {',
    '  display: none !important;',
    '}',
    /* ⇩ 窄屏（安卓手机）自适应 ⇩ */
    '@media (max-width: 40rem) {',
    '  /* 无封面紧凑行：标题独占一行，meta+按钮转到第二行缩进 */',
    '  .bookshelf-grid .shelf-card:has(> .shelf-cover-empty) {',
    '    flex-wrap: wrap;',
    '    padding: var(--sp-1h, 6px) var(--sp-2, 8px);',
    '    gap: var(--sp-1h, 6px);',
    '  }',
    '  .bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-card-body {',
    '    flex-wrap: wrap;',
    '    flex-basis: calc(100% - 2.5rem);',
    '  }',
    '  .bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-title {',
    '    flex-basis: 100%;',
    '    -webkit-line-clamp: 2;',
    '    white-space: normal;',
    '    line-height: 1.3;',
    '  }',
    '  .bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-meta {',
    '    width: auto;',
    '    min-width: 0;',
    '    text-align: left;',
    '  }',
    '  .bookshelf-grid .shelf-card:has(> .shelf-cover-empty) .shelf-actions {',
    '    margin-left: auto;',
    '    width: auto;',
    '    justify-content: flex-end;',
    '  }',
    '  /* 有封面大卡：窄屏限制宽度避免占满整行 */',
    '  .bookshelf-grid .shelf-card:not(:has(> .shelf-cover-empty)) {',
    '    grid-column: span 2;',
    '  }',
    '  .bookshelf-grid {',
    '    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));',
    '  }',
    '}'
  ].join('\n');
  document.head.appendChild(st);
})();