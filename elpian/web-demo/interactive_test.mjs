// Interactive end-to-end: real browser clicks -> VM event handler -> state
// mutation -> re-render, verified by sampling canvas pixels as the bar grows.
const PW = process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright/index.js';
const { chromium } = (await import(PW)).default;
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const DIR = path.dirname(new URL(import.meta.url).pathname);
const MIME = { '.html': 'text/html', '.wasm': 'application/wasm', '.dart': 'text/plain' };
const server = http.createServer((req, res) => {
  const file = path.join(DIR, req.url === '/' ? 'interactive.html' : decodeURIComponent(req.url));
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
  await page.goto(`http://localhost:${port}/interactive.html`);
  await page.waitForFunction(() => window.__ready === true || window.__error, null, { timeout: 15000 });
  if (await page.evaluate(() => window.__error)) fail('page error: ' + await page.evaluate(() => window.__error));

  const canvas = page.locator('#c');
  const box = await canvas.boundingBox();
  const blueAt = (x) => page.evaluate((x) => {
    const d = document.getElementById('c').getContext('2d').getImageData(x, 150, 1, 1).data;
    return d[2] > 200 && d[0] < 80 && d[1] < 80; // ~blue
  }, x);

  // Before any click the bar has zero width.
  if (await blueAt(25)) fail('bar should start empty');
  else console.log('ok  bar empty at start');

  // Click the button (canvas-local ~90,50) three times.
  for (let i = 0; i < 3; i++) {
    await page.mouse.click(box.x + 90, box.y + 50);
  }
  // Bar right edge is now 20 + 3*30 = 110.
  if (!(await blueAt(25))) fail('bar should be filled after clicks (x=25)');
  else console.log('ok  bar filled at x=25 after 3 clicks');
  if (!(await blueAt(100))) fail('bar should reach x=100 after 3 clicks');
  else console.log('ok  bar reaches x=100 after 3 clicks');
  if (await blueAt(130)) fail('bar should not reach x=130 after 3 clicks');
  else console.log('ok  bar stops before x=130 (width tracks count)');

  // A click outside the button must not advance the counter.
  const rightBefore = await blueAt(100);
  await page.mouse.click(box.x + 300, box.y + 220);
  if ((await blueAt(140)) && !rightBefore) fail('outside click should not advance');
  else console.log('ok  click outside button ignored');

  await page.screenshot({ path: path.join(DIR, 'interactive.png') });
  console.log('screenshot -> web-demo/interactive.png');
} finally {
  await browser.close();
  server.close();
}
if (process.exitCode) console.error('\nINTERACTIVE E2E FAILED'); else console.log('\nINTERACTIVE E2E PASSED');
