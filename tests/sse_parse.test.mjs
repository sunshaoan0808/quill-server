import { describe, it, expect } from 'vitest';
import { parseSseLine } from '../src/js/sse_parse.js';

describe('parseSseLine — 服务端 SSE 行解析契约', () => {
  it('解析标准 data: 行为 JSON', () => {
    const r = parseSseLine('data: {"eventType":"delta","delta":"你好"}');
    expect(r.skip).toBeUndefined();
    expect(r.json).toEqual({ eventType: 'delta', delta: '你好' });
  });

  it('容忍 data : 空格变体（历史宽松写法）', () => {
    const r = parseSseLine('data : {"eventType":"done"}');
    expect(r.json).toEqual({ eventType: 'done' });
  });

  it('容忍 CRLF 行尾（服务端 \r\n 分隔）', () => {
    const r = parseSseLine('data: {"a":1}\r');
    expect(r.json).toEqual({ a: 1 });
  });

  it('跳过空行与 : 开头 keepalive 注释', () => {
    expect(parseSseLine('').skip).toBe(true);
    expect(parseSseLine(': ping').skip).toBe(true);
  });

  it('跳过非 data 行（event:/id: 等）', () => {
    expect(parseSseLine('event: delta').skip).toBe(true);
  });

  it('[DONE] 哨兵 → done 终止', () => {
    const r = parseSseLine('data: [DONE]');
    expect(r.done).toBe(true);
    expect(r.data).toBe('[DONE]');
  });

  it('非 JSON data 容错：json=null 且原文透传', () => {
    const r = parseSseLine('data: 普通文本块');
    expect(r.json).toBeNull();
    expect(r.data).toBe('普通文本块');
  });

  it('trimStart：data: 后多空格不影响 JSON', () => {
    const r = parseSseLine('data:    {"ok":true}');
    expect(r.json).toEqual({ ok: true });
  });
});
