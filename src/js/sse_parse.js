/**
 * src/js/sse_parse.js — SSE 行解析纯函数（R9 从 jobs.js readSSE 抽出）。
 *
 * 服务端 SSE 约定（crates/kaleido-server/src/utf8_stream.rs / state.rs）：
 *   - 事件行 `data: {json}`；容忍 `data :` 变体（历史宽松写法）
 *   - `data: [DONE]` 为流终止哨兵 → { done: true }
 *   - 空行 / `:` 开头注释（keepalive）跳过
 *   - 非 data 行跳过；JSON 解析失败容错（json=null, data 原文透传）
 * UTF-8 跨块切分由调用方 TextDecoder({stream:true}) 处理，本层只管行内语义。
 */

/**
 * @param {string} line 一行（不含行尾换符；允许带 \r，函数内归一）
 * @returns {{skip:true} | {done:true, data:string} | {data:string, json:any}}
 */
export function parseSseLine(line) {
  if (line.endsWith('\r')) line = line.slice(0, -1);
  if (!line) return { skip: true };
  if (line.startsWith(':')) return { skip: true }; // comment / keepalive
  let data = null;
  if (line.startsWith('data:')) {
    data = line.slice(5).trimStart();
  } else if (line.startsWith('data :')) {
    data = line.slice(6).trimStart();
  } else {
    return { skip: true };
  }
  if (data === '[DONE]') {
    return { done: true, data };
  }
  let json = null;
  try {
    json = JSON.parse(data);
  } catch (_) {}
  return { data, json };
}
