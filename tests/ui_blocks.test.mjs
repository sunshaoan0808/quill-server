import { describe, it, expect } from 'vitest';
import {
  extractUIBlocks,
  stripUIBlockFences,
  scanUIBlockFences,
  replaceUIBlockFences,
  hashFenceBody,
} from '../src/js/ui_blocks/fence.js';
import { parseUiBlockSpec, rendererNames, hasRenderer } from '../src/js/ui_blocks/render.js';

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

  it('deeix-widget 围栏此阶段不抽（live PR 前留在正文）', () => {
    const text = '```deeix-widget\n<b>hi</b>\n```';
    const { cleanText, blocks } = extractUIBlocks(text);
    expect(blocks).toHaveLength(0);
    expect(cleanText).toBe(text);
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
  it('注册了 8 个渲染器（6 静态 + chart/gantt）', () => {
    expect(rendererNames().sort()).toEqual(
      ['card-grid', 'chart', 'data-table', 'decision-tree', 'diff', 'gantt', 'quiz', 'stat-grid'].sort(),
    );
  });

  it('hasRenderer 识别已知/未知', () => {
    expect(hasRenderer('calculator')).toBe(false); // 表达式引擎批次
    expect(hasRenderer('chart')).toBe(true);
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
