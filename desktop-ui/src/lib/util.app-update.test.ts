/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import type { TerminalSessionDto, UpdateStatusDto } from "../types/localify.ts";

import { describeUpdate, lastCheckLine, restartCloses } from "./util.app-update.ts";

function tab(
  id: string,
  options: {
    service?: string;
    environment?: string;
    status?: TerminalSessionDto["status"];
    ended?: boolean;
  } = {},
): TerminalSessionDto {
  const environment = options.environment ?? "primary";
  return {
    id,
    title: options.service ?? "zsh",
    workingDirectory: "/repo",
    target: {
      kind: options.service ? "service" : "project",
      projectId: "shop",
      projectName: "Shop",
      environmentSlug: environment,
      environmentName: environment === "primary" ? "main" : environment,
      serviceId: options.service ?? null,
      serviceName: options.service ?? null,
    },
    status: options.status ?? "running",
    run: options.service
      ? {
          runId: `run-${id}`,
          profile: null,
          ended: options.ended
            ? { code: 0, registered: true, stopped: true, at: 1, error: null }
            : null,
        }
      : null,
  };
}

test("a restart lists the running services and open shells by name", () => {
  const closes = restartCloses([
    tab("1", { service: "web" }),
    tab("2", { service: "api", environment: "feat-cart" }),
    tab("3"),
    tab("4", { environment: "feat-cart" }),
  ]);

  assert.deepEqual(closes, {
    services: ["web in Shop", "api in Shop (feat-cart)"],
    shells: ["Shell in Shop", "Shell in Shop (feat-cart)"],
  });
});

test("tabs that already ended are not listed", () => {
  const closes = restartCloses([
    tab("1", { service: "web", ended: true }),
    tab("2", { service: "api", status: "exited" }),
    tab("3", { status: "exited" }),
  ]);

  assert.deepEqual(closes, { services: [], shells: [] });
});

function status(overrides: Partial<UpdateStatusDto>): UpdateStatusDto {
  return {
    installed: "1.6.0",
    available: null,
    state: "upToDate",
    reason: null,
    automatic: true,
    lastCheckAt: 1_000,
    canInstall: true,
    ...overrides,
  };
}

test("an available update offers a restart only where it can install", () => {
  const ready = describeUpdate(
    status({ state: "available", available: "1.7.0" }),
    "Localify",
    1_000,
  );
  assert.deepEqual(ready, {
    title: "Localify 1.7.0 is available",
    detail: "Restart to update when you're ready.",
    canRestart: true,
  });

  const devBuild = describeUpdate(
    status({
      state: "available",
      available: "1.7.0",
      canInstall: false,
      reason: "This build of Localify is a development build: update it by hand.",
    }),
    "Localify",
    1_000,
  );
  assert.equal(devBuild.canRestart, false);
  assert.equal(
    devBuild.detail,
    "This build of Localify is a development build: update it by hand.",
  );
});

test("an install that waits says why and still offers the restart", () => {
  const summary = describeUpdate(
    status({
      state: "available",
      available: "1.7.0",
      automatic: false,
      reason: "Automatic updates are off.",
    }),
    "Localify",
    1_000,
  );

  assert.equal(summary.detail, "Automatic updates are off.");
  assert.equal(summary.canRestart, true);
});

test("the other states read as sentences", () => {
  assert.deepEqual(describeUpdate(status({}), "Localify", 1_000 + 7_200), {
    title: "Localify 1.6.0 is up to date",
    detail: "Last checked 2 hours ago.",
    canRestart: false,
  });
  assert.equal(
    describeUpdate(status({ state: "failed", reason: "offline" }), "Localify", 1_000).detail,
    "offline",
  );
  assert.equal(
    describeUpdate(status({ state: "installing", available: "1.7.0" }), "Localify", 1_000).title,
    "Installing Localify 1.7.0…",
  );
  assert.deepEqual(
    describeUpdate(status({ state: "unknown", lastCheckAt: null }), "Localify", 1_000),
    { title: "Localify 1.6.0", detail: "Not checked yet.", canRestart: false },
  );
});

test("a previous version the last install couldn't move says where it is", () => {
  const stranded = "The version the last update replaced could not be moved to previous/.";
  assert.equal(describeUpdate(status({ reason: stranded }), "Localify", 1_000).detail, stranded);
  assert.equal(
    describeUpdate(status({ state: "unknown", reason: stranded }), "Localify", 1_000).detail,
    stranded,
  );
});

test("the last check reads in the largest whole unit", () => {
  assert.equal(lastCheckLine(null, 100), "Not checked yet.");
  assert.equal(lastCheckLine(100, 130), "Last checked just now.");
  assert.equal(lastCheckLine(100, 160), "Last checked 1 minute ago.");
  assert.equal(lastCheckLine(0, 59 * 60), "Last checked 59 minutes ago.");
  assert.equal(lastCheckLine(0, 3 * 86_400), "Last checked 3 days ago.");
  // A clock that went back reads as now.
  assert.equal(lastCheckLine(500, 100), "Last checked just now.");
});
