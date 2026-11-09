import type { Currency, Money, Pricing } from "./pricing";

// Checks a GET /v1/pricing response before the site shows it. It is kept out of
// lib/pricing.ts, which reads request headers through Next.js, so that
// lib/pricing-response.test.mjs runs it with plain Node (`pnpm web:test`).

// Sane bounds: anything outside them is a bad response, not a price to show.
const MAX_AMOUNT = 100_000; // minor units, so 1,000.00
const MAX_COUNT = 1_000;

const isIntegerIn = (value: unknown, max: number): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 1 && value <= max;

// The endpoint names currencies as Polar does, in upper case ("EUR"); the site keeps
// them in lower case, so either is accepted.
const isCurrency = (value: unknown, currency: Currency) =>
  typeof value === "string" && value.toLowerCase() === currency;

const isMoney = (value: unknown, currency: Currency): value is Money => {
  const money = value as { amount?: unknown; currency?: unknown } | null;
  return (
    typeof money === "object" &&
    money !== null &&
    isIntegerIn(money.amount, MAX_AMOUNT) &&
    isCurrency(money.currency, currency)
  );
};

// The copy says "+1 serving machine" and "+10 public URLs", so the units must match it.
const isAddon = (value: unknown, currency: Currency, unit: number): value is Money & { unit: number } =>
  isMoney(value, currency) && (value as { unit?: unknown }).unit === unit;

// Keeps only the fields the site shows, after checking each one.
export const parsePricing = (body: unknown, currency: Currency): Pricing | null => {
  const data = body as {
    currency?: unknown;
    pro?: { monthly?: unknown; yearly?: unknown; includes?: { servingMachines?: unknown; publicUrls?: unknown } };
    addons?: { machine?: unknown; urls?: unknown };
  } | null;
  if (typeof data !== "object" || data === null || !isCurrency(data.currency, currency)) return null;

  const { pro, addons } = data;
  if (
    !isMoney(pro?.monthly, currency) ||
    !isMoney(pro?.yearly, currency) ||
    !isIntegerIn(pro?.includes?.servingMachines, MAX_COUNT) ||
    !isIntegerIn(pro?.includes?.publicUrls, MAX_COUNT) ||
    !isAddon(addons?.machine, currency, 1) ||
    !isAddon(addons?.urls, currency, 10)
  ) {
    return null;
  }

  return {
    currency,
    pro: {
      monthly: { amount: pro.monthly.amount, currency },
      yearly: { amount: pro.yearly.amount, currency },
      includes: { servingMachines: pro.includes.servingMachines, publicUrls: pro.includes.publicUrls },
    },
    addons: {
      machine: { amount: addons.machine.amount, currency, unit: addons.machine.unit },
      urls: { amount: addons.urls.amount, currency, unit: addons.urls.unit },
    },
  };
};
