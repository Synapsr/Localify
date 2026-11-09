/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import type {
  RemoteInventoryItemDto,
  RemoteMachineDto,
  RemoteStatusDto,
} from "../types/localify.ts";

import {
  describeMachine,
  describeRelay,
  groupInventory,
  isFinalCode,
  needsPro,
  ownerLabel,
  PAIRING_CANCELLED,
  pairingCancelledMessage,
  remoteErrorMessage,
  remoteHost,
  serviceChips,
  serviceTone,
} from "./util.remote.ts";

function status(overrides: Partial<RemoteStatusDto> = {}): RemoteStatusDto {
  return {
    relay: "connected",
    blocked: null,
    retryAt: null,
    settings: { share: false, control: false, backgroundStarts: false },
    fingerprint: "1a2b 3c4d 5e6f 7a8b",
    ...overrides,
  };
}

function machine(overrides: Partial<RemoteMachineDto> = {}): RemoteMachineDto {
  return {
    id: "mB",
    name: "teo-mini",
    self: false,
    paired: true,
    state: "connected",
    version: "1.5.0",
    sharing: true,
    fingerprint: null,
    since: null,
    ...overrides,
  };
}

function item(overrides: Partial<RemoteInventoryItemDto> = {}): RemoteInventoryItemDto {
  return {
    project: { id: "louez", name: "Louez" },
    environment: { slug: null, name: "main", primary: true, branch: "main" },
    service: { id: "web", name: "web", configRev: "0123456789abcdef" },
    localUrl: "https://web.louez.localify",
    owner: null,
    run: null,
    process: "stopped",
    port: { number: 3000, reachable: false },
    output: { available: false, reason: "no_run" },
    publicUrl: null,
    controls: {
      start: { allowed: false, code: "control_disabled" },
      stop: { allowed: false, code: "control_disabled" },
      restart: { allowed: false, code: "control_disabled" },
    },
    ...overrides,
  };
}

test("the relay header says what is known about the others", () => {
  assert.equal(describeRelay(status()).label, "Remote access: connected");
  assert.equal(describeRelay(status({ relay: "connecting" })).label, "Connecting…");
  const now = Date.parse("2026-11-08T10:00:00Z");
  const reconnecting = describeRelay(
    status({ relay: "reconnecting", retryAt: "2026-11-08T10:00:08Z" }),
    now,
  );
  assert.equal(reconnecting.label, "Reconnecting… (in 8 s)");
  assert.equal(reconnecting.retry, true);
  const lapsed = status({
    relay: "blocked",
    blocked: { code: "subscription_required", message: "Remote access needs Localify Cloud Pro." },
  });
  assert.equal(describeRelay(lapsed).label, "Off");
  assert.equal(describeRelay(lapsed).detail, "Remote access needs Localify Cloud Pro.");
  assert.equal(needsPro(lapsed), true);
  assert.equal(needsPro(status()), false);
});

test("each machine state reads as the contract's table", () => {
  assert.equal(describeMachine(machine()).label, null);
  assert.equal(describeMachine(machine({ state: "not_sharing" })).label, "Paired · not sharing");
  assert.equal(describeMachine(machine({ state: "not_responding" })).label, "Not responding");
  const unpaired = describeMachine(machine({ state: "not_paired", paired: false }));
  assert.equal(unpaired.label, "Not paired");
  assert.equal(unpaired.action, "pair");
  assert.equal(unpaired.viewable, false);
  const mismatch = describeMachine(machine({ state: "key_mismatch" }));
  assert.equal(mismatch.detail, "Don't trust this machine until you pair it again.");
  assert.equal(mismatch.action, "unpair");
  assert.equal(mismatch.viewable, false);
  assert.match(describeMachine(machine({ state: "incompatible" })).detail ?? "", /can't talk/);
  assert.equal(describeMachine(machine({ state: "signed_out" })).action, "unpair");
  assert.equal(describeMachine(machine({ state: "unavailable" })).action, "upgrade");
  assert.equal(describeMachine(machine({ state: "offline" })).viewable, true);
});

test("errors from a target use the viewer's words, with the machine's name", () => {
  assert.equal(
    remoteErrorMessage("not_shared", "B", "x"),
    "B doesn't share its projects. Turn on sharing in Localify on B.",
  );
  assert.equal(
    remoteErrorMessage("control_disabled", "B", "x"),
    "B doesn't allow remote controls.",
  );
  assert.equal(remoteErrorMessage("something_new", "B", "As it came."), "As it came.");
  assert.equal(isFinalCode("revoked"), true);
  assert.equal(isFinalCode("relay_lost"), false);
});

test("a pairing nobody can confirm says how to fix it", () => {
  assert.equal(
    pairingCancelledMessage("nobody_to_confirm", "", "B"),
    "Nobody can confirm on B. Open Localify on B, or run `localify machines pair --accept` there, then try again.",
  );
  assert.equal(pairingCancelledMessage("cancelled", PAIRING_CANCELLED, "B"), PAIRING_CANCELLED);
  assert.equal(
    pairingCancelledMessage("timeout", "It took too long.", "B"),
    "Pairing cancelled. Nothing was saved. It took too long.",
  );
});

test("a running service whose port does not answer yet is starting", () => {
  const chips = serviceChips(item({ process: "running", owner: "background" }));
  assert.deepEqual(
    chips.map((chip) => `${chip.name}: ${chip.value}`),
    ["Process: Starting", "Port: :3000 not reachable", "Output: No run yet", "Public URL: None"],
  );
  const serving = serviceChips(
    item({
      process: "running",
      port: { number: 3000, reachable: true },
      output: { available: true, runId: "AAAAAAAAAAAAAAAAAAAAAA" },
      publicUrl: { url: "https://louez.localify.fyi", serving: true, reason: null },
    }),
  );
  assert.deepEqual(
    serving.map((chip) => chip.value),
    ["Running", ":3000 reachable", "Available", "Serving"],
  );
  const down = serviceChips(
    item({ publicUrl: { url: "https://x.localify.fyi", serving: false, reason: "tunnel_down" } }),
  );
  assert.equal(down[3].value, "Tunnel down");
  assert.equal(ownerLabel("terminal"), "In a terminal");
  assert.equal(ownerLabel(null), null);
});

test("a target's local URL is shown without its scheme", () => {
  assert.equal(remoteHost("https://web.louez.localify/"), "web.louez.localify");
});

test("a service's dot is green once it answers, red after a failed exit", () => {
  assert.equal(serviceTone(item({ process: "running", owner: "background" })), "pending");
  assert.equal(
    serviceTone(item({ process: "running", port: { number: 3000, reachable: true } })),
    "ok",
  );
  assert.equal(
    serviceTone(item({ port: { number: 3000, reachable: null }, process: "running" })),
    "pending",
  );
  // A port that answers is up, even when Localify has no run for it.
  assert.equal(serviceTone(item({ port: { number: 3000, reachable: true } })), "ok");
  assert.equal(serviceTone(item({})), "off");
  assert.equal(
    serviceTone(
      item({
        run: {
          runId: null,
          legacy: false,
          startedAt: "2026-11-09T09:00:00Z",
          profile: null,
          profileDanger: false,
          exit: { at: "2026-11-09T09:01:00Z", code: 1 },
        },
      }),
    ),
    "error",
  );
});

test("the inventory groups by project, then environment, and filters", () => {
  const items = [
    item(),
    item({ service: { id: "api", name: "api", configRev: "1" } }),
    item({
      environment: { slug: "feature", name: "feature", primary: false, branch: "feature" },
    }),
    item({ project: { id: "other", name: "Other" } }),
  ];
  const groups = groupInventory(items);
  assert.deepEqual(
    groups.map((group) => [group.name, group.environments.map((env) => env.services.length)]),
    [
      ["Louez", [2, 1]],
      ["Other", [1]],
    ],
  );
  const filtered = groupInventory(items, "API");
  assert.equal(filtered.length, 1);
  assert.equal(filtered[0].environments[0].services[0].service.id, "api");
});
