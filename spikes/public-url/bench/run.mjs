// Checks the public URL spike end to end in a real browser (installed Chrome)
// and compares it with direct access to the Vite fixture.
//
// Prerequisites: the fixture on 127.0.0.1:5199, `wrangler dev` on
// 127.0.0.1:8787, and the connector running. See ../README.md.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const DIRECT = "http://127.0.0.1:5199";
const TUNNEL = "http://feature--app-a8f3.localhost:8787";
// Spike-only labels that skip part of the path (see edge/src/index.ts), so the
// tunnel cost can be split between the local runtime and the connector protocol.
const PATHS = [
  ["direct", DIRECT],
  ["workerOnly", "http://baseline-worker--app-a8f3.localhost:8787"],
  ["workerAndDurableObject", "http://baseline-do--app-a8f3.localhost:8787"],
  ["tunnel", TUNNEL],
];
const PAGE_LOAD_RUNS = 7;
const SEQUENTIAL_REQUESTS = 200;
const HMR_FILE = join(import.meta.dirname, "../fixture/src/hmr-target.js");

const browser = await chromium.launch({
  channel: "chrome",
  headless: true,
  // Resolve every *.localhost name to the IPv4 address wrangler listens on.
  args: ["--host-resolver-rules=MAP *.localhost 127.0.0.1"],
});
const results = {};
const originalHmrSource = readFileSync(HMR_FILE, "utf8");

try {
  await warmUp();
  results.pageLoad = await comparePageLoads();
  results.sequentialRequests = await compareSequentialRequests();
  results.download = await compare(downloadCheck);
  results.upload = await compare(uploadCheck);
  results.sse = await sseCheck(TUNNEL);
  results.abortThenReuse = await abortThenReuseCheck(TUNNEL);
  results.hmr = await hmrCheck(TUNNEL);
} finally {
  writeFileSync(HMR_FILE, originalHmrSource);
  await browser.close();
}

mkdirSync(join(import.meta.dirname, "results"), { recursive: true });
const file = join(import.meta.dirname, "results", `${new Date().toISOString().replaceAll(":", "-")}.json`);
writeFileSync(file, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));
console.log(`\nSaved ${file}`);

async function withPage(origin, work) {
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    return await work(page, origin);
  } finally {
    await context.close();
  }
}

/** Vite transforms modules on first request; keep that out of the timings. */
async function warmUp() {
  for (const [, origin] of PATHS) {
    await withPage(origin, (page) => loadPage(page, origin));
  }
}

async function loadPage(page, origin) {
  let requests = 0;
  page.on("request", () => requests++);
  await page.goto(`${origin}/`);
  await page.waitForFunction(() => window.__spike);
  const spike = await page.evaluate(() => window.__spike);
  return { ms: spike.loadedAt, requests, total: spike.total, depth: spike.depth };
}

async function comparePageLoads() {
  const runs = Object.fromEntries(PATHS.map(([name]) => [name, []]));
  for (let i = 0; i < PAGE_LOAD_RUNS; i++) {
    for (const [name, origin] of PATHS) {
      runs[name].push(await withPage(origin, (page) => loadPage(page, origin)));
    }
  }
  const summary = (list) => ({
    medianMs: round(median(list.map((run) => run.ms))),
    requests: list[0].requests,
    // 20 branches of 15 leaves (sum of 0..299) and a chain of depth 10.
    correct: list.every((run) => run.total === 44850 && run.depth === 10),
  });
  return Object.fromEntries(PATHS.map(([name]) => [name, summary(runs[name])]));
}

async function compareSequentialRequests() {
  const measure = (origin) =>
    withPage(origin, async (page) => {
      await page.goto(`${origin}/headers`);
      return page.evaluate(async (count) => {
        const started = performance.now();
        for (let i = 0; i < count; i++) {
          const response = await fetch("/src/generated/leaf_0_0.js", { cache: "no-store" });
          await response.text();
        }
        return (performance.now() - started) / count;
      }, SEQUENTIAL_REQUESTS);
    });
  const averages = {};
  for (const [name, origin] of PATHS) averages[`${name}AvgMs`] = round(await measure(origin));
  return averages;
}

async function compare(check) {
  return { direct: await check(DIRECT), tunnel: await check(TUNNEL) };
}

async function downloadCheck(origin) {
  return withPage(origin, async (page) => {
    await page.goto(`${origin}/headers`);
    return page.evaluate(async () => {
      const megabytes = 20;
      const started = performance.now();
      const bytes = new Uint8Array(await (await fetch(`/big?mb=${megabytes}`)).arrayBuffer());
      const ms = performance.now() - started;
      let intact = bytes.length === megabytes * 1024 * 1024;
      for (let i = 0; intact && i < bytes.length; i++) intact = bytes[i] === i % 251;
      return { intact, ms: Math.round(ms), mbPerSecond: Math.round((megabytes / ms) * 1000) };
    });
  });
}

async function uploadCheck(origin) {
  return withPage(origin, async (page) => {
    await page.goto(`${origin}/headers`);
    return page.evaluate(async () => {
      const size = 8 * 1024 * 1024;
      const body = new Uint8Array(size);
      for (let offset = 0; offset < size; offset += 65536) {
        crypto.getRandomValues(body.subarray(offset, offset + 65536));
      }
      const expected = [...new Uint8Array(await crypto.subtle.digest("SHA-256", body))]
        .map((byte) => byte.toString(16).padStart(2, "0"))
        .join("");
      const started = performance.now();
      const result = await (await fetch("/echo", { method: "POST", body })).json();
      return {
        intact: result.bytes === size && result.sha256 === expected,
        ms: Math.round(performance.now() - started),
      };
    });
  });
}

/** Events must arrive one by one (~200 ms apart), not buffered until the end. */
async function sseCheck(origin) {
  return withPage(origin, async (page) => {
    await page.goto(`${origin}/headers`);
    const arrivals = await page.evaluate(
      () =>
        new Promise((resolve, reject) => {
          const times = [];
          const source = new EventSource("/sse?count=10");
          source.onmessage = () => {
            times.push(performance.now());
            if (times.length === 10) {
              source.close();
              resolve(times);
            }
          };
          source.onerror = () => {
            source.close();
            reject(new Error(`SSE failed after ${times.length} events`));
          };
        }),
    );
    const gaps = arrivals.slice(1).map((time, i) => time - arrivals[i]);
    return {
      events: arrivals.length,
      spreadMs: round(arrivals.at(-1) - arrivals[0]),
      minGapMs: round(Math.min(...gaps)),
      streamed: arrivals.at(-1) - arrivals[0] > 1500,
    };
  });
}

/** Cancelling a stream mid-flight must not break the next request. */
async function abortThenReuseCheck(origin) {
  return withPage(origin, async (page) => {
    await page.goto(`${origin}/headers`);
    return page.evaluate(async () => {
      const controller = new AbortController();
      const response = await fetch("/sse?count=100", { signal: controller.signal });
      const reader = response.body.getReader();
      await reader.read();
      controller.abort();
      const after = await fetch("/headers");
      return { nextRequestStatus: after.status };
    });
  });
}

async function hmrCheck(origin) {
  return withPage(origin, async (page) => {
    const connected = page.waitForEvent("console", {
      predicate: (message) => message.text().includes("[vite] connected"),
      timeout: 10_000,
    });
    await loadPage(page, origin);
    await connected;
    await page.evaluate(() => {
      window.__beforeHmr = true;
    });

    const version = `v-${Date.now()}`;
    const started = Date.now();
    writeFileSync(HMR_FILE, `export const message = "${version}";\n`);
    await page.waitForFunction(
      (expected) => document.querySelector("#hmr")?.textContent === expected,
      version,
      { timeout: 10_000 },
    );
    return {
      connected: true,
      updateMs: Date.now() - started,
      // A full reload would have cleared the marker.
      hotUpdateWithoutReload: await page.evaluate(() => window.__beforeHmr === true),
    };
  });
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function round(value) {
  return Math.round(value * 10) / 10;
}
