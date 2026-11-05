import { chromium } from "playwright-core";
const ORIGIN = process.argv[2];
const RUNS = Number(process.argv[3] ?? 6);
// SPIKE_COOKIE (`name=value`) passes an edge access gate.
const COOKIE = process.env.SPIKE_COOKIE;
const browser = await chromium.launch({ channel: "chrome", headless: true });
for (let i = 0; i < RUNS; i++) {
  const context = await browser.newContext();
  if (COOKIE) {
    const [name, ...value] = COOKIE.split("=");
    await context.addCookies([{ name, value: value.join("="), url: ORIGIN }]);
  }
  const page = await context.newPage();
  const pending = new Map();
  const failed = [];
  const statuses = {};
  page.on("request", (r) => pending.set(r, Date.now()));
  page.on("requestfinished", async (r) => {
    pending.delete(r);
    const s = (await r.response())?.status();
    statuses[s] = (statuses[s] ?? 0) + 1;
  });
  page.on("requestfailed", (r) => { pending.delete(r); failed.push(`${r.url().slice(-40)} ${r.failure()?.errorText}`); });
  const started = Date.now();
  let outcome = "ok";
  try {
    await page.goto(`${ORIGIN}/`);
    await page.waitForFunction(() => window.__spike, null, { timeout: 15000 });
  } catch (e) { outcome = e.name; }
  const ms = Date.now() - started;
  const loaded = await page.evaluate(() => window.__spike?.loadedAt).catch(() => null);
  console.log(JSON.stringify({ run: i, outcome, ms, loadedAt: loaded, statuses, failed: failed.slice(0, 5), stillPending: [...pending.keys()].slice(0, 5).map((r) => r.url().slice(-45)) }));
  await context.close();
}
await browser.close();
