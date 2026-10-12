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
  it('注册了 6 个渲染器', () => {
    expect(rendererNames().sort()).toEqual(
      ['card-grid', 'data-table', 'decision-tree', 'diff', 'quiz', 'stat-grid'].sort(),
    );
  });

  it('hasRenderer 识别已知/未知', () => {
    expect(hasRenderer('chart')).toBe(false); // 图表在 ECharts 批次
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
