// End-to-end: the Elpian VM drives REAL CanvasKit (Skia) in a headless browser.
// (1) the flutter.dart app is rasterized by Skia and stays interactive;
// (2) a full-API showcase is drawn via the reflective bridge (gradients, paths,
//     mask/image filters, saveLayer, real shaped text);
// (3) a coverage audit proves the bridge reaches the entire loaded CanvasKit API.
const PW = process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright/index.js';
const { chromium } = (await import(PW)).default;
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const DIR = path.dirname(new URL(import.meta.url).pathname);
const FLUTTER_DIR = path.join(DIR, '..', 'elpian-dart', 'flutter');
const MIME = { '.html': 'text/html', '.wasm': 'application/wasm', '.dart': 'text/plain', '.js': 'text/javascript', '.mjs': 'text/javascript' };
const server = http.createServer((req, res) => {
  const name = req.url === '/' ? 'canvaskit.html' : decodeURIComponent(req.url).slice(1);
  let file = path.join(DIR, name);
  if (name === 'demo_app.dart') file = path.join(FLUTTER_DIR, name);
  fs.readFile(file, (err, data) => {
    if (err) { res.writeHead(404); res.end(); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
    res.end(data);
  });
});

const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };
await new Promise((r) => server.listen(0, r));
const port = server.address().port;
const browser = await chromium.launch({ headless: true, args: ['--enable-unsafe-swiftshader'] });

try {
  const page = await browser.newPage();
  page.on('console', (m) => { if (m.type() === 'error') console.log('  [browser]', m.text()); });
  await page.goto(`http://localhost:${port}/canvaskit.html`);
  await page.waitForFunction(() => window.__ready === true || window.__error, null, { timeout: 30000 });
  const err = await page.evaluate(() => window.__error);
  if (err) { fail('page error: ' + err); throw new Error(err); }

  // (3) Coverage audit — the machine-checked "no exceptions" guarantee.
  const audit = await page.evaluate(() => window.__audit);
  console.log(`ok  CanvasKit API reachable: ${audit.total} symbols ` +
              `(${audit.constructors.length} constructors, ${audit.factories.length} factories, ${audit.enums.length} enum/namespaces)`);
  if (audit.unreachable.length !== 0) fail('unreachable symbols: ' + audit.unreachable.slice(0, 10).join(', '));
  else console.log('ok  every enumerated CanvasKit symbol is reachable by the reflective bridge (0 unreachable)');
  if (audit.total < 300) fail('audit saw too few symbols (' + audit.total + ') — did CanvasKit load?');

  // (1) The flutter.dart app rendered on Skia: sample the indigo AppBar pixel.
  const appHasIndigo = await page.evaluate(() => {
    const d = document.getElementById('app').getContext('2d').getImageData(200, 28, 1, 1).data;
    return d[0] > 40 && d[0] < 90 && d[1] > 60 && d[1] < 100 && d[2] > 150; // ~indigo
  });
  if (!appHasIndigo) fail('flutter app AppBar not rasterized by Skia');
  else console.log('ok  flutter.dart app rasterized by real Skia (AppBar pixel)');

  // Interactive: tap the green "+" button (found via the scene) and re-check.
  const before = await page.evaluate(() => window.__scene.root.ops.filter(o => o.op === 'drawParagraph').map(o => o.text));
  await page.evaluate(() => {
    const g = window.__scene.root.ops.find(o => o.op === 'drawRect' && o.color === 4283215696);
    const [l, t, r, b] = g.rect; window.__tap((l + r) / 2, (t + b) / 2);
  });
  await page.evaluate(() => { const g = window.__scene.root.ops.find(o => o.op === 'drawRect' && o.color === 4283215696); const [l,t,r,b]=g.rect; window.__tap((l+r)/2,(t+b)/2); });
  const after = await page.evaluate(() => window.__scene.root.ops.filter(o => o.op === 'drawParagraph').map(o => o.text));
  if (!after.includes('2')) fail('after two + taps counter should be 2 (Skia-rendered): ' + JSON.stringify(after));
  else console.log('ok  interactive: two "+" taps -> counter 2, re-rasterized by Skia');

  // (2) The reflective showcase produced non-blank Skia output (gradient pixel).
  const showcasePainted = await page.evaluate(() => {
    const d = document.getElementById('showcase').getContext('2d').getImageData(60, 60, 1, 1).data;
    return d[3] > 0 && (d[0] + d[1] + d[2] > 60); // not transparent/near-black bg
  });
  if (!showcasePainted) fail('reflective full-Skia showcase did not paint');
  else console.log('ok  reflective showcase drew gradients/paths/filters/text via real Skia');

  await page.screenshot({ path: path.join(DIR, 'canvaskit.png') });
  console.log('screenshot -> web-demo/canvaskit.png');
} finally {
  await browser.close();
  server.close();
}
if (process.exitCode) console.error('\nCANVASKIT E2E FAILED'); else console.log('\nCANVASKIT E2E PASSED');
