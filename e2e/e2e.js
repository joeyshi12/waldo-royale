// E2E harness for the all-Rust waldo-royale client.
// Usage: node e2e.js [stage]  — stages: load, lobby, game, full
const { chromium } = require('playwright');

const BASE = 'http://localhost:8017';
const SHOT = (n) => `/tmp/e2e/shot-${n}.png`;


async function shot(page, name) {
  try {
    const cdp = await page.context().newCDPSession(page);
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
    require('fs').writeFileSync(SHOT(name), Buffer.from(data, 'base64'));
    await cdp.detach();
  } catch (e) { console.log('shot failed:', name, String(e).slice(0, 120)); }
}

async function newPage(browser, tag) {
  const ctx = await browser.newContext({ viewport: { width: 960, height: 600 } });
  const page = await ctx.newPage();
  page.on('console', (msg) => {
    const t = msg.type();
    console.log(`[${tag}:${t}]`, msg.text().slice(0, 300));
  });
  page.on('pageerror', (err) => console.log(`[${tag}:PAGEERROR]`, String(err).slice(0, 800)));
  return page;
}

(async () => {
  require('fs').readdirSync('/tmp/e2e').filter(f => f.startsWith('shot-')).forEach(f => require('fs').unlinkSync('/tmp/e2e/' + f));
  const stage = process.argv[2] ?? 'full';
  const browser = await chromium.launch({
    headless: true,
    args: [
      '--enable-unsafe-swiftshader',
      '--use-angle=swiftshader',
      '--disable-gpu-sandbox',
    ],
  });

  const a = await newPage(browser, 'A');
  console.log('--- loading page A');
  await a.goto(BASE, { waitUntil: 'networkidle' });
  await a.waitForTimeout(3000);
  await shot(a, 'menu');
  const menuVisible = await a.locator('.card').count();
  console.log('menu cards visible:', menuVisible);
  if (stage === 'load') return await browser.close();

  // --- create lobby ---
  console.log('--- creating lobby');
  await a.locator('input').first().fill('Alice');
  await a.locator('button', { hasText: 'Create lobby' }).click();
  await a.waitForTimeout(1500);
  await shot(a, 'lobby');
  const code = (await a.locator('#lobbyCode').textContent().catch(() => '')) ?? '';
  console.log('lobby code:', JSON.stringify(code));
  if (!code || code.trim().length !== 4) {
    console.log('LOBBY FAILED'); return await browser.close();
  }
  if (stage === 'lobby') return await browser.close();

  // --- player B joins ---
  console.log('--- B joining');
  const b = await newPage(browser, 'B');
  await b.goto(BASE, { waitUntil: 'networkidle' });
  await b.waitForTimeout(2500);
  await b.locator('input').first().fill('Bob');
  await b.locator('input[placeholder="Code"]').fill(code.trim());
  await b.locator('button', { hasText: 'Join' }).click();
  await b.waitForTimeout(1500);
  const playersA = await a.locator('#playerList li').count();
  console.log('players visible to A:', playersA);

  // --- configure short round and start ---
  console.log('--- starting game (1 round, 30s)');
  await a.locator('select').first().selectOption('1');
  await a.locator('select').nth(1).selectOption('60');
  await a.waitForTimeout(500);
  await a.locator('button', { hasText: 'Start game' }).click();
  for (let i = 0; i < 60; i++) {
    if (await a.evaluate(() => !!document.querySelector('#roundInfo')).catch(() => false)) break;
    await a.waitForTimeout(1000);
  }
  console.log('round started (HUD present)');
  console.log('--- waiting for round to build (assets)');
  const poll = setInterval(async () => {
    try {
      const state = await a.evaluate(() => ({
        hud: !!document.querySelector('#roundInfo'),
        timer: document.querySelector('#timerText')?.textContent,
        win: document.querySelector('#winnerName')?.textContent,
        results: !!document.querySelector('table'),
        lobby: !!document.querySelector('#lobbyCode'),
        menu: !!document.querySelector('input[placeholder="Code"]'),
        toast: document.querySelector('#toast.show')?.textContent,
        ws: window.__ws_state,
      }));
      console.log(new Date().toISOString().slice(17, 23), JSON.stringify(state));
    } catch {}
  }, 1000);
  await a.waitForTimeout(30000); // scene build is slow under SwiftShader
  await shot(a, 'game-a');
  await shot(b, 'game-b');
  const hudA = await a.evaluate(() => !!document.querySelector('#roundInfo')).catch(() => false);
  console.log('HUD visible on A:', hudA);

  // --- clicks: a drag (rotate) then a click (guess) ---
  console.log('--- input: drag then click');
  await a.mouse.move(480, 300);
  await a.mouse.down();
  await a.mouse.move(560, 330, { steps: 10 });
  await a.mouse.up();
  await a.waitForTimeout(500);
  await a.mouse.click(480, 310);
  let toast = '';
  for (let i = 0; i < 20; i++) {
    toast = await a.evaluate(() => document.querySelector('#toast.show')?.textContent ?? '').catch(() => '');
    if (toast) break;
    await a.waitForTimeout(400);
  }
  console.log('toast after click:', JSON.stringify(toast));
  await shot(a, 'after-click');

  // --- wheel zoom ---
  await a.mouse.move(480, 300);
  await a.mouse.wheel(0, -600);
  await a.waitForTimeout(800);
  await shot(a, 'zoomed');

  if (stage === 'game') { clearInterval(poll); return await browser.close(); }

  // --- wait out the round: results, then game over ---
  console.log('--- waiting for round timeout');
  for (let i = 0; i < 70; i++) {
    if (await a.evaluate(() => !!document.querySelector('table')).catch(() => false)) break;
    await a.waitForTimeout(1000);
  }
  await shot(a, 'reveal');
  const rows = await a.evaluate(() =>
    [...document.querySelectorAll('table tbody tr')].map((r) => r.textContent)).catch(() => []);
  console.log('result rows:', JSON.stringify(rows));
  await shot(a, 'results');
  for (let i = 0; i < 20; i++) {
    if (await a.evaluate(() => !!document.querySelector('#winnerName')).catch(() => false)) break;
    await a.waitForTimeout(1000);
  }
  await shot(a, 'final');
  const win = await a.evaluate(() => document.querySelector('#winnerName')?.textContent ?? '').catch(() => '');
  console.log('win screen headline:', JSON.stringify(win));

  await browser.close();
  console.log('E2E COMPLETE');
})().catch((e) => { console.error('HARNESS ERROR', e); process.exit(1); });
