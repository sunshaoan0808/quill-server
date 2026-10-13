// Interactive block model: from the raw fenced text to a tree of nodes whose
// props are plain values, recomputed whenever the block's state changes.
//
//   parseBlockSource(raw)          lenient JSON parse -> BlockSource
//   compileBlock(source)           validate, compile formulas, order computed
//   resolveBlock(compiled, state)  evaluate computed + templates -> tree
//
// The model writes JSON and formulas, never code. A block with `state` or
// `computed` is live: its templates are evaluated. A block without them is
// static: props pass through untouched.
//
// Nothing here throws on model mistakes. Broken parts are dropped or degraded
// and reported in `issues`, so one bad node never blanks the whole block.

import {
  createBudget,
  ExpressionError,
  parseExpression,
  truthy,
  valuesEqual,
} from './expression.js';
import {
  compileTemplate,
  POINT_VARIABLES,
  unwrapExpressionSource,
} from './template.js';

// Exceeding a limit drops the excess, never the block.
export const BLOCK_LIMITS = {
  computed: 64,
  nodes: 300,
  depth: 8,
  // Steps shared by every formula of one resolution, including per-point ones.
  steps: 200_000,
  stateDepth: 4,
  askLength: 120,
};

// Keys of a node that are structure, not props.
const STRUCTURE_KEYS = new Set(['component', 'kind', 'id', 'version', 'state', 'computed', 'children', 'visible', 'bind', 'props']);

// ---- Source ----

function readNode(value, path) {
  // Props sit either in a `props` object or flat on the node.
  const props = {};
  const legacy = isObject(value.props) ? value.props : undefined;
  for (const [key, field] of Object.entries(legacy ?? value)) {
    if (legacy || !STRUCTURE_KEYS.has(key)) setOwn(props, key, field);
  }
  const children = [];
  if (Array.isArray(value.children)) {
    value.children.forEach((child, index) => {
      if (isObject(child) && (typeof child.component === 'string' && child.component !== '' || typeof child.kind === 'string' && child.kind !== '')) {
        children.push(readNode(child, path ? `${path}.${index}` : String(index)));
      }
    });
  }
  // The model writes `component`; `kind` is accepted as an alias (same shape
  // the DEEIX renderer consumes).
  const componentName = typeof value.component === 'string' && value.component !== ''
    ? value.component
    : (typeof value.kind === 'string' ? value.kind : '');
  return {
    component: componentName,
    props,
    children,
    bind: typeof value.bind === 'string' && value.bind !== '' ? value.bind : undefined,
    visible: value.visible,
    path,
  };
}

// A component version as models write it: a number, "v2.1", or nothing (1).
function normalizeVersion(version) {
  if (typeof version === 'number' && Number.isFinite(version)) {
    return Math.max(1, Math.trunc(version));
  }
  if (typeof version === 'string') {
    const match = /^v?(\d+)(?:\.\d+)?$/i.exec(version.trim());
    if (match) return Number.parseInt(match[1] ?? '1', 10);
  }
  return 1;
}

const BLOCK_ID_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

function normalizeID(id) {
  const text = typeof id === 'number' && Number.isFinite(id) ? String(id) : typeof id === 'string' ? id.trim() : '';
  return BLOCK_ID_PATTERN.test(text) ? text : '';
}

// FNV-1a, 32 bits, base 36: a block without an id is keyed by its content, so
// the state survives a re-render and stays independent of whitespace.
export function hashText(text) {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(36);
}

function contentKey(value) {
  return `auto-${hashText(JSON.stringify(value))}`;
}

// Reads the fenced body. Returns { status: 'ok', block } or { status, message }.
export function parseBlockSource(raw) {
  let value;
  try {
    value = JSON.parse(raw);
  } catch (error) {
    return { status: 'invalid', message: error instanceof Error ? error.message : 'invalid JSON' };
  }
  if (!isObject(value) || typeof value.component !== 'string' || value.component === '') {
    return { status: 'invalid', message: 'expected an object with a component' };
  }
  const state = isObject(value.state) ? value.state : undefined;
  const computed = isObject(value.computed) ? value.computed : undefined;
  const root = readNode(value, '');
  const id = normalizeID(value.id);
  return {
    status: 'ok',
    block: {
      component: value.component,
      id,
      key: id || contentKey(value),
      version: normalizeVersion(value.version),
      state,
      computed,
      root,
      complete: true,
      live: state !== undefined || computed !== undefined || Array.isArray(value.children),
    },
  };
}

// ---- Compilation ----

// Props shown verbatim, never evaluated, per component.
export function compileBlock(source, options = {}) {
  const issues = [];
  const initialState = readInitialState(source.state, issues);
  const computed = orderComputed(source.computed, new Set(Object.keys(initialState)), issues);
  // A point variable the block declares itself is an ordinary value, not a per-point one.
  const declared = new Set([...Object.keys(initialState), ...computed.map((entry) => entry.name)]);
  const pointNames = POINT_VARIABLES.filter((name) => !declared.has(name));
  const context = { options, issues, nodes: 0, live: source.live, pointNames };
  const root = compileNode(source.root, 0, context) ?? emptyNode(source.root);
  // Controls bound to undeclared state get a default.
  for (const [name, value] of Object.entries(implicitState(root, initialState))) {
    setOwn(initialState, name, value);
  }
  return { source, initialState, computed, root, issues };
}

function emptyNode(source) {
  return { component: source.component, path: source.path, props: [], children: [] };
}

function compileNode(source, depth, context) {
  if (depth >= BLOCK_LIMITS.depth) {
    context.issues.push({ path: source.path, message: `nesting deeper than ${BLOCK_LIMITS.depth} dropped` });
    return null;
  }
  context.nodes += 1;
  if (context.nodes > BLOCK_LIMITS.nodes) {
    if (context.nodes === BLOCK_LIMITS.nodes + 1) {
      context.issues.push({ path: source.path, message: `more than ${BLOCK_LIMITS.nodes} nodes; the rest are dropped` });
    }
    return null;
  }
  const raw = new Set(context.options.rawProps?.(source.component) ?? []);
  const props = [];
  let bind = source.bind;
  for (const [key, value] of Object.entries(source.props)) {
    if (!context.live || raw.has(key)) {
      props.push([key, { kind: 'static', value }]);
      continue;
    }
    // A control written as `"value": "{x}"` binds x.
    if (key === 'value' && bind === undefined && typeof value === 'string') {
      const bound = /^\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}$/.exec(value);
      if (bound?.[1] && isControl(source.component)) {
        bind = bound[1];
        continue;
      }
    }
    props.push([key, compileValue(value, `${source.path}.${key}`, context.pointNames, context.issues)]);
  }
  // `"bind": "amount"` on a control names the state it writes (example format).
  if (bind !== undefined && isControl(source.component) && !props.some(([key]) => key === 'state')) {
    if (isIdentifier(bind)) props.push(['state', { kind: 'static', value: bind }]);
  }
  let visible;
  if (context.live && source.visible !== undefined) {
    visible = compileCondition(source.visible, `${source.path}.visible`, context.issues);
  }
  const children = [];
  for (const child of source.children) {
    const compiled = compileNode(child, depth + 1, context);
    if (compiled) children.push(compiled);
  }
  return { component: source.component, path: source.path, bind, visible, props, children };
}

function compileValue(value, path, pointNames, issues) {
  if (typeof value === 'string') {
    try {
      const template = compileTemplate(value);
      if (!template) return { kind: 'static', value };
      const point = template.variables.some((name) => pointNames.includes(name));
      return point ? { kind: 'point', template } : { kind: 'template', template };
    } catch (error) {
      issues.push({ path, message: errorMessage(error) });
      return { kind: 'static', value: undefined };
    }
  }
  if (Array.isArray(value)) {
    return { kind: 'list', items: value.map((item, index) => compileValue(item, `${path}[${index}]`, pointNames, issues)) };
  }
  if (isObject(value)) {
    if (isExpressionValue(value)) return compileExpressionValue(value, path, issues);
    return { kind: 'object', fields: Object.entries(value).map(([key, field]) => [key, compileValue(field, `${path}.${key}`, pointNames, issues)]) };
  }
  return { kind: 'static', value };
}

// `{ source: "expression", bind?: name }` — a formula as a prop value.
function isExpressionValue(value) {
  if (!isObject(value) || typeof value.source !== 'string') return false;
  return Object.keys(value).every((key) => key === 'source' || key === 'bind');
}

function compileExpressionValue(value, path, issues) {
  try {
    parseExpression(unwrapExpressionSource(value.source));
  } catch (error) {
    issues.push({ path, message: errorMessage(error) });
    return { kind: 'static', value: undefined };
  }
  const bind = typeof value.bind === 'string' && isIdentifier(value.bind) ? value.bind : undefined;
  return { kind: 'expression', source: value.source, bind };
}

// `visible` takes an expression with or without braces, or a literal boolean.
function compileCondition(value, path, issues) {
  if (typeof value === 'boolean') return parseExpression(String(value));
  if (typeof value !== 'string') {
    issues.push({ path, message: 'visible must be an expression' });
    return parseExpression('false');
  }
  try {
    return parseExpression(unwrapExpressionSource(value));
  } catch (error) {
    issues.push({ path, message: errorMessage(error) });
    return parseExpression('false');
  }
}

// Expression sources are re-evaluated on every change; parsing is cached.
const expressionCache = new Map();
function getExpression(source) {
  const key = unwrapExpressionSource(source);
  let expr = expressionCache.get(key);
  if (!expr) {
    expr = parseExpression(key);
    if (expressionCache.size > 100) expressionCache.clear();
    expressionCache.set(key, expr);
  }
  return expr;
}

// Declared state: JSON values only, at most four levels deep.
function readInitialState(state, issues) {
  const result = {};
  if (!state) return result;
  for (const [name, value] of Object.entries(state)) {
    if (!isIdentifier(name)) {
      issues.push({ path: `state.${name}`, message: 'state names are letters, digits and _' });
      continue;
    }
    const json = toUIValue(value, 1);
    if (json === undefined) {
      issues.push({ path: `state.${name}`, message: `state values are JSON, at most ${BLOCK_LIMITS.stateDepth} levels deep` });
      continue;
    }
    setOwn(result, name, json);
  }
  return result;
}

// Orders computed entries so each one is evaluated after what it reads.
// Entries in a cycle, or reading a name defined nowhere, still compile:
// they evaluate to an error and display as "—".
function orderComputed(computed, stateNames, issues) {
  if (!computed) return [];
  const entries = Object.entries(computed).filter(([name]) => {
    if (!isIdentifier(name)) {
      issues.push({ path: `computed.${name}`, message: 'computed names are letters, digits and _' });
      return false;
    }
    if (stateNames.has(name)) {
      issues.push({ path: `computed.${name}`, message: 'already declared in state' });
      return false;
    }
    return true;
  });
  if (entries.length > BLOCK_LIMITS.computed) {
    issues.push({ path: 'computed', message: `more than ${BLOCK_LIMITS.computed} entries; the rest are dropped` });
    entries.length = BLOCK_LIMITS.computed;
  }

  const compiled = new Map();
  for (const [name, value] of entries) {
    if (typeof value === 'string') {
      try {
        compiled.set(name, { expression: parseExpression(unwrapExpressionSource(value)) });
      } catch (error) {
        issues.push({ path: `computed.${name}`, message: errorMessage(error) });
        compiled.set(name, { expression: undefined });
      }
      continue;
    }
    // Non-string values are constants: lookup tables, lists, flags.
    const json = toUIValue(value, 1);
    if (json === undefined) issues.push({ path: `computed.${name}`, message: 'constant is not a JSON value' });
    compiled.set(name, { value: json ?? null });
  }

  const order = [];
  const visiting = new Set();
  const done = new Set();
  const visit = (name, trail) => {
    if (done.has(name)) return;
    if (visiting.has(name)) {
      issues.push({ path: `computed.${name}`, message: `circular: ${[...trail, name].join(' → ')}` });
      return;
    }
    const entry = compiled.get(name);
    if (!entry) return;
    visiting.add(name);
    for (const dependency of entry.expression?.variables ?? []) {
      if (compiled.has(dependency)) visit(dependency, [...trail, name]);
    }
    visiting.delete(name);
    done.add(name);
    order.push({ name, ...entry });
  };
  for (const name of compiled.keys()) visit(name, []);
  return order;
}

const CONTROLS = new Set(['slider', 'select', 'segmented', 'switch', 'input', 'tabs']);

export function isControl(component) {
  return CONTROLS.has(component);
}

// Default for a control bound to state the block never declared.
function implicitState(root, declared) {
  const result = {};
  const walk = (node) => {
    if (node.bind && isIdentifier(node.bind) && !Object.hasOwn(declared, node.bind) && !Object.hasOwn(result, node.bind)) {
      const value = controlDefault(node);
      if (value !== undefined) setOwn(result, node.bind, value);
    }
    for (const child of node.children) walk(child);
  };
  walk(root);
  return result;
}

function controlDefault(node) {
  const prop = (name) => {
    const entry = node.props.find(([key]) => key === name)?.[1];
    return entry ? staticValue(entry) : undefined;
  };
  switch (node.component) {
    case 'slider': {
      const min = prop('min');
      return typeof min === 'number' ? min : 0;
    }
    case 'select':
    case 'segmented':
    case 'tabs': {
      const options = prop(node.component === 'tabs' ? 'items' : 'options');
      const first = Array.isArray(options) ? options[0] : undefined;
      if (isObject(first)) return toUIValue(first.value, 1) ?? null;
      return toUIValue(first, 1) ?? null;
    }
    case 'switch':
      return false;
    case 'input':
      return prop('type') === 'number' ? 0 : '';
    default:
      return undefined;
  }
}

// The literal behind a compiled value, or undefined when any part is a formula.
function staticValue(value) {
  switch (value.kind) {
    case 'static': return value.value;
    case 'list': {
      const items = [];
      for (const item of value.items) {
        const resolved = staticValue(item);
        if (resolved === undefined) return undefined;
        items.push(resolved);
      }
      return items;
    }
    case 'object': {
      const result = {};
      for (const [key, field] of value.fields) {
        const resolved = staticValue(field);
        if (resolved === undefined) return undefined;
        setOwn(result, key, resolved);
      }
      return result;
    }
    default: return undefined;
  }
}

// ---- Resolution ----

// Evaluates the block against the current state (declared values overlaid
// with the user's changes).
export function resolveBlock(block, state) {
  const issues = [];
  const budget = createBudget(BLOCK_LIMITS.steps);
  const values = { ...state };
  // A computed entry that fails is left undefined: whatever reads it fails
  // too and shows "—", instead of silently using a stale value.
  for (const entry of block.computed) {
    if (entry.expression === undefined) {
      if (entry.value !== undefined) setOwn(values, entry.name, entry.value);
      continue;
    }
    try {
      setOwn(values, entry.name, entry.expression.evaluate(scopeOf(values, budget)));
    } catch (error) {
      issues.push({ path: `computed.${entry.name}`, message: errorMessage(error) });
    }
  }
  const context = { values, budget, issues };
  const root = resolveNode(block.root, context) ?? { ...block.root, props: {}, children: [] };
  return { root, values, issues };
}

function scopeOf(values, budget, point) {
  return {
    lookup: (name) => {
      if (point && Object.hasOwn(point, name)) return point[name];
      return Object.hasOwn(values, name) ? values[name] : undefined;
    },
    budget,
  };
}

function resolveNode(node, context) {
  if (node.visible) {
    try {
      if (!truthy(node.visible.evaluate(scopeOf(context.values, context.budget)))) return null;
    } catch (error) {
      // A condition that cannot be evaluated hides the node.
      context.issues.push({ path: `${node.path}.visible`, message: errorMessage(error) });
      return null;
    }
  }
  const props = {};
  for (const [key, value] of node.props) {
    const resolved = resolveValue(value, `${node.path}.${key}`, context);
    if (resolved !== undefined) setOwn(props, key, resolved);
  }
  const children = [];
  for (const child of node.children) {
    const resolved = resolveNode(child, context);
    if (resolved) children.push(resolved);
  }
  return { component: node.component, path: node.path, bind: node.bind, props, children };
}

// Returns undefined to drop the prop: the component then uses its default.
function resolveValue(value, path, context) {
  switch (value.kind) {
    case 'static': return value.value;
    case 'expression': {
      try {
        const out = getExpression(value.source).evaluate(context.values);
        return out?.value;
      } catch {
        return undefined;
      }
    }
    case 'template':
      try {
        // Mixed text degrades per hole; a single-hole value is dropped so the
        // component falls back to its default.
        return value.template.evaluate(scopeOf(context.values, context.budget), (error) => {
          context.issues.push({ path, message: errorMessage(error) });
        });
      } catch (error) {
        context.issues.push({ path, message: errorMessage(error) });
        return undefined;
      }
    case 'point': return pointTemplate(value.template, context, path);
    case 'list': {
      const items = [];
      for (const [index, item] of value.items.entries()) {
        const resolved = resolveValue(item, `${path}[${index}]`, context);
        if (resolved !== undefined) items.push(resolved);
      }
      return items;
    }
    case 'object': {
      const result = {};
      for (const [key, field] of value.fields) {
        const resolved = resolveValue(field, `${path}.${key}`, context);
        if (resolved !== undefined) setOwn(result, key, resolved);
      }
      return result;
    }
  }
}

// Point templates share the resolution budget: a chart with a 1000-point
// range and three series spends from the same steps as everything else.
function pointTemplate(template, context, path) {
  let reported = false;
  return {
    kind: 'point-template',
    source: template.source,
    evaluate: (point) => {
      try {
        return template.evaluate(scopeOf(context.values, context.budget, point));
      } catch (error) {
        if (!reported) {
          reported = true;
          context.issues.push({ path, message: errorMessage(error) });
        }
        return null;
      }
    },
  };
}

export function isPointTemplate(value) {
  return isObject(value) && value.kind === 'point-template' && typeof value.evaluate === 'function';
}

// ---- State ----

// Merges stored user changes over the declared state. Only names the block
// still declares are kept, and only values of a compatible shape: a number
// stays a number. Anything else falls back to the declared value.
export function mergeState(initial, saved) {
  const result = { ...initial };
  if (!saved) return result;
  for (const [name, value] of Object.entries(saved)) {
    if (!Object.hasOwn(initial, name)) continue;
    const json = toUIValue(value, 1);
    const declared = initial[name] ?? null;
    if (json !== undefined && sameShape(declared, json)) setOwn(result, name, json);
  }
  return result;
}

// The keys whose value differs from the declared state: what gets stored.
export function stateDiff(initial, state) {
  const diff = {};
  for (const [name, value] of Object.entries(state)) {
    if (Object.hasOwn(initial, name) && !valuesEqual(value, initial[name] ?? null)) setOwn(diff, name, value);
  }
  return Object.keys(diff).length > 0 ? diff : undefined;
}

function sameShape(declared, value) {
  if (declared === null || value === null) return true;
  if (Array.isArray(declared)) return Array.isArray(value);
  return typeof declared === typeof value && Array.isArray(declared) === Array.isArray(value);
}

// ---- Actions ----

// Reads a button's action from its resolved props. `set` values were already
// evaluated as templates against the state at the time of this resolution.
export function readAction(props, stateNames) {
  if (typeof props.ask === 'string' && props.ask.trim() !== '') {
    return { kind: 'ask', text: props.ask.trim().slice(0, BLOCK_LIMITS.askLength) };
  }
  if (typeof props.copy === 'string' && props.copy !== '') {
    return { kind: 'copy', text: props.copy };
  }
  if (isObject(props.set)) {
    const patch = {};
    for (const [name, value] of Object.entries(props.set)) {
      const json = toUIValue(value, 1);
      if (stateNames.has(name) && json !== undefined) setOwn(patch, name, json);
    }
    return Object.keys(patch).length > 0 ? { kind: 'set', patch } : undefined;
  }
  return undefined;
}

// ---- Helpers ----

function isObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

// Keys come from model output: a plain `target[key] = value` with "__proto__"
// would replace the object's prototype instead of adding a field.
function setOwn(target, key, value) {
  Object.defineProperty(target, key, { value, enumerable: true, writable: true, configurable: true });
}

export function isIdentifier(name) {
  return /^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name);
}

// A JSON value within the state depth limit, or undefined.
export function asUIValue(value) {
  return toUIValue(value, 1);
}

function toUIValue(value, depth) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return value;
  if (typeof value === 'number') return Number.isFinite(value) ? value : undefined;
  if (depth > BLOCK_LIMITS.stateDepth) return undefined;
  if (Array.isArray(value)) {
    const items = [];
    for (const item of value) {
      const json = toUIValue(item, depth + 1);
      if (json === undefined) return undefined;
      items.push(json);
    }
    return items;
  }
  if (isObject(value)) {
    const result = {};
    for (const [key, field] of Object.entries(value)) {
      const json = toUIValue(field, depth + 1);
      if (json === undefined) return undefined;
      setOwn(result, key, json);
    }
    return result;
  }
  return undefined;
}

function errorMessage(error) {
  if (error instanceof ExpressionError || error instanceof Error) return error.message;
  return String(error);
}
