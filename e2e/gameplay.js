// Definitive gameplay check: solo lobby, long round, click → server verdict toast.
const { chromium } = require('playwright');
const BASE = 'http://localhost:8017';

(async () => {
  const browser = await chromium.launch({
    headless: true,
    args: ['--enable-unsafe-swiftshader', '--use-angle=swiftshader'],
  });
  const ctx = await browser.newContext({ viewport: { width: 960, height: 600 } });
  const page = await ctx.newPage();
  const logs = [];
  page.on('console', (m) => logs.push(m.text()));
  page.on('pageerror', (e) => console.log('[PAGEERROR]', String(e).slice(0, 300)));

  await page.addInitScript(() => {
    const OrigWS = window.WebSocket;
    window.WebSocket = class extends OrigWS {
      constructor(...args) {
        super(...args);
        this.addEventListener('message', (ev) => console.log('[WS<-]', String(ev.data).slice(0, 200)));
        const send = this.send.bind(this);
        this.send = (d) => { console.log('[WS->]', String(d).slice(0, 200)); send(d); };
      }
    };
  });
  await page.goto(BASE, { waitUntil: 'networkidle' });
  await page.waitForTimeout(2500);
  await page.locator('input').first().fill('Solo');
  await page.locator('button', { hasText: 'Create lobby' }).click();
  await page.waitForTimeout(1000);
  await page.locator('select').nth(1).selectOption('300');
  await page.waitForTimeout(300);
  await page.locator('button', { hasText: 'Start game' }).click();
  console.log('--- waiting 45s for scene build');
  await page.waitForTimeout(45000);

  // timer ticking?
  const t1 = await page.evaluate(() => document.querySelector('#timerText')?.textContent);
  await page.waitForTimeout(3000);
  const t2 = await page.evaluate(() => document.querySelector('#timerText')?.textContent);
  console.log('timer:', t1, '→', t2, t1 !== t2 ? '(ticking)' : '(STUCK)');

  // drag rotate
  await page.mouse.move(480, 300);
  await page.mouse.down();
  await page.mouse.move(580, 340, { steps: 8 });
  await page.mouse.up();
  await page.waitForTimeout(1000);

  // click guess
  await page.mouse.click(470, 290);
  let toast = '';
  for (let i = 0; i < 25; i++) {
    toast = await page.evaluate(() => document.querySelector('#toast.show')?.textContent ?? '');
    if (toast) break;
    await page.waitForTimeout(400);
  }
  console.log('toast:', JSON.stringify(toast));
  console.log('click debug lines:', logs.filter((l) => l.includes('click at') || l.includes('pick resolved')).length);

  // screenshot via CDP
  const cdp = await ctx.newCDPSession(page);
  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
  require('fs').writeFileSync('/tmp/e2e/shot-gameplay.png', Buffer.from(data, 'base64'));
  await browser.close();
  console.log('GAMEPLAY TEST DONE');
})().catch((e) => { console.error('HARNESS ERROR', e); process.exit(1); });
