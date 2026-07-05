// End-to-end: REAL Flutter widget code (StatefulWidget + GestureDetector) runs
// on the Elpian VM in a headless browser and rasterizes to actual pixels; real
// clicks drive onTap -> setState -> repaint, verified by sampling the canvas.
const PW = process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright/index.js';
const { chromium } = (await import(PW)).default;
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const DIR = path.dirname(new URL(import.meta.url).pathname);
const MIME = { '.html': 'text/html', '.wasm': 'application/wasm', '.dart': 'text/plain' };
const server = http.createServer((req, res) => {
  const file = path.join(DIR, req.url === '/' ? 'widgets.html' : decodeURIComponent(req.url));
  fs.readFile(file, (err, data) => {
    if (err) { res.writeHead(404); res.end(); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
    res.end(data);
  });
});

const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };
await new Promise((r) => server.listen(0, r));
const port = server.address().port;
const browser = await chromium.launch({ headless: true });

try {
  const page = await browser.newPage();
  await page.goto(`http://localhost:${port}/widgets.html`);
  await page.waitForFunction(() => window.__ready === true || window.__error, null, { timeout: 15000 });
  if (await page.evaluate(() => window.__error)) fail('page error: ' + await page.evaluate(() => window.__error));

  const canvas = page.locator('#c');
  const box = await canvas.boundingBox();
  const green = (x, y) => page.evaluate(([x, y]) => {
    const d = document.getElementById('c').getContext('2d').getImageData(x, y, 1, 1).data;
    return d[1] > 120 && d[0] < 80 && d[2] < 80; // ~green
  }, [x, y]);
  const blueAt = (x) => page.evaluate((x) => {
    const d = document.getElementById('c').getContext('2d').getImageData(x, 75, 1, 1).data;
    return d[2] > 200 && d[0] < 80 && d[1] < 80; // ~blue
  }, x);

  // The GestureDetector button is a green 140x60 box at the top-left (0,0)-(140,60).
  if (!(await green(70, 30))) fail('green button should render at start');
  else console.log('ok  green widget button rendered');

  // Progress bar starts nearly empty (blue at y=75, count=0 -> width 20).
  if (await blueAt(60)) fail('bar should start short');
  else console.log('ok  bar short at start');

  // Click the button (canvas-local ~70,30) three times -> count 3 -> bar width 110.
  for (let i = 0; i < 3; i++) {
    await page.mouse.click(box.x + 70, box.y + 30);
  }
  if (!(await blueAt(30))) fail('bar should be filled after clicks (x=30)');
  else console.log('ok  bar filled at x=30 after 3 taps');
  if (!(await blueAt(100))) fail('bar should reach x=100 after 3 taps');
  else console.log('ok  bar reaches x=100 after 3 taps');
  if (await blueAt(130)) fail('bar should not reach x=130 after 3 taps');
  else console.log('ok  bar stops before x=130 (width tracks state)');

  // A click outside the button must not advance the counter.
  await page.mouse.click(box.x + 300, box.y + 200);
  if (await blueAt(130)) fail('outside click should not advance');
  else console.log('ok  tap outside button ignored');

  await page.screenshot({ path: path.join(DIR, 'widgets.png') });
  console.log('screenshot -> web-demo/widgets.png');
} finally {
  await browser.close();
  server.close();
}
if (process.exitCode) console.error('\nWIDGETS E2E FAILED'); else console.log('\nWIDGETS E2E PASSED');
