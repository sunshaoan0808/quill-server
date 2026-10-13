// @vitest-environment jsdom
import { describe, it, it as test, expect } from 'vitest';
import {
  extractUIBlocks,
  stripUIBlockFences,
  scanUIBlockFences,
  replaceUIBlockFences,
  hashFenceBody,
} from '../src/js/ui_blocks/fence.js';
import { parseUiBlockSpec, rendererNames, hasRenderer, renderUiBlockBody } from '../src/js/ui_blocks/render.js';
import {
  parseBlockSource, compileBlock, resolveBlock, mergeState, stateDiff, readAction,
} from '../src/js/ui_blocks/block.js';
import { formulaToLatex } from '../src/js/ui_blocks/expression.js';
import { mountLiveBlock } from '../src/js/ui_blocks/live.js';

const CARD = '{"component":"card-grid","items":[{"title":"甲","body":"内容甲"}]}';
const STAT = '{"component":"stat-grid","items":[{"label":"HP","value":"88"}]}';

describe('extractUIBlocks — deeix-ui 围栏抽取', () => {
  it('抽出 deeix-ui 块，正文剥离围栏', () => {
    const text = `先看卡片：\n\`\`\`deeix-ui\n${CARD}\n\`\`\`\n结束。`;
    const { cleanText, blocks } = extractUIBlocks(text);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].language).toBe('deeix-ui');
    expect(blocks[0].rawBody).toBe(CARD);
    expect(cleanText).toBe('先看卡片：\n\n结束。');
  });

  it('普通代码围栏保持不动（非 UI 语言不吞）', () => {
    const text = '示例：\n```js\nconsole.log(1)\n```\n完了';
    const { cleanText, blocks } = extractUIBlocks(text);
    expect(blocks).toHaveLength(0);
    expect(cleanText).toBe(text);
  });

  it('deeix-widget 围栏也被抽取（沙箱 iframe 已落地）', () => {
    const text = '```deeix-widget\n<b>hi</b>\n```';
    const { cleanText, blocks } = extractUIBlocks(text);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].language).toBe('deeix-widget');
    expect(cleanText).toBe('');
  });

  it('多块按序抽取，hash 稳定且内容变则 hash 变', () => {
    const text = `A\n\`\`\`deeix-ui\n${CARD}\n\`\`\`\nB\n\`\`\`deeix-ui\n${STAT}\n\`\`\`\nC`;
    const { cleanText, blocks } = extractUIBlocks(text);
    expect(blocks).toHaveLength(2);
    expect(cleanText).toBe('A\n\nB\n\nC');
    expect(blocks[0].hash).not.toBe(blocks[1].hash);
    const again = extractUIBlocks(text);
    expect(again.blocks[0].hash).toBe(blocks[0].hash);
    const changed = extractUIBlocks(text.replace('"value":"88"', '"value":"89"'));
    expect(changed.blocks[1].hash).not.toBe(blocks[1].hash);
  });

  it('CRLF 文本：rawBody 与 cleanText 均处理', () => {
    const text = `行一\r\n\`\`\`deeix-ui\r\n${CARD}\r\n\`\`\`\r\n行二`;
    const { cleanText, blocks } = extractUIBlocks(text);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].rawBody).toBe(CARD);
    expect(cleanText.replace(/\r/g, '')).toBe('行一\n\n行二');
  });

  it('未闭合围栏（流式中间态）不抽，留在正文', () => {
    const text = `卡：\n\`\`\`deeix-ui\n${CARD.slice(0, 30)}`;
    const { cleanText, blocks } = extractUIBlocks(text);
    expect(blocks).toHaveLength(0);
    expect(cleanText).toBe(text);
  });
});

describe('strip / scan / replace', () => {
  it('stripUIBlockFences 只剥支持的围栏', () => {
    const text = `x\n\`\`\`deeix-ui\n${CARD}\n\`\`\`\ny\n\`\`\`py\n1\n\`\`\`\nz`;
    const out = stripUIBlockFences(text);
    expect(out).not.toContain('deeix-ui');
    expect(out).toContain('```py');
    expect(out).toContain('x');
    expect(out).toContain('z');
  });

  it('scanUIBlockFences 返回原文区间', () => {
    const text = '前\n```deeix-ui\n' + CARD + '\n```\n后';
    const { fences } = scanUIBlockFences(text);
    expect(fences).toHaveLength(1);
    expect(text.slice(fences[0].rawStart, fences[0].rawEnd)).toBe(fences[0].raw);
    expect(fences[0].language).toBe('deeix-ui');
  });

  it('replaceUIBlockFences 支持语言替换、普通围栏原样', () => {
    const text = '```deeix-ui\n' + CARD + '\n```\n```x\nkeep\n```';
    const out = replaceUIBlockFences(text, (f) => `<UI:${f.language}>`);
    expect(out).toBe('<UI:deeix-ui>\n```x\nkeep\n```');
  });
});

describe('parseUiBlockSpec — 载荷校验', () => {
  it('合法载荷 → name + spec', () => {
    const { name, spec } = parseUiBlockSpec({ language: 'deeix-ui', rawBody: CARD });
    expect(name).toBe('card-grid');
    expect(spec.items).toHaveLength(1);
  });

  it('JSON 坏 → 带位置的错误', () => {
    expect(() => parseUiBlockSpec({ language: 'deeix-ui', rawBody: '{"component":' }))
      .toThrow(/invalid JSON/);
  });

  it('非对象载荷 → 拒绝', () => {
    expect(() => parseUiBlockSpec({ language: 'deeix-ui', rawBody: '[1,2]' }))
      .toThrow(/not an object/);
    expect(() => parseUiBlockSpec({ language: 'deeix-ui', rawBody: '"str"' }))
      .toThrow(/not an object/);
  });

  it('缺 component 字段 → 拒绝', () => {
    expect(() => parseUiBlockSpec({ language: 'deeix-ui', rawBody: '{"foo":1}' }))
      .toThrow(/missing "component"/);
  });

  it('非 deeix-ui 语言 → 拒绝', () => {
    expect(() => parseUiBlockSpec({ language: 'other', rawBody: '{}' }))
      .toThrow(/unsupported block language/);
  });
});

describe('registry — PR2 静态组件注册', () => {
  it('注册了 10 个渲染器（6 静态 + chart/gantt/calculator/function-plot）', () => {
    expect(rendererNames().sort()).toEqual(
      ['card-grid', 'chart', 'data-table', 'decision-tree', 'diff', 'gantt', 'quiz', 'stat-grid',
        'calculator', 'function-plot'].sort(),
    );
  });

  it('hasRenderer 识别已知/未知', () => {
    expect(hasRenderer('calculator')).toBe(true);
    expect(hasRenderer('function-plot')).toBe(true);
    expect(hasRenderer('nope')).toBe(false);
    expect(hasRenderer('card-grid')).toBe(true);
  });
});

describe('hashFenceBody — 稳定与区分', () => {
  it('同输入同 hash，不同输入不同 hash', () => {
    expect(hashFenceBody('abc')).toBe(hashFenceBody('abc'));
    expect(hashFenceBody('abc')).not.toBe(hashFenceBody('abd'));
    expect(hashFenceBody('')).toHaveLength(8);
  });
});


describe('buildChartOption — ChartProps → ECharts option', () => {
  it('line 图：category x + 同名 series', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    const opt = buildChartOption({
      type: 'line',
      x: ['一', '二', '三'],
      series: [{ name: '销量', data: [3, 1, 4] }],
      unit: '件',
    });
    expect(opt.xAxis.data).toEqual(['一', '二', '三']);
    expect(opt.series).toHaveLength(1);
    expect(opt.series[0].type).toBe('line');
    expect(opt.series[0].data).toEqual([3, 1, 4]);
    expect(opt.title).toBeUndefined();
  });

  it('bar+双轴：右侧系列 yAxisIndex=1 且有第二个 y 轴', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    const opt = buildChartOption({
      title: '经营', type: 'bar',
      series: [
        { name: '收入', data: [1, 2], type: 'bar' },
        { name: '利润率', data: [0.1, 0.2], type: 'line', axis: 'right' },
      ],
    });
    expect(opt.title.text).toBe('经营');
    expect(Array.isArray(opt.yAxis)).toBe(true);
    expect(opt.yAxis).toHaveLength(2);
    expect(opt.series[1].yAxisIndex).toBe(1);
    expect(opt.series[1].type).toBe('line');
    expect(opt.series[0].type).toBe('bar');
  });

  it('stacked+percent：列归一到 100', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    const opt = buildChartOption({
      type: 'bar', stacked: true, percent: true,
      series: [{ name: 'A', data: [1, 3] }, { name: 'B', data: [3, 1] }],
    });
    expect(opt.series[0].data).toEqual([25, 75]);
    expect(opt.series[1].data).toEqual([75, 25]);
    expect(opt.series[0].stack).toBe('t');
  });

  it('pie：单系列摊成 {name,value}', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    const opt = buildChartOption({
      type: 'pie',
      x: ['甲', '乙'],
      series: [{ name: '占比', data: [60, 40] }],
    });
    expect(opt.series[0].type).toBe('pie');
    expect(opt.series[0].data).toEqual([{ name: '甲', value: 60 }, { name: '乙', value: 40 }]);
    expect(opt.tooltip.trigger).toBe('item');
  });

  it('scatter：x 数值与 y 成对', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    const opt = buildChartOption({
      type: 'scatter',
      x: [10, 20],
      series: [{ name: '点', data: [5, 6] }],
    });
    expect(opt.series[0].data).toEqual([[10, 5], [20, 6]]);
    expect(opt.xAxis.type).toBe('value');
  });

  it('radar：indicator 对齐 x', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    const opt = buildChartOption({
      type: 'radar',
      x: ['攻', '防', '速'],
      series: [{ name: '武将', data: [80, 60, 90] }],
    });
    expect(opt.radar.indicator.map((i) => i.name)).toEqual(['攻', '防', '速']);
    expect(opt.series[0].type).toBe('radar');
  });

  it('null 数据点保留（断线）', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    const opt = buildChartOption({ type: 'line', series: [{ name: 's', data: [1, null, 3] }] });
    expect(opt.series[0].data).toEqual([1, null, 3]);
  });

  it('空 series → null（渲染端出空态卡）', async () => {
    const { buildChartOption } = await import('../src/js/ui_blocks/chart.js');
    expect(buildChartOption({ type: 'line', series: [] })).toBeNull();
    expect(buildChartOption({})).toBeNull();
  });
});

describe('parseGantt — 任务解析', () => {
  it('日期解析 + days 补 end + 分组排序', async () => {
    const { parseGantt, parseDay } = await import('../src/js/ui_blocks/gantt.js');
    expect(parseDay('2026-10-12')).toBe(Date.UTC(2026, 9, 12));
    expect(parseDay('bad')).toBeNull();
    const r = parseGantt({
      tasks: [
        { name: '甲组内', start: '2026-01-01', end: '2026-01-10', group: '甲' },
        { name: '散活', start: '2026-01-03', days: 2 },
        { name: '乙组内', start: '2026-01-05', end: '2026-01-08', group: '乙' },
      ],
    });
    // 无组任务在前,然后组头+组内任务
    expect(r.rows.map((x) => x.kind + (x.label || (x.task ? x.task.name : '')))).toEqual([
      'task散活', 'group甲', 'task甲组内', 'group乙', 'task乙组内',
    ]);
    // days 补 end
    const d = r.tasks.find((t) => t.name === '散活');
    expect(d.end - d.start).toBe(2 * 86400000);
    expect(r.spanStart).toBe(Date.UTC(2026, 0, 1));
    expect(r.spanEnd).toBe(Date.UTC(2026, 0, 10));
  });

  it('坏任务丢弃 + 上限 60', async () => {
    const { parseGantt } = await import('../src/js/ui_blocks/gantt.js');
    const many = Array.from({ length: 65 }, (_, i) => ({ name: `t${i}`, start: '2026-01-01' }));
    many.push({ name: '坏', start: 'n/a' });
    const r = parseGantt({ tasks: many });
    expect(r.tasks).toHaveLength(60);
    expect(r.dropped).toBe(6); // 65+1 - 60(坏的在 60 外也计)
    expect(r.tasks.some((t) => t.name === '坏')).toBe(false);
  });

  it('progress 钳制 0..100，milestone 不补 days', async () => {
    const { parseGantt } = await import('../src/js/ui_blocks/gantt.js');
    const r = parseGantt({
      tasks: [
        { name: 'p', start: '2026-01-01', end: '2026-01-02', progress: 150 },
        { name: 'm', start: '2026-01-03', milestone: true },
      ],
    });
    expect(r.tasks[0].progress).toBe(100);
    expect(r.tasks[1].end).toBe(r.tasks[1].start);
  });

  it('空任务 → 空结构不抛', async () => {
    const { parseGantt } = await import('../src/js/ui_blocks/gantt.js');
    expect(parseGantt({}).tasks).toHaveLength(0);
    expect(parseGantt({ tasks: [] }).spanEnd).toBe(1);
  });
});

/* ---------- expression engine ---------- */

import {
  compileExpression,
  ExpressionError,
} from '../src/js/ui_blocks/expression.js';
import { parseCurve } from '../src/js/ui_blocks/function-plot.js';

describe('expression engine', () => {
  test('arithmetic precedence and unary minus binds tighter than power', () => {
    expect(compileExpression('2 + 3 * 4').evaluate({})).toBe(14);
    expect(compileExpression('-2^2').evaluate({})).toBe(-4);
    expect(compileExpression('2 ^ 3 ^ 2').evaluate({})).toBe(512);
    expect(compileExpression('(2 + 3) * 4').evaluate({})).toBe(20);
  });

  test('variables, ternary and boolean operators', () => {
    expect(compileExpression('price * qty').evaluate({ price: 12.5, qty: 3 })).toBe(37.5);
    expect(compileExpression('price > 10 ? 0.9 : 1').evaluate({ price: 12 })).toBe(0.9);
    expect(compileExpression('x % 2').evaluate({ x: 7 })).toBe(1);
    expect(compileExpression('2 ** 10').evaluate({})).toBe(1024);
  });

  test('function library', () => {
    expect(compileExpression('round(3.14159, 2)').evaluate({})).toBe(3.14);
    expect(compileExpression('clamp(x, 0, 100)').evaluate({ x: 150 })).toBe(100);
    expect(compileExpression('sum(list)').evaluate({ list: [1, 2, 3, 4] })).toBe(10);
    expect(compileExpression('avg(range(1, 5))').evaluate({})).toBe(3);
    expect(compileExpression('contains(list, "b")').evaluate({ list: ['a', 'b'] })).toBe(true);
    expect(compileExpression('max(3, 9, 4)').evaluate({})).toBe(9);
    expect(compileExpression('sqrt(144)').evaluate({})).toBe(12);
    expect(compileExpression('min(range(1, 5))').evaluate({})).toBe(1);
  });

  test('numeric floor so integer divisions stay numbers', () => {
    const v = compileExpression('1 / 4').evaluate({});
    expect(typeof v).toBe('number');
    expect(v).toBe(0.25);
  });

  test('string concat + and string comparisons', () => {
    expect(compileExpression('"a" + "b"').evaluate({})).toBe('ab');
    expect(compileExpression('name == "x" ? 1 : 0').evaluate({ name: 'x' })).toBe(1);
  });

  test('unknown variable / function are ExpressionError', () => {
    expect(() => compileExpression('nope + 1').evaluate({})).toThrow(ExpressionError);
    expect(() => compileExpression('nope(1)').evaluate({})).toThrow(ExpressionError);
  });

  test('forbidden members are rejected', () => {
    expect(() => compileExpression('a.__proto__').evaluate({ a: {} })).toThrow(ExpressionError);
    expect(() => compileExpression('a["constructor"]').evaluate({ a: {} })).toThrow(ExpressionError);
  });

  test('budget caps runaway formulas', () => {
    const many = new Array(500).fill(1).join(' + ');
    expect(() => compileExpression(many)).toThrow(ExpressionError);
  });

  test('top-level concat is allowed; broken syntax rejected', () => {
    expect(compileExpression('"total: " + total').evaluate({ total: 5 })).toBe('total: 5');
    expect(() => compileExpression('(1 + 2')).toThrow(ExpressionError);
  });

  test('values equal compares records deeply for lists', () => {
    expect(compileExpression('list == other ? 1 : 0').evaluate({
      list: [1, [2, 3]], other: [1, [2, 3]],
    })).toBe(1);
  });
});

describe('parseCurve', () => {
  test('implicit row', () => {
    expect(() => parseCurve('x^2 + y^2 = 25')).not.toThrow();
    expect(() => parseCurve('y = sin(x)')).not.toThrow();
    expect(() => parseCurve('y > 3')).toThrow();
  });
});


// ---- live block engine (PR4) ----

describe('live block: compile, state, resolve', () => {
  const raw = JSON.stringify({
    component: 'panel', title: '房贷',
    state: { years: 15, mode: '等额本息' },
    computed: { monthly: '20000 * (years/10)' },
    children: [
      { kind: 'stat', label: '年限', value: '{years}', unit: '年' },
      { bind: 'years', component: 'slider', label: '年数', min: 1, max: 30 },
      { kind: 'text', text: '月供 {monthly}' },
      { component: 'button', label: '跑', set: { years: 20 } },
      { component: 'formula', expr: '2 * pi * r' },
      { component: 'select', bind: 'mode', options: ['等额本息', '等额本金'] },
    ],
  });

  function compile(rawText) {
    const parsed = parseBlockSource(rawText);
    expect(parsed.status).toBe('ok');
    return compileBlock(parsed.block, { streamComplete: true });
  }

  test('parses and compiles without issues', () => {
    const compiled = compile(raw);
    expect(compiled.issues).toEqual([]);
    expect(compiled.initialState).toEqual({ years: 15, mode: '等额本息' });
  });

  test('resolved values: computed, templates, bind', () => {
    const compiled = compile(raw);
    const res = resolveBlock(compiled, compiled.initialState);
    expect(res.values.monthly).toBe(30000);
    expect(res.root.children[0].props.value).toBe(15);
    expect(res.root.children[2].props.text).toBe('月供 30,000');
    expect(res.root.children[1].props.state).toBe('years');
    expect(res.root.children[1].bind).toBe('years');
    expect(res.root.children[5].bind).toBe('mode');
  });

  test('state change recomputes downstream', () => {
    const compiled = compile(raw);
    const next = mergeState(compiled.initialState, { years: 20 });
    expect(stateDiff(compiled.initialState, next)).toEqual({ years: 20 });
    const res = resolveBlock(compiled, next);
    expect(res.values.monthly).toBe(40000);
    expect(res.root.children[0].props.value).toBe(20);
  });

  test('button action patches declared state', () => {
    const compiled = compile(raw);
    const res = resolveBlock(compiled, compiled.initialState);
    const action = readAction(res.root.children[3].props, new Set(Object.keys(compiled.initialState)));
    expect(action).toEqual({ kind: 'set', patch: { years: 20 } });
  });

  test('mergeState keeps shape and drops unknown keys', () => {
    const compiled = compile(raw);
    const merged = mergeState(compiled.initialState, { years: 25, ghost: 1, mode: 7 });
    expect(merged.years).toBe(25);
    expect(merged.ghost).toBeUndefined();
    expect(merged.mode).toBe('等额本息'); // wrong shape falls back
  });

  test('bad JSON and wrong root are rejected', () => {
    expect(parseBlockSource('{oops').status).toBe('invalid');
    expect(parseBlockSource('[]').status).toBe('invalid');
  });

  test('circular computed is reported, not crashed', () => {
    const compiled = compile(JSON.stringify({
      component: 'panel',
      state: { a: 1 },
      computed: { x: 'y + 1', y: 'x + 1' },
      children: [],
    }));
    const messages = compiled.issues.map((i) => i.message).join(' ');
    expect(messages).toContain('circular');
  });
});

// ---- LaTeX (formula primitive) ----

describe('formulaToLatex', () => {
  test('symbolic line with greek and functions', () => {
    const latex = formulaToLatex('m * g', { name: 'F_g', values: {} });
    expect(latex.symbolic).toBe('F_{g} = m g');
  });

  test('worked line substitutes the numbers', () => {
    const latex = formulaToLatex('m * g', { values: { m: 9, g: 9.8 } });
    expect(latex.worked).toBe('= (9) (9.8) = 88.2');
  });

  test('identifiers are variables, known functions stay symbols', () => {
    const latex = formulaToLatex('sin(theta) / 2', { name: 'v', values: {} });
    expect(latex.symbolic).toBe('v = \\frac{\\sin\\left(\\theta\\right)}{2}');
  });

  test('bad expression throws ExpressionError', () => {
    expect(() => formulaToLatex('2 +', { values: {} })).toThrow();
  });
});


// ---- live rendering (DOM) ----

describe('mountLiveBlock', () => {
  function mount(body) {
    const host = document.createElement('div');
    const instance = mountLiveBlock(JSON.stringify(body), host, { renderers: new Map() });
    return { host, instance };
  }

  test('renders panel, stat, slider, text and formula', () => {
    const { host, instance } = mount({
      component: 'panel', title: '速度表',
      state: { v: 12 },
      computed: { t: '100 / v' },
      children: [
        { kind: 'stat', label: '速度', value: '{v}', unit: 'km/h' },
        { bind: 'v', component: 'slider', min: 1, max: 200, label: '调速' },
        { kind: 'text', text: '用时 {t}' },
        { component: 'formula', expr: 'v * t', name: 'd' },
      ],
    });
    expect(instance).toBeTruthy();
    expect(host.querySelector('.dx-panel-title').textContent).toBe('速度表');
    expect(host.querySelector('.dx-stat-value').textContent).toContain('12');
    expect(host.querySelector('.dx-text').textContent).toBe('用时 8.33');
    expect(host.querySelector('.dx-slider')).toBeTruthy();
    expect(host.querySelector('.dx-formula-symbolic')).toBeTruthy();

    // drag the slider: everything downstream recomputes
    const slider = host.querySelector('.dx-slider');
    slider.value = '100';
    slider.dispatchEvent(new Event('input', { bubbles: true }));
    expect(host.querySelector('.dx-text').textContent).toBe('用时 1');
  });

  test('button action writes state; reset restores', () => {
    const { host, instance } = mount({
      component: 'panel',
      state: { level: 1 },
      children: [
        { kind: 'stat', label: '档位', value: '{level}' },
        { component: 'button', label: '升档', set: { level: 3 } },
      ],
    });
    const button = host.querySelector('.dx-button');
    button.click();
    expect(host.querySelector('.dx-stat-value').textContent).toBe('3');
    expect(instance.changed).toBe(true);
    const reset = host.querySelector('.dx-reset');
    expect(reset).toBeTruthy();
    reset.click();
    expect(host.querySelector('.dx-stat-value').textContent).toBe('1');
  });

  test('visible hides a child', () => {
    const { host } = mount({
      component: 'panel',
      state: { on: false },
      children: [
        { kind: 'text', text: 'always' },
        { kind: 'text', text: 'gated', visible: 'on' },
      ],
    });
    const texts = [...host.querySelectorAll('.dx-text')].map((n) => n.textContent);
    expect(texts).toContain('always');
    expect(texts).not.toContain('gated');
  });

  test('tabs switch panels; callout tones render', () => {
    const { host } = mount({
      component: 'panel',
      children: [
        {
          kind: 'tabs',
          items: [
            { label: '甲' }, { label: '乙' },
          ],
          children: [
            { kind: 'text', text: 'AAA' },
            { kind: 'text', text: 'BBB' },
          ],
        },
        { kind: 'callout', tone: 'warning', title: '注意', text: '别忘了' },
      ],
    });
    expect(host.querySelector('.dx-text').textContent).toBe('AAA');
    const chips = host.querySelectorAll('.dx-chip');
    chips[1].click();
    expect(host.querySelector('.dx-text').textContent).toBe('BBB');
    expect(host.querySelector('.dx-callout').className).toContain('dx-tone-warning');
    expect(host.querySelector('.dx-callout-title').textContent).toBe('注意');
  });

  test('invalid body returns null', () => {
    const host = document.createElement('div');
    expect(mountLiveBlock('{broken', host, {})).toBeNull();
  });
});


// ---- widget sandbox (PR4) ----

describe('deeix-widget sandbox', () => {
  test('widget fences extract alongside deeix-ui', () => {
    const { cleanText, blocks } = extractUIBlocks(
      'before\n```deeix-widget\n<p>hello widget</p>\n```\nafter',
    );
    expect(cleanText).toBe('before\n\nafter');
    expect(blocks).toHaveLength(1);
    expect(blocks[0].language).toBe('deeix-widget');
    expect(blocks[0].rawBody).toBe('<p>hello widget</p>');
  });

  test('renderUiBlockBody builds a sandboxed iframe with the runtime', () => {
    const node = renderUiBlockBody({
      language: 'deeix-widget',
      rawBody: '<div id="w">hi</div>',
      normalizedBody: '<div id="w">hi</div>',
      hash: 'abc',
    });
    expect(node.className).toBe('dx-widget-host');
    const iframe = node.querySelector('iframe');
    expect(iframe.getAttribute('sandbox')).toBe('allow-scripts');
    expect(iframe.srcdoc).toContain('<div id="w">hi</div>');
    expect(iframe.srcdoc).toContain('window.quill');
    expect(iframe.srcdoc).toContain('ask(text)');
    expect(iframe.srcdoc).toContain('suggest(text)');
    // no same-origin: the frame cannot touch the page
    expect(iframe.getAttribute('sandbox')).not.toContain('allow-same-origin');
  });

  test('oversized widget is refused with a notice', () => {
    const big = 'x'.repeat(200_001);
    const node = renderUiBlockBody({
      language: 'deeix-widget',
      rawBody: big,
      normalizedBody: big,
      hash: 'big',
    });
    expect(node.querySelector('iframe')).toBeNull();
    expect(node.textContent).toContain('过大');
  });
});
