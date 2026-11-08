// `GET /v1/pricing` (docs/public-urls.md, "Pricing"): the live prices of
// Pro and its add-ons, read from Polar, for the website. Public, with CORS
// for the website's origins. Nothing else reads it: no other route waits on
// it or on the Polar call behind it.
//
// One Polar call reads the four products in both currencies. The answer is
// kept an hour per currency in the Cache API and in the isolate, and served
// stale for up to 7 days when it cannot be read again; with nothing kept, it
// is a 503. The public traffic shares Polar's rate limit with billing, so
// whatever the traffic, a location asks Polar at most once a minute, and
// after a failure waits 5 minutes or what `Retry-After` asks: a refresh
// marker in the Cache API and in the isolate, and the `PRICING_LIMITER`
// binding before any call.

import { setting } from "./config";
import { PRO, URLS_PER_PACK } from "./entitlement";
import { isRateLimited, PolarApi, type PolarProduct } from "./polar";

const MINUTE = 60 * 1000;
const HOUR = 60 * MINUTE;
/** A price is read again from Polar after this. */
const FRESH_MS = HOUR;
/** The longest a price is served, stale, when it cannot be read again. */
const STALE_MS = 7 * 24 * HOUR;
/** At most one Polar call per location in this time, for both currencies. */
const REFRESH_INTERVAL_MS = MINUTE;
/** After a failed call, the next one waits this long, or what `Retry-After` asks. */
const FAILURE_BACKOFF_MS = 5 * MINUTE;
/** How long a browser may reuse an answer. */
const BROWSER_MAX_AGE_SECONDS = 300;

const ALLOWED_ORIGINS = new Set(["https://localify.dev", "https://www.localify.dev"]);

export type Currency = "USD" | "EUR";
const CURRENCIES: Currency[] = ["USD", "EUR"];

/**
 * The EU's 27 countries and the rest of the EEA (Iceland, Liechtenstein,
 * Norway) see euros; every other country, Switzerland and the United
 * Kingdom included, sees dollars. Only a default for display: Polar picks
 * the checkout's currency from the buyer's address itself.
 */
const EURO_COUNTRIES = new Set([
  "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE",
  "IT", "LV", "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE",
  "IS", "LI", "NO",
]); // prettier-ignore

type Amount = { amount: number; currency: Currency };

export type Pricing = {
  currency: Currency;
  pro: {
    monthly: Amount;
    yearly: Amount;
    trialDays: number | null;
    includes: { servingMachines: number; publicUrls: number };
  };
  addons: {
    machine: Amount & { unit: number };
    urls: Amount & { unit: number };
  };
  /** When the prices were read from Polar, RFC 3339. */
  updatedAt: string;
};

/** The prices each isolate keeps, by currency. */
const kept = new Map<Currency, Pricing>();
/** When this isolate may next ask Polar. */
let nextAttemptAt = 0;

/** Forgets what this isolate keeps, for the tests: a new isolate. */
export function forgetPricing(): void {
  kept.clear();
  nextAttemptAt = 0;
}

export async function handlePricing(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
): Promise<Response> {
  const cors = corsHeaders(request);
  if (request.method === "OPTIONS") return new Response(null, { status: 204, headers: cors });
  const currency = currencyOf(request);
  const pricing = await pricingIn(currency, env, ctx, new URL(request.url).origin);
  if (pricing === null) {
    return Response.json(
      {
        error: "pricing_unavailable",
        message: "Prices cannot be read right now. Try again in a minute.",
      },
      { status: 503, headers: { ...cors, "cache-control": "no-store" } },
    );
  }
  return Response.json(pricing, {
    headers: {
      ...cors,
      "cache-control": `public, max-age=${BROWSER_MAX_AGE_SECONDS}`,
      vary: "Origin, CF-IPCountry",
    },
  });
}

/** `?currency=` when it is USD or EUR, in any case; otherwise from `CF-IPCountry`. */
export function currencyOf(request: Request): Currency {
  const asked = new URL(request.url).searchParams.get("currency")?.trim().toUpperCase();
  if (asked === "USD" || asked === "EUR") return asked;
  const country = request.headers.get("cf-ipcountry")?.trim().toUpperCase() ?? "";
  return EURO_COUNTRIES.has(country) ? "EUR" : "USD";
}

function corsHeaders(request: Request): Record<string, string> {
  const origin = request.headers.get("origin") ?? "";
  if (!ALLOWED_ORIGINS.has(origin)) return {};
  return {
    "access-control-allow-origin": origin,
    "access-control-allow-methods": "GET, OPTIONS",
    "access-control-max-age": "86400",
  };
}

/**
 * The prices in `currency`: fresh from the isolate or the Cache API, else
 * from Polar when a refresh may run, else stale up to 7 days, else null.
 */
async function pricingIn(
  currency: Currency,
  env: Env,
  ctx: ExecutionContext,
  origin: string,
): Promise<Pricing | null> {
  const now = Date.now();
  const cache = typeof caches === "undefined" ? null : caches.default;
  const key = (name: string) => new Request(`${origin}/v1/pricing/cached/${name}`);
  let stale = usable(kept.get(currency), now);
  if (stale === null) kept.delete(currency);
  if (stale !== null && isFresh(stale, now)) return stale;
  if (cache !== null) {
    try {
      const hit = await cache.match(key(currency.toLowerCase()));
      const cached = usable(hit ? ((await hit.json()) as Pricing) : null, now);
      if (cached !== null && (stale === null || cached.updatedAt > stale.updatedAt)) {
        kept.set(currency, cached);
        stale = cached;
        if (isFresh(cached, now)) return cached;
      }
    } catch (error) {
      console.error("Reading the cached prices failed", error);
    }
  }
  if (!(await mayRefresh(env, cache, key("refresh"), now))) return stale;

  let read: Map<Currency, Pricing>;
  try {
    read = await readPricing(env, now);
  } catch (error) {
    const wait = isRateLimited(error)
      ? Math.max(error.retryAfterMs, FAILURE_BACKOFF_MS)
      : FAILURE_BACKOFF_MS;
    await holdRefreshes(cache, key("refresh"), now + wait);
    console.error(
      stale === null
        ? "Reading the prices from Polar failed, with none kept: /v1/pricing answers 503"
        : "Reading the prices from Polar failed: /v1/pricing serves the ones kept",
      error,
    );
    return stale;
  }
  for (const [c, pricing] of read) {
    kept.set(c, pricing);
    if (cache !== null) {
      const response = Response.json(pricing, {
        headers: { "cache-control": `max-age=${STALE_MS / 1000}` },
      });
      ctx.waitUntil(
        cache
          .put(key(c.toLowerCase()), response)
          .catch((error: unknown) => console.error("Caching the prices failed", error)),
      );
    }
  }
  return read.get(currency) ?? null;
}

/**
 * Whether this request may ask Polar: no refresh attempted here within the
 * interval, in this isolate or by the marker in the Cache API, and room left
 * in `PRICING_LIMITER`. A refresh that may run claims the interval first,
 * so the requests behind it serve what is kept. Concurrent isolates can both
 * pass the marker, which has no compare-and-set: the limiter bounds them.
 */
async function mayRefresh(
  env: Env,
  cache: Cache | null,
  marker: Request,
  now: number,
): Promise<boolean> {
  if (now < nextAttemptAt) return false;
  if (cache !== null) {
    try {
      const hit = await cache.match(marker);
      const until = hit ? Number(((await hit.json()) as { until?: unknown }).until) : 0;
      if (Number.isFinite(until) && until > now) {
        nextAttemptAt = until;
        return false;
      }
    } catch (error) {
      console.error("Reading the prices' refresh marker failed", error);
    }
  }
  if (!(await env.PRICING_LIMITER.limit({ key: "polar-products" })).success) {
    nextAttemptAt = now + REFRESH_INTERVAL_MS;
    return false;
  }
  await holdRefreshes(cache, marker, now + REFRESH_INTERVAL_MS);
  return true;
}

/** No refresh before `until`, in this isolate and, through the marker, at this location. */
async function holdRefreshes(cache: Cache | null, marker: Request, until: number): Promise<void> {
  nextAttemptAt = Math.max(nextAttemptAt, until);
  if (cache === null) return;
  try {
    const seconds = Math.max(1, Math.ceil((until - Date.now()) / 1000));
    await cache.put(
      marker,
      Response.json({ until }, { headers: { "cache-control": `max-age=${seconds}` } }),
    );
  } catch (error) {
    console.error("Writing the prices' refresh marker failed", error);
  }
}

/** The prices when they are at most 7 days old, else null. */
function usable(pricing: Pricing | null | undefined, now: number): Pricing | null {
  return pricing && Date.parse(pricing.updatedAt) > now - STALE_MS ? pricing : null;
}

function isFresh(pricing: Pricing, now: number): boolean {
  return Date.parse(pricing.updatedAt) > now - FRESH_MS;
}

/** Both currencies' prices, from one Polar call. Throws when a price is missing. */
async function readPricing(env: Env, now: number): Promise<Map<Currency, Pricing>> {
  const ids = {
    monthly: setting(env, "POLAR_PRODUCT_PRO_MONTHLY"),
    yearly: setting(env, "POLAR_PRODUCT_PRO_YEARLY"),
    machine: setting(env, "POLAR_PRODUCT_ADDON_MACHINE"),
    urls: setting(env, "POLAR_PRODUCT_ADDON_URLS"),
  };
  const products = await new PolarApi(env).listProducts(Object.values(ids));
  const product = (id: string): PolarProduct => {
    const found = products.find((p) => p.id === id);
    if (!found) throw new Error(`Polar returned no product ${id}`);
    return found;
  };
  const amount = (id: string, currency: Currency): Amount => {
    const price = product(id).prices.find((p) => p.currency === currency);
    if (!price) throw new Error(`Product ${id} has no ${currency} price`);
    return { amount: price.amount, currency };
  };
  const updatedAt = new Date(now).toISOString();
  const trialDays = trialDaysOf(product(ids.monthly));
  return new Map(
    CURRENCIES.map((currency) => [
      currency,
      {
        currency,
        pro: {
          monthly: amount(ids.monthly, currency),
          yearly: amount(ids.yearly, currency),
          trialDays,
          includes: { servingMachines: PRO.servingMachines, publicUrls: PRO.publicUrls },
        },
        addons: {
          machine: { ...amount(ids.machine, currency), unit: 1 },
          urls: { ...amount(ids.urls, currency), unit: URLS_PER_PACK },
        },
        updatedAt,
      },
    ]),
  );
}

/** The trial in days, or null without one or with one counted in months or years. */
function trialDaysOf(product: PolarProduct): number | null {
  const count = product.trialIntervalCount;
  if (count === null) return null;
  if (product.trialInterval === "day") return count;
  if (product.trialInterval === "week") return count * 7;
  return null;
}
