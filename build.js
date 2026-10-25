/**
 * Kaleido build script — esbuild bundler
 * Outputs to web/assets/app.js + web/assets/styles.css
 *
 * S9.20: JS split into 16 part files, concatenated at build time.
 * S2.19: parts.json retired; the IIFE body is empty EXCEPT a keep-alive
 *        reference to every imported binding (rollup/esbuild tree-shake
 *        otherwise deletes them, and closure-hoisted consumers rely on them).
 *        The keep-alive set is computed from the same 26 import lines that
 *        vite.config.mjs's appPartsVirtual() plugin uses — single ground truth.
 */
const esbuild = require('esbuild');
const path = require('path');
const fs = require('fs');

// Single source of truth for the converted ESM imports.
// MUST stay in sync with vite.config.mjs appPartsVirtual() plugin's `converted` array.
const CONVERTED_IMPORTS = [
  'import { $ } from "./dom.js";',
  'import { initTavernUI } from "./tavern.js"; initTavernUI();',
  'import { username, setUsername, loginView, mainView, loginErr, messagesEl, sessionList, input, stopBtn, sendBtn, ST_VISIBLE_TURNS, stHistoryExpanded } from "./state.js";',
  'import { PLAYABLE_LABELS, ST_ICONS, stripChoicesBlock, resolveMessageOptions } from "./utils.js";',
  'import { stApi, stStatus, stGoBack, stSwitchView, stDisplayTitle, stHasOpenOverlay, stBindImmChrome, stRefresh, stLoadPacks, stLoadSessions, stLoadSaves, stLoadSession, stRefreshCharSummary, stRenderContinueCard, renderHomeRecent, loadBookshelf } from "./tavern.js";',
  'import { setPanel, refreshJobs, initJobsUI } from "./jobs.js"; initJobsUI();',
  'import { P5AiLoad } from "./aiadmin.js";',
  'import { MoaLoadPanels, MoaLoadSessions } from "./moa.js";',
  'import { loadAnKinds, loadAnTasks, loadGraph, loadForeshadows } from "./insight.js";',
  'import { advShowReader, advShowSetup, ensureStorySession, renderBondPage, renderStoryMessages } from "./story.js";',
  'import { renderAsVisual, loadEmbedLabStatus, loadEmbedLabEvents } from "./agent.js";',
  'import { loadPartner } from "./partner.js";',
  'import { loadAuthorProjects, loadWorksTree, refreshPackSelect, loadWorksVersionsSidebar, sgRefreshChapters, sgWire } from "./authoring.js";',
  'import { friendlyError, apiBase, showLogin, showMain } from "./api_shell.js";',
  'import { getActiveStRegexScripts, compileStFindRegex, applyStRegexScripts } from "./st_regex.js";',
  'import { _bindPartner as __bp } from "./st_regex.js"; __bp(() => (window.__kaleidoChatState ? window.__kaleidoChatState.partner : null));',
  'import { wireListSearch, openGlobalSearch, closeGlobalSearch } from "./search.js";',
  'import { loadSettings, loadStylePresets } from "./settings.js";',
  'import { renderMessages, refreshSessions, showChatSetup, setupChatStart, ensureSession, saveSession, closeEs, setStreaming, scheduleChatStreamPaint, cssEscape, buildBubbleEl, fillBubbleBody, ensureBubbleDom, refreshPartnerSelects, refreshStorySelects, refreshAdventureSelects, sendMessage, stopStream } from "./chat.js";',
  'import "./tabs.js";',
  'import { switchTab as __tabSwitchTab, switchAzView as __tabSwitchAzView, applyAutoUi as __tabApplyAutoUi, parseLocationHash as __tabParseLocationHash, getCurrentTab as __tabGetCurrentTab } from "./tabs_bridge.js";',
  'export { friendlyError, apiBase, showLogin, showMain, getActiveStRegexScripts, compileStFindRegex, applyStRegexScripts, wireListSearch, openGlobalSearch, closeGlobalSearch };',
  'import { api as _apiReal, setApiBase as _setApiBaseReal, getSseTicket as _getSseTicketReal } from "./api.js";',
  'const api = _apiReal, setApiBase = _setApiBaseReal, getSseTicket = _getSseTicketReal;',
];

async function build() {
  // --- JS: assemble converted ESM imports + keep-alive IIFE ---
  const partDir = path.join(__dirname, 'src/js');

  // Read index.js to get base imports (kept verbatim as the IIFE preamble)
  const indexJs = fs.readFileSync(path.join(partDir, 'index.js'), 'utf8');
  const importLines = indexJs.split('\n').filter(l => l.startsWith('import '));

  // Extract every imported binding name (handles `as` aliases) for keep-alive
  const keepNames = new Set();
  for (const line of CONVERTED_IMPORTS) {
    const m = line.match(/import\s*\{([^}]*)\}\s*from\s*["']/);
    if (!m) continue;
    for (const part of m[1].split(',')) {
      const name = part.trim().split(/\s+as\s+/).pop().trim();
      if (name) keepNames.add(name);
    }
  }
  const body = `globalThis.__appParts={${[...keepNames].join(',')}};`;

  // Assemble: index.js preamble + converted imports + IIFE(body)
  const assembled =
    importLines.join('\n') + '\n' +
    CONVERTED_IMPORTS.join('\n') + '\n' +
    `(function () {\n${body}\n})();\n`;

  // Write assembled file to temp location
  const tempPath = path.join(partDir, '_assembled.js');
  fs.writeFileSync(tempPath, assembled);

  // Bundle with esbuild
  await esbuild.build({
    entryPoints: [tempPath],
    bundle: true,
    minify: true,
    outfile: path.join(__dirname, 'web/assets/app.js'),
    target: ['es2020'],
    format: 'iife',
  });

  // Clean up temp
  fs.unlinkSync(tempPath);

  // --- CSS bundle ---
  await esbuild.build({
    entryPoints: [path.join(__dirname, 'src/css/main.css')],
    bundle: true,
    minify: true,
    outfile: path.join(__dirname, 'web/assets/styles.css'),
    loader: { '.woff2': 'copy', '.woff': 'copy' },
  });

  const jsStat = fs.statSync(path.join(__dirname, 'web/assets/app.js'));
  const cssStat = fs.statSync(path.join(__dirname, 'web/assets/styles.css'));
  console.log(`✓ built  app.js  (${(jsStat.size / 1024).toFixed(0)} KB)`);
  console.log(`✓ built  styles.css  (${(cssStat.size / 1024).toFixed(0)} KB)`);

  // --- Stamp cache-bust version into web/index.html (mobile WebView caches ?v= URL; without a new stamp, updates never reach devices) ---
  const indexPath = path.join(__dirname, 'web/index.html');
  const stamp = Math.floor(Date.now() / 1000);
  let html = fs.readFileSync(indexPath, 'utf8');
  html = html.replace(/(app\.js\?v=)\d+/g, '$1' + stamp);
  html = html.replace(/(styles\.css\?v=)\d+/g, '$1' + stamp);
  fs.writeFileSync(indexPath, html);
  console.log(`✓ stamped index.html v=${stamp}`);
}

build().catch((err) => {
  console.error('build failed:', err);
  process.exit(1);
});
