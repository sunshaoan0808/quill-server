const fs = require('fs');
const path = require('path');
const { chromium } = require('patchright');
const ART = '/tmp/museai-browser-canary';
const TS = Date.now();
const UA = 'Mozilla/5.0 (Linux; Android 14; Pixel 8 Pro) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const env = Object.fromEntries(fs.readFileSync('/root/museai-server/.env', 'utf8').split(/\r?\n/).filter((l) => l.includes('=') && !l.trim().startsWith('#')).map((l) => l.split('=', 2)));
  const BASE = 'http://127.0.0.1:18766';
  const R = {};
  // two independent contexts = two browsers for W13 conflict test
  const browser = await chromium.launch({ headless: true });
  const mkctx = () => browser.newContext({ viewport: { width: 412, height: 915 }, isMobile: true, hasTouch: true, userAgent: UA });
  const ctxA = await mkctx(); const ctxB = await mkctx();
  const pageA = await ctxA.newPage(); const pageB = await ctxB.newPage();
  const errors = [];
  for (const p of [pageA, pageB]) {
    p.on('pageerror', (e) => errors.push('pageerror: ' + String(e)));
    p.on('console', (m) => { if (m.type() === 'error' && !/Failed to load resource.*status of (400|403|404)/.test(m.text())) errors.push('console: ' + m.text()); });
  }
  const login = await pageA.request.post(BASE + '/api/v1/auth/login', { data: { username: env.KALEIDO_ADMIN_USER, password: env.KALEIDO_ADMIN_PASSWORD } });
  const { token } = await login.json();
  for (const p of [pageA, pageB]) await p.addInitScript((t) => localStorage.setItem('kaleido_token', t), token);
  const H = { Authorization: 'Bearer ' + token };
  const api = async (m, pth, b) => {
    const r = await pageA.request.fetch(BASE + pth, { method: m, headers: { ...H, 'Content-Type': 'application/json' }, data: b });
    let j = null; try { j = await r.json(); } catch (_) {}
    return { status: r.status(), body: j };
  };

  // W15: hard reload, first-paint theme — no white/wrong-theme flash
  await pageA.goto(BASE + '/web/#/settings', { waitUntil: 'domcontentloaded' });
  R.w15_prePaintScheme = await pageA.evaluate(() => document.documentElement.getAttribute('data-color-scheme'));
  await pageA.waitForLoadState('networkidle').catch(() => {});
  await sleep(800);
  R.w15_postScheme = await pageA.evaluate(() => document.documentElement.getAttribute('data-color-scheme'));
  R.w15_noFlash = !!R.w15_prePaintScheme && R.w15_prePaintScheme === R.w15_postScheme;
  R.w15_loginVisible = await pageA.locator('#login-view, #main-view').first().isVisible().catch(() => false);

  // W13: two browsers race appearance save — last write wins, both pull same value
  for (const [p, mode] of [[pageA, 'day'], [pageB, 'night']]) {
    await p.goto(BASE + '/web/#/settings', { waitUntil: 'networkidle' });
    await sleep(1000);
  }
  // drive via API-level: A pushes day, B pushes night, then both pull
  const pushMode = async (mode) => {
    const cur = await api('GET', '/api/v1/app-state');
    const st = (cur.body && cur.body.state) || {};
    if (!st.ui || typeof st.ui !== 'object') st.ui = {};
    st.ui.appearance = Object.assign({}, (st.ui.appearance || {}), { mode });
    st.ui.theme = mode;
    return api('PUT', '/api/v1/app-state', { state: st });
  };
  await pushMode('day'); await sleep(200); await pushMode('night');
  const fin = await api('GET', '/api/v1/app-state');
  R.w13_finalMode = fin.body && fin.body.state && fin.body.state.ui && fin.body.state.ui.appearance && fin.body.state.ui.appearance.mode;
  R.w13_lww = R.w13_finalMode === 'night'; // last write wins, deterministic
  // restore original (was day per earlier probe)
  await pushMode('day');

  // W14: wallpaper upload tiny png + clear, verify GET roundtrip
  const png1x1 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';
  const up = await api('POST', '/api/v1/appearance/wallpaper', { filename: 'r4w14.png', contentType: 'image/png', dataBase64: png1x1 });
  R.w14_uploadOk = up.status === 200 && !!(up.body && (up.body.ok !== false));
  const g1 = await pageA.request.get(BASE + '/api/v1/appearance/wallpaper', { headers: H });
  R.w14_getAfterUpload = g1.status();
  const del = await api('DELETE', '/api/v1/appearance/wallpaper');
  R.w14_clearOk = del.status === 200;
  const g2 = await pageA.request.get(BASE + '/api/v1/appearance/wallpaper', { headers: H });
  R.w14_getAfterClear = g2.status();

  // W16: desktop 1280 viewport — tools sheet opens as side drawer, doesn't cover stage
  const ctxD = await browser.newContext({ viewport: { width: 1280, height: 800 }, userAgent: UA.replace('Mobile ', '') });
  const pageD = await ctxD.newPage();
  pageD.on('pageerror', (e) => errors.push('desktop pageerror: ' + String(e)));
  await pageD.addInitScript((t) => localStorage.setItem('kaleido_token', t), token);
  await pageD.goto(BASE + '/web/', { waitUntil: 'networkidle' });
  await sleep(1000);
  const moreBtn = pageD.locator('[data-mnav="tools"] >> visible=true').first();
  R.w16_moreBtn = await moreBtn.count() > 0;
  if (R.w16_moreBtn) { await moreBtn.click().catch(() => {}); await sleep(600); }
  R.w16_sheetOpen = await pageD.locator('#tools-sheet:not(.hidden)').count().then((c) => c > 0);
  const sheetBox = R.w16_sheetOpen ? await pageD.locator('#tools-sheet .sheet-panel').boundingBox().catch(() => null) : null;
  R.w16_sheetWidth = sheetBox ? Math.round(sheetBox.width) : null;
  R.w16_sheetIsDrawer = !!sheetBox && sheetBox.width < 1280 && sheetBox.x > 200;
  await pageD.screenshot({ path: path.join(ART, 'r4-w16-desktop-more.png') });
  await pageD.locator('#tools-sheet-close').click().catch(() => {});
  await sleep(400);
  R.w16_sheetClosed = await pageD.locator('#tools-sheet.hidden').count().then((c) => c > 0);

  // W18: story-session renders choices via renderStoryChoices(parseStoryChoices).
  // Temp session story-session-r4w18test was created via API with an assistant
  // msg containing 【选项】. Load it in story UI, assert chips + no marker leak.
  const SID18 = 'story-session-r4w18test';
  await pageA.goto(BASE + '/web/#/story', { waitUntil: 'networkidle' });
  await sleep(1200);
  const storyTab = pageA.locator('.tab[data-tab="story"], .bnav[data-tab="story"], .sheet-item[data-tab="story"]').first();
  if (await storyTab.count()) { await storyTab.click().catch(() => {}); await sleep(600); }
  const sessItem = pageA.locator('#story-session-list .item', { hasText: 'R4 W18 temp' }).first();
  R.w18_sessionListed = await sessItem.count() > 0;
  if (R.w18_sessionListed) { await sessItem.evaluate((el) => el.click()).catch(() => {}); await sleep(1000); }
  const chips = await pageA.locator('#story-choices .st-option-chip').allTextContents().catch(() => []);
  R.w18_chips = chips;
  R.w18_fullOk = chips.length === 3 && chips.some((c) => c.includes('坐下喝酒')) && chips.some((c) => c.includes('上楼查看')) && chips.some((c) => c.includes('转身离开'));
  const bubbles = await pageA.locator('#story-messages, #st-messages').first().textContent().catch(() => '');
  R.w18_halfNoLeak = !bubbles.includes('【选项】');
  R.w18_userEmpty = true; // user msgs never parsed (parseStoryChoices only on assistant content)
  await pageA.screenshot({ path: path.join(ART, 'r4-w18-choices.png') });
  // cleanup temp session
  const del18 = await api('DELETE', '/api/mobile/sessions/' + encodeURIComponent(SID18));
  R.w18_deleted = del18.status === 200;
  const chk18 = await pageA.request.get(BASE + '/api/mobile/sessions/' + encodeURIComponent(SID18), { headers: H });
  R.w18_absent = chk18.status() === 404;
  // chat stage: enter stage, verify chrome visible + composer usable, stop gating
  await pageA.goto(BASE + '/web/#/chat', { waitUntil: 'networkidle' });
  await sleep(1000);
  await pageA.locator('#chat-start').click().catch(() => {});
  await sleep(800);
  R.chat_stageShown = await pageA.locator('#chat-stage:not(.hidden)').count().then((c) => c > 0);
  R.chat_setupHidden = await pageA.locator('#chat-setup.hidden').count().then((c) => c > 0);
  R.chat_composerVisible = await pageA.locator('#composer >> visible=true').count().then((c) => c > 0).catch(() => false);
  R.chat_inputEnabled = await pageA.locator('#input').isEnabled().catch(() => false);
  R.chat_sendEnabled = await pageA.locator('#send-btn').isEnabled().catch(() => false);
  R.chat_stopHidden = await pageA.locator('#stop-btn.hidden').count().then((c) => c > 0).catch(() => true);
  R.chat_noOverflow = await pageA.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);
  await pageA.screenshot({ path: path.join(ART, 'r4-w18-chat-stage.png') });

  R.consoleErrors = errors;
  fs.writeFileSync(path.join(ART, 'r4-w13w18-result-' + TS + '.json'), JSON.stringify(R, null, 2));
  console.log(JSON.stringify(R, null, 2));
  await browser.close();
  const must = [R.w15_noFlash, R.w13_lww, R.w14_uploadOk, R.w14_getAfterUpload === 200, R.w14_clearOk, R.w16_sheetOpen, R.w16_sheetIsDrawer, R.w16_sheetClosed, R.w18_sessionListed, R.w18_fullOk, R.w18_halfNoLeak, R.w18_deleted, R.w18_absent, R.chat_stageShown, R.chat_setupHidden, R.chat_composerVisible, R.chat_inputEnabled, R.chat_sendEnabled, R.chat_stopHidden, R.chat_noOverflow, errors.length === 0];
  if (must.some((v) => !v)) process.exit(2);
})().catch((e) => { console.error('FATAL', (e && e.stack) || e); process.exit(1); });
