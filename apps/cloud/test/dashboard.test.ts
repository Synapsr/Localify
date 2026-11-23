// `pnpm cloud:dashboard`: its SELECT-only guard, its reads against the real
// migrations with fixture rows, the aggregation, the cache, and what its
// routes serve. Every row here is made up.

import type { AddressInfo } from "node:net";
import type { DatabaseSync, SQLInputValue } from "node:sqlite";

import { createServer, request as httpRequest, type Server } from "node:http";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NotReadOnlyError, assertSelect, readOnly, type ReadDb } from "../scripts/dashboard/db";
import {
  QUERIES,
  buildDashboard,
  readRaw,
  weekStart,
  type Dashboard,
} from "../scripts/dashboard/model";
import { esc, signupChart } from "../scripts/dashboard/page";
import {
  CACHE_MS,
  MIN_RELOAD_MS,
  allowedHost,
  cachedLoader,
  createHandler,
} from "../scripts/dashboard/server";
import { createTestDb } from "./support";

const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.UTC(2026, 10, 23, 12, 0, 0);

describe("assertSelect", () => {
  it("lets one SELECT through", () => {
    expect(assertSelect("  SELECT id FROM users  ")).toBe("SELECT id FROM users");
    expect(assertSelect("select deleted_at, updated FROM public_urls")).toContain("deleted_at");
    expect(assertSelect("SELECT 'DELETE FROM users' AS words")).toContain("DELETE");
    expect(assertSelect(`SELECT "update" FROM t -- a DROP in a comment`)).toContain("SELECT");
  });

  it.each([
    ["INSERT INTO users (id) VALUES ('x')"],
    ["UPDATE users SET login = 'x'"],
    ["DELETE FROM users"],
    ["DROP TABLE users"],
    ["ALTER TABLE users ADD COLUMN x"],
    ["CREATE TABLE x (y)"],
    ["REPLACE INTO users (id) VALUES ('x')"],
    ["PRAGMA table_info(users)"],
    ["ATTACH DATABASE 'x' AS y"],
    ["VACUUM"],
    ["WITH x AS (SELECT 1) DELETE FROM users"],
    ["WITH x AS (SELECT 1) SELECT * FROM x"],
    ["SELECT 1; DELETE FROM users"],
    ["SELECT 1;"],
    [
      "SELECT * FROM users WHERE id IN (SELECT user_id FROM (DELETE FROM tokens RETURNING user_id))",
    ],
    ["SELECT 'unterminated"],
    ["SELECT 1 /* DELETE FROM users"],
    ['SELECT "x FROM users'],
    ["  insert into users (id) values ('x')"],
    [""],
  ])("refuses %j", (sql) => {
    expect(() => assertSelect(sql)).toThrow(NotReadOnlyError);
  });

  it("refuses the whole call before anything is sent", async () => {
    const run = vi.fn<ReadDb>(async () => [[]]);
    const db = readOnly(run);
    await expect(db(["SELECT 1", "UPDATE users SET banned_at = 1"])).rejects.toThrow(
      NotReadOnlyError,
    );
    expect(run).not.toHaveBeenCalled();
    await expect(db(["SELECT 1 AS one"])).resolves.toEqual([[]]);
    expect(run).toHaveBeenCalledWith(["SELECT 1 AS one"]);
  });

  it("lets every query of the dashboard through", () => {
    for (const sql of Object.values(QUERIES)) expect(() => assertSelect(sql)).not.toThrow();
  });
});

// --- Fixtures ---

type Values = Record<string, SQLInputValue>;

function insert(sqlite: DatabaseSync, table: string, values: Values): void {
  const keys = Object.keys(values);
  sqlite
    .prepare(`INSERT INTO ${table} (${keys.join(", ")}) VALUES (${keys.map(() => "?").join(", ")})`)
    .run(...keys.map((key) => values[key]!));
}

const SECRETS = [
  "PASSWORD-HASH-1",
  "PASSWORD-SALT-1",
  "TOKEN-HASH-1",
  "TUNNEL-ID-1",
  "DNS-ID-1",
  "REMOTE-KEY-1",
  "48123",
];

function user(
  sqlite: DatabaseSync,
  id: string,
  login: string,
  ageDays: number,
  extra: Values = {},
) {
  insert(sqlite, "users", {
    id,
    github_id: 1000 + Number(id.slice(1)),
    login,
    created_at: NOW - ageDays * DAY,
    ...extra,
  });
}

function machine(sqlite: DatabaseSync, id: string, userId: string, extra: Values = {}) {
  insert(sqlite, "machines", {
    id,
    user_id: userId,
    name: `${id}-laptop`,
    status: "active",
    ingress_port: 48123,
    created_at: NOW - 30 * DAY,
    ...extra,
  });
}

let urlCount = 0;
function url(sqlite: DatabaseSync, userId: string, extra: Values = {}) {
  urlCount++;
  insert(sqlite, "public_urls", {
    id: `url-${String(urlCount).padStart(3, "0")}`,
    user_id: userId,
    slug: `slug-${urlCount}`,
    local_domain: `app${urlCount}.localify`,
    wildcard: 0,
    protected: 1,
    password_hash: "PASSWORD-HASH-1",
    password_salt: "PASSWORD-SALT-1",
    password_version: 1,
    created_at: NOW - 20 * DAY + urlCount,
    ...extra,
  });
}

function billing(sqlite: DatabaseSync, userId: string, extra: Values) {
  insert(sqlite, "billing", { user_id: userId, revision: 1, refreshed_at: NOW - DAY, ...extra });
}

/**
 * alice: team override, 8 of 10 URLs (one awaiting its reconcile);
 * bob: Pro active, both serving slots used, a takedown; carol: an override
 * ending in 10 days; dave: banned, and past_due; erin: trialing with
 * add-ons; frank: no plan, one suspended URL, a revoked machine.
 */
function seed(sqlite: DatabaseSync): void {
  urlCount = 0;
  user(sqlite, "u1", "alice", 100);
  user(sqlite, "u2", "bob", 3);
  user(sqlite, "u3", "carol", 10);
  user(sqlite, "u4", "dave", 40, { banned_at: NOW - 2 * DAY, banned_reason: "phishing" });
  user(sqlite, "u5", "erin", 20);
  user(sqlite, "u6", "frank", 5);

  insert(sqlite, "plan_overrides", {
    user_id: "u1",
    serving_machines: 5,
    public_urls: 10,
    remote_access: 1,
    reason: "team",
    expires_at: null,
    created_at: NOW - 90 * DAY,
  });
  insert(sqlite, "plan_overrides", {
    user_id: "u3",
    serving_machines: 1,
    public_urls: 5,
    remote_access: 0,
    reason: "beta",
    expires_at: NOW + 10 * DAY,
    created_at: NOW - 9 * DAY,
  });
  billing(sqlite, "u2", {
    pro_status: "active",
    pro_interval: "month",
    current_period_end: NOW + 20 * DAY,
  });
  billing(sqlite, "u4", { pro_status: "past_due", pro_interval: "year", past_due_at: NOW - DAY });
  const grant = (id: string, kind: string, units: number) => ({
    id,
    kind,
    units,
    status: "active",
    cancel_at_period_end: false,
    current_period_end: NOW + 20 * DAY,
    trial_end: null,
    ends_at: null,
    ended_at: null,
    past_due_at: null,
  });
  billing(sqlite, "u5", {
    pro_status: "trialing",
    pro_interval: "month",
    trial_end: NOW + 4 * DAY,
    current_period_end: NOW + 4 * DAY,
    addon_grants: JSON.stringify([grant("sub-a", "machine", 2), grant("sub-b", "urls", 1)]),
  });

  machine(sqlite, "m1", "u1", {
    tunnel_state: "ready",
    tunnel_id: "TUNNEL-ID-1",
    dns_record_id: "DNS-ID-1",
    tunnel_ready_at: NOW - 2 * 60 * 60 * 1000,
    remote_key: "REMOTE-KEY-1",
    remote_key_at: NOW - 3 * DAY,
  });
  machine(sqlite, "m2", "u1", { name: `<img src=x onerror=alert(1)>` });
  machine(sqlite, "m3", "u2", { tunnel_state: "ready" });
  machine(sqlite, "m4", "u2", { tunnel_state: "provisioning" });
  machine(sqlite, "m5", "u6", { status: "revoked", revoked_at: NOW - DAY });
  insert(sqlite, "tokens", {
    hash: "TOKEN-HASH-1",
    user_id: "u1",
    machine_id: "m1",
    created_at: NOW - DAY,
  });

  for (let i = 0; i < 8; i++)
    url(sqlite, "u1", { machine_id: "m1", suspended_at: i === 7 ? NOW - DAY : null });
  url(sqlite, "u1", { deleted_at: NOW - DAY });
  url(sqlite, "u2", { machine_id: "m3", wildcard: 1 });
  url(sqlite, "u2", { taken_down_at: NOW - DAY, taken_down_reason: "malware" });
  url(sqlite, "u6", { machine_id: "m5" });

  insert(sqlite, "webhook_events", {
    id: "evt-1",
    type: "subscription.active",
    user_id: "u2",
    received_at: NOW - 60 * 60 * 1000,
  });
}

function sqliteDb(sqlite: DatabaseSync): ReadDb {
  return readOnly(async (statements) =>
    statements.map((sql) =>
      sqlite
        .prepare(sql)
        .all()
        .map((row) => ({ ...row })),
    ),
  );
}

let sqlite: DatabaseSync;
let db: ReadDb;

beforeEach(() => {
  sqlite = createTestDb().sqlite;
  seed(sqlite);
  db = sqliteDb(sqlite);
});

async function dashboard(): Promise<Dashboard> {
  return buildDashboard(await readRaw(db), NOW);
}

describe("buildDashboard", () => {
  it("counts the headline numbers", async () => {
    const d = await dashboard();
    expect(d.totals).toEqual({
      users: 6,
      newLast7Days: 2,
      newLast30Days: 4,
      activeMachines: 4,
      servingMachines: 3,
      liveUrls: 10,
      servingUrls: 8,
      banned: 1,
      takedowns: 1,
    });
    expect(d.statuses).toEqual({
      override: 2,
      trialing: 1,
      active: 1,
      past_due: 1,
      canceling: 0,
      none: 1,
    });
    expect(d.addonUnits).toEqual({ machines: 2, urlPacks: 1 });
  });

  it("counts sign-ups per week up to now's week", async () => {
    const d = await dashboard();
    expect(d.signupsByWeek.reduce((sum, week) => sum + week.count, 0)).toBe(6);
    expect(d.signupsByWeek[0]!.weekStart).toBe(weekStart(NOW - 100 * DAY));
    expect(d.signupsByWeek.at(-1)!.weekStart).toBe(weekStart(NOW));
    expect(new Date(weekStart(NOW)).getUTCDay()).toBe(1);
  });

  it("lists what needs attention, most urgent first", async () => {
    const d = await dashboard();
    expect(d.attention.map(({ login, kind, detail }) => [login, kind, detail])).toEqual([
      ["dave", "banned", "phishing"],
      ["dave", "past_due", "Pro past_due"],
      ["carol", "override_expiring", `override "beta" ends in 10 days`],
      ["alice", "urls", "8 of 10 URLs"],
      ["bob", "machines", "2 of 2 serving machines"],
    ]);
  });

  it("gives each account its plan, limits and URLs, through the Worker's rules", async () => {
    const d = await dashboard();
    const byLogin = Object.fromEntries(d.users.map((u) => [u.login, u]));
    expect(d.users.map((u) => u.login)).toEqual(["bob", "frank", "carol", "erin", "dave", "alice"]);

    const alice = byLogin.alice!;
    expect(alice.status).toBe("override");
    expect(alice.override).toMatchObject({ reason: "team", expiresAt: null, expired: false });
    expect(alice.entitlement).toEqual({ servingMachines: 5, publicUrls: 10, remoteAccess: true });
    expect([alice.servingSlots, alice.signedIn, alice.signedInCap]).toEqual([1, 2, 13]);
    expect(alice.urls.map((u) => u.state)).toEqual([...Array(7).fill("serving"), "pending"]);
    expect(alice.machines[0]).toMatchObject({ name: "m1-laptop", serving: true, linked: true });
    expect(alice.lastActivityAt).toBe(NOW - 2 * 60 * 60 * 1000);

    const bob = byLogin.bob!;
    expect(bob.status).toBe("active");
    expect(bob.entitlement.servingMachines).toBe(2);
    expect(bob.urls.map((u) => [u.state, u.machineName])).toEqual([
      ["serving", "m3-laptop"],
      ["taken_down", null],
    ]);
    expect(bob.lastActivityAt).toBe(NOW - 60 * 60 * 1000);

    const dave = byLogin.dave!;
    expect(dave.status).toBe("past_due");
    expect(dave.entitlement).toEqual({ servingMachines: 0, publicUrls: 0, remoteAccess: false });

    expect(byLogin.erin!.entitlement).toEqual({
      servingMachines: 4,
      publicUrls: 20,
      remoteAccess: true,
    });
    expect(byLogin.frank!.urls.map((u) => [u.state, u.machineName])).toEqual([
      ["suspended", "m5-laptop"],
    ]);
    expect(byLogin.frank!.signedIn).toBe(0);
  });

  it("shows an expired override as such, with the plan behind it", () => {
    const later = NOW + 11 * DAY;
    return readRaw(db).then((raw) => {
      const carol = buildDashboard(raw, later).users.find((u) => u.login === "carol")!;
      expect(carol.status).toBe("none");
      expect(carol.override).toMatchObject({ reason: "beta", expired: true });
    });
  });

  it("handles an empty database", () => {
    const empty = buildDashboard(
      { accounts: [], users: [], overrides: [], billing: [], machines: [], urls: [], activity: [] },
      NOW,
    );
    expect(empty.totals.users).toBe(0);
    expect(empty.signupsByWeek).toEqual([]);
    expect(signupChart(empty.signupsByWeek)).toContain("No sign-ups yet");
  });
});

describe("cachedLoader", () => {
  it("reads once per 30 seconds, and again on a refresh after 5", async () => {
    let now = NOW;
    const run = vi.fn(db);
    const load = cachedLoader(run, () => now);
    await Promise.all([load(false), load(false)]);
    expect(run).toHaveBeenCalledTimes(1);
    now += MIN_RELOAD_MS - 1;
    await load(true);
    expect(run).toHaveBeenCalledTimes(1);
    now += 1;
    await load(true);
    expect(run).toHaveBeenCalledTimes(2);
    now += CACHE_MS - 1;
    await load(false);
    expect(run).toHaveBeenCalledTimes(2);
    now += 1;
    await load(false);
    expect(run).toHaveBeenCalledTimes(3);
  });

  it("keeps no failed read", async () => {
    const run = vi.fn<ReadDb>().mockRejectedValueOnce(new Error("D1 down")).mockImplementation(db);
    const load = cachedLoader(run, () => NOW);
    await expect(load(false)).rejects.toThrow("D1 down");
    await expect(load(false)).resolves.toMatchObject({ totals: { users: 6 } });
  });
});

describe("allowedHost", () => {
  it.each([
    ["dashboard.cloud.localify", true],
    ["dashboard.cloud.localify:443", true],
    ["127.0.0.1:41234", true],
    ["localhost", true],
    ["evil.example", false],
    ["localify.evil.example", false],
    ["dashboard.localify.fyi", false],
    [undefined, false],
  ])("%s: %s", (host, allowed) => {
    expect(allowedHost(host)).toBe(allowed);
  });
});

describe("routes", () => {
  let server: Server;
  let port: number;
  const log = vi.spyOn(console, "error");

  beforeEach(async () => {
    log.mockImplementation(() => {});
    const handler = createHandler(
      cachedLoader(db, () => NOW),
      { source: "Local D1", clock: () => NOW },
    );
    server = createServer((req, res) => void handler(req, res));
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    port = (server.address() as AddressInfo).port;
  });

  afterEach(async () => {
    await new Promise((resolve) => server.close(resolve));
    log.mockReset();
  });

  function get(
    path: string,
    { method = "GET", host = "dashboard.cloud.localify" } = {},
  ): Promise<{ status: number; headers: Record<string, unknown>; body: string }> {
    return new Promise((resolve, reject) => {
      const req = httpRequest(
        { host: "127.0.0.1", port, path, method, headers: { host } },
        (res) => {
          let body = "";
          res.setEncoding("utf8");
          res.on("data", (chunk) => (body += chunk));
          res.on("end", () => resolve({ status: res.statusCode!, headers: res.headers, body }));
        },
      );
      req.on("error", reject);
      req.end();
    });
  }

  it("serves the overview, with no secret in it", async () => {
    const response = await get("/");
    expect(response.status).toBe(200);
    expect(response.headers["cache-control"]).toBe("no-store");
    expect(response.headers["content-security-policy"]).toContain("default-src 'none'");
    for (const login of ["alice", "bob", "carol", "dave", "erin", "frank"]) {
      expect(response.body).toContain(login);
    }
    expect(response.body).toContain("Needs attention");
    expect(response.body).toContain("<svg");
    for (const secret of SECRETS) expect(response.body).not.toContain(secret);
  });

  it("serves an account's detail, escaped, with no secret in it", async () => {
    const response = await get("/users/u1");
    expect(response.status).toBe(200);
    expect(response.body).toContain("slug-1");
    expect(response.body).toContain("m1-laptop");
    expect(response.body).toContain("Suspended until the next reconcile");
    expect(response.body).not.toContain("<img src=x");
    expect(response.body).toContain(esc("<img src=x onerror=alert(1)>"));
    for (const secret of SECRETS) expect(response.body).not.toContain(secret);
  });

  it("answers an unknown account or page with 404", async () => {
    expect((await get("/users/nobody")).status).toBe(404);
    expect((await get("/users/%E0%A4%A")).status).toBe(404);
    expect((await get("/admin")).status).toBe(404);
  });

  it("refuses anything but GET and HEAD", async () => {
    for (const method of ["POST", "PUT", "DELETE", "PATCH"]) {
      const response = await get("/", { method });
      expect(response.status).toBe(405);
      expect(response.headers.allow).toBe("GET, HEAD");
    }
    expect((await get("/", { method: "HEAD" })).status).toBe(200);
  });

  it("refuses a host that is not Localify's or loopback", async () => {
    const response = await get("/", { host: "evil.example" });
    expect(response.status).toBe(421);
    expect(response.body).not.toContain("alice");
  });

  it("serves a failed read as 503 without its error", async () => {
    const handler = createHandler(
      async () => {
        throw new Error("wrangler failed: TOKEN-HASH-1");
      },
      { source: "Production D1", clock: () => NOW },
    );
    const failing = createServer((req, res) => void handler(req, res));
    await new Promise<void>((resolve) => failing.listen(0, "127.0.0.1", resolve));
    const previous = port;
    port = (failing.address() as AddressInfo).port;
    const response = await get("/");
    port = previous;
    await new Promise((resolve) => failing.close(resolve));
    expect(response.status).toBe(503);
    expect(response.body).not.toContain("TOKEN-HASH-1");
    expect(log).toHaveBeenCalled();
  });
});

describe("wranglerOutputAsync", () => {
  const original = process.env.PATH;
  let dir: string;

  beforeEach(async () => {
    const { mkdtempSync, writeFileSync } = await import("node:fs");
    const { tmpdir } = await import("node:os");
    const { join } = await import("node:path");
    dir = mkdtempSync(join(tmpdir(), "fake-wrangler-"));
    // A wrangler that prints its first argument, or stalls when it is `stall`.
    writeFileSync(
      join(dir, "wrangler"),
      '#!/bin/sh\nif [ "$1" = stall ]; then exec sleep 30; fi\necho "$1"\n',
      { mode: 0o755 },
    );
    process.env.PATH = `${dir}:${original}`;
  });

  afterEach(async () => {
    process.env.PATH = original;
    const { rmSync } = await import("node:fs");
    rmSync(dir, { recursive: true, force: true });
  });

  it("returns what wrangler printed", async () => {
    const { wranglerOutputAsync } = await import("../scripts/infisical.mjs");
    expect(await wranglerOutputAsync({}, ["hello"])).toBe("hello\n");
  });

  it("kills a stalled wrangler past its deadline, without blocking the event loop", async () => {
    const { wranglerOutputAsync } = await import("../scripts/infisical.mjs");
    let ticks = 0;
    const timer = setInterval(() => ticks++, 20);
    const started = Date.now();
    await expect(wranglerOutputAsync({}, ["stall"], { timeoutMs: 300 })).rejects.toThrow(
      /stopped by SIGKILL/,
    );
    clearInterval(timer);
    expect(Date.now() - started).toBeLessThan(5_000);
    expect(ticks).toBeGreaterThan(3);
  });

  it("kills wrangler when the signal aborts", async () => {
    const { wranglerOutputAsync } = await import("../scripts/infisical.mjs");
    const controller = new AbortController();
    const read = wranglerOutputAsync({}, ["stall"], { signal: controller.signal });
    setTimeout(() => controller.abort(), 100);
    await expect(read).rejects.toThrow();
  });
});
