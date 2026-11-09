import assert from "node:assert/strict";
import { test } from "node:test";

import { parsePricing } from "./pricing-response.ts";

// GET https://api.localify.dev/v1/pricing?currency=eur, as it answers.
const live = {
  currency: "EUR",
  pro: {
    monthly: { amount: 799, currency: "EUR" },
    yearly: { amount: 7900, currency: "EUR" },
    trialDays: 7,
    includes: { servingMachines: 2, publicUrls: 10 },
  },
  addons: {
    machine: { amount: 300, currency: "EUR", unit: 1 },
    urls: { amount: 200, currency: "EUR", unit: 10 },
  },
  updatedAt: "2026-11-09T07:48:36.863Z",
};

test("accepts the endpoint's upper-case currency and keeps it lower case", () => {
  assert.deepEqual(parsePricing(live, "eur"), {
    currency: "eur",
    pro: {
      monthly: { amount: 799, currency: "eur" },
      yearly: { amount: 7900, currency: "eur" },
      includes: { servingMachines: 2, publicUrls: 10 },
    },
    addons: {
      machine: { amount: 300, currency: "eur", unit: 1 },
      urls: { amount: 200, currency: "eur", unit: 10 },
    },
  });
});

test("accepts lower-case currencies too", () => {
  const lower = JSON.parse(JSON.stringify(live).replaceAll('"EUR"', '"eur"'));
  assert.equal(parsePricing(lower, "eur")?.currency, "eur");
});

test("rejects another currency than the one asked for", () => {
  assert.equal(parsePricing(live, "usd"), null);
  const mixed = { ...live, addons: { ...live.addons, urls: { ...live.addons.urls, currency: "USD" } } };
  assert.equal(parsePricing(mixed, "eur"), null);
});

test("rejects units the copy doesn't describe", () => {
  const urls = { ...live.addons.urls, unit: 5 };
  assert.equal(parsePricing({ ...live, addons: { ...live.addons, urls } }, "eur"), null);
});
