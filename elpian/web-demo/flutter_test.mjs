// End-to-end: a real Flutter app that `import 'flutter.dart'` — the full widget
// library — compiled to Elpian bytecode, run on the VM in a headless browser,
// and rasterized to pixels. Real clicks on the +/- buttons drive setState and
// the scene updates. Verifies both the produced scene and the rendered canvas.
const PW = process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright/index.js';
const { chromium } = (await import(PW)).default;
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const DIR = path.dirname(new URL(import.meta.url).pathname);
const FLUTTER_DIR = path.join(DIR, '..', 'elpian-dart', 'flutter');
const MIME = { '.html': 'text/html', '.wasm': 'application/wasm', '.dart': 'text/plain' };
const server = http.createServer((req, res) => {
  const name = req.url === '/' ? 'flutter.html' : decodeURIComponent(req.url).slice(1);
  // The app source lives with the library; everything else is served locally.
  let file = path.join(DIR, name);
  if (name === 'demo_app.dart') { file = path.join(FLUTTER_DIR, name); }
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

// Find the center of the first drawRect with the given ARGB color in the scene.
const centerOf = (scene, color) => {
  for (const op of scene.root.ops) {
    if (op.op === 'drawRect' && op.color === color) {
      const [l, t, r, b] = op.rect;
      return [(l + r) / 2, (t + b) / 2];
    }
  }
  return null;
};
const texts = (scene) => scene.root.ops.filter(o => o.op === 'drawParagraph').map(o => o.text);

try {
  const page = await browser.newPage();
  await page.goto(`http://localhost:${port}/flutter.html`);
  await page.waitForFunction(() => window.__ready === true || window.__error, null, { timeout: 15000 });
  if (await page.evaluate(() => window.__error)) fail('page error: ' + await page.evaluate(() => window.__error));

  const box = await page.locator('#c').boundingBox();
  let scene = await page.evaluate(() => window.__scene);

  // The app chrome rendered.
  const t0 = texts(scene);
  if (!t0.includes('Elpian Dashboard')) fail('app bar title missing: ' + JSON.stringify(t0));
  else console.log('ok  Flutter app rendered (AppBar, Card, chips)');
  if (t0.filter(s => s === '0').length !== 4) fail('counter/chips should start at 0: ' + JSON.stringify(t0));
  else console.log('ok  counter + 3 stat chips start at 0');

  const green = centerOf(scene, 4283215696); // Colors.green (+)
  const red = centerOf(scene, 4294198070);   // Colors.red (-)
  if (!green || !red) fail('could not find +/- buttons in scene');

  // Tap "+" three times.
  for (let i = 0; i < 3; i++) await page.mouse.click(box.x + green[0], box.y + green[1]);
  scene = await page.evaluate(() => window.__scene);
  const t1 = texts(scene);
  if (!t1.includes('3') || !t1.includes('6') || !t1.includes('9'))
    fail('after 3 taps expect count 3 / double 6 / square 9: ' + JSON.stringify(t1));
  else console.log('ok  +3 -> count 3, double 6, square 9 (setState in the browser)');

  // Tap "-" once -> count 2.
  await page.mouse.click(box.x + red[0], box.y + red[1]);
  scene = await page.evaluate(() => window.__scene);
  if (!texts(scene).includes('2')) fail('after a decrement expect count 2');
  else console.log('ok  -1 -> count 2');

  // Pixel proof: the cyan progress bar (Colors.cyan) is actually painted.
  const cyanPresent = await page.evaluate(() => {
    const d = document.getElementById('c').getContext('2d');
    const bar = window.__scene.root.ops.find(o => o.op === 'drawRect' && o.color === 4278238420);
    if (!bar) return false;
    const [l, t, r, b] = bar.rect;
    const px = d.getImageData((l + r) / 2, (t + b) / 2, 1, 1).data;
    return px[2] > 150 && px[1] > 120 && px[0] < 90; // ~cyan
  });
  if (!cyanPresent) fail('cyan progress bar not rasterized');
  else console.log('ok  cyan progress bar rasterized to pixels');

  await page.screenshot({ path: path.join(DIR, 'flutter.png') });
  console.log('screenshot -> web-demo/flutter.png');
} finally {
  await browser.close();
  server.close();
}
if (process.exitCode) console.error('\nFLUTTER APP E2E FAILED'); else console.log('\nFLUTTER APP E2E PASSED');
