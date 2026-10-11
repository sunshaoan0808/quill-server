const fs = require('fs');
const path = require('path');
const { chromium } = require('patchright');

const ART = '/tmp/museai-browser-canary';
const TS = Date.now();
const ANDROID_UA = 'Mozilla/5.0 (Linux; Android 14; Pixel 8 Pro) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function expect_text(page, sel, regex, timeoutMs) {
  const start = Date.now();
  for (;;) {
    const t = (await page.locator(sel).textContent().catch(() => '')) || '';
    if (regex.test(t.trim())) return t.trim();
    if (Date.now() - start > timeoutMs) throw new Error('timeout ' + sel + ' ~ ' + regex + ' got: ' + t.trim().slice(0, 200));
    await sleep(250);
  }
}

(async () => {
  const env = Object.fromEntries(
    fs.readFileSync('/root/museai-server/.env', 'utf8').split(/\r?\n/)
      .filter((l) => l.includes('=') && !l.trim().startsWith('#')).map((l) => l.split('=', 2)));
  const BASE = 'http://127.0.0.1:18766';
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 412, height: 915 }, isMobile: true, hasTouch: true, userAgent: ANDROID_UA });
  const page = await ctx.newPage();
  const errors = [];
  const nonGet = [];
  page.on('pageerror', (e) => errors.push('pageerror: ' + String(e)));
  page.on('console', (m) => { if (m.type() === 'error') errors.push('console: ' + m.text()); });
  page.on('request', (r) => { if (r.method() !== 'GET') nonGet.push(r.method() + ' ' + r.url()); });

  const login = await page.request.post(BASE + '/api/v1/auth/login', { data: { username: env.KALEIDO_ADMIN_USER, password: env.KALEIDO_ADMIN_PASSWORD } });
  const { token } = await login.json();
  await page.addInitScript((t) => localStorage.setItem('kaleido_token', t), token);
  const H = { Authorization: 'Bearer ' + token };
  const api = async (m, p, b) => {
    const r = await page.request.fetch(BASE + p, { method: m, headers: { ...H, 'Content-Type': 'application/json' }, data: b });
    let j = null; try { j = await r.json(); } catch (_) {}
    return { status: r.status(), body: j };
  };

  const stamp = String(TS).slice(-6);
  const TITLE = 'R4 W3 tmp ' + stamp;
  const results = { title: TITLE };

  await page.goto(BASE + '/web/#/works', { waitUntil: 'networkidle' });
  await sleep(1200);
  const worksTab = page.locator('.tab[data-tab="works"], .bnav[data-tab="works"], .sheet-item[data-tab="works"]').first();
  if (await worksTab.count()) { await worksTab.click().catch(() => {}); await sleep(800); }
  // author panel controls live under compose/files views; ensure compose visible for project create
  const composeBtn = page.locator('button[data-azview="compose"] >> visible=true').first();
  if (await composeBtn.count()) { await composeBtn.click().catch(() => {}); await sleep(500); }
  await page.locator('#az-project-new').waitFor({ state: 'visible', timeout: 10000 });
  await page.screenshot({ path: path.join(ART, 'r4-w3-1-works.png') });

  // baseline project count, then create project via prompt dialog
  const list0 = await api('GET', '/api/v1/author/projects');
  const count0 = (list0.body.projects || list0.body || []).length;
  await page.locator('#az-project-new').click();
  await page.locator('.kaleido-dialog-input').waitFor({ state: 'visible', timeout: 8000 });
  await page.locator('.kaleido-dialog-input').fill(TITLE);
  await page.locator('#kaleido-dialog-ok').click();
  // createAuthorProject shows 已创建 then loadAuthorProjects overwrites with count — accept either
  await expect_text(page, '#az-project-msg', /已创建|个项目/, 12000);
  await sleep(1200);
  results.createdMsg = await page.locator('#az-project-msg').textContent().then((t) => t.trim());
  const list1 = await api('GET', '/api/v1/author/projects');
  const proj = (list1.body.projects || list1.body || []).find((p) => (p.title || '') === TITLE);
  if (!proj) throw new Error('project not in list after create');
  results.projectId = proj.id;
  results.listedAfterCreate = true;
  results.countDelta = (list1.body.projects || list1.body || []).length - count0;
  results.selectedTitle = await page.locator('#az-selected-title').textContent().then((t) => t.trim()).catch(() => '');

  // files view: create untitled doc in project cwd
  await page.locator('button[data-azview="files"] >> visible=true').first().click();
  await sleep(800);
  await page.locator('#works-create-untitled').click();
  await expect_text(page, '#works-msg', /已创建/, 12000);
  const openPath = await page.locator('#works-path').textContent().then((t) => t.trim());
  results.openPath = openPath;
  const BODY = 'R4 W3 regression ' + stamp;
  await page.locator('#works-content').fill(BODY);
  await sleep(300);
  await page.locator('#works-save').click();
  await expect_text(page, '#works-msg', /已保存/, 12000);
  results.savedMsg = await page.locator('#works-msg').textContent().then((t) => t.trim());
  await page.reload({ waitUntil: 'networkidle' });
  await sleep(1200);
  // navigate works tab via hash + tab button (reload lands on default tab)
  await page.goto(BASE + '/web/#/works', { waitUntil: 'networkidle' });
  await sleep(1200);
  const worksTab2 = page.locator('.tab[data-tab="works"], .bnav[data-tab="works"], .sheet-item[data-tab="works"]').first();
  if (await worksTab2.count()) { await worksTab2.click().catch(() => {}); await sleep(800); }
  // re-select the temp project (reload loses in-memory selection), then files view.
  // Compose view holds #az-project-list; click via JS to avoid mobile-viewport occlusion.
  const composeBtn2 = page.locator('button[data-azview="compose"] >> visible=true').first();
  if (await composeBtn2.count()) { await composeBtn2.click().catch(() => {}); await sleep(500); }
  const projCard = page.locator('#az-project-list .az-item', { hasText: TITLE }).first();
  if (await projCard.count()) { await projCard.click({ force: true }).catch(() => {}); await sleep(800); }
  await page.locator('button[data-azview="files"] >> visible=true').first().click();
  await sleep(800);
  // reopen file: tree clicks are occluded in 412px layout → dispatch click via JS
  const tail = openPath.split('/').pop();
  const treeItem = page.locator('#works-tree .item', { hasText: tail }).first();
  if (await treeItem.count()) { await treeItem.evaluate((el) => el.click()).catch(() => {}); await sleep(800); }
  results.contentReloaded = (await page.locator('#works-content').inputValue().catch(() => '')) === BODY;
  const fileApi = await api('GET', '/api/v1/works/file?path=' + encodeURIComponent(openPath));
  results.apiContentMatch = fileApi.body && fileApi.body.content === BODY;

  // publish open file as lore
  await page.selectOption('#az-publish-kind', 'lore');
  await sleep(300);
  await page.locator('#az-publish-btn').click();
  await expect_text(page, '#az-desk-msg', /已发布/, 15000);
  results.publishMsg = await page.locator('#az-desk-msg').textContent().then((t) => t.trim());
  await page.screenshot({ path: path.join(ART, 'r4-w3-2-publish.png') });

  // cleanup: project dir then project index; verify project absent from list.
  // Project deletion is soft-delete (index only); deleting the whole project dir
  // via UI/API is blocked for non-empty dirs, so we remove the single temp file
  // then the single temp dir explicitly. File DELETE uses query path, not body.
  const enc = encodeURIComponent(openPath);
  const delF = await api('DELETE', '/api/v1/works?path=' + enc);
  results.fileDeleted = delF.status === 200 || (delF.body && delF.body.ok);
  const projRoot = 'projects/' + proj.id;
  const delD = await api('DELETE', '/api/v1/works?path=' + encodeURIComponent(projRoot));
  results.dirDeleted = delD.status === 200 || (delD.body && delD.body.ok);
  const delP = await api('DELETE', '/api/v1/author/projects/' + encodeURIComponent(proj.id));
  results.projectDeleted = delP.status === 200 || (delP.body && delP.body.ok);
  const list2 = await api('GET', '/api/v1/author/projects');
  results.projectAbsent = !(JSON.stringify(list2.body).includes(proj.id) && JSON.stringify(list2.body).includes(TITLE));
  const statD = await api('GET', '/api/v1/works?path=' + encodeURIComponent(projRoot) + '&depth=1');
  // project deletion is soft-delete: the dir skeleton (canon/exports/imports/sessions/project.json)
  // remains by design; fileAbsent means OUR temp Untitled file is gone
  const dirList = statD.body && statD.body.children ? statD.body.children.map((c) => c.path) : [];
  results.tempFileGoneFromDir = !dirList.some((p) => p === openPath);
  results.fileAbsent = statD.status === 404 || statD.status === 400 || results.tempFileGoneFromDir;

  results.nonGet = nonGet.filter((u) => /author|works/.test(u));
  results.consoleErrors = errors;
  fs.writeFileSync(path.join(ART, 'r4-w3-result-' + stamp + '.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
  await browser.close();
  const must = [results.listedAfterCreate, results.apiContentMatch, /已发布/.test(results.publishMsg || ''), results.projectAbsent, results.fileAbsent, errors.length === 0];
  if (must.some((v) => !v)) process.exit(2);
})().catch((e) => { console.error('FATAL', (e && e.stack) || e); process.exit(1); });
