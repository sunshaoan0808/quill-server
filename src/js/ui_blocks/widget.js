// deeix-widget: model-authored HTML in a sandboxed iframe. The frame gets no
// same origin and no network permission; everything it may do is what its own
// inline script does, talking to the host only through postMessage. The host
// accepts size, state and follow-up suggestions, field by field — a widget
// cannot touch the chat around it.

const WIDGET_MAX_BYTES = 200_000;
const MIN_HEIGHT = 120;
const MAX_HEIGHT = 900;
const FRAME_SOURCE = 'quill-widget';

const WIDGET_BASE_STYLE = `<style data-quill-widget-base>
:where(body) { padding: 16px; font: 13px/1.6 system-ui, sans-serif; color: #e8e8ea; background: transparent; }
:where(h1, h2, h3, h4) { margin: 0 0 4px; font-size: 15px; line-height: 1.4; font-weight: 600; }
:where(p) { margin: 0 0 8px; }
:where(button, select, input, textarea) { font: inherit; font-size: 12px; color: inherit; }
:where(button) { height: 28px; padding: 0 12px; border: 1px solid #3a3a40; border-radius: 8px; background: #242428; cursor: pointer; white-space: nowrap; }
:where(button:hover) { background: #2e2e34; }
:where(button[aria-pressed="true"]) { background: #4a5cff; border-color: #4a5cff; color: #fff; }
:where(button:disabled) { opacity: .5; cursor: not-allowed; }
:where(select), :where(input[type="text"]), :where(input[type="number"]) { height: 28px; padding: 0 8px; border: 1px solid #3a3a40; border-radius: 8px; background: #1d1d21; }
:where(input[type="range"]) { accent-color: #4a5cff; }
:where(label) { color: #9a9aa2; font-size: 12px; }
:where(:focus-visible) { outline: 2px solid rgba(90, 110, 255, .5); outline-offset: 2px; border-radius: 8px; }
:where(svg, canvas, img) { max-width: 100%; }
:where(.controls) { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; }
:where(.num) { font-variant-numeric: tabular-nums; }
:where(.muted) { color: #9a9aa2; }
:where(button.ask) { display: flex; align-items: center; width: 100%; min-height: 36px; padding: 8px 12px; border-radius: 10px; background: #242428; text-align: left; white-space: normal; }
:where(button.ask)::after { content: "→"; margin-left: auto; padding-left: 12px; color: #9a9aa2; }
</style>`;

// The runtime the frame itself runs: resize reports, state relay, and the
// quill.ask / quill.suggest calls a widget uses to hand a follow-up back to
// the composer.
function widgetRuntime() {
  return `<script>
(() => {
  const FRAME = ${JSON.stringify(FRAME_SOURCE)};
  let currentState = {};
  const post = (message) => window.parent.postMessage({ source: FRAME, ...message }, "*");
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (!data || data.source !== "quill-host") return;
    if (data.type === "state" && data.state && typeof data.state === "object") {
      currentState = data.state;
    }
  });
  const reportHeight = () => post({ type: "resize", height: Math.ceil(document.documentElement.scrollHeight) });
  new ResizeObserver(reportHeight).observe(document.documentElement);
  window.quill = Object.freeze({
    getState() { return currentState; },
    setState(patch) {
      if (!patch || typeof patch !== "object" || Array.isArray(patch)) return;
      const next = { ...currentState };
      for (const [key, value] of Object.entries(patch)) {
        if (value === null || value === undefined) delete next[key]; else next[key] = value;
      }
      currentState = next;
      post({ type: "state", state: next });
    },
    ask(text) { if (typeof text === "string" && text.trim()) post({ type: "ask", text }); },
    suggest(text) { post({ type: "suggest", text: typeof text === "string" ? text : "" }); },
  });
  window.addEventListener("DOMContentLoaded", () => { post({ type: "ready" }); reportHeight(); });
})();
<\/script>`;
}

function widgetDocument(source) {
  return `<!doctype html><html><head><meta charset="utf-8">${WIDGET_BASE_STYLE}${widgetRuntime()}</head><body>${source}</body></html>`;
}

export function renderWidget(body, host, options = {}) {
  const source = String(body ?? '').trim();
  const frame = document.createElement('div');
  frame.className = 'dx-widget';
  if (!source) {
    frame.appendChild(document.createTextNode('widget 为空'));
    host.appendChild(frame);
    return null;
  }
  if (new TextEncoder().encode(source).length > WIDGET_MAX_BYTES) {
    frame.appendChild(document.createTextNode('widget 过大'));
    host.appendChild(frame);
    return null;
  }
  const iframe = document.createElement('iframe');
  iframe.className = 'dx-widget-frame';
  iframe.setAttribute('sandbox', 'allow-scripts');
  iframe.setAttribute('title', 'widget');
  iframe.style.height = `${MIN_HEIGHT}px`;
  iframe.srcdoc = widgetDocument(source);
  frame.appendChild(iframe);
  host.appendChild(frame);

  // Field-narrowed host side of the bridge: only this frame's window, only
  // the three message types the runtime sends.
  const onMessage = (event) => {
    if (event.source !== iframe.contentWindow) return;
    const data = event.data;
    if (!data || data.source !== FRAME_SOURCE) return;
    if (data.type === 'resize' && Number.isFinite(data.height)) {
      iframe.style.height = `${Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round(data.height)))}px`;
    } else if (data.type === 'ask' && typeof data.text === 'string') {
      if (options.onAsk) options.onAsk(data.text);
    } else if (data.type === 'suggest' && typeof data.text === 'string') {
      if (options.onSuggest) options.onSuggest(data.text);
    }
  };
  window.addEventListener('message', onMessage);
  return { iframe, dispose() { window.removeEventListener('message', onMessage); } };
}
