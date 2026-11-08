/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import type { TerminalSessionDto } from "../types/localify.ts";

import { laterSession } from "./util.terminal-sessions.ts";

function serviceTab(status: TerminalSessionDto["status"]): TerminalSessionDto {
  return {
    id: "terminal-1",
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
