/**
 * calculator — DEEIX CalculatorProps (vanilla port).
 *
 * Props: { title?, description?,
 *   inputs:  [{ key, label, min, max, step?, default, unit? }],
 *   outputs: [{ label, expr, unit?, precision?, highlight? }] }
 *
 * Each output compiles once via the expression engine (expression.js) and
 * re-evaluates live while sliders move — the DEEIX live-panel core, in its
 * static calculator form.
 */
import { compileExpression, ExpressionError } from './expression.js';

function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = String(text);
  return n;
}

function formatValue(value, precision) {
  if (!Number.isFinite(value)) return '—';
  const p = precision == null ? null : Math.min(10, Math.max(0, Math.trunc(Number(precision))));
  if (p != null) return value.toFixed(p).replace(/\.?0+$/, '');
  // default: trim binary noise, keep up to 6 decimals
  if (Number.isInteger(value)) return String(value);
  return String(Math.round(value * 1e6) / 1e6);
}

export function buildCalculator(props) {
  const p = props || {};
  const wrap = el('div', 'stui-calc');
  if (p.title) wrap.appendChild(el('div', 'stui-gantt-title', p.title));
  if (p.description) wrap.appendChild(el('div', 'stui-calc-desc', p.description));

  const inputs = Array.isArray(p.inputs) ? p.inputs : [];
  const outputs = Array.isArray(p.outputs) ? p.outputs : [];
  if (!inputs.length && !outputs.length) {
    wrap.appendChild(el('div', 'stui-empty', '（计算器无输入输出）'));
    return wrap;
  }

  // current values + compiled outputs
  const values = {};
  const compiled = [];
  const compileErrors = [];
  for (const inp of inputs) {
    const def = Number(inp && inp.default);
    values[inp && inp.key != null ? String(inp.key) : ''] = Number.isFinite(def) ? def : 0;
  }
  for (const out of outputs) {
    try {
      compiled.push(compileExpression(String(out && out.expr != null ? out.expr : '')));
      compileErrors.push(null);
    } catch (e) {
      compiled.push(null);
      compileErrors.push(e instanceof Error ? e.message : String(e));
    }
  }

  const grid = el('div', 'stui-calc-grid');
  const outRow = el('div', 'stui-calc-outs');

  // live re-evaluation of every output from the shared value record
  const recompute = () => {
    outputs.forEach((out, i) => {
      const cell = outRow.children[i];
      if (!cell) return;
      const valueEl = cell.querySelector('.stui-calc-value');
      const errEl = cell.querySelector('.stui-calc-err');
      if (!compiled[i]) {
        valueEl.textContent = '—';
        errEl.textContent = compileErrors[i] || '公式错误';
        return;
      }
      try {
        const value = compiled[i].evaluate(values);
        valueEl.textContent = formatValue(value, out && out.precision)
          + (out && out.unit ? ` ${out.unit}` : '');
        errEl.textContent = '';
        cell.classList.toggle('stui-calc-hl', !!(out && out.highlight));
      } catch (e) {
        valueEl.textContent = '—';
        errEl.textContent = e instanceof ExpressionError ? e.message : '计算错误';
      }
    });
  };

  // inputs: slider + numeric box, both bound to the same record
  for (const inp of inputs) {
    if (!inp || typeof inp !== 'object') continue;
    const key = inp.key != null ? String(inp.key) : '';
    const min = Number(inp.min);
    const max = Number(inp.max);
    const step = inp.step != null && Number(inp.step) > 0
      ? Number(inp.step)
      : (Number.isFinite(min) && Number.isFinite(max) && max > min ? (max - min) / 100 : 1);
    const row = el('div', 'stui-calc-input');
    const head = el('div', 'stui-calc-ihead');
    head.appendChild(el('span', 'stui-calc-label', inp.label != null ? String(inp.label) : key));
    const readout = el('span', 'stui-calc-readout');
    head.appendChild(readout);
    row.appendChild(head);

    const slider = document.createElement('input');
    slider.type = 'range';
    slider.min = String(Number.isFinite(min) ? min : 0);
    slider.max = String(Number.isFinite(max) ? max : 100);
    slider.step = String(step);
    slider.value = String(values[key]);

    const number = document.createElement('input');
    number.type = 'number';
    number.min = slider.min;
    number.max = slider.max;
    number.step = slider.step;
    number.value = String(values[key]);
    number.className = 'stui-calc-num';

    const paint = (v, from) => {
      const n = Number(v);
      if (!Number.isFinite(n)) return;
      values[key] = n;
      if (from !== 'slider') slider.value = String(n);
      if (from !== 'number') number.value = String(n);
      readout.textContent = `${n}${inp.unit ? ` ${inp.unit}` : ''}`;
      recompute();
    };
    slider.addEventListener('input', () => paint(slider.value, 'slider'));
    number.addEventListener('input', () => paint(number.value, 'number'));

    row.appendChild(slider);
    row.appendChild(number);
    grid.appendChild(row);
    readout.textContent = `${values[key]}${inp.unit ? ` ${inp.unit}` : ''}`;
  }

  // outputs
  for (const out of outputs) {
    if (!out || typeof out !== 'object') continue;
    const cell = el('div', 'stui-calc-out');
    cell.appendChild(el('div', 'stui-calc-olabel', out.label != null ? String(out.label) : '结果'));
    cell.appendChild(el('div', 'stui-calc-value', '—'));
    cell.appendChild(el('div', 'stui-calc-err', ''));
    outRow.appendChild(cell);
  }

  if (inputs.length) wrap.appendChild(grid);
  if (outputs.length) wrap.appendChild(outRow);
  recompute();
  return wrap;
}
