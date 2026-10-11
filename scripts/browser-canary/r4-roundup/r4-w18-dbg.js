const fs = require('fs');
const { chromium } = require('patchright');
(async () => {
  const env = Object.fromEntries(fs.readFileSync('/root/museai-server/.env', 'utf8').split('\n').filter((l) => l.includes('=') && !l.trim().startsWith('#')).map((l) => l.split('=', 2)));
  const b = await chromium.launch({ headless: true });
  const p = await (await b.newContext({ viewport: { width: 412, height: 915 } })).newPage();
  const l = await p.request.post('http://127.0.0.1:18766/api/v1/auth/login', { data: { username: env.KALEIDO_ADMIN_USER, password: env.KALEIDO_ADMIN_PASSWORD } });
  const r = await l.json();
  await p.addInitScript((t) => localStorage.setItem('kaleido_token', t), r.token);
  await p.goto('http://127.0.0.1:18766/web/#/story', { waitUntil: 'networkidle' });
  await p.waitForTimeout(1500);
  const items = await p.locator('#story-session-list .item').allTextContents();
  console.log('list:', JSON.stringify(items.slice(0, 6)));
  const rec = await p.evaluate(async () => {
    const r = await fetch('/api/mobile/sessions/story-session-r4w18dbg', { headers: { Authorization: 'Bearer ' + localStorage.getItem('kaleido_token') } });
    const j = await r.json();
    return { keys: Object.keys(j), hasMessages: Array.isArray(j.messages), n: (j.messages || []).length, hasC7: typeof j.__c7 };
  });
  console.log('rec:', JSON.stringify(rec));
  const it = p.locator('#story-session-list .item', { hasText: 'R4 W18 dbg' }).first();
  console.log('found:', await it.count());
  if (await it.count()) { await it.evaluate((el) => el.click()); await p.waitForTimeout(2500); }
  console.log('msgs:', (await p.locator('#story-messages').textContent()).slice(0, 300));
  console.log('story-choices html:', (await p.locator('#story-choices').innerHTML()).slice(0, 300));
  console.log('adv-choices html:', (await p.locator('#adv-choices').innerHTML()).slice(0, 200));
  console.log('msg count:', await p.locator('#story-messages .bubble, #story-messages .msg').count());
  await b.close();
})().catch((e) => { console.error(e); process.exit(1); });
