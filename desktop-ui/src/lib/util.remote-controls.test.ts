/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import type { RemoteInventoryItemDto } from "../types/localify.ts";

import {
  accessCommand,
  accessRequestTitle,
  accessEventView,
  controlLostText,
  controlProgressText,
  controlRequest,
  controlResultText,
  formatWait,
  quickAction,
  relaunchConfirmation,
  relaunchStillConfirmed,
  sessionDownReason,
  shortFingerprint,
} from "./util.remote-controls.ts";
import { remoteStreamRequestSchema } from "./validators/validator.remote.ts";

type Control = RemoteInventoryItemDto["controls"]["start"];
const allowed: Control = { allowed: true };
const refused = (code: string): Control => ({ allowed: false, code });

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
      start: refused("control_disabled"),
      stop: refused("control_disabled"),
      restart: refused("control_disabled"),
    },
    ...overrides,
  };
}

function legacy(controls: RemoteInventoryItemDto["controls"]): RemoteInventoryItemDto {
  return item({
    owner: "app",
    process: "running",
    run: {
      runId: null,
      legacy: true,
      startedAt: "2026-11-09T09:00:00Z",
      profile: null,
      profileDanger: false,
      legacyId: "boot.123.456",
      exit: null,
    },
    output: { available: false, reason: "legacy_run" },
    controls,
  });
}

test("an allowed control runs; a disabled one asks on protocol 2", () => {
  const ready = item({ controls: { start: allowed, stop: allowed, restart: allowed } });
  assert.deepEqual(quickAction(ready, "start", 2, "B", true), { kind: "run" });
  assert.deepEqual(quickAction(item(), "start", 2, "B", true), { kind: "ask", wants: "control" });
  const background = item({
    controls: {
      start: refused("background_disabled"),
      stop: allowed,
      restart: refused("background_disabled"),
    },
  });
  assert.deepEqual(quickAction(background, "start", 2, "B", true), {
    kind: "ask",
    wants: "backgroundStarts",
  });
});

test("with controls off, an action that runs in the background asks for background starts", () => {
  // A start while B's app is absent runs in the background: one request for both switches.
  assert.deepEqual(quickAction(item(), "start", 2, "B", false), {
    kind: "ask",
    wants: "backgroundStarts",
  });
  assert.deepEqual(quickAction(item(), "start", 1, "B", false), {
    kind: "command",
    wants: "backgroundStarts",
  });
  // A restart of a background run, whether or not the app runs.
  const background = item({ owner: "background", process: "running" });
  assert.deepEqual(quickAction(background, "restart", 2, "B", true), {
    kind: "ask",
    wants: "backgroundStarts",
  });
  // A stop starts nothing.
  assert.deepEqual(quickAction(background, "stop", 2, "B", false), {
    kind: "ask",
    wants: "control",
  });
  // A restart of an app run goes through the app (and is `owner_gone` without it).
  const inApp = item({ owner: "app", process: "running" });
  assert.deepEqual(quickAction(inApp, "restart", 2, "B", false), {
    kind: "ask",
    wants: "control",
  });
  // The relaunch of a legacy `dev --detach` run is a background restart.
  const detached: RemoteInventoryItemDto = {
    ...legacy({
      start: refused("control_disabled"),
      stop: refused("control_disabled"),
      restart: refused("control_disabled"),
    }),
    owner: "background",
  };
  assert.deepEqual(quickAction(detached, "restart", 2, "B", true), {
    kind: "relaunch",
    access: "backgroundStarts",
  });
});

test("a target of protocol 1 only gets the command to run there", () => {
  assert.deepEqual(quickAction(item(), "start", 1, "B", true), {
    kind: "command",
    wants: "control",
  });
  assert.equal(accessCommand("control"), "localify machines settings --control on");
  assert.equal(
    accessCommand("backgroundStarts"),
    "localify machines settings --control on --background-starts on",
  );
});

test("other refusals say why, with nothing to do", () => {
  const terminal = item({
    owner: "terminal",
    process: "running",
    controls: {
      start: refused("conflict"),
      stop: refused("not_owned"),
      restart: refused("not_owned"),
    },
  });
  assert.deepEqual(quickAction(terminal, "stop", 2, "B", true), {
    kind: "message",
    text: "This runs in a terminal on B. Stop it there.",
  });
  const gone = item({
    controls: { start: allowed, stop: allowed, restart: refused("owner_gone") },
  });
  assert.deepEqual(quickAction(gone, "restart", 2, "B", true), {
    kind: "message",
    text: "Stop it, then start it again.",
  });
});

test("a legacy run without a legacyId can only be relaunched on its machine", () => {
  const unproven = item({
    process: "running",
    controls: {
      start: refused("legacy_run"),
      stop: refused("legacy_run"),
      restart: refused("legacy_run"),
    },
  });
  assert.deepEqual(quickAction(unproven, "stop", 2, "B", true), {
    kind: "message",
    text: "Relaunch it on B.",
  });
});

test("a legacy run with a legacyId is relaunched, asking for access first when needed", () => {
  const open = legacy({
    start: refused("conflict"),
    stop: refused("legacy_run"),
    restart: allowed,
  });
  assert.deepEqual(quickAction(open, "stop", 2, "B", true), { kind: "relaunch", access: null });
  // Restart of such a run is a relaunch even when restart is allowed.
  assert.deepEqual(quickAction(open, "restart", 2, "B", true), { kind: "relaunch", access: null });
  const closed = legacy({
    start: refused("control_disabled"),
    stop: refused("control_disabled"),
    restart: refused("control_disabled"),
  });
  assert.deepEqual(quickAction(closed, "restart", 2, "B", true), {
    kind: "relaunch",
    access: "control",
  });
  const background = legacy({
    start: refused("conflict"),
    stop: refused("legacy_run"),
    restart: refused("background_disabled"),
  });
  assert.deepEqual(quickAction(background, "stop", 2, "B", true), {
    kind: "relaunch",
    access: "backgroundStarts",
  });
  const appGone = legacy({
    start: refused("conflict"),
    stop: refused("legacy_run"),
    restart: refused("owner_gone"),
  });
  assert.deepEqual(quickAction(appGone, "restart", 2, "B", true), {
    kind: "message",
    text: "Stop it, then start it again.",
  });
  // Protocol 1 cannot name a legacy run.
  assert.deepEqual(quickAction(open, "restart", 1, "B", true), {
    kind: "message",
    text: "Update Localify on B.",
  });
});

test("a control expects what the snapshot showed", () => {
  const running = item({
    environment: { slug: "feature", name: "feature", primary: false, branch: "feature" },
    process: "running",
    owner: "app",
    run: {
      runId: "RUN1",
      legacy: false,
      startedAt: "2026-11-09T09:00:00Z",
      profile: "staging",
      profileDanger: false,
      exit: null,
    },
  });
  assert.deepEqual(controlRequest(running, "restart"), {
    action: "restart",
    service: { projectId: "louez", environment: "feature", serviceId: "web" },
    profile: null,
    expect: { runId: "RUN1", configRev: "0123456789abcdef" },
  });
  assert.deepEqual(controlRequest(item(), "start").expect, {
    runId: null,
    configRev: "0123456789abcdef",
  });
});

test("a relaunch names the legacy run the person confirmed", () => {
  const run = legacy({ start: allowed, stop: allowed, restart: allowed });
  const confirmed = relaunchConfirmation(run);
  assert.deepEqual(confirmed, { legacyId: "boot.123.456", configRev: "0123456789abcdef" });
  assert.deepEqual(controlRequest(run, "restart", confirmed).expect, {
    runId: null,
    configRev: "0123456789abcdef",
    legacyId: "boot.123.456",
  });
  assert.equal(relaunchConfirmation(item()), null);
});

test("a control passes the stream's validator as it is built", () => {
  const run = legacy({ start: allowed, stop: allowed, restart: allowed });
  for (const request of [
    controlRequest(run, "restart", relaunchConfirmation(run)),
    controlRequest(item(), "start"),
  ]) {
    const stream = { kind: "control" as const, machineId: "mB", ...request };
    assert.deepEqual(remoteStreamRequestSchema.parse(stream), stream);
  }
});

test("after access is granted, a relaunch goes ahead only on what was confirmed", () => {
  const run = legacy({ start: allowed, stop: allowed, restart: allowed });
  const confirmed = relaunchConfirmation(run);
  assert.ok(confirmed);
  assert.equal(relaunchStillConfirmed(confirmed, run), true);
  const edited = { ...run, service: { ...run.service, configRev: "fedcba9876543210" } };
  assert.equal(relaunchStillConfirmed(confirmed, edited), false);
  const replaced = legacy({ start: allowed, stop: allowed, restart: allowed });
  replaced.run = { ...replaced.run!, legacyId: "boot.999.1" };
  assert.equal(relaunchStillConfirmed(confirmed, replaced), false);
  assert.equal(relaunchStillConfirmed(confirmed, item()), false);
});

test("controls are off while the session is down, saying why", () => {
  const ready = { machine: "B", machineLabel: null, phase: "ready" as const, proto: 2 };
  assert.equal(sessionDownReason(ready), null);
  assert.equal(sessionDownReason({ ...ready, machineLabel: "Offline" }), "B: Offline");
  assert.equal(sessionDownReason({ ...ready, phase: "lost" }), "The connection to B is down.");
  assert.equal(sessionDownReason({ ...ready, phase: "connecting" }), "Connecting to B…");
  assert.equal(sessionDownReason({ ...ready, proto: null }), "Connecting to B…");
});

test("a control's progress and result read as the contract says", () => {
  assert.equal(controlProgressText("start", "dispatched", "B", true), "Starting in the app on B…");
  assert.equal(
    controlProgressText("restart", "dispatched", "B", false),
    "Starting in the background on B…",
  );
  assert.equal(controlProgressText("stop", "sent", "B", true), "Stopping on B…");
  assert.equal(controlProgressText("start", "unknown", "B", true), "Outcome unknown. Checking B…");
  assert.deepEqual(
    controlResultText({ outcome: "done", code: null, message: "" }, "start", "web", "B"),
    { ok: true, text: "web is running on B." },
  );
  assert.deepEqual(
    controlResultText({ outcome: "starting", code: null, message: "" }, "start", "web", "B"),
    { ok: true, text: "Started on B, not listening yet." },
  );
  assert.deepEqual(
    controlResultText(
      { outcome: "conflict", code: "conflict", message: "It is running already." },
      "start",
      "web",
      "B",
    ),
    { ok: false, text: "B changed in the meantime." },
  );
  assert.equal(
    controlResultText(
      { outcome: "failed", code: "failed", message: "exit 1: boom" },
      "start",
      "web",
      "B",
    ).text,
    "exit 1: boom",
  );
});

test("a control refused before it was sent, or lost, says why", () => {
  const refusedWith = (code: string, message = "") =>
    controlResultText({ outcome: "refused", code, message }, "start", "web", "B").text;
  // Without an open session: the connection state.
  assert.equal(refusedWith("not_connected"), "Not connected to B.");
  assert.equal(refusedWith("connecting"), "Still connecting to B. Try again once it is connected.");
  assert.equal(refusedWith("not_responding"), "B isn't responding right now.");
  assert.equal(refusedWith("peer_offline"), "B is offline.");
  assert.equal(refusedWith("not_paired"), "This Mac is not paired with B. Pair them again.");
  assert.equal(
    refusedWith("unavailable", "The relay dropped a frame; try again in 3 s."),
    "The relay dropped a frame; try again in 3 s.",
  );
  assert.equal(
    refusedWith("busy"),
    "Something else is starting or stopping it on B. Try again in a moment.",
  );
  assert.equal(
    refusedWith("too_many"),
    "This Mac sent B too many controls lately. Try again later.",
  );
  // `unknown` is never the last event; were it, the toast would not stay on "Checking B…".
  assert.equal(
    controlResultText({ outcome: "unknown", code: null, message: "" }, "stop", "web", "B").text,
    "B didn't say what happened. The list shows its current state.",
  );
  assert.equal(
    controlLostText("not_shared", "B"),
    "B stopped sharing before it said what happened.",
  );
  assert.equal(
    controlLostText("timeout", "B"),
    "B didn't say what happened. The list shows its current state.",
  );
  assert.equal(
    controlLostText("key_mismatch", "B"),
    "B didn't say what happened. Don't trust this machine until you pair it again.",
  );
});

test("an access request's events read as the card shows them", () => {
  assert.deepEqual(accessEventView({ event: "waiting" }, "control", "B"), {
    step: "waiting",
    text: "Waiting for an answer on B…",
  });
  assert.equal(
    accessEventView({ event: "result", outcome: "granted" }, "control", "B").text,
    "B allowed remote controls.",
  );
  assert.equal(
    accessEventView({ event: "result", outcome: "declined" }, "control", "B").text,
    "B didn't allow it.",
  );
  assert.equal(
    accessEventView({ event: "result", outcome: "nobody" }, "control", "B").text,
    "Nobody can answer on B: Localify isn't open there.",
  );
  assert.equal(
    accessEventView(
      { event: "error", code: "too_many", message: "", retryAfter: 540 },
      "control",
      "B",
    ).text,
    "You asked B recently. Try again in 9 min.",
  );
  assert.equal(
    accessEventView({ event: "error", code: "busy", message: "" }, "control", "B").text,
    "B is answering another request.",
  );
  assert.equal(
    accessEventView({ event: "error", code: "unsupported", message: "" }, "control", "B").text,
    "Update Localify on B.",
  );
  assert.equal(
    accessEventView({ event: "lost", code: "peer_offline", message: "" }, "control", "B").text,
    "The connection to B ended before an answer.",
  );
  assert.equal(formatWait(45), "45 s");
  assert.equal(formatWait(3541), "60 min");
});

test("the access dialog names a peer by its fingerprint's first 8 characters", () => {
  assert.equal(shortFingerprint("3f2a 91c0 5e6f 7a8b"), "3F2A 91C0");
  assert.equal(
    accessRequestTitle("teo-laptop", "3f2a 91c0 5e6f 7a8b"),
    "teo-laptop (3F2A 91C0) asks for remote controls.",
  );
});
