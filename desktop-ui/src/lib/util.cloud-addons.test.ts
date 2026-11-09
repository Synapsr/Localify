/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import type { CloudAddonPricingDto, CloudPlanDto } from "../types/localify";

import {
  addonPriceLine,
  canAddCapacity,
  clampAddonUnits,
  hasAddons,
  heldAddonLine,
  heldAddonUnits,
} from "./util.cloud-addons.ts";

const usd: CloudAddonPricingDto = { currency: "USD", machine: 300, urlPack: 200, live: true };
const eur: CloudAddonPricingDto = { currency: "EUR", machine: 300, urlPack: 250, live: false };

function plan(overrides: Partial<CloudPlanDto> = {}): CloudPlanDto {
  return {
    status: "active",
    entitled: true,
    interval: "month",
    trialEndsAt: null,
    renewsAt: null,
    endsAt: null,
    trialAvailable: false,
    limits: { servingMachines: 2, publicUrls: 10 },
    usage: { servingMachines: 1, publicUrls: 3 },
    addons: { machines: 0, urlPacks: 0 },
    ...overrides,
  };
}

test("capacity is added to an entitled Pro plan only", () => {
  assert.equal(canAddCapacity(plan()), true);
  assert.equal(canAddCapacity(plan({ status: "trialing" })), true);
  assert.equal(canAddCapacity(plan({ status: "none", entitled: false })), false);
  assert.equal(canAddCapacity(plan({ status: "past_due", entitled: false })), false);
  assert.equal(canAddCapacity(plan({ status: "override" })), false);
});

test("held add-ons send the person to the portal", () => {
  assert.equal(hasAddons(plan()), false);
  const machines = plan({ addons: { machines: 2, urlPacks: 0 } });
  assert.equal(hasAddons(machines), true);
  assert.equal(heldAddonUnits(machines, "machine"), 2);
  assert.equal(heldAddonUnits(machines, "urls"), 0);
  assert.equal(
    heldAddonLine("machine", 2),
    "You have 2 extra serving machines. Change how many in the customer portal.",
  );
  assert.equal(heldAddonLine("machine", 1).startsWith("You have 1 extra serving machine."), true);
  assert.equal(heldAddonLine("urls", 3).startsWith("You have 30 extra public URLs."), true);
});

test("the stepper stays between 1 and 20", () => {
  assert.equal(clampAddonUnits(0), 1);
  assert.equal(clampAddonUnits(7), 7);
  assert.equal(clampAddonUnits(21), 20);
  assert.equal(clampAddonUnits(Number.NaN), 1);
});

test("the price line says what the units cost and buy", () => {
  assert.equal(addonPriceLine("machine", 1, usd), "$3 a month for 1 more serving machine");
  assert.equal(
    addonPriceLine("machine", 3, usd),
    "$9 a month for 3 more serving machines ($3 each)",
  );
  assert.equal(addonPriceLine("urls", 1, usd), "$2 a month for 10 more public URLs");
  assert.equal(addonPriceLine("urls", 2, eur), "€5 a month for 20 more public URLs (€2.50 per 10)");
});
