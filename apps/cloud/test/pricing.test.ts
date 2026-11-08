// `GET /v1/pricing` (docs/public-urls.md, "Pricing"): live prices from a
// fake Polar, the currency choice, the Cache API, stale answers and CORS.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { currencyOf, forgetPricing } from "../src/pricing";
import { api, createContext, jsonResponse, settle, type TestContext } from "./support";

const POLAR = "https://polar.test";
const HOUR = 60 * 60 * 1000;

let context: TestContext;
let cache: FakeCache;
let failing: boolean;

/** The Cache API's `caches.default`, kept in memory. */
class FakeCache {
  readonly entries = new Map<string, string>();

  async match(request: Request): Promise<Response | undefined> {
    const text = this.entries.get(request.url);
    return text === undefined ? undefined : new Response(text);
  }

  async put(request: Request, response: Response): Promise<void> {
    this.entries.set(request.url, await response.text());
  }
}

function fixed(currency: string, amount: number, archived = false) {
  return {
    amount_type: "fixed",
    price_currency: currency,
    price_amount: amount,
    is_archived: archived,
  };
}

function unitBased(currency: string, rate: string) {
  return {
    amount_type: "unit_based",
    price_currency: currency,
    is_archived: false,
    tiers: { type: "volume", tiers: [{ bound: null, unit_amount: rate }] },
  };
}

const PRODUCTS = [
  {
    id: "prod-monthly",
    trial_interval: "day",
    trial_interval_count: 7,
    prices: [fixed("usd", 999, true), fixed("usd", 799), fixed("eur", 799)],
  },
  {
    id: "prod-yearly",
    trial_interval: "day",
    trial_interval_count: 7,
    prices: [fixed("usd", 7900), fixed("eur", 7900)],
  },
  { id: "prod-machine", prices: [unitBased("usd", "300"), unitBased("eur", "300")] },
  { id: "prod-urls", prices: [unitBased("usd", "200"), unitBased("eur", "200.00")] },
];

beforeEach(() => {
  context = createContext({
    POLAR_PRODUCT_ADDON_MACHINE: "prod-machine",
    POLAR_PRODUCT_ADDON_URLS: "prod-urls",
  });
  forgetPricing();
  cache = new FakeCache();
  vi.stubGlobal("caches", { default: cache });
  failing = false;
  context.routes.set(`GET ${POLAR}/v1/products/`, () =>
    failing
      ? jsonResponse({ error: "Internal" }, 500)
      : jsonResponse({ items: PRODUCTS, pagination: { total_count: 4, max_page: 1 } }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function pricing(query = "", headers: Record<string, string> = {}) {
  const response = await api(context, "GET", `/v1/pricing${query}`, { headers });
  await settle(context);
  return { response, body: (await response.json()) as Record<string, unknown> };
}

function productCalls() {
  return context.calls.filter((call) => call.url.startsWith(`${POLAR}/v1/products/`));
}

function priced(currency: "USD" | "EUR", updatedAt: string) {
  return {
    currency,
    pro: {
      monthly: { amount: 799, currency },
      yearly: { amount: 7900, currency },
      trialDays: 7,
      includes: { servingMachines: 2, publicUrls: 10 },
    },
    addons: {
      machine: { amount: 300, currency, unit: 1 },
      urls: { amount: 200, currency, unit: 10 },
    },
    updatedAt,
  };
}

describe("GET /v1/pricing", () => {
  it("reads the four products from Polar in one call, without a token, in minor units", async () => {
    const { response, body } = await pricing("?currency=usd");
    expect(response.status).toBe(200);
    expect(body).toEqual(priced("USD", body.updatedAt as string));
    expect(Date.parse(body.updatedAt as string)).toBeGreaterThan(Date.now() - 1000);

    const [call] = productCalls();
    expect(productCalls()).toHaveLength(1);
    const query = new URL(call!.url).searchParams;
    expect(query.getAll("id")).toEqual([
      "prod-monthly",
      "prod-yearly",
      "prod-machine",
      "prod-urls",
    ]);
    expect(call!.headers.get("polar-version")).toBe("2026-10");
    expect(response.headers.get("cache-control")).toBe("public, max-age=300");
  });

  it("takes the currency from ?currency=, else from the visitor's country", () => {
    const currency = (query: string, country?: string) =>
      currencyOf(
        new Request(`https://api.localify.dev/v1/pricing${query}`, {
          headers: country ? { "cf-ipcountry": country } : {},
        }),
      );
    expect(currency("?currency=eur", "US")).toBe("EUR");
    expect(currency("?currency=USD", "FR")).toBe("USD");
    // An unknown currency falls back to the country.
    expect(currency("?currency=gbp", "DE")).toBe("EUR");
    for (const country of ["FR", "DE", "IE", "GR", "NO", "IS", "LI"]) {
      expect(currency("", country), country).toBe("EUR");
    }
    for (const country of ["US", "CH", "GB", "JP", "XX"]) {
      expect(currency("", country), country).toBe("USD");
    }
    expect(currency("")).toBe("USD");
  });

  it("answers in the visitor's currency", async () => {
    const { body } = await pricing("", { "cf-ipcountry": "FR" });
    expect(body).toEqual(priced("EUR", body.updatedAt as string));
  });

  it("serves both currencies from the cache for an hour, across isolates", async () => {
    const first = await pricing("?currency=usd");
    expect((await pricing("?currency=eur")).body.currency).toBe("EUR");
    // A new isolate finds them in the Cache API.
    forgetPricing();
    expect((await pricing("?currency=usd")).body).toEqual(first.body);
    expect(productCalls()).toHaveLength(1);

    vi.spyOn(Date, "now").mockReturnValue(Date.parse(first.body.updatedAt as string) + HOUR + 1);
    const later = await pricing("?currency=usd");
    expect(productCalls()).toHaveLength(2);
    expect(later.body.updatedAt).not.toBe(first.body.updatedAt);
  });

  it("serves the prices kept when Polar fails once they are stale", async () => {
    const first = await pricing("?currency=eur");
    vi.spyOn(console, "error").mockImplementation(() => {});
    failing = true;
    vi.spyOn(Date, "now").mockReturnValue(Date.parse(first.body.updatedAt as string) + 3 * HOUR);

    const stale = await pricing("?currency=eur");
    expect(stale.response.status).toBe(200);
    expect(stale.body).toEqual(first.body);
    // From the Cache API too, in a new isolate.
    forgetPricing();
    expect((await pricing("?currency=eur")).body).toEqual(first.body);
  });

  it("answers 503 when nothing is kept and Polar fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    failing = true;
    const { response, body } = await pricing("?currency=usd", { origin: "https://localify.dev" });
    expect(response.status).toBe(503);
    expect(body.error).toBe("pricing_unavailable");
    expect(response.headers.get("access-control-allow-origin")).toBe("https://localify.dev");
    expect(cache.entries.size).toBe(0);
  });

  it("answers 503 rather than a partial answer when a price is missing", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    context.routes.set(`GET ${POLAR}/v1/products/`, () =>
      jsonResponse({ items: PRODUCTS.slice(0, 3), pagination: { total_count: 3, max_page: 1 } }),
    );
    expect((await pricing("?currency=usd")).response.status).toBe(503);
  });

  it("allows GET from the website's origins only", async () => {
    for (const origin of ["https://localify.dev", "https://www.localify.dev"]) {
      const { response } = await pricing("", { origin });
      expect(response.headers.get("access-control-allow-origin")).toBe(origin);
      expect(response.headers.get("vary")).toContain("Origin");
    }
    const { response } = await pricing("", { origin: "https://evil.example" });
    expect(response.headers.get("access-control-allow-origin")).toBeNull();

    const preflight = await api(context, "OPTIONS", "/v1/pricing", {
      headers: { origin: "https://www.localify.dev" },
    });
    expect(preflight.status).toBe(204);
    expect(preflight.headers.get("access-control-allow-methods")).toBe("GET, OPTIONS");
    expect((await api(context, "POST", "/v1/pricing")).status).toBe(405);
  });

  it("is on no other route's path", async () => {
    await api(context, "GET", "/v1/me");
    await api(context, "POST", "/v1/auth/device/token", { body: {} });
    expect(productCalls()).toEqual([]);
  });
});
