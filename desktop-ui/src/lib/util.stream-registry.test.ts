/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import { createStreamRegistry } from "./util.stream-registry.ts";

test("closeAll closes the open streams, and their late events are dropped", () => {
  const registry = createStreamRegistry();
  const closed: string[] = [];
  const web = registry.add("web");
  const api = registry.add("api");
  web.opened(() => closed.push("web"));
  api.opened(() => closed.push("api"));

  assert.deepEqual(registry.closeAll(), ["web", "api"]);
  assert.deepEqual(closed, ["web", "api"]);
  assert.equal(web.live(), false);
  assert.equal(api.live(), false);
  assert.equal(registry.size(), 0);
});

test("a stream whose opening resolves after closeAll is closed at once", () => {
  const registry = createStreamRegistry();
  const web = registry.add("web");
  // Still opening: its key is returned, so its toast is dismissed.
  assert.deepEqual(registry.closeAll(), ["web"]);

  let closed = 0;
  web.opened(() => closed++);
  assert.equal(closed, 1);
  assert.equal(web.live(), false);
  assert.equal(registry.size(), 0);
});

test("a stream that settled has its transport closed once, and is forgotten", () => {
  // The daemon can send the last event, then keep the stream open while it fetches a
  // snapshot: settling closes it, so leaving the machine finds nothing left open.
  const registry = createStreamRegistry();
  let closed = 0;
  const web = registry.add("web");
  web.opened(() => closed++);
  web.ended();

  assert.equal(closed, 1);
  assert.equal(web.live(), false);
  assert.deepEqual(registry.closeAll(), []);
  assert.equal(closed, 1);
});

test("a stream that ended before its opening resolved is closed when it does", () => {
  const registry = createStreamRegistry();
  const web = registry.add("web");
  web.ended();
  let closed = 0;
  web.opened(() => closed++);
  assert.equal(closed, 1);
  assert.equal(registry.size(), 0);
});

test("a newer stream of the same key is its own: the old one's end leaves it live", () => {
  const registry = createStreamRegistry();
  const old = registry.add("web");
  old.ended();
  const fresh = registry.add("web");
  old.ended();

  assert.equal(old.live(), false);
  assert.equal(fresh.live(), true);
  assert.equal(registry.size(), 1);
});
