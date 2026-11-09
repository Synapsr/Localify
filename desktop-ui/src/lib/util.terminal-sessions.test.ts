/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import type { TerminalSessionDto } from "../types/localify.ts";

import { laterSession, mergeSessionList } from "./util.terminal-sessions.ts";

function serviceTab(status: TerminalSessionDto["status"], id = "terminal-1"): TerminalSessionDto {
  return {
    id,
    title: "web",
    workingDirectory: "/repo",
    target: {
      kind: "service",
      projectId: "app",
      projectName: "app",
      environmentSlug: "main",
      environmentName: "main",
      serviceId: "web",
      serviceName: "web",
    },
    status,
    run: {
      runId: "AAAAAAAAAAAAAAAAAAAAAA",
      profile: null,
      ended:
        status === "exited"
          ? { code: 1, registered: true, stopped: false, at: 1, error: null }
          : null,
    },
  };
}

test("a launch's late answer never shows an exited tab running again", () => {
  const exited = serviceTab("exited");

  assert.equal(laterSession(exited, serviceTab("running")), exited);
});

test("a tab moves forward from running to exited", () => {
  const exited = serviceTab("exited");

  assert.equal(laterSession(serviceTab("running"), exited), exited);
  assert.equal(laterSession(undefined, exited), exited);
});

test("a tab the panel has not seen yet is taken as it comes", () => {
  const running = serviceTab("running");

  assert.equal(laterSession(undefined, running), running);
});

test("a tab an event opened while the sessions were listed is kept", () => {
  const opened = serviceTab("running", "terminal-2");
  const listed = [serviceTab("running", "terminal-1")];

  const merged = mergeSessionList([opened], listed, new Set());

  assert.deepEqual(
    merged.map((session) => session.id),
    ["terminal-1", "terminal-2"],
  );
});

test("an older list never shows a tab an event ended running again", () => {
  const ended = serviceTab("exited");

  const merged = mergeSessionList([ended], [serviceTab("running")], new Set());

  assert.deepEqual(merged, [ended]);
});

test("a tab replaced or closed while the sessions were listed stays out", () => {
  const replacement = serviceTab("running", "terminal-3");
  const listed = [serviceTab("running", "terminal-1"), serviceTab("running", "terminal-2")];

  const merged = mergeSessionList([replacement], listed, new Set(["terminal-1", "terminal-2"]));

  assert.deepEqual(merged, [replacement]);
});

test("a later state from the list replaces an event's", () => {
  const exited = serviceTab("exited");

  const merged = mergeSessionList([serviceTab("running")], [exited], new Set());

  assert.deepEqual(merged, [exited]);
});
