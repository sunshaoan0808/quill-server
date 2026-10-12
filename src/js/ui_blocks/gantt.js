/**
 * gantt renderer — DEEIX GanttProps → DOM bars (vanilla port).
 *
 * GanttProps (DEEIX contract):
 *   { title?, tasks: { id?, name, start: "YYYY-MM-DD", end?: "YYYY-MM-DD",
 *     days?: number, group?, progress?: 0..100, milestone?, depends?: string[] }[] }
 *
 * parseGantt is pure (unit-tested): date resolution, duration fill-in,
 * grouping order and row assignment happen there. buildGantt renders.
 */

const DAY = 86400000;
const MAX_TASKS = 60;
const PALETTE = ['var(--chart-1, #4a9eff)', 'var(--chart-2, #47c97e)', 'var(--chart-3, #e8b84a)', 'var(--chart-4, #c678dd)', 'var(--chart-5, #ff7b72)'];

/** "YYYY-MM-DD" → UTC ms (DEEIX parseDay semantics). */
export function parseDay(value) {
  if (!value) return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(String(value).trim());
  if (!m) return null;
  const time = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return Number.isFinite(time) ? time : null;
}

/**
 * Resolve the task list into positioned rows. Pure.
 * Returns { tasks, groups, spanStart, spanEnd, dropped } or throws on bad input.
 */
export function parseGantt(props) {
  const raw = props && Array.isArray(props.tasks) ? props.tasks : [];
  const dropped = Math.max(0, raw.length - MAX_TASKS);
  const tasks = [];
  const groupOrder = [];
  let idx = 0;
  for (const t of raw.slice(0, MAX_TASKS)) {
    if (!t || typeof t !== 'object') continue;
    const start = parseDay(t.start);
    if (start == null) continue; // unparseable rows are dropped (DEEIX behaves likewise)
    let end = parseDay(t.end);
    if (end == null && Number.isFinite(Number(t.days))) {
      end = start + Math.max(1, Number(t.days)) * DAY;
    }
    if (end == null) end = start; // milestone-shaped row without end
    if (end < start) end = start; // inverted range collapses to a point
    const group = typeof t.group === 'string' && t.group ? t.group : null;
    if (group && !groupOrder.includes(group)) groupOrder.push(group);
    tasks.push({
      key: t.id != null ? String(t.id) : `t${idx}`,
      id: t.id != null ? String(t.id) : `t${idx}`,
      name: String(t.name != null ? t.name : '任务'),
      group,
      start,
      end,
      progress: Math.max(0, Math.min(100, Number(t.progress == null ? 0 : t.progress))),
      milestone: !!t.milestone,
      depends: Array.isArray(t.depends) ? t.depends.map(String) : [],
    });
    idx++;
  }
  if (!tasks.length) return { tasks: [], groups: [], spanStart: 0, spanEnd: 1, dropped };

  // rows: group headers consume a row, then their tasks; ungrouped tasks first
  const rows = [];
  const byGroup = new Map();
  for (const t of tasks) {
    const g = t.group || '';
    if (!byGroup.has(g)) byGroup.set(g, []);
    byGroup.get(g).push(t);
  }
  if (byGroup.has('')) {
    for (const t of byGroup.get('')) rows.push({ kind: 'task', task: t });
  }
  for (const g of groupOrder) {
    rows.push({ kind: 'group', label: g });
    for (const t of byGroup.get(g) || []) rows.push({ kind: 'task', task: t });
  }

  let min = Infinity;
  let max = -Infinity;
  for (const t of tasks) {
    if (t.start < min) min = t.start;
    if (t.end > max) max = t.end;
  }
  if (max <= min) max = min + DAY;
  return { tasks, rows, groups: groupOrder, spanStart: min, spanEnd: max, dropped };
}

function tickEvery(spanDays) {
  if (spanDays <= 14) return { step: DAY, fmt: (ms) => `${new Date(ms).getUTCDate()}` };
  if (spanDays <= 90) return { step: 7 * DAY, fmt: (ms) => `${new Date(ms).getUTCMonth() + 1}/${new Date(ms).getUTCDate()}` };
  return { step: 30 * DAY, fmt: (ms) => `${new Date(ms).getUTCMonth() + 1}月` };
}

function pct(ms, parsed) {
  return ((ms - parsed.spanStart) / (parsed.spanEnd - parsed.spanStart)) * 100;
}

function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = String(text);
  return n;
}

/** Build the gantt DOM for parsed props (no dependencies on echarts). */
export function buildGantt(props) {
  const parsed = parseGantt(props);
  const wrap = el('div', 'stui-gantt');
  if (props && props.title) wrap.appendChild(el('div', 'stui-gantt-title', props.title));
  if (!parsed.tasks.length) {
    wrap.appendChild(el('div', 'stui-empty', '（无有效任务）'));
    return wrap;
  }

  const spanDays = Math.ceil((parsed.spanEnd - parsed.spanStart) / DAY);
  const tick = tickEvery(spanDays);

  // scale header
  const header = el('div', 'stui-gantt-head');
  const spacer = el('div', 'stui-gantt-namecell', '');
  header.appendChild(spacer);
  const scale = el('div', 'stui-gantt-scale');
  const firstTick = Math.ceil(parsed.spanStart / tick.step) * tick.step;
  for (let ms = firstTick; ms <= parsed.spanEnd; ms += tick.step) {
    const mark = el('span', 'stui-gantt-tick', tick.fmt(ms));
    mark.style.left = `${pct(ms, parsed)}%`;
    scale.appendChild(mark);
  }
  header.appendChild(scale);
  wrap.appendChild(header);

  const body = el('div', 'stui-gantt-body');
  const posByKey = new Map(parsed.tasks.map((t) => [t.key, t]));

  for (const row of parsed.rows) {
    if (row.kind === 'group') {
      const g = el('div', 'stui-gantt-group', row.label);
      body.appendChild(g);
      continue;
    }
    const t = row.task;
    const line = el('div', 'stui-gantt-row');
    const name = el('div', 'stui-gantt-namecell', t.name);
    name.title = t.depends.length ? `依赖: ${t.depends.join(', ')}` : '';
    line.appendChild(name);

    const lane = el('div', 'stui-gantt-lane');
    const left = Math.max(0, pct(t.start, parsed));
    const right = Math.min(100, pct(t.end, parsed));
    // dependency lead-in: dashed segment from each dep's end to this task's start
    for (const d of t.depends) {
      const src = posByKey.get(d);
      if (!src) continue;
      const dLeft = Math.max(0, Math.min(100, pct(src.end, parsed)));
      if (dLeft >= left) continue;
      const lead = el('div', 'stui-gantt-lead');
      lead.style.left = `${dLeft}%`;
      lead.style.width = `${left - dLeft}%`;
      lead.title = `依赖: ${src.name}`;
      lane.appendChild(lead);
    }
    const bar = el('div', t.milestone ? 'stui-gantt-milestone' : 'stui-gantt-bar');
    bar.style.left = `${left}%`;
    const w = Math.max(t.milestone ? 0 : 0.6, right - left);
    bar.style.width = `${t.milestone ? 0 : w}%`;
    const colorIdx = parsed.tasks.indexOf(t) % PALETTE.length;
    bar.style.setProperty('--gantt-color', PALETTE[colorIdx]);
    if (!t.milestone) {
      const fill = el('div', 'stui-gantt-progress');
      fill.style.width = `${t.progress}%`;
      bar.appendChild(fill);
      const tip = el('span', 'stui-gantt-tip', `${t.name} ${Math.round(t.progress)}%`);
      bar.appendChild(tip);
    } else {
      bar.title = `${t.name}（里程碑）`;
    }
    lane.appendChild(bar);
    line.appendChild(lane);
    body.appendChild(line);
  }
  wrap.appendChild(body);

  if (parsed.dropped > 0) {
    wrap.appendChild(el('div', 'stui-empty', `（已省略 ${parsed.dropped} 个任务，上限 ${MAX_TASKS}）`));
  }
  return wrap;
}
