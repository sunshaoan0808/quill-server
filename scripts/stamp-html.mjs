#!/usr/bin/env node
/**
 * Cache-bust stamp for web/index.html (the sole build chain: vite build → this).
 * Mobile WebView caches ?v= URLs; without a new stamp updates never reach devices.
 * [2026-10-04] Bumps EVERY numeric ?v= stamp (was app.js/styles.css only), so the
 * hand-written asset scripts (scene_cards / dual_agent / … ) get fresh URLs too.
 */
import fs from 'node:fs';

const indexPath = new URL('../web/index.html', import.meta.url);
const stamp = Math.floor(Date.now() / 1000);
let html = fs.readFileSync(indexPath, 'utf8');
html = html.replace(/(\?v=)\d+/g, '$1' + stamp);
fs.writeFileSync(indexPath, html);
console.log(`✓ stamped index.html v=${stamp}`);
