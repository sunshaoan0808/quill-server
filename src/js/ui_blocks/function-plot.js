/**
 * function-plot — DEEIX FunctionPlotProps (vanilla port, no katex).
 *
 * Props: { title?, functions: [{ expr }], x?: [min,max], y?: [min,max] }
 *
 * Curve syntax per row (DEEIX parseCurve):
 *   expr                → y = expr        (sampled along x)
 *   y = expr / f(x)=…   → explicit along x
 *   x = expr            → explicit along y
 *   lhs = rhs           → implicit, zero set traced with marching squares
 *
 * Interactions: wheel zoom at cursor, drag pan, +/-/reset buttons, per-row
 * eye toggle and inline expression editing (recompiles live).
 */
import { compileExpression } from './expression.js';

const MAX_FUNCTIONS = 8;
const PLOT_HEIGHT = 280;
const DEFAULT_X = [-5, 5];
const SERIES_COLORS = ['#4a9eff', '#47c97e', '#22c55e', '#0ea5e9', '#eab308', '#ec4899', '#8b5cf6', '#14b8a6'];
const ZOOM_STEP = 1.5;
const WHEEL_SENSITIVITY = 0.002;
const MIN_SPAN = 1e-4;
const MAX_SPAN = 1e6;
const IMPLICIT_CELL = 4;
const NS = 'http://www.w3.org/2000/svg';

function svgEl(tag, attrs) {
  const n = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs || {})) n.setAttribute(k, String(v));
  return n;
}

function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = String(text);
  return n;
}

// ---- curve parsing ----

function assertVariables(compiled, allowed) {
  const foreign = compiled.variables.find((name) => !allowed.includes(name));
  if (foreign) throw new Error(`unknown variable ${foreign}`);
}

export function parseCurve(text) {
  const parts = String(text).split('=');
  if (parts.length > 2) throw new Error('only one = allowed');
  if (parts.length === 1) {
    const fn = compileExpression(text);
    assertVariables(fn, ['x']);
    return { kind: 'explicit-y', fn };
  }
  const lhs = (parts[0] || '').trim();
  const rhs = (parts[1] || '').trim();
  if (lhs === '' || rhs === '') throw new Error('incomplete equation');
  if (lhs === 'y' || /^[a-zA-Z]\(x\)$/.test(lhs)) {
    const fn = compileExpression(rhs);
    assertVariables(fn, ['x']);
    return { kind: 'explicit-y', fn };
  }
  if (lhs === 'x') {
    const fn = compileExpression(rhs);
    assertVariables(fn, ['y']);
    return { kind: 'explicit-x', fn };
  }
  const fn = compileExpression(`(${lhs}) - (${rhs})`);
  assertVariables(fn, ['x', 'y']);
  return { kind: 'implicit', fn };
}

function classifyError(error) {
  const message = error instanceof Error ? error.message : String(error);
  const variable = /^unknown variable (\S+)/.exec(message);
  if (variable && variable[1]) return { kind: 'variable', name: variable[1] };
  const fn = /^unknown function (\S+)/.exec(message);
  if (fn && fn[1]) return { kind: 'function', name: fn[1] };
  return { kind: 'invalid' };
}

// ---- viewport math ----

function rangeOr(range, fallback) {
  if (Array.isArray(range) && range.length === 2) {
    const a = Number(range[0]);
    const b = Number(range[1]);
    if (Number.isFinite(a) && Number.isFinite(b) && a < b) return [a, b];
  }
  return [fallback[0], fallback[1]];
}

function initialViewport(props, width) {
  const [xMin, xMax] = rangeOr(props && props.x, DEFAULT_X);
  if (props && props.y) {
    const [yMin, yMax] = rangeOr(props.y, DEFAULT_X);
    return { xMin, xMax, yMin, yMax };
  }
  const half = ((xMax - xMin) * PLOT_HEIGHT) / Math.max(width, 1) / 2;
  return { xMin, xMax, yMin: -half, yMax: half };
}

// 1 / 2 / 5 × 10^n step that yields roughly `target` grid lines across `span`.
function niceStep(span, target) {
  const rough = span / target;
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  const residual = rough / magnitude;
  const factor = residual >= 5 ? 5 : residual >= 2 ? 2 : 1;
  return factor * magnitude;
}

function ticks(min, max, step) {
  const out = [];
  const start = Math.ceil(min / step) * step;
  for (let value = start; value <= max + step / 2; value += step) {
    out.push(Math.abs(value) < step / 1e6 ? 0 : value);
  }
  return out;
}

function formatTick(value, step) {
  const digits = Math.max(0, -Math.floor(Math.log10(step)));
  return value.toFixed(Math.min(digits, 6)).replace(/\.?0+$/, '');
}

function safeEvaluate(fn, variables) {
  try {
    return fn.evaluate(variables);
  } catch {
    return Number.NaN;
  }
}

// ---- path building (DEEIX algorithms, verbatim semantics) ----

// Samples one value per pixel along the independent axis and splits the path
// wherever the value is non-finite or jumps by more than twice the visible
// span (poles, tan, 1/x).
function buildExplicitPath(fn, view, width, height, alongX) {
  const parts = [];
  const steps = alongX ? width : height;
  const inMin = alongX ? view.xMin : view.yMin;
  const inSpan = alongX ? view.xMax - view.xMin : view.yMax - view.yMin;
  const outMin = alongX ? view.yMin : view.xMin;
  const outSpan = alongX ? view.yMax - view.yMin : view.xMax - view.xMin;
  const outPixels = alongX ? height : width;
  const jumpLimit = outSpan * 2;
  let open = false;
  let previous = null;
  for (let step = 0; step <= steps; step += 1) {
    const input = inMin + (step / steps) * inSpan;
    const value = safeEvaluate(fn, alongX ? { x: input } : { y: input });
    if (!Number.isFinite(value) || (previous !== null && Math.abs(value - previous) > jumpLimit)) {
      open = false;
      previous = Number.isFinite(value) ? value : null;
      continue;
    }
    // Clamp far-off-screen points so the SVG path stays numerically sane.
    const clamped = Math.max(outMin - jumpLimit, Math.min(outMin + outSpan + jumpLimit, value));
    const outPx = ((clamped - outMin) / outSpan) * outPixels;
    const px = alongX ? step : outPx;
    const py = alongX ? height - outPx : height - step;
    parts.push(`${open ? 'L' : 'M'}${px.toFixed(1)} ${py.toFixed(1)}`);
    open = true;
    previous = value;
  }
  return parts.join('');
}

// Marching squares over a pixel grid: every cell whose corners change sign
// contributes the linearly interpolated crossing segment(s).
function buildImplicitPath(fn, view, width, height) {
  const cols = Math.ceil(width / IMPLICIT_CELL);
  const rows = Math.ceil(height / IMPLICIT_CELL);
  const values = new Float64Array((cols + 1) * (rows + 1));
  for (let row = 0; row <= rows; row += 1) {
    const y = view.yMax - (row / rows) * (view.yMax - view.yMin);
    for (let col = 0; col <= cols; col += 1) {
      const x = view.xMin + (col / cols) * (view.xMax - view.xMin);
      values[row * (cols + 1) + col] = safeEvaluate(fn, { x, y });
    }
  }
  const parts = [];
  const cellW = width / cols;
  const cellH = height / rows;
  const crossing = (a, b) => (a === b ? 0.5 : a / (a - b));
  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      const v0 = values[row * (cols + 1) + col];
      const v1 = values[row * (cols + 1) + col + 1];
      const v2 = values[(row + 1) * (cols + 1) + col + 1];
      const v3 = values[(row + 1) * (cols + 1) + col];
      if (!Number.isFinite(v0) || !Number.isFinite(v1) || !Number.isFinite(v2) || !Number.isFinite(v3)) continue;
      const x0 = col * cellW;
      const y0 = row * cellH;
      const points = [];
      if (v0 < 0 !== v1 < 0) points.push([x0 + crossing(v0, v1) * cellW, y0]);
      if (v1 < 0 !== v2 < 0) points.push([x0 + cellW, y0 + crossing(v1, v2) * cellH]);
      if (v3 < 0 !== v2 < 0) points.push([x0 + crossing(v3, v2) * cellW, y0 + cellH]);
      if (v0 < 0 !== v3 < 0) points.push([x0, y0 + crossing(v0, v3) * cellH]);
      for (let index = 0; index + 1 < points.length; index += 2) {
        const [ax, ay] = points[index];
        const [bx, by] = points[index + 1];
        parts.push(`M${ax.toFixed(1)} ${ay.toFixed(1)}L${bx.toFixed(1)} ${by.toFixed(1)}`);
      }
    }
  }
  return parts.join('');
}

function buildPath(curve, view, width, height) {
  switch (curve.kind) {
    case 'explicit-y': return buildExplicitPath(curve.fn, view, width, height, true);
    case 'explicit-x': return buildExplicitPath(curve.fn, view, width, height, false);
    case 'implicit': return buildImplicitPath(curve.fn, view, width, height);
    default: return '';
  }
}

// ---- component ----

export function buildFunctionPlot(props) {
  const p = props || {};
  const wrap = el('div', 'stui-fplot');
  if (p.title) wrap.appendChild(el('div', 'stui-gantt-title', p.title));

  const width = 640;
  const initial = initialViewport(p, width);
  let view = { ...initial };

  // rows: expression + hidden flag (colour = row index, stable like DEEIX)
  const rows = (Array.isArray(p.functions) ? p.functions : [])
    .slice(0, MAX_FUNCTIONS)
    .map((fn, index) => ({
      key: index,
      expr: String(fn && fn.expr != null ? fn.expr : ''),
      hidden: false,
      curve: null,
      error: null,
    }));

  const compileRow = (row) => {
    if (row.expr.trim() === '') {
      row.curve = null;
      row.error = null;
      return;
    }
    try {
      row.curve = parseCurve(row.expr);
      row.error = null;
    } catch (e) {
      row.curve = null;
      row.error = classifyError(e);
    }
  };
  for (const row of rows) compileRow(row);

  // toolbar
  const bar = el('div', 'stui-fplot-bar');
  const zoomBtn = (label, fn) => {
    const b = el('button', 'stui-fplot-btn', label);
    b.type = 'button';
    b.addEventListener('click', fn);
    return b;
  };

  const svg = svgEl('svg', {
    class: 'stui-fplot-svg', viewBox: `0 0 ${width} ${PLOT_HEIGHT}`,
    preserveAspectRatio: 'none', height: PLOT_HEIGHT,
  });
  const gGrid = svgEl('g', {});
  const gAxes = svgEl('g', {});
  const gCurves = svgEl('g', {});
  svg.appendChild(gGrid);
  svg.appendChild(gAxes);
  svg.appendChild(gCurves);

  const px = (x) => ((x - view.xMin) / (view.xMax - view.xMin)) * width;
  const py = (y) => PLOT_HEIGHT - ((y - view.yMin) / (view.yMax - view.yMin)) * PLOT_HEIGHT;

  const redraw = () => {
    gGrid.textContent = '';
    gAxes.textContent = '';
    gCurves.textContent = '';

    // grid + tick labels
    const xStep = niceStep(view.xMax - view.xMin, 8);
    const yStep = niceStep(view.yMax - view.yMin, 6);
    for (const t of ticks(view.xMin, view.xMax, xStep)) {
      const x = px(t);
      gGrid.appendChild(svgEl('line', { x1: x, y1: 0, x2: x, y2: PLOT_HEIGHT, class: 'stui-fplot-grid' }));
      const label = svgEl('text', { x, y: PLOT_HEIGHT - 4, class: 'stui-fplot-tick' });
      label.textContent = formatTick(t, xStep);
      gAxes.appendChild(label);
    }
    for (const t of ticks(view.yMin, view.yMax, yStep)) {
      const y = py(t);
      gGrid.appendChild(svgEl('line', { x1: 0, y1: y, x2: width, y2: y, class: 'stui-fplot-grid' }));
      const label = svgEl('text', { x: 4, y: y - 3, class: 'stui-fplot-tick' });
      label.textContent = formatTick(t, yStep);
      gAxes.appendChild(label);
    }
    // zero axes
    if (view.yMin < 0 && view.yMax > 0) {
      gAxes.appendChild(svgEl('line', { x1: 0, y1: py(0), x2: width, y2: py(0), class: 'stui-fplot-axis' }));
    }
    if (view.xMin < 0 && view.xMax > 0) {
      gAxes.appendChild(svgEl('line', { x1: px(0), y1: 0, x2: px(0), y2: PLOT_HEIGHT, class: 'stui-fplot-axis' }));
    }

    // curves
    rows.forEach((row, index) => {
      if (!row.curve || row.hidden) return;
      const d = buildPath(row.curve, view, width, PLOT_HEIGHT);
      if (!d) return;
      gCurves.appendChild(svgEl('path', {
        d, fill: 'none', stroke: SERIES_COLORS[index % SERIES_COLORS.length], 'stroke-width': 2,
      }));
    });
  };

  const clampSpan = (v) => Math.min(MAX_SPAN, Math.max(MIN_SPAN, v));
  const zoomAt = (factor, anchorPx, anchorPy) => {
    const fx = anchorPx == null ? 0.5 : anchorPx / width;
    const fy = anchorPy == null ? 0.5 : anchorPy / PLOT_HEIGHT;
    const xSpan = clampSpan((view.xMax - view.xMin) * factor);
    const ySpan = clampSpan((view.yMax - view.yMin) * factor);
    view = {
      xMin: view.xMin + (view.xMax - view.xMin - xSpan) * fx,
      xMax: view.xMin + (view.xMax - view.xMin - xSpan) * fx + xSpan,
      yMin: view.yMin + (view.yMax - view.yMin - ySpan) * (1 - fy),
      yMax: view.yMin + (view.yMax - view.yMin - ySpan) * (1 - fy) + ySpan,
    };
    redraw();
  };

  bar.appendChild(zoomBtn('−', () => zoomAt(ZOOM_STEP)));
  bar.appendChild(zoomBtn('+', () => zoomAt(1 / ZOOM_STEP)));
  bar.appendChild(zoomBtn('⌂', () => { view = { ...initial }; redraw(); }));
  bar.appendChild(el('span', 'stui-fplot-hint', '滚轮缩放 · 拖拽平移'));

  // wheel zoom at cursor
  svg.addEventListener('wheel', (ev) => {
    ev.preventDefault();
    const rect = svg.getBoundingClientRect();
    const ax = ((ev.clientX - rect.left) / Math.max(1, rect.width)) * width;
    const ay = ((ev.clientY - rect.top) / Math.max(1, rect.height)) * PLOT_HEIGHT;
    const factor = Math.exp(ev.deltaY * WHEEL_SENSITIVITY);
    zoomAt(Math.min(2, Math.max(0.5, factor)), ax, ay);
  }, { passive: false });

  // drag pan
  let drag = null;
  svg.addEventListener('pointerdown', (ev) => {
    drag = { x: ev.clientX, y: ev.clientY, view: { ...view } };
    svg.setPointerCapture(ev.pointerId);
    svg.classList.add('stui-fplot-grab');
  });
  svg.addEventListener('pointermove', (ev) => {
    if (!drag) return;
    const rect = svg.getBoundingClientRect();
    const dx = ((ev.clientX - drag.x) / Math.max(1, rect.width)) * (drag.view.xMax - drag.view.xMin);
    const dy = ((ev.clientY - drag.y) / Math.max(1, rect.height)) * (drag.view.yMax - drag.view.yMin);
    view = {
      xMin: drag.view.xMin - dx, xMax: drag.view.xMax - dx,
      yMin: drag.view.yMin + dy, yMax: drag.view.yMax + dy,
    };
    redraw();
  });
  const endDrag = () => { drag = null; svg.classList.remove('stui-fplot-grab'); };
  svg.addEventListener('pointerup', endDrag);
  svg.addEventListener('pointercancel', endDrag);

  // function rows: eye toggle + inline editing
  const list = el('div', 'stui-fplot-rows');
  const rebuildRows = () => {
    list.textContent = '';
    rows.forEach((row, index) => {
      const line = el('div', 'stui-fplot-row');
      line.style.setProperty('--plot-color', SERIES_COLORS[index % SERIES_COLORS.length]);
      const dot = el('span', 'stui-fplot-dot');
      dot.style.background = SERIES_COLORS[index % SERIES_COLORS.length];
      if (row.hidden) dot.classList.add('stui-fplot-dot-off');
      line.appendChild(dot);

      const input = document.createElement('input');
      input.type = 'text';
      input.className = 'stui-fplot-input';
      input.value = row.expr;
      input.placeholder = '如 y = sin(x) 或 x^2 + y^2 = 25';
      input.addEventListener('input', () => {
        row.expr = input.value;
        compileRow(row);
        errorLine.textContent = row.error
          ? (row.error.kind === 'variable' ? `未知变量 ${row.error.name}`
            : row.error.kind === 'function' ? `未知函数 ${row.error.name}` : '表达式无效')
          : '';
        errorLine.style.display = row.error ? '' : 'none';
        redraw();
      });
      line.appendChild(input);

      const eye = el('button', row.hidden ? 'stui-fplot-eye off' : 'stui-fplot-eye', row.hidden ? '○' : '●');
      eye.type = 'button';
      eye.title = row.hidden ? '显示' : '隐藏';
      eye.addEventListener('click', () => {
        row.hidden = !row.hidden;
        rebuildRows();
        redraw();
      });
      line.appendChild(eye);

      const errorLine = el('div', 'stui-fplot-rowerr', '');
      errorLine.style.display = row.error ? '' : 'none';
      if (row.error) {
        errorLine.textContent = row.error.kind === 'variable' ? `未知变量 ${row.error.name}`
          : row.error.kind === 'function' ? `未知函数 ${row.error.name}` : '表达式无效';
      }
      line.appendChild(errorLine);
      list.appendChild(line);
    });
  };

  wrap.appendChild(bar);
  wrap.appendChild(svg);
  wrap.appendChild(list);
  rebuildRows();
  redraw();
  return wrap;
}
