/**
 * UI block renderers — static component set (PR2).
 *
 * The model emits ```deeix-ui fences whose JSON is { component: "<type>", ... }.
 * Each renderer builds a detached HTMLElement; mountUiBlock wraps it with a
 * common card shell (title area + body). All text goes through textContent —
 * renderer output never uses innerHTML for model-controlled strings.
 *
 * Renderers registered here must match the backend catalog component names
 * (crates/kaleido-core/src/ui_components.rs builtin_specs).
 */

import { mountChart } from './chart.js';
import { buildGantt } from './gantt.js';

const registry = new Map();

function reg(name, builder) {
  registry.set(name, builder);
}

export function hasRenderer(name) {
  return registry.has(name);
}

export function rendererNames() {
  return [...registry.keys()];
}

/* ---------- shared helpers ---------- */

function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text != null) node.textContent = String(text);
  return node;
}

function asArray(v) {
  if (Array.isArray(v)) return v;
  if (v == null) return [];
  return [v];
}

function str(v, fallback) {
  return typeof v === 'string' && v ? v : (fallback || '');
}

/* ---------- card-grid ---------- */

reg('card-grid', (props) => {
  const wrap = el('div', 'stui-card-grid');
  for (const item of asArray(props.items)) {
    const card = el('div', 'stui-card');
    if (str(item.icon)) card.appendChild(el('div', 'stui-card-icon', item.icon));
    if (str(item.title)) card.appendChild(el('div', 'stui-card-title', item.title));
    if (str(item.subtitle)) card.appendChild(el('div', 'stui-card-sub', item.subtitle));
    if (str(item.body)) card.appendChild(el('div', 'stui-card-body', item.body));
    const fields = asArray(item.fields);
    if (fields.length) {
      const dl = el('div', 'stui-card-fields');
      for (const f of fields) {
        if (!f || typeof f !== 'object') continue;
        const row = el('div', 'stui-field');
        row.appendChild(el('span', 'stui-field-label', str(f.label)));
        row.appendChild(el('span', 'stui-field-value', str(f.value)));
        dl.appendChild(row);
      }
      card.appendChild(dl);
    }
    wrap.appendChild(card);
  }
  if (!wrap.children.length) wrap.appendChild(el('div', 'stui-empty', '（空卡片组）'));
  return wrap;
});

/* ---------- stat-grid ---------- */

reg('stat-grid', (props) => {
  const wrap = el('div', 'stui-stat-grid');
  for (const item of asArray(props.items)) {
    if (!item || typeof item !== 'object') continue;
    const tile = el('div', 'stui-stat');
    if (item.tone) tile.dataset.tone = str(item.tone, 'neutral');
    tile.appendChild(el('div', 'stui-stat-label', str(item.label)));
    tile.appendChild(el('div', 'stui-stat-value', str(item.value, '—')));
    if (item.delta != null && item.delta !== '') {
      const d = el('div', 'stui-stat-delta', String(item.delta));
      const n = Number(String(item.delta).replace(/[^0-9.-]/g, ''));
      if (Number.isFinite(n)) d.dataset.dir = n > 0 ? 'up' : n < 0 ? 'down' : 'flat';
      tile.appendChild(d);
    }
    wrap.appendChild(tile);
  }
  if (!wrap.children.length) wrap.appendChild(el('div', 'stui-empty', '（空指标组）'));
  return wrap;
});

/* ---------- data-table (sortable columns) ---------- */

reg('data-table', (props) => {
  const columns = asArray(props.columns).map((c) =>
    typeof c === 'string' ? { label: c, key: c } : { label: str(c && c.label, ''), key: str(c && c.key, ''), numeric: !!(c && c.numeric) },
  ).filter((c) => c.label);
  const rows = asArray(props.rows).map((r) => (Array.isArray(r) ? r : asArray(r && r.cells)));

  const wrap = el('div', 'stui-table-wrap');
  if (str(props.title)) wrap.appendChild(el('div', 'stui-table-title', props.title));
  if (!columns.length) {
    wrap.appendChild(el('div', 'stui-empty', '（表格无列定义）'));
    return wrap;
  }
  const table = el('table', 'stui-table');
  const thead = el('thead', null);
  const trh = el('tr', null);
  let sortState = { idx: -1, dir: 0 }; // 0 none, 1 asc, -1 desc
  const tbody = el('tbody', null);

  function paintBody() {
    tbody.textContent = '';
    for (const r of rows) {
      const tr = el('tr', null);
      for (let i = 0; i < columns.length; i++) {
        const td = el('td', columns[i].numeric ? 'num' : null, r[i] == null ? '' : String(r[i]));
        tr.appendChild(td);
      }
      tbody.appendChild(tr);
    }
  }

  columns.forEach((c, i) => {
    const th = el('th', null, c.label);
    th.title = '点击排序';
    th.addEventListener('click', () => {
      if (sortState.idx !== i) sortState = { idx: i, dir: 1 };
      else if (sortState.dir === 1) sortState.dir = -1;
      else { sortState.dir = 0; sortState.idx = -1; }
      const idx = i;
      if (sortState.dir === 0) {
        rows.sort((a, b) => rows.indexOf(a) - rows.indexOf(b));
        th.removeAttribute('data-dir');
      } else {
        const dir = sortState.dir;
        const numericCol = c.numeric || rows.every((r) => r[idx] == null || r[idx] === '' || Number.isFinite(Number(r[idx])));
        rows.sort((a, b) => {
          const av = a[idx]; const bv = b[idx];
          let cmp;
          if (numericCol) cmp = Number(av || 0) - Number(bv || 0);
          else cmp = String(av == null ? '' : av).localeCompare(String(bv == null ? '' : bv), 'zh');
          return dir === 1 ? cmp : -cmp;
        });
        th.dataset.dir = dir === 1 ? 'asc' : 'desc';
      }
      for (const sib of trh.children) if (sib !== th) sib.removeAttribute('data-dir');
      paintBody();
    });
    trh.appendChild(th);
  });

  thead.appendChild(trh);
  table.appendChild(thead);
  paintBody();
  table.appendChild(tbody);
  wrap.appendChild(table);
  return wrap;
});

/* ---------- quiz (click option → reveal) ---------- */

reg('quiz', (props) => {
  const wrap = el('div', 'stui-quiz');
  wrap.appendChild(el('div', 'stui-quiz-q', str(props.question, '（无题干）')));
  const options = asArray(props.options).map((o) => String(o));
  const answerIdx = (typeof props.answer === 'number' && Number.isInteger(props.answer))
    ? props.answer
    : options.findIndex((o) => o === String(props.answer == null ? '' : props.answer));
  const list = el('div', 'stui-quiz-options');
  let answered = false;
  options.forEach((opt, i) => {
    const btn = el('button', 'stui-quiz-option', opt);
    btn.type = 'button';
    btn.addEventListener('click', () => {
      if (answered) return;
      answered = true;
      for (let k = 0; k < list.children.length; k++) {
        const b = list.children[k];
        b.disabled = true;
        if (k === answerIdx) b.classList.add('is-answer');
        if (k === i && i !== answerIdx) b.classList.add('is-wrong');
      }
      if (str(props.explanation)) {
        const exp = el('div', 'stui-quiz-exp', props.explanation);
        wrap.appendChild(exp);
      }
    });
    list.appendChild(btn);
  });
  wrap.appendChild(list);
  return wrap;
});

/* ---------- diff (line-level LCS) ---------- */

function diffLines(oldText, newText) {
  const a = String(oldText || '').replace(/\r\n/g, '\n').split('\n');
  const b = String(newText || '').replace(/\r\n/g, '\n').split('\n');
  const n = a.length; const m = b.length;
  const dp = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const ops = [];
  let i = 0; let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) { ops.push(['same', a[i]]); i++; j++; }
    else if (dp[i + 1][j] >= dp[i][j + 1]) { ops.push(['del', a[i]]); i++; }
    else { ops.push(['add', b[j]]); j++; }
  }
  while (i < n) { ops.push(['del', a[i++]]); }
  while (j < m) { ops.push(['add', b[j++]]); }
  return ops;
}

reg('diff', (props) => {
  const wrap = el('div', 'stui-diff');
  const head = el('div', 'stui-diff-head');
  head.appendChild(el('span', 'stui-diff-title', str(props.title, '对比')));
  const stat = el('span', 'stui-diff-stat');
  head.appendChild(stat);
  wrap.appendChild(head);
  const body = el('div', 'stui-diff-body');
  const ops = diffLines(props.old != null ? props.old : props.from, props.new != null ? props.new : props.to);
  let add = 0; let del = 0;
  for (const [kind, line] of ops) {
    const row = el('div', 'stui-diff-line is-' + kind, line);
    row.dataset.kind = kind;
    if (kind === 'add') { add++; row.insertBefore(el('span', 'stui-diff-mark', '+'), row.firstChild); }
    else if (kind === 'del') { del++; row.insertBefore(el('span', 'stui-diff-mark', '-'), row.firstChild); }
    else row.insertBefore(el('span', 'stui-diff-mark', ' '), row.firstChild);
    body.appendChild(row);
  }
  stat.textContent = `+${add} / -${del}`;
  wrap.appendChild(body);
  return wrap;
});

/* ---------- decision-tree (recursive) ---------- */

reg('decision-tree', (props) => {
  const wrap = el('div', 'stui-tree');
  const build = (node, depth) => {
    if (node == null) return;
    if (typeof node === 'string') {
      wrap.appendChild(el('div', 'stui-tree-leaf', node));
      return;
    }
    const row = el('div', 'stui-tree-node');
    row.style.marginLeft = `${Math.min(depth, 6) * 14}px`;
    if (str(node.label) || str(node.text)) {
      row.appendChild(el('span', 'stui-tree-label', str(node.label, str(node.text))));
    }
    const outcome = str(node.outcome) || str(node.result);
    if (outcome) {
      const badge = el('span', 'stui-tree-badge', outcome);
      badge.dataset.tone = outcome.includes('成') || outcome.toLowerCase().includes('pass') ? 'ok' : 'risk';
      row.appendChild(badge);
    }
    if (row.childNodes.length) wrap.appendChild(row);
    for (const child of asArray(node.children || node.branches)) build(child, depth + 1);
  };
  build(props.root != null ? props.root : props.tree, 0);
  if (!wrap.children.length) wrap.appendChild(el('div', 'stui-empty', '（空决策树）'));
  return wrap;
});

/* ---------- chart (lazy echarts) ---------- */

reg('chart', (props) => {
  const box = el('div', 'stui-chart');
  box.style.height = '260px';
  // lazy vendor chunk — never blocks the bubble; failure degrades to text
  mountChart(box, props).catch((e) => {
    box.textContent = '';
    box.appendChild(buildUiBlockError(`图表加载失败: ${e && e.message}`, JSON.stringify(props).slice(0, 200)));
  });
  return box;
});

/* ---------- gantt ---------- */

reg('gantt', (props) => buildGantt(props));

/* ---------- error card (bad JSON / unknown component) ---------- */

export function buildUiBlockError(reason, raw) {
  const wrap = el('div', 'stui-block-error');
  wrap.appendChild(el('div', 'stui-block-error-title', 'UI 组件渲染失败'));
  wrap.appendChild(el('div', 'stui-block-error-reason', String(reason || '未知错误')));
  if (raw) {
    const pre = el('pre', 'stui-block-error-raw', String(raw).slice(0, 400));
    wrap.appendChild(pre);
  }
  return wrap;
}

/**
 * Parse + validate a deeix-ui block payload (pure — unit-testable without DOM).
 * Returns { name, spec }; throws Error with a human reason on bad input.
 */
export function parseUiBlockSpec(block) {
  if (!block || block.language !== 'deeix-ui') {
    throw new Error(`unsupported block language: ${block && block.language}`);
  }
  let spec;
  try {
    spec = JSON.parse(block.rawBody);
  } catch (e) {
    const m = /position (\d+)/.exec(String(e && e.message));
    const p = m ? ` @${m[1]}` : '';
    throw new Error(`invalid JSON${p}`);
  }
  if (!spec || typeof spec !== 'object' || Array.isArray(spec)) {
    throw new Error('payload is not an object');
  }
  const name = String(spec.component || spec.type || '');
  if (!name) throw new Error('missing "component" field');
  return { name, spec };
}

/**
 * Render one extracted deeix-ui block → HTMLElement (unwrapped, no shell).
 * Throws are caught by the caller via tryMountUiBlock.
 */
export function renderUiBlockBody(block) {
  const { name, spec } = parseUiBlockSpec(block);
  const builder = registry.get(name);
  if (!builder) throw new Error(`unknown component "${name}"`);
  return builder(spec);
}
