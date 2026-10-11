/**
 * R9 前端工程化 — ESLint 平面配置（eslint 9）。
 *
 * 分层策略（存量代码现实约束）：
 * - src/js/tavern/parts/**：IIFE 闭包拼接部件（vite 虚拟模块连接，跨文件共享
 *   词法作用域），单独解析不是合法 ESM 且 no-undef 全是噪音 → 整体排除。
 * - src/js/*.js（真 ESM 模块）：correctness 规则全开（no-undef/no-dupe-keys/
 *   no-unreachable）；no-unused-vars 降为 warn —— 存量防御性 catch(e) 太多，
 *   不阻塞门禁，但新代码应保持零 warning。
 * - web/bookshelf：第三方 foliate-js，排除。
 */
import js from '@eslint/js';

const browserGlobals = {
  window: 'readonly', document: 'readonly', navigator: 'readonly',
  localStorage: 'readonly', sessionStorage: 'readonly',
  fetch: 'readonly', EventSource: 'readonly', FileReader: 'readonly',
  TextDecoder: 'readonly', TextEncoder: 'readonly', URLSearchParams: 'readonly',
  URL: 'readonly', btoa: 'readonly', atob: 'readonly',
  FormData: 'readonly', Blob: 'readonly', File: 'readonly',
  console: 'readonly', setTimeout: 'readonly', clearTimeout: 'readonly',
  setInterval: 'readonly', clearInterval: 'readonly',
  requestAnimationFrame: 'readonly', cancelAnimationFrame: 'readonly',
  history: 'readonly', location: 'readonly', CustomEvent: 'readonly', Event: 'readonly',
  alert: 'readonly', confirm: 'readonly', AbortController: 'readonly',
  ResizeObserver: 'readonly', IntersectionObserver: 'readonly', MutationObserver: 'readonly',
  crypto: 'readonly', performance: 'readonly', screen: 'readonly',
  HTMLElement: 'readonly', CSS: 'readonly', Node: 'readonly', URLPattern: 'readonly',
  Capacitor: 'readonly', globalThis: 'readonly',
  marked: 'readonly', // 第三方渲染库（CDN/全局注入）
};

export default [
  {
    ignores: [
      'web/assets/**',
      'web/bookshelf/**',
      'node_modules/**',
      'target/**',
      'src/js/tavern/parts/**',
      'dist/**',
    ],
  },
  js.configs.recommended,
  {
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: browserGlobals,
    },
    rules: {
      'no-unused-vars': ['warn', { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrors: 'none' }],
      'no-undef': 'error',
      'no-dupe-keys': 'error',
      'no-unreachable': 'error',
      'no-empty': ['warn', { allowEmptyCatch: true }],
      'no-useless-escape': 'warn',
      'no-constant-condition': ['error', { checkLoops: 'allExceptWhileTrue' }],
      'no-async-promise-executor': 'error',
    },
  },
  {
    files: ['tests/**/*.mjs', 'scripts/**/*.mjs', 'scripts/**/*.js'],
    languageOptions: {
      globals: {
        process: 'readonly', console: 'readonly', URL: 'readonly',
        require: 'readonly', Buffer: 'readonly', module: 'readonly', // browser-canary 为 CJS
      },
    },
  },
  {
    // 跨模块运行时注入的绑定（bundle 拼接作用域 / window 门面）：
    // 这些名字在拼接后由其它部件提供，调用点均有 typeof/存在性守卫。
    languageOptions: {
      globals: {
        bgSyncApplyButton: 'readonly',
        stOpenAssistModal: 'readonly',
        stFocusAssistInput: 'readonly',
        stRenderBookmarks: 'readonly',
        stRefreshImmerseBg: 'readonly',
        sgRefreshChapters: 'readonly',
        sgWire: 'readonly',
        Co: 'readonly', // aiadmin: 全局遥测/观察者（typeof 守卫）
      },
    },
  },
];
