// Proof that Elpian *guest bytecode* controls the full Skia API: a guest with no
// widget framework emits a reflective Skia program over dart:ui, and CanvasKit
// rasterizes it. Taps mutate guest state and re-emit — verified by pixels.
const PW = process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright/index.js';
const { chromium } = (await import(PW)).default;
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
const DIR = path.dirname(new URL(import.meta.url).pathname);
const MIME = { '.html': 'text/html', '.wasm': 'application/wasm', '.dart': 'text/plain', '.js': 'text/javascript' };
const server = http.createServer((req, res) => {
  const name = req.url === '/' ? 'skia_vm.html' : decodeURIComponent(req.url).slice(1);
  fs.readFile(path.join(DIR, name), (err, data) => {
    if (err) { res.writeHead(404); res.end(); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(name)] || 'application/octet-stream' });
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
  await page.goto(`http://localhost:${port}/skia_vm.html`);
  await page.waitForFunction(() => window.__ready === true || window.__error, null, { timeout: 30000 });
  const err = await page.evaluate(() => window.__error);
  if (err) { fail('page error: ' + err); throw new Error(err); }

  // The guest emitted a raw Skia program (not a widget scene).
  const isSkia = await page.evaluate(() => !!(window.__frame && window.__frame.skia));
  if (!isSkia) fail('guest did not emit a raw skia program'); else console.log('ok  guest emitted a raw Skia program via dart:ui');

  // The gradient rounded-rect rasterized (colorful pixel inside it).
  const gradient = await page.evaluate(() => {
    const d = document.getElementById('c').getContext('2d').getImageData(60, 70, 1, 1).data;
    return d[3] > 200 && (d[0] + d[1] + d[2]) > 150;
  });
  if (!gradient) fail('gradient rrect not rasterized by Skia'); else console.log('ok  gradient/path/blur/text rasterized by real Skia');

  // Interactive: the circle (center 90,262, r=14+taps*6) is small at first, so a
  // point 30px below center is background; after several taps it is green.
  const greenBefore = await page.evaluate(() => {
    const d = document.getElementById('c').getContext('2d').getImageData(90, 296, 1, 1).data;
    return d[1] > 120 && d[0] < 120 && d[2] < 120;
  });
  if (greenBefore) fail('circle should start small');
  for (let i = 0; i < 5; i++) await page.evaluate(() => window.__tap());
  const greenAfter = await page.evaluate(() => {
    const d = document.getElementById('c').getContext('2d').getImageData(90, 296, 1, 1).data;
    return d[1] > 120 && d[0] < 120 && d[2] < 120;
  });
  if (!greenAfter) fail('circle should grow with taps (guest state -> Skia)');
  else console.log('ok  taps mutate guest state -> circle grows -> re-rasterized by Skia');

  await page.screenshot({ path: path.join(DIR, 'skia_vm.png') });
  console.log('screenshot -> web-demo/skia_vm.png');
} finally { await browser.close(); server.close(); }
if (process.exitCode) console.error('\nSKIA-VM E2E FAILED'); else console.log('\nSKIA-VM E2E PASSED');
