const fs = require('fs');
const path = require('path');
const { chromium } = require('patchright');

const ART = '/tmp/museai-browser-canary';
const TS = Date.now();
const ANDROID_UA = 'Mozilla/5.0 (Linux; Android 14; Pixel 8 Pro) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  const env = Object.fromEntries(
    fs.readFileSync('/root/museai-server/.env', 'utf8')
      .split(/\r?\n/)
      .filter((line) => line.includes('=') && !line.trim().startsWith('#'))
      .map((line) => line.split('=', 2))
  );
  const BASE = 'http://127.0.0.1:18766';

  const switchView = async (v) => {
    const btn = page.locator(`button[data-azview="${v}"] >> visible=true`).first();
    await btn.click();
    await sleep(400);
  };

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 412, height: 915 },
    isMobile: true,
    hasTouch: true,
    userAgent: ANDROID_UA,
  });
  const page = await context.newPage();

  // ── auth ──
  const login = await page.request.post(BASE + '/api/v1/auth/login', {
    data: { username: env.KALEIDO_ADMIN_USER, password: env.KALEIDO_ADMIN_PASSWORD },
  });
  const loginBody = await login.json();
  const token = loginBody.token;
  await page.addInitScript((t) => localStorage.setItem('kaleido_token', t), token);
  const authGet = async (p) => {
    const r = await page.request.get(BASE + p, { headers: { Authorization: 'Bearer ' + token } });
    return { status: r.status(), body: await r.json().catch(() => null) };
  };
  const authSend = async (method, p, body) => {
    const r = await page.request.fetch(BASE + p, {
      method,
      headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' },
      data: body,
    });
    return { status: r.status(), body: await r.json().catch(() => null) };
  };

  const errors = [];
  page.on('pageerror', (e) => errors.push('pageerror: ' + String(e)));
  page.on('console', (m) => { if (m.type() === 'error') errors.push('console: ' + m.text()); });

  const results = { wb: {}, entries: {}, cc: {}, ccr: {}, rx: {}, cleanup: {}, consoleErrors: errors };
  const stamp = String(TS).slice(-6);
  const WB_NAME = 'R4 deep WB ' + stamp;
  const CC_NAME = 'R4 deep CC ' + stamp;
  const RX_NAME = 'R4 temp rx ' + stamp;
  let wbId = '', ccId = '', entryUid = '';

  const msg = async (sel) => (await page.locator(sel).textContent()).trim();
  const confirmDialog = async () => {
    await page.locator('#kaleido-dialog-ok').click();
    await sleep(500);
  };

  await page.goto(BASE + '/web/#/partner', { waitUntil: 'networkidle' });
  await sleep(1000);
  await switchView('worldbook');
  await sleep(500);
  await page.locator('#wb-new').waitFor({ state: 'visible' });

  // ── 1. World book create ──
  await page.locator('#wb-new').click();
  await page.locator('#wb-name').fill(WB_NAME);
  await page.locator('#wb-theme').fill('R4 deep regression');
  await page.locator('#wb-content').fill('R4 legacy constant content ' + stamp + '.');
  await page.locator('#wb-save').click();
  await expect_text(page, '#wb-msg', /已保存/, 10000);
  wbId = (await msg('#wb-msg')).replace('已保存 ', '').trim();
  results.wb.id = wbId;

  // ── 2. W8 entry create ──
  await page.locator('#wbe-comment').fill('R4 entry ' + stamp);
  await page.locator('#wbe-keys').fill('r4key' + stamp);
  await page.locator('#wbe-content').fill('R4 entry content ' + stamp);
  await page.locator('#wbe-order').fill('120');
  await page.locator('#wbe-save').click();
  await expect_text(page, '#wbe-msg', /条目已保存/, 10000);
  const entryList1 = await page.locator('#wb-entry-list').textContent();
  results.entries.createListed = entryList1.includes('R4 entry ' + stamp);

  // reload → select → verify persistence
  await page.locator('#wb-entry-reload').click();
  await sleep(600);
  await page.locator('#wb-entry-list .item', { hasText: 'R4 entry ' + stamp }).first().click();
  await sleep(300);
  results.entries.keysReloaded = (await page.locator('#wbe-keys').inputValue()).includes('r4key' + stamp);
  results.entries.orderReloaded = (await page.locator('#wbe-order').inputValue()) === '120';
  const entriesApi0 = await authGet('/api/v1/partner/world-books/' + encodeURIComponent(wbId) + '/entries');
  const eRow0 = (entriesApi0.body.entries || []).find((e) => JSON.stringify(e).includes('r4key' + stamp));
  entryUid = eRow0 ? String(eRow0.uid || eRow0.id || eRow0.key || '') : '';
  results.entries.uid = entryUid;

  // PATCH: set constant + vectorized flags, save
  await page.locator('#wbe-constant').check();
  await page.locator('#wbe-save').click();
  await expect_text(page, '#wbe-msg', /条目已保存/, 10000);
  await page.locator('#wb-entry-reload').click();
  await sleep(600);
  await page.locator('#wb-entry-list .item', { hasText: 'R4 entry ' + stamp }).first().click();
  await sleep(300);
  results.entries.constantPersisted = await page.locator('#wbe-constant').isChecked();

  // API readback of entries
  const entriesApi = await authGet('/api/v1/partner/world-books/' + encodeURIComponent(wbId) + '/entries');
  results.entries.apiCount = entriesApi.body.entries.length;
  results.entries.apiHasKey = JSON.stringify(entriesApi.body).includes('r4key' + stamp);

  // WI preview
  await page.locator('#wbe-preview').click();
  await expect_text(page, '#wbe-msg', /预览完成/, 15000);
  const previewMsg = await msg('#wbe-msg');
  results.entries.previewMsg = previewMsg;
  results.entries.previewHasJson = (await page.locator('#wbe-preview-out').textContent()).trim().startsWith('{');

  // rebuild ST book (legacy content → constant entry)
  await page.locator('#wb-rebuild').click();
  await expect_text(page, '#wb-msg', /重建完成/, 15000);
  results.entries.rebuildMsg = await msg('#wb-msg');
  const entriesApi2 = await authGet('/api/v1/partner/world-books/' + encodeURIComponent(wbId) + '/entries');
  results.entries.apiCountAfterRebuild = entriesApi2.body.entries.length;
  results.entries.rebuildProducedEntries = entriesApi2.body.entries.length >= 1;
  await page.screenshot({ path: path.join(ART, 'r4-deep-1-wb-entries.png') });

  // delete the created entry with confirm
  await page.locator('#wb-entry-list .item', { hasText: 'R4 entry ' + stamp }).first().click().catch(() => {});
  await sleep(200);
  if (await page.locator('#wbe-comment').inputValue().then((v) => v.includes('R4 entry ' + stamp)).catch(() => false)) {
    await page.locator('#wbe-del').click();
    await sleep(200);
    await confirmDialog();
    await expect_text(page, '#wbe-msg', /已删除|条目\s*0/, 10000);
  }
  const entriesApi3 = await authGet('/api/v1/partner/world-books/' + encodeURIComponent(wbId) + '/entries');
  results.entries.remainingAfterUiDelete = entriesApi3.body.entries
    .filter((e) => JSON.stringify(e).includes('r4key' + stamp)).length;

  // ── 3. Character card create with card regex ──
  await switchView('charcard'); await sleep(500);
  await sleep(600);
  await page.locator('#cc-new').click();
  await page.locator('#cc-name').fill(CC_NAME);
  await page.locator('#cc-occupation').fill('R4 tester');
  await page.locator('#cc-description').fill('R4 character description ' + stamp);
  await page.locator('#cc-first-mes').fill('R4 greeting ' + stamp);
  await page.locator('#cc-content').fill('R4 card content ' + stamp);
  // link WB by value (select is refreshed after wb save)
  const wbOpts = await page.locator('#cc-wb option').evaluateAll((os) => os.map((o) => o.value));
  if (wbOpts.includes(wbId)) await page.selectOption('#cc-wb', wbId);
  // card regex: add via UI local buffer
  await page.locator('#cc-regex-new').click();
  await page.locator('#ccr-name').fill('r4-card-rx ' + stamp);
  await page.locator('#ccr-find').fill('alpha');
  await page.locator('#ccr-replace').fill('β');
  await page.locator('#ccr-save').click();
  await sleep(400);
  results.ccr.listShown = (await page.locator('#cc-regex-list').textContent()).includes('r4-card-rx ' + stamp);
  // in-browser regex test
  await page.locator('#ccr-sample').fill('alpha and alpha');
  await page.locator('#ccr-test').click();
  await sleep(400);
  results.ccr.testOutput = (await page.locator('#ccr-test-out').textContent()).trim();
  results.ccr.testReplaced = results.ccr.testOutput.includes('β and β');

  await page.locator('#cc-save').click();
  await expect_text(page, '#cc-msg', /已保存/, 10000);
  ccId = (await msg('#cc-msg')).replace('已保存 ', '').trim();
  results.cc.id = ccId;

  // reload page → card persistence
  await page.reload({ waitUntil: 'networkidle' });
  await sleep(1200);
  await switchView('charcard'); await sleep(500);
  await sleep(600);
  await page.locator('#cc-list .item', { hasText: CC_NAME }).first().click();
  await sleep(400);
  results.cc.nameReloaded = (await page.locator('#cc-name').inputValue()) === CC_NAME;
  results.cc.descReloaded = (await page.locator('#cc-description').inputValue()).includes(stamp);
  results.cc.linkedWb = (await page.locator('#cc-wb').inputValue()) === wbId;
  results.cc.regexReloaded = (await page.locator('#cc-regex-list').textContent()).includes('r4-card-rx ' + stamp);
  const ccApi = await authGet('/api/v1/partner');
  const ccRow = ccApi.body.characterCards.find((c) => c.id === ccId);
  results.cc.apiFieldsPersisted = !!(ccRow && ccRow.fields && ccRow.fields.description && ccRow.fields.description.includes(stamp));
  results.cc.apiRegexCount = ccRow && ccRow.fields && Array.isArray(ccRow.fields.stRegexScripts) ? ccRow.fields.stRegexScripts.length : -1;

  // rebuild linked WB from card UI
  await page.locator('#cc-rebuild-wb').click();
  await expect_text(page, '#cc-msg', /重建完成/, 20000);
  results.cc.rebuildMsg = await msg('#cc-msg');

  // ST export — wait until output panel populated or message shows an error
  await page.locator('#cc-st-export').click();
  {
    const start = Date.now();
    for (;;) {
      const out = (await page.locator('#cc-st-out').textContent().catch(() => '')) || '';
      const cm = (await page.locator('#cc-msg').textContent().catch(() => '')) || '';
      if (out.trim().length > 50) break;
      if (/失败|错误|Error/i.test(cm) && !/保存中|重建/.test(cm)) break;
      if (Date.now() - start > 15000) break;
      await sleep(300);
    }
  }
  const stOut = await page.locator('#cc-st-out').textContent();
  results.cc.stExportHasName = stOut.includes(CC_NAME);
  results.cc.stExportHasPng = stOut.includes('pngBase64') || (await page.locator('#cc-st-out').textContent()).length > 200;
  await page.screenshot({ path: path.join(ART, 'r4-deep-2-cc.png') });

  // ── 4. Global regex library (W6) ──
  await switchView('regex'); await sleep(1200);
  await sleep(1200);
  const libBefore = (await page.locator('#rx-list').textContent());
  await page.locator('#rx-new').click();
  await page.locator('#rx-name').fill(RX_NAME);
  await page.locator('#rx-find').fill('gamma');
  await page.locator('#rx-replace').fill('Γ');
  // NOTE: #rx-disabled label reads 启用 but checked=true maps to disabled; uncheck to keep enabled
  await page.locator('#rx-disabled').uncheck();
  await page.locator('#rx-sample').fill('gamma gamma');
  await page.locator('#rx-test').click();
  await sleep(400);
  results.rx.testOutput = (await page.locator('#rx-test-out').textContent()).trim();
  results.rx.testReplaced = results.rx.testOutput.includes('Γ Γ');
  results.rx.disabledLabelSuspicious = true; // 启用 checkbox checked ⇒ disabled=true in collectRxForm
  await page.locator('#rx-save-lib').click();
  await expect_text(page, '#rx-msg', /整库已保存/, 10000);
  results.rx.savedMsg = await msg('#rx-msg');
  // reload library and verify persistence
  await page.locator('#rx-reload-library').click();
  await sleep(800);
  results.rx.listedAfterReload = (await page.locator('#rx-list').textContent()).includes(RX_NAME);
  const rxApi = await authGet('/api/v1/regex-library');
  const rxRow = rxApi.body.scripts.find((s) => (s.scriptName || s.name) === RX_NAME);
  results.rx.apiPersisted = !!rxRow;
  results.rx.apiDisabledFlag = rxRow ? !!rxRow.disabled : null;
  results.rx.libCountBeforeCleanup = rxApi.body.scripts.length;

  // remove temp script through UI, persist (preserves pre-existing library scripts)
  await page.locator('#rx-list .item', { hasText: RX_NAME }).first().click();
  await sleep(300);
  await page.locator('#rx-del').click();
  await sleep(300);
  await page.locator('#rx-save-lib').click();
  await expect_text(page, '#rx-msg', /整库已保存/, 10000);
  const rxApi2 = await authGet('/api/v1/regex-library');
  results.rx.tempRemoved = !rxApi2.body.scripts.some((s) => (s.scriptName || s.name) === RX_NAME);
  results.rx.libCountAfterCleanup = rxApi2.body.scripts.length;
  results.rx.preExistingPreserved = rxApi2.body.scripts.length === rxApi.body.scripts.length - 1;
  await page.screenshot({ path: path.join(ART, 'r4-deep-3-rx.png') });

  // ── 5. Cleanup: delete CC then WB via UI confirm ──
  await switchView('charcard'); await sleep(500);
  await sleep(500);
  await page.locator('#cc-list .item', { hasText: CC_NAME }).first().click();
  await sleep(300);
  await page.locator('#cc-del').click();
  await sleep(200);
  await confirmDialog();
  await sleep(800);

  await switchView('worldbook'); await sleep(500);
  await sleep(500);
  await page.locator('#wb-list .item', { hasText: WB_NAME }).first().click();
  await sleep(300);
  await page.locator('#wb-del').click();
  await sleep(200);
  await confirmDialog();
  await sleep(800);

  const finalApi = await authGet('/api/v1/partner');
  results.cleanup.ccRemaining = finalApi.body.characterCards.filter((c) => c.name === CC_NAME).length;
  results.cleanup.wbRemaining = finalApi.body.worldBooks.filter((w) => w.name === WB_NAME).length;

  results.consoleErrors = errors;
  fs.writeFileSync(path.join(ART, 'r4-deep-result-' + stamp + '.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
  await browser.close();
})().catch(async (e) => {
  console.error('FATAL', e && e.stack || e);
  process.exit(1);
});

async function expect_text(page, sel, regex, timeoutMs, invert = false) {
  const start = Date.now();
  for (;;) {
    const t = (await page.locator(sel).textContent().catch(() => '')) || '';
    const ok = invert ? !regex.test(t.trim()) : regex.test(t.trim());
    if (ok) return t.trim();
    if (Date.now() - start > timeoutMs) throw new Error('timeout waiting ' + sel + ' ~ ' + regex + ' got: ' + t.trim());
    await sleep(250);
  }
}
