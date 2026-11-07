/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import { createEntitlementTracker } from "./plan-entitlement.ts";

const entitled = { entitled: true };
const notEntitled = { entitled: false };

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

test("the first answer, a missing plan or a cloud without plans is no change", () => {
  const track = createEntitlementTracker();
  assert.equal(track(undefined), null);
  assert.equal(track(entitled), null);
  assert.equal(track(null), null);
  assert.equal(track(entitled), null);
});
