/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import { createEntitlementTracker } from "./plan-entitlement.ts";

const limits = { servingMachines: 2, publicUrls: 10 };
const entitled = { entitled: true, limits };
const notEntitled = { entitled: false, limits };

test("a plan entitled outside any checkout is a change", () => {
  const track = createEntitlementTracker();
  assert.equal(track(notEntitled), null);
  // Subscribed through the CLI, or a checkout finished after the 15-minute watch.
  assert.equal(track(entitled), "entitled");
  assert.equal(track(entitled), null);
});

test("losing the plan is a change too", () => {
  const track = createEntitlementTracker();
  assert.equal(track(entitled), null);
  assert.equal(track(notEntitled), "lost");
  assert.equal(track(notEntitled), null);
});

test("new limits on an entitled plan are a change", () => {
  const track = createEntitlementTracker();
  assert.equal(track(entitled), null);
  // A machine add-on: a Mac blocked on serving_slots_full can serve now.
  assert.equal(track({ entitled: true, limits: { ...limits, servingMachines: 3 } }), "limits");
  assert.equal(track({ entitled: true, limits: { ...limits, servingMachines: 3 } }), null);
  assert.equal(track({ entitled: true, limits: { servingMachines: 3, publicUrls: 20 } }), "limits");
});

test("the first answer, a missing plan or a cloud without plans is no change", () => {
  const track = createEntitlementTracker();
  assert.equal(track(undefined), null);
  assert.equal(track(entitled), null);
  assert.equal(track(null), null);
  assert.equal(track(entitled), null);
});
