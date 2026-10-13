// Live blocks: state, computed values and a tree of primitives and catalog
// components, re-resolved on every change. The model writes data and formulas;
// everything here is interpretation, no model code runs.
//
//   mountLiveBlock(body, { ... })   compile, render, wire controls
//
// Every state change re-resolves the whole tree and re-renders: blocks are
// capped at 300 nodes, so a full pass is cheaper than diffing. Only what the
// user changed is kept (as a diff against the declared state) and persisted
// per block key in localStorage.

import katex from 'katex';
import 'katex/dist/katex.min.css';

import {
  compileBlock,
  isPointTemplate,
  mergeState,
  parseBlockSource,
  readAction,
  resolveBlock,
  stateDiff,
} from './block.js';
import { formulaToLatex } from './expression.js';
import { formatTemplateNumber } from './template.js';

const LIVE_TONES = ['default', 'muted', 'positive', 'negative', 'warning', 'info'];
const TEXT_SIZES = ['sm', 'base', 'lg'];

// ---- prop readers (missing props fall back to a sensible default) ----

function readText(props, name) {
  const value = props[name];
  if (typeof value === 'string') return value;
  return typeof value === 'number' && Number.isFinite(value) ? String(value) : undefined;
}

function readNumber(props, name) {
  const value = props[name];
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function readBoolean(props, name) {
  return typeof props[name] === 'boolean' ? props[name] : undefined;
}

function readEnum(props, name, allowed) {
  const value = props[name];
  return allowed.includes(value) ? value : undefined;
}

// Options as `["a", "b"]` or `[{ label, value }, …]`.
function readOptions(props, name) {
  const list = Array.isArray(props[name]) ? props[name] : [];
  const result = [];
  const seen = new Set();
  for (const entry of list) {
    const option = typeof entry === 'object' && entry !== null
      ? { label: displayValue(entry.label ?? entry.value ?? ''), value: jsonValue(entry.value) }
      : { label: displayValue(entry), value: jsonValue(entry) };
    const key = option.value === undefined ? `#${result.length}:${option.label}` : JSON.stringify(option.value);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(option);
    if (result.length >= 40) break;
  }
  return result;
}

function optionIndex(options, value) {
  return options.findIndex((option) => JSON.stringify(option.value) === JSON.stringify(value ?? null));
}

function jsonValue(value) {
  if (value === null || typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return value;
  return JSON.stringify(value);
}

function displayValue(value) {
  if (typeof value === 'number') return Number.isFinite(value) ? formatTemplateNumber(value) : '—';
  if (typeof value === 'string') return value;
  if (value === null || value === undefined) return '';
  if (typeof value === 'boolean') return String(value);
  return JSON.stringify(value);
}

// ---- state persistence ----

const STORAGE_PREFIX = 'quill-deeix-ui:';

function loadOverrides(key) {
  try {
    const raw = localStorage.getItem(STORAGE_PREFIX + key);
    const value = raw ? JSON.parse(raw) : undefined;
    return typeof value === 'object' && value !== null && !Array.isArray(value) ? value : undefined;
  } catch {
    return undefined;
  }
}

function saveOverrides(key, overrides) {
  try {
    if (overrides) localStorage.setItem(STORAGE_PREFIX + key, JSON.stringify(overrides));
    else localStorage.removeItem(STORAGE_PREFIX + key);
  } catch {
    // Private mode or a full quota: the block still works, just forgetfully.
  }
}

// ---- the block instance ----

class LiveBlock {
  constructor(parsed, options) {
    this.compiled = compileBlock(parsed.block, options.compileOptions ?? {});
    this.key = parsed.block.key || 'live';
    this.options = options;
    this.local = new Map(); // per-path local UI state (open tabs, collapse)
    const saved = loadOverrides(this.key);
    this.overrides = {};
    for (const [name, value] of Object.entries(saved ?? {})) {
      if (Object.hasOwn(this.compiled.initialState, name)) this.overrides[name] = value;
    }
    this.state = mergeState(this.compiled.initialState, this.overrides);
  }

  update(patch) {
    for (const [name, value] of Object.entries(patch)) {
      if (Object.hasOwn(this.compiled.initialState, name)) this.overrides[name] = value;
    }
    this.state = mergeState(this.compiled.initialState, this.overrides);
    saveOverrides(this.key, stateDiff(this.compiled.initialState, this.state));
    this.render();
  }

  reset() {
    this.overrides = {};
    this.state = { ...this.compiled.initialState };
    saveOverrides(this.key, undefined);
    this.render();
  }

  get changed() {
    return stateDiff(this.compiled.initialState, this.state) !== undefined;
  }

  render() {
    const resolved = resolveBlock(this.compiled, this.state);
    const root = renderNode(resolved.root, this, resolved, 0);
    if (this.container.firstChild) this.container.replaceChildren(root);
    else this.container.appendChild(root);
  }
}

// ---- node rendering ----

const SVG_NS = 'http://www.w3.org/2000/svg';

function el(tag, className, children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (children) for (const child of children) if (child) node.appendChild(child);
  return node;
}

function svgEl(tag, attrs) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, String(value));
  return node;
}

function toneClass(base, tone) {
  return `${base} dx-tone-${tone ?? 'default'}`;
}

function renderNode(node, block, resolved, depth) {
  if (depth > 12) return el('span', 'dx-live-notice', '…');
  const component = node.component;
  if (PRIMITIVES.has(component)) {
    try {
      return PRIMITIVES.get(component)(node, block, resolved, depth);
    } catch {
      return el('span', 'dx-live-notice', '');
    }
  }
  return renderCatalogNode(node, block, resolved, depth);
}

// A catalog component inside a live block: per-point formulas are expanded,
// then it renders through the registry as usual.
function renderCatalogNode(node, block, resolved, depth) {
  const builder = block.options.renderers?.get?.(node.component) ?? block.options.renderers?.[node.component];
  if (!builder) return el('p', 'dx-live-notice', `未知组件 ${node.component}`);
  const props = {};
  for (const [key, value] of Object.entries(node.props)) {
    props[key] = expandValue(value, resolved.values);
  }
  const holder = el('div', 'dx-live-catalog');
  try {
    holder.appendChild(builder(props));
  } catch {
    return el('p', 'dx-live-notice', '组件参数无效');
  }
  return holder;
}

// A point-template over a list becomes a plain array of evaluated values;
// elsewhere it is left for the component to evaluate per point.
function expandValue(value, values) {
  if (isPointTemplate(value)) return value; // components evaluate per point
  if (Array.isArray(value)) return value.map((item) => expandValue(item, values));
  if (typeof value === 'object' && value !== null) {
    const result = {};
    for (const [key, field] of Object.entries(value)) result[key] = expandValue(field, values);
    return result;
  }
  return value;
}

// ---- primitives ----

// A control names its state either as `bind` or as a `state` prop
// (the catalog format the model sees); both mean "read block.state[name]".
function controlBind(node) {
  if (typeof node.bind === 'string' && node.bind !== '') return node.bind;
  const s = node.props.state;
  return typeof s === 'string' && s !== '' ? s : undefined;
}

const PRIMITIVES = new Map();

// container: panel, stack, row, grid, card, tabs, collapse
// leaf: divider, text, stat, badge, progress, callout, formula, scene
// controls: slider, select, segmented, switch, input, button

PRIMITIVES.set('panel', (node, block, resolved, depth) => {
  const root = node.path === '';
  const title = readText(node.props, 'title');
  const description = readText(node.props, 'description');
  const showReset = root && block.changed;
  const section = el('section', root ? 'dx-panel dx-panel-root' : 'dx-panel');
  if (title || showReset) {
    const head = el('div', 'dx-panel-head');
    head.appendChild(el('h3', 'dx-panel-title', []));
    head.firstChild.textContent = title ?? '';
    if (showReset) {
      const button = el('button', 'dx-reset');
      button.type = 'button';
      button.textContent = '重置';
      button.addEventListener('click', () => block.reset());
      head.appendChild(button);
    }
    section.appendChild(head);
  }
  if (description) section.appendChild(el('p', 'dx-panel-desc', [el('span', '', [document.createTextNode(description)])]));
  const flow = renderFlow(node, block, resolved, depth);
  if ((title || description || showReset)) flow.classList.add('dx-panel-flow-gap');
  section.appendChild(flow);
  return section;
});

const VISUAL_PRIMITIVES = new Set(['scene']);

// The children of a panel or card. A diagram next to formulas or controls
// becomes an explorable: the diagram on the own tile, everything else left.
function renderFlow(node, block, resolved, depth) {
  const visuals = node.children.filter((child) => VISUAL_PRIMITIVES.has(child.component));
  const asks = node.children.filter((child) => child.component === 'button' && typeof child.props.ask === 'string');
  const main = node.children.filter((child) => !visuals.includes(child) && !asks.includes(child));
  const wrap = el('div', 'dx-flow');
  if (visuals.length === 0 || main.length === 0) {
    for (const child of node.children) wrap.appendChild(renderNode(child, block, resolved, depth + 1));
    return wrap;
  }
  const columns = el('div', 'dx-flow-columns');
  const left = el('div', 'dx-flow-main');
  for (const child of main) left.appendChild(renderNode(child, block, resolved, depth + 1));
  const right = el('div', 'dx-flow-visual');
  for (const child of visuals) right.appendChild(renderNode(child, block, resolved, depth + 1));
  columns.appendChild(left);
  columns.appendChild(right);
  wrap.appendChild(columns);
  for (const child of asks) wrap.appendChild(renderNode(child, block, resolved, depth + 1));
  return wrap;
}

PRIMITIVES.set('stack', (node, block, resolved, depth) => {
  const box = el('div', 'dx-stack');
  for (const child of node.children) box.appendChild(renderNode(child, block, resolved, depth + 1));
  return box;
});

PRIMITIVES.set('row', (node, block, resolved, depth) => {
  const box = el('div', 'dx-row');
  for (const child of node.children) box.appendChild(renderNode(child, block, resolved, depth + 1));
  return box;
});

const GRID_COLUMNS = { 1: 'dx-grid-1', 2: 'dx-grid-2', 3: 'dx-grid-3', 4: 'dx-grid-4' };

PRIMITIVES.set('grid', (node, block, resolved, depth) => {
  const columns = Math.round(readNumber(node.props, 'columns') ?? 0);
  const box = el('div', `dx-grid ${GRID_COLUMNS[columns] ?? 'dx-grid-auto'}`);
  for (const child of node.children) box.appendChild(renderNode(child, block, resolved, depth + 1));
  return box;
});

PRIMITIVES.set('card', (node, block, resolved, depth) => {
  const card = el('div', 'dx-card');
  const title = readText(node.props, 'title');
  if (title) card.appendChild(el('p', 'dx-card-title', [document.createTextNode(title)]));
  card.appendChild(renderFlow(node, block, resolved, depth));
  return card;
});

PRIMITIVES.set('tabs', (node, block, resolved, depth) => {
  const options = readOptions(node.props, 'items');
  const box = el('div', 'dx-tabs');
  const list = el('div', 'dx-tablist');
  list.setAttribute('role', 'tablist');
  let active;
  if (node.bind) {
    active = Math.max(0, optionIndex(options, block.state[node.bind]));
  } else {
    const stored = block.local.get(`tab:${node.path}`);
    active = Math.min(typeof stored === 'number' ? stored : 0, Math.max(0, options.length - 1));
  }
  options.forEach((option, index) => {
    const chip = el('button', `dx-chip${index === active ? ' dx-chip-active' : ''}`, [document.createTextNode(option.label)]);
    chip.type = 'button';
    chip.setAttribute('role', 'tab');
    chip.addEventListener('click', () => {
      if (node.bind) block.update({ [node.bind]: option.value });
      else { block.local.set(`tab:${node.path}`, index); block.render(); }
    });
    list.appendChild(chip);
  });
  box.appendChild(list);
  const child = node.children[active];
  if (child) {
    const panel = el('div', 'dx-tabpanel');
    panel.setAttribute('role', 'tabpanel');
    panel.appendChild(renderNode(child, block, resolved, depth + 1));
    box.appendChild(panel);
  }
  return box;
});

PRIMITIVES.set('collapse', (node, block, resolved, depth) => {
  const key = `collapse:${node.path}`;
  const open = block.local.get(key) ?? (readBoolean(node.props, 'open') ?? false);
  const box = el('div', 'dx-collapse');
  const button = el('button', `dx-collapse-head${open ? ' open' : ''}`, [
    el('span', 'dx-collapse-title', [document.createTextNode(readText(node.props, 'title') ?? '')]),
    el('span', 'dx-chevron', [document.createTextNode('▾')]),
  ]);
  button.type = 'button';
  button.setAttribute('aria-expanded', String(open));
  button.addEventListener('click', () => { block.local.set(key, !open); block.render(); });
  box.appendChild(button);
  if (open) {
    const body = el('div', 'dx-collapse-body');
    for (const child of node.children) body.appendChild(renderNode(child, block, resolved, depth + 1));
    box.appendChild(body);
  }
  return box;
});

PRIMITIVES.set('divider', () => el('hr', 'dx-divider'));

PRIMITIVES.set('text', (node) => {
  const text = displayValue(node.props.text);
  if (!text) return el('span');
  const tone = readEnum(node.props, 'tone', LIVE_TONES) ?? 'default';
  const size = readEnum(node.props, 'size', TEXT_SIZES) ?? 'base';
  const p = el('p', toneClass('dx-text', tone) + ` dx-text-${size}`);
  p.textContent = text;
  return p;
});

PRIMITIVES.set('stat', (node) => {
  const box = el('div', 'dx-stat');
  if (readBoolean(node.props, 'highlight')) box.classList.add('dx-stat-highlight');
  const label = readText(node.props, 'label');
  if (label) box.appendChild(el('p', 'dx-stat-label', [document.createTextNode(label)]));
  const tone = readEnum(node.props, 'tone', LIVE_TONES) ?? 'default';
  const value = node.props.value;
  const precision = readNumber(node.props, 'precision');
  const display = value === undefined
    ? '—'
    : typeof value === 'number' && precision !== undefined
      ? formatFixed(value, precision)
      : displayValue(value);
  const row = el('p', toneClass('dx-stat-value', tone));
  row.appendChild(document.createTextNode(display));
  const unit = readText(node.props, 'unit');
  if (unit) row.appendChild(el('span', 'dx-stat-unit', [document.createTextNode(unit)]));
  box.appendChild(row);
  const note = readText(node.props, 'note');
  if (note) box.appendChild(el('p', 'dx-stat-note', [document.createTextNode(note)]));
  return box;
});

function formatFixed(value, precision) {
  if (!Number.isFinite(value)) return '—';
  const digits = Math.min(6, Math.max(0, Math.round(precision)));
  return new Intl.NumberFormat('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value);
}

PRIMITIVES.set('badge', (node) => {
  const text = displayValue(node.props.text ?? node.props.label);
  const span = el('span', 'dx-badge');
  if (!text) return span;
  span.textContent = text;
  span.className = toneClass('dx-badge', readEnum(node.props, 'tone', LIVE_TONES) ?? 'default');
  return span;
});

PRIMITIVES.set('progress', (node) => {
  const max = readNumber(node.props, 'max') ?? 100;
  const value = readNumber(node.props, 'value') ?? 0;
  const ratio = max > 0 ? Math.min(1, Math.max(0, value / max)) : 0;
  const box = el('div', 'dx-progress');
  const head = el('div', 'dx-progress-head');
  head.appendChild(el('span', 'dx-progress-label', [document.createTextNode(readText(node.props, 'label') ?? '')]));
  head.appendChild(el('span', 'dx-progress-pct', [document.createTextNode(`${Math.round(ratio * 100)}%`)]));
  box.appendChild(head);
  const track = el('div', 'dx-progress-track');
  track.setAttribute('role', 'progressbar');
  track.setAttribute('aria-valuemin', '0');
  track.setAttribute('aria-valuemax', String(max));
  track.setAttribute('aria-valuenow', String(value));
  const fill = el('div', 'dx-progress-fill');
  fill.style.width = `${ratio * 100}%`;
  track.appendChild(fill);
  box.appendChild(track);
  return box;
});

const CALLOUT_TONES = ['info', 'positive', 'warning', 'negative'];

PRIMITIVES.set('callout', (node) => {
  const tone = readEnum(node.props, 'tone', CALLOUT_TONES) ?? 'info';
  const box = el('div', toneClass('dx-callout', tone));
  box.appendChild(el('span', 'dx-callout-icon', [document.createTextNode(tone === 'warning' ? '⚠' : tone === 'negative' ? '✕' : 'ℹ')]));
  const body = el('div', 'dx-callout-body');
  const title = readText(node.props, 'title');
  if (title) body.appendChild(el('p', 'dx-callout-title', [document.createTextNode(title)]));
  const text = displayValue(node.props.text);
  if (text) body.appendChild(el('p', 'dx-callout-text', [document.createTextNode(text)]));
  box.appendChild(body);
  return box;
});

// ---- formula: name = expr, then the worked line with current values ----

PRIMITIVES.set('formula', (node, block, resolved) => {
  const expr = readText(node.props, 'expr');
  const box = el('div', 'dx-formula');
  if (!expr) return box;
  let latex;
  try {
    latex = formulaToLatex(expr, {
      name: readText(node.props, 'name'),
      unit: readText(node.props, 'unit'),
      precision: readNumber(node.props, 'precision'),
      values: resolved.values,
      with: node.props.with,
    });
  } catch {
    return box;
  }
  const label = readText(node.props, 'label');
  if (label) box.appendChild(el('p', 'dx-formula-label', [document.createTextNode(label)]));
  box.appendChild(katexLine(latex.symbolic, 'dx-formula-symbolic'));
  if (latex.worked) box.appendChild(katexLine(latex.worked, 'dx-formula-worked'));
  return box;
});

function katexLine(latex, className) {
  const span = el('span', className);
  span.innerHTML = katex.renderToString(latex, { throwOnError: false, output: 'html', strict: 'ignore' });
  return span;
}

// ---- scene: a small SVG diagram whose geometry follows the state ----

const MAX_SHAPES = 60;
const MAX_SIZE = 1000;
const LABEL_MAX_LENGTH = 40;
const LABEL_SIZE = 12;
const LABEL_GAP = 8;

const TONE_STROKE = {
  default: 'stroke-55', muted: 'stroke-muted', positive: 'stroke-pos',
  negative: 'stroke-neg', warning: 'stroke-warn', info: 'stroke-info',
};
const TONE_FILL = {
  default: 'fill-5', muted: 'fill-muted', positive: 'fill-pos',
  negative: 'fill-neg', warning: 'fill-warn', info: 'fill-info',
};
const TONE_TEXT = {
  default: 'svg-text-80', muted: 'svg-text-muted', positive: 'svg-text-pos',
  negative: 'svg-text-neg', warning: 'svg-text-warn', info: 'svg-text-info',
};
const TONE_MARKER = TONE_STROKE;

PRIMITIVES.set('scene', (node) => {
  const markerID = `dx-scene-arrow-${Math.random().toString(36).slice(2, 10)}`;
  const width = clampSize(readNumber(node.props, 'width') ?? 400);
  const height = clampSize(readNumber(node.props, 'height') ?? 240);
  const shapes = (Array.isArray(node.props.shapes) ? node.props.shapes : [])
    .filter((shape) => typeof shape === 'object' && shape !== null)
    .slice(0, MAX_SHAPES);
  const box = el('div', 'dx-scene');
  const svg = svgEl('svg', { width, height, viewBox: `0 0 ${width} ${height}`, class: 'dx-scene-svg', role: 'img' });
  const label = readText(node.props, 'label');
  if (label) svg.setAttribute('aria-label', label);
  const defs = svgEl('defs', {});
  const usedTones = new Set(shapes.filter((shape) => shape.type === 'arrow').map((shape) => readEnum(shape, 'tone', LIVE_TONES) ?? 'default'));
  for (const tone of usedTones) {
    const marker = svgEl('marker', {
      id: `${markerID}-${tone}`, viewBox: '0 0 10 10', refX: 9, refY: 5,
      markerWidth: 9, markerHeight: 9, markerUnits: 'userSpaceOnUse', orient: 'auto-start-reverse',
    });
    const path = svgEl('path', { d: 'M 0 1 L 10 5 L 0 9 z', class: TONE_MARKER[tone] });
    marker.appendChild(path);
    defs.appendChild(marker);
  }
  svg.appendChild(defs);
  for (const shape of shapes) svg.appendChild(renderShape(shape, markerID));
  box.appendChild(svg);
  return box;
});

function renderShape(shape, markerID) {
  const tone = readEnum(shape, 'tone', LIVE_TONES) ?? 'default';
  const label = shapeText(shape.label);
  const n = (key) => finite(shape[key]);
  switch (shape.type) {
    case 'rect': {
      const [x, y, w, h] = [n('x'), n('y'), n('w') ?? n('width'), n('h') ?? n('height')];
      if (x === undefined || y === undefined || w === undefined || h === undefined) return svgEl('g', {});
      const left = Math.min(x, x + w);
      const top = Math.min(y, y + h);
      const g = svgEl('g', {});
      g.appendChild(svgEl('rect', {
        x: left, y: top, width: Math.abs(w), height: Math.abs(h), rx: 8,
        class: `${TONE_FILL[tone]} ${TONE_STROKE[tone]}`, 'stroke-width': 1.25,
      }));
      if (label) {
        const text = svgEl('text', { x: left + Math.abs(w) / 2, y: top + Math.abs(h) / 2, 'text-anchor': 'middle', 'dominant-baseline': 'central', 'font-size': LABEL_SIZE, class: TONE_TEXT[tone] });
        text.textContent = label;
        g.appendChild(text);
      }
      return g;
    }
    case 'circle': {
      const [cx, cy, r] = [n('cx') ?? n('x'), n('cy') ?? n('y'), n('r')];
      if (cx === undefined || cy === undefined || r === undefined) return svgEl('g', {});
      const g = svgEl('g', {});
      g.appendChild(svgEl('circle', { cx, cy, r: Math.abs(r), class: `${TONE_FILL[tone]} ${TONE_STROKE[tone]}`, 'stroke-width': 1.25 }));
      if (label) {
        const text = svgEl('text', { x: cx, y: cy, 'text-anchor': 'middle', 'dominant-baseline': 'central', 'font-size': LABEL_SIZE, class: TONE_TEXT[tone] });
        text.textContent = label;
        g.appendChild(text);
      }
      return g;
    }
    case 'line':
    case 'arrow': {
      const [x1, y1, x2, y2] = [n('x1'), n('y1'), n('x2'), n('y2')];
      if (x1 === undefined || y1 === undefined || x2 === undefined || y2 === undefined) return svgEl('g', {});
      const arrow = shape.type === 'arrow';
      const placement = lineLabelPlacement(x1, y1, x2, y2);
      const g = svgEl('g', {});
      const line = svgEl('line', {
        x1, y1, x2, y2, class: TONE_STROKE[tone],
        'stroke-width': arrow ? 2 : 1.25, 'stroke-linecap': 'round',
      });
      if (arrow) line.setAttribute('marker-end', `url(#${markerID}-${tone})`);
      g.appendChild(line);
      if (label) {
        const text = svgEl('text', { x: placement.x, y: placement.y, 'text-anchor': placement.anchor, 'dominant-baseline': placement.baseline, 'font-size': LABEL_SIZE, class: TONE_TEXT[tone] });
        text.textContent = label;
        g.appendChild(text);
      }
      return g;
    }
    case 'text': {
      const [x, y] = [n('x'), n('y')];
      const text = label || shapeText(shape.text);
      if (x === undefined || y === undefined || !text) return svgEl('g', {});
      const node = svgEl('text', { x, y, 'text-anchor': 'middle', 'dominant-baseline': 'central', 'font-size': LABEL_SIZE, class: TONE_TEXT[tone] });
      node.textContent = text;
      return node;
    }
    default:
      return svgEl('g', {});
  }
}

// A line's label sits beside its midpoint, off the line.
function lineLabelPlacement(x1, y1, x2, y2) {
  const length = Math.hypot(x2 - x1, y2 - y1) || 1;
  let nx = -(y2 - y1) / length;
  let ny = (x2 - x1) / length;
  if (nx < -0.01 || (Math.abs(nx) <= 0.01 && ny < 0)) { nx = -nx; ny = -ny; }
  return {
    x: (x1 + x2) / 2 + nx * LABEL_GAP,
    y: (y1 + y2) / 2 + ny * LABEL_GAP,
    anchor: nx > 0.3 ? 'start' : nx < -0.3 ? 'end' : 'middle',
    baseline: ny > 0.3 ? 'hanging' : ny < -0.3 ? 'auto' : 'central',
  };
}

function shapeText(value) {
  if (typeof value === 'number') return Number.isFinite(value) ? String(Number(value.toFixed(2))) : '';
  return typeof value === 'string' ? value.slice(0, LABEL_MAX_LENGTH) : '';
}

function finite(value) {
  const number = typeof value === 'string' && value.trim() !== '' ? Number(value) : value;
  return typeof number === 'number' && Number.isFinite(number) ? number : undefined;
}

function clampSize(value) {
  return Math.min(MAX_SIZE, Math.max(40, value));
}

// ---- controls: they write to the state named by `bind` ----

PRIMITIVES.set('slider', (node, block) => {
  const bind = controlBind(node);
  if (!bind) return el('span');
  const min = readNumber(node.props, 'min') ?? 0;
  const declaredMax = readNumber(node.props, 'max') ?? 100;
  const max = declaredMax > min ? declaredMax : min + 1;
  const declaredStep = readNumber(node.props, 'step');
  const step = declaredStep !== undefined && declaredStep > 0 ? declaredStep : max - min >= 10 ? 1 : (max - min) / 100;
  const current = block.state[bind];
  const value = typeof current === 'number' && Number.isFinite(current) ? Math.min(max, Math.max(min, current)) : min;
  const box = el('div', 'dx-control');
  box.appendChild(controlLabel(readText(node.props, 'label'), `${displayValue(value)}${readText(node.props, 'unit') ? ` ${readText(node.props, 'unit')}` : ''}`));
  const input = el('input', 'dx-slider');
  input.type = 'range';
  input.min = String(min);
  input.max = String(max);
  input.step = String(step);
  input.value = String(value);
  input.addEventListener('input', () => block.update({ [bind]: Number(input.value) }));
  box.appendChild(input);
  return box;
});

PRIMITIVES.set('select', (node, block) => {
  const bind = controlBind(node);
  const options = readOptions(node.props, 'options');
  if (!bind || options.length === 0) return el('span');
  const box = el('div', 'dx-control');
  box.appendChild(controlLabel(readText(node.props, 'label'), undefined));
  const select = el('select', 'dx-select');
  const index = optionIndex(options, block.state[bind]);
  options.forEach((option, i) => {
    const item = el('option', '', [document.createTextNode(option.label)]);
    item.value = String(i);
    if (i === index) item.selected = true;
    select.appendChild(item);
  });
  select.addEventListener('change', () => {
    const option = options[Number(select.value)];
    if (option) block.update({ [bind]: option.value });
  });
  box.appendChild(select);
  return box;
});

PRIMITIVES.set('segmented', (node, block) => {
  const bind = controlBind(node);
  const options = readOptions(node.props, 'options');
  if (!bind || options.length === 0) return el('span');
  const box = el('div', 'dx-control');
  const label = readText(node.props, 'label');
  if (label) box.appendChild(el('p', 'dx-control-label', [document.createTextNode(label)]));
  const list = el('div', 'dx-segmented');
  const index = optionIndex(options, block.state[bind]);
  options.forEach((option, i) => {
    const chip = el('button', `dx-chip${i === index ? ' dx-chip-active' : ''}`, [document.createTextNode(option.label)]);
    chip.type = 'button';
    chip.addEventListener('click', () => block.update({ [bind]: option.value }));
    list.appendChild(chip);
  });
  box.appendChild(list);
  return box;
});

PRIMITIVES.set('switch', (node, block) => {
  const bind = controlBind(node);
  if (!bind) return el('span');
  const box = el('div', 'dx-switch-row');
  box.appendChild(el('label', 'dx-switch-label', [document.createTextNode(readText(node.props, 'label') ?? '')]));
  const input = el('input', 'dx-switch');
  input.type = 'checkbox';
  input.checked = block.state[bind] === true;
  input.addEventListener('change', () => block.update({ [bind]: input.checked }));
  box.appendChild(input);
  return box;
});

PRIMITIVES.set('input', (node, block) => {
  const bind = controlBind(node);
  const numeric = readEnum(node.props, 'type', ['text', 'number']) === 'number';
  if (!bind) return el('span');
  const box = el('div', 'dx-control');
  box.appendChild(controlLabel(readText(node.props, 'label'), undefined));
  const input = el('input', 'dx-input');
  input.type = 'text';
  if (numeric) input.setAttribute('inputmode', 'decimal');
  input.maxLength = 200;
  const placeholder = readText(node.props, 'placeholder');
  if (placeholder) input.placeholder = placeholder;
  const current = block.state[bind];
  input.value = typeof current === 'number' ? String(current) : typeof current === 'string' ? current : '';
  input.addEventListener('input', () => {
    if (!numeric) { block.update({ [bind]: input.value }); return; }
    const text = input.value;
    const value = Number(text);
    if (text.trim() !== '' && Number.isFinite(value)) block.update({ [bind]: value });
  });
  box.appendChild(input);
  return box;
});

function controlLabel(label, value) {
  const box = el('div', 'dx-control-head');
  box.appendChild(el('label', 'dx-control-label', [document.createTextNode(label ?? '')]));
  if (value !== undefined) box.appendChild(el('span', 'dx-control-value', [document.createTextNode(value)]));
  return box;
}

// `ask` becomes a follow-up suggestion (via the host's ask hook), `set`
// changes block state, `copy` copies text; without an action: disabled.
PRIMITIVES.set('button', (node, block) => {
  const action = readAction(node.props, new Set(Object.keys(block.compiled.initialState)));
  const label = readText(node.props, 'label') ?? '';
  const variant = readEnum(node.props, 'variant', ['default', 'outline']) ?? 'default';

  if (action?.kind === 'ask') {
    const box = el('div', 'dx-ask');
    const text = el('p', 'dx-ask-text', [document.createTextNode(action.text)]);
    box.appendChild(text);
    const send = el('button', 'dx-ask-send', [document.createTextNode('发送')]);
    send.type = 'button';
    send.addEventListener('click', () => block.options.ask?.(action.text));
    box.appendChild(send);
    return box;
  }

  const button = el('button', variant === 'outline' ? 'dx-button dx-button-outline' : 'dx-button');
  button.type = 'button';
  if (action?.kind === 'copy') {
    button.appendChild(el('span', 'dx-button-icon', [document.createTextNode('⧉')]));
    button.appendChild(el('span', '', [document.createTextNode(label || '复制')]));
    button.addEventListener('click', () => {
      try {
        navigator.clipboard?.writeText(action.text);
        button.classList.add('dx-copied');
        setTimeout(() => button.classList.remove('dx-copied'), 1200);
      } catch { /* clipboard denied: nothing to do */ }
    });
  } else if (action?.kind === 'set') {
    const patch = action.patch;
    button.appendChild(el('span', '', [document.createTextNode(label)]));
    button.addEventListener('click', () => block.update(patch));
  } else {
    button.appendChild(el('span', '', [document.createTextNode(label)]));
    button.disabled = true;
  }
  return button;
});

// ---- entry ----

// Renders a parsed live block into `container`. Returns the LiveBlock
// instance (or null when the body is not a block at all).
export function mountLiveBlock(body, container, options = {}) {
  const parsed = parseBlockSource(body);
  if (parsed.status !== 'ok') return null;
  const block = new LiveBlock(parsed, options);
  block.container = container;
  block.render();
  return block;
}

// True when the fenced body declares a live-capable block (state/computed/
// children) whose root is a known primitive or catalog component.
export function looksLikeLiveBlock(body) {
  const parsed = parseBlockSource(body);
  return parsed.status === 'ok' && parsed.block.live;
}
