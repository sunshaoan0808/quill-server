const fs = require('fs');
const path = require('path');
const { chromium } = require('patchright');
const ART = '/tmp/museai-browser-canary';
const TS = Date.now();
const UA = 'Mozilla/5.0 (Linux; Android 14; Pixel 8 Pro) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function expect_text(page, sel, re, ms) {
  const t0 = Date.now();
  for (;;) {
    const t = (await page.locator(sel).textContent().catch(() => '')) || '';
    if (re.test(t.trim())) return t.trim();
    if (Date.now() - t0 > ms) throw new Error('timeout ' + sel + ' got: ' + t.trim().slice(0, 160));
    await sleep(250);
  }
}
(async () => {
  const env = Object.fromEntries(fs.readFileSync('/root/museai-server/.env', 'utf8').split(/\r?\n/).filter((l) => l.includes('=') && !l.trim().startsWith('#')).map((l) => l.split('=', 2)));
  const BASE = 'http://127.0.0.1:18766';
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 412, height: 915 }, isMobile: true, hasTouch: true, userAgent: UA });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push('pageerror: ' + String(e)));
  page.on('console', (m) => { if (m.type() === 'error' && !/Failed to load resource.*status of (400|403|404)/.test(m.text())) errors.push('console: ' + m.text()); });
  const login = await page.request.post(BASE + '/api/v1/auth/login', { data: { username: env.KALEIDO_ADMIN_USER, password: env.KALEIDO_ADMIN_PASSWORD } });
  const { token } = await login.json();
  await page.addInitScript((t) => localStorage.setItem('kaleido_token', t), token);
  const H = { Authorization: 'Bearer ' + token };
  const api = async (m, p, b) => {
    const r = await page.request.fetch(BASE + p, { method: m, headers: { ...H, 'Content-Type': 'application/json' }, data: b });
    let j = null; try { j = await r.json(); } catch (_) {}
    return { status: r.status(), body: j };
  };
  const R = {};
  // W10 UI: agent tab defaults + read/list works against new default path
  await page.goto(BASE + '/web/#/agent', { waitUntil: 'networkidle' });
  await sleep(1200);
  const tabBtn = page.locator('.tab[data-tab="agent"], .bnav[data-tab="agent"], .sheet-item[data-tab="agent"]').first();
  if (await tabBtn.count()) { await tabBtn.click().catch(() => {}); await sleep(600); }
  R.defaultPath = await page.locator('#agent-path').inputValue().catch(() => '');
  R.safetyCopy = await page.locator('#tab-agent').textContent().then((t) => t.includes('二次确认')).catch(() => false);
  await page.locator('#agent-list').click();
  await expect_text(page, '#agent-msg', /./, 10000).catch(() => '');
  await sleep(600);
  R.listOut = (await page.locator('#agent-out').textContent()).slice(0, 120);
  R.listOk = R.listOut.includes('entries') || (await page.locator('#agent-msg').textContent()).trim() === '';
  await page.locator('#agent-path').fill('outlines/demo-outline.md');
  await page.locator('#agent-read').click();
  await sleep(800);
  R.readOut = (await page.locator('#agent-out').textContent()).slice(0, 120);
  R.readOk = R.readOut.includes('demo') || R.readOut.includes('content') || (await page.locator('#agent-msg').textContent()).trim() === '';
  // write opens confirm dialog -> cancel it (no side effect), assert no request needed
  await page.locator('#agent-path').fill('outlines');
  await page.locator('#agent-write').click();
  await sleep(500);
  R.writeConfirmShown = await page.locator('.kaleido-dialog-backdrop').count().then((c) => c > 0);
  if (R.writeConfirmShown) { await page.locator('#kaleido-dialog-cancel').click(); await sleep(400); }
  // bash opens confirm -> cancel
  await page.locator('#agent-bash').click();
  await sleep(500);
  R.bashConfirmShown = await page.locator('.kaleido-dialog-backdrop').count().then((c) => c > 0);
  if (R.bashConfirmShown) { await page.locator('#kaleido-dialog-cancel').click(); await sleep(400); }
  await page.screenshot({ path: path.join(ART, 'r4-w10-agent.png') });
  // W10 settings switches exist + reflect server truth
  await page.goto(BASE + '/web/#/settings', { waitUntil: 'networkidle' });
  await sleep(1200);
  const setBtn = page.locator('.tab[data-tab="settings"], .bnav[data-tab="settings"], .sheet-item[data-tab="settings"]').first();
  if (await setBtn.count()) { await setBtn.click().catch(() => {}); await sleep(600); }
  const s = await api('GET', '/api/v1/settings');
  const sv = s.body.settings || s.body;
  R.swTools = await page.locator('#set-agent-tools').isChecked().catch(() => null);
  R.swWrite = await page.locator('#set-agent-write').isChecked().catch(() => null);
  R.swBash = await page.locator('#set-agent-bash').isChecked().catch(() => null);
  R.swConfirm = await page.locator('#set-agent-confirm').isChecked().catch(() => null);
  R.serverTruth = { t: sv.agentToolsEnabled, w: sv.agentWriteEnabled, b: sv.bashSandboxEnabled, c: sv.agentConfirmDangerous };
  R.switchesMatch = (R.swTools === !!sv.agentToolsEnabled) && (R.swWrite === !!sv.agentWriteEnabled) && (R.swBash === !!sv.bashSandboxEnabled) && (R.swConfirm === !!sv.agentConfirmDangerous);
  await page.screenshot({ path: path.join(ART, 'r4-w10-settings.png') });
  // W12: auth sessions stats readable (read-only)
  const stats = await api('GET', '/api/v1/auth/sessions/stats');
  R.statsOk = stats.status === 200 && stats.body && stats.body.ok === true;
  R.statsFields = stats.body ? { active: stats.body.active, cap: stats.body.cap, policy: stats.body.policy, actions: !!stats.body.actions } : null;
  R.consoleErrors = errors;
  fs.writeFileSync(path.join(ART, 'r4-w10w12-result-' + TS + '.json'), JSON.stringify(R, null, 2));
  console.log(JSON.stringify(R, null, 2));
  await browser.close();
  const must = [R.defaultPath === 'outlines', R.listOk, R.readOk, R.writeConfirmShown, R.bashConfirmShown, R.switchesMatch, R.statsOk, errors.length === 0];
  if (must.some((v) => !v)) process.exit(2);
})().catch((e) => { console.error('FATAL', (e && e.stack) || e); process.exit(1); });
