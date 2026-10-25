#!/usr/bin/env node
/** Verify exported UI initializers are imported and called by main.js. */
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(new URL('..', import.meta.url).pathname);
const main = fs.readFileSync(path.join(root, 'src/main.js'), 'utf8');
const files = fs.readdirSync(path.join(root, 'src/js')).filter((f) => f.endsWith('.js'));
const exported = [];
for (const file of files) {
  const text = fs.readFileSync(path.join(root, 'src/js', file), 'utf8');
  for (const m of text.matchAll(/export\s+(?:async\s+)?function\s+(init[A-Za-z0-9]+UI)\s*\(/g)) {
    exported.push({ file: `src/js/${file}`, name: m[1] });
  }
}
const rows = exported.map((x) => ({
  ...x,
  imported: new RegExp(`import\\s*\\{[^}]*\\b${x.name}\\b[^}]*\\}\\s*from`).test(main),
  called: new RegExp(`\\b${x.name}\\s*\\(\\s*\\)`).test(main),
}));
const bundle = fs.readFileSync(path.join(root, 'web/assets/app.js'), 'utf8');
const bundleChecks = {
  appJsBytes: Buffer.byteLength(bundle),
  aiAdminFacade: bundle.includes('P5AiLoad'),
  moaFacade: bundle.includes('MoaLoadPanels'),
};
const missing = rows.filter((x) => !x.imported || !x.called);
const result = { entry: 'src/main.js', initializers: rows, bundle: bundleChecks, missing, ok: missing.length === 0 && Object.values(bundleChecks).every((v) => v === true || typeof v === 'number') };
console.log(JSON.stringify(result, null, 2));
process.exitCode = result.ok ? 0 : 1;
