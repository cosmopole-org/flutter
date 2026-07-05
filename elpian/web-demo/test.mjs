// End-to-end test: run the Elpian VM (wasm) + a dynamic Dart miniapp inside a
// real headless Chromium via Playwright, then assert the *rendered pixels* on
// the canvas — proving Dart source -> VM-in-browser -> scene tree -> pixels.
// Playwright is installed globally here; resolve it explicitly (ESM ignores
// NODE_PATH). Override with PLAYWRIGHT_MODULE if it lives elsewhere.
const PW = process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright/index.js';
const { chromium } = (await import(PW)).default;
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const DIR = path.dirname(new URL(import.meta.url).pathname);
const MIME = { '.html': 'text/html', '.wasm': 'application/wasm', '.dart': 'text/plain' };

const server = http.createServer((req, res) => {
  const file = path.join(DIR, req.url === '/' ? 'index.html' : decodeURIComponent(req.url));
  fs.readFile(file, (err, data) => {
    if (err) { res.writeHead(404); res.end('not found'); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
    res.end(data);
  });
});

function pixel(data) { return `rgb(${data[0]},${data[1]},${data[2]})`; }

const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };

await new Promise((r) => server.listen(0, r));
const port = server.address().port;

const browser = await chromium.launch({
  headless: true,

});
try {
  const page = await browser.newPage();
  const logs = [];
  page.on('console', (m) => logs.push(m.text()));

  await page.goto(`http://localhost:${port}/index.html`);
  // Wait for the VM to run and the canvas to be painted.
  await page.waitForFunction(() => window.__rendered === true || window.__error, null, { timeout: 15000 });

  const error = await page.evaluate(() => window.__error || null);
  if (error) { fail('page error: ' + error); }

  const opsPainted = await page.evaluate(() => window.__opsPainted);
  if (opsPainted !== 5) fail(`expected 5 painted ops (3 rects + circle + text), got ${opsPainted}`);

  // Sample canvas pixels for the three swatches and the circle.
  const samples = await page.evaluate(() => {
    const ctx = document.getElementById('c').getContext('2d');
    const at = (x, y) => Array.from(ctx.getImageData(x, y, 1, 1).data);
    return {
      redSwatch: at(60, 60),     // first swatch (red)   ~ (20..110, 20..110)
      greenSwatch: at(170, 60),  // second swatch (green) ~ (130..220)
      blueSwatch: at(280, 60),   // third swatch (blue)  ~ (240..330)
      circle: at(175, 200),      // blue circle centre (delivered via await)
      empty: at(360, 20),        // corner, should be untouched
    };
  });

  const expect = (name, got, want) => {
    if (pixel(got) !== want) fail(`${name}: expected ${want}, got ${pixel(got)} (alpha ${got[3]})`);
    else console.log(`ok  ${name} = ${want}`);
  };
  expect('red swatch', samples.redSwatch, 'rgb(255,0,0)');
  expect('green swatch', samples.greenSwatch, 'rgb(0,255,0)');
  expect('blue swatch', samples.blueSwatch, 'rgb(0,0,255)');
  expect('await-driven circle', samples.circle, 'rgb(0,0,255)');
  if (samples.empty[3] !== 0) fail(`corner should be transparent, got alpha ${samples.empty[3]}`);
  else console.log('ok  corner transparent');

  await page.screenshot({ path: path.join(DIR, 'rendered.png') });
  console.log('screenshot -> web-demo/rendered.png');
  console.log('browser logs:\n  ' + logs.join('\n  '));
} finally {
  await browser.close();
  server.close();
}

if (process.exitCode) console.error('\nE2E FAILED'); else console.log('\nE2E PASSED');
