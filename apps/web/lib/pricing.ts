import { headers } from "next/headers";

import { parsePricing } from "./pricing-response";

// Localify Cloud prices, as Polar sells them. The cloud serves them from Polar at
// GET /v1/pricing (docs/public-urls.md, "Plans"); the site falls back to the values
// below whenever that endpoint fails, so a page never breaks over a price.

export type Currency = "usd" | "eur";

// An amount in minor units (cents), in one currency.
export type Money = { amount: number; currency: Currency };

export type Pricing = {
  currency: Currency;
  pro: {
    monthly: Money;
    yearly: Money;
    includes: { servingMachines: number; publicUrls: number };
  };
  addons: {
    machine: Money & { unit: number };
    urls: Money & { unit: number };
  };
};

const PRICING_URL = "https://api.localify.dev/v1/pricing";
const REVALIDATE_SECONDS = 60 * 60;
// After a failure, the fallback is kept this long before the endpoint is tried again.
const RETRY_SECONDS = 5 * 60;
// A stalled endpoint costs a page at most this much render time.
const TIMEOUT_MS = 1000;

// The same numbers in both currencies.
const fallback = (currency: Currency): Pricing => ({
  currency,
  pro: {
    monthly: { amount: 799, currency },
    yearly: { amount: 7900, currency },
    includes: { servingMachines: 2, publicUrls: 10 },
  },
  addons: {
    machine: { amount: 300, currency, unit: 1 },
    urls: { amount: 200, currency, unit: 10 },
  },
});

// Team isn't sold through Polar yet, so the endpoint doesn't price it: the site quotes
// it, with the same number in both currencies, and the Team button opens a contact form.
export const TEAM_PLAN = { monthly: 4900, seats: 3 } as const;

// EU and EEA countries pay in euros, everyone else in US dollars, as the cloud decides.
const EURO_COUNTRIES = new Set([
  "AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "ES", "FI", "FR", "GR", "HR", "HU",
  "IE", "IT", "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
  "IS", "LI", "NO",
]);

const currencyForCountry = (country: string | null | undefined): Currency =>
  country && EURO_COUNTRIES.has(country.toUpperCase()) ? "eur" : "usd";

// Per Worker isolate: OpenNext keeps no data cache by default, so this keeps the
// endpoint from being asked on every page view. It holds plain, validated data
// only: a Worker must not await another request's promise or touch its Response,
// so concurrent misses each fetch.
const cache = new Map<Currency, { pricing: Pricing; expiresAt: number }>();

const fetchPricing = async (currency: Currency): Promise<Pricing | null> => {
  try {
    const response = await fetch(`${PRICING_URL}?currency=${currency}`, {
      headers: { accept: "application/json" },
      next: { revalidate: REVALIDATE_SECONDS },
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    if (!response.ok) return null;
    return parsePricing(await response.json(), currency);
  } catch {
    return null;
  }
};

const getPricingFor = async (currency: Currency): Promise<Pricing> => {
  const cached = cache.get(currency);
  if (cached && cached.expiresAt > Date.now()) return cached.pricing;

  const live = await fetchPricing(currency);
  const pricing = live ?? fallback(currency);
  const ttl = live ? REVALIDATE_SECONDS : RETRY_SECONDS;
  cache.set(currency, { pricing, expiresAt: Date.now() + ttl * 1000 });
  return pricing;
};

// The prices for this visitor, in the currency of the country Cloudflare sees
// (CF-IPCountry). Reading the header renders the page per request.
export const getPricing = async (): Promise<Pricing> => {
  const country = (await headers()).get("cf-ipcountry");
  return getPricingFor(currencyForCountry(country));
};

// "$7.99", "79 €", "$3": cents only when there are some.
export const formatMoney = (amount: number, currency: Currency, locale: string) => {
  const value = amount / 100;
  const digits = Number.isInteger(value) ? 0 : 2;
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: currency.toUpperCase(),
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(value);
};
