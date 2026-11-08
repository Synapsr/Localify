/// <reference types="node" />
// A D1 stand-in over node:sqlite with the real migrations, a test Env, a
// fake `fetch` for GitHub, Cloudflare, Polar and the machines' tunnels, and an
// ExecutionContext whose `waitUntil` work the tests can wait for.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync, type SQLInputValue } from "node:sqlite";
import ts from "typescript";
import { vi } from "vitest";

import { D1_NOW_MS } from "../src/entitlement";
import { QUIET_AFTER_STOP_MS } from "../src/tunnels";

const MIGRATIONS = join(import.meta.dirname, "..", "migrations");
const WRANGLER = join(import.meta.dirname, "..", "wrangler.jsonc");

/** A rate-limit binding as wrangler.jsonc declares it. */
export function rateLimit(name: string): { limit: number; period: number } {
  const { config, error } = ts.parseConfigFileTextToJson(WRANGLER, readFileSync(WRANGLER, "utf8"));
  if (error) throw new Error(`wrangler.jsonc does not parse: ${String(error.messageText)}`);
  const binding = (
    config.ratelimits as { name: string; simple: { limit: number; period: number } }[]
  ).find((entry) => entry.name === name);
  if (!binding) throw new Error(`wrangler.jsonc has no rate limit ${name}`);
  return binding.simple;
}

/**
 * A rate-limit binding that counts per key within one period, with the limit
 * of wrangler.jsonc. Clearing `counts` starts the next period.
 */
export type CountingLimiter = { limit: number; counts: Map<string, number> };

function countingLimiter(limiter: CountingLimiter): RateLimit {
  return {
    limit: async ({ key }: { key: string }) => {
      const count = (limiter.counts.get(key) ?? 0) + 1;
      limiter.counts.set(key, count);
      return { success: count <= limiter.limit };
    },
  } as RateLimit;
}

/**
 * The queries a test D1 has run, and the most it accepts, as D1 refuses
 * query 1,001 of a Worker invocation. A batch counts as one, as in D1.
 */
export type QueryCounter = { count: number; limit: number };

function countQueries(counter: QueryCounter, queries: number): void {
  counter.count += queries;
  if (counter.count > counter.limit)
    throw new Error("Too many API requests by single worker invocation");
}

class TestStatement {
  constructor(
    private readonly db: DatabaseSync,
    readonly sql: string,
    private readonly counter: QueryCounter,
    private readonly params: SQLInputValue[] = [],
  ) {}

  bind(...params: SQLInputValue[]): TestStatement {
    return new TestStatement(this.db, this.sql, this.counter, params);
  }

  async first<T>(): Promise<T | null> {
    countQueries(this.counter, 1);
    const row = this.db.prepare(this.text).get(...this.params);
    return row ? ({ ...row } as T) : null;
  }

  async all<T>(): Promise<{ results: T[]; success: true; meta: Record<string, never> }> {
    countQueries(this.counter, 1);
    return { results: this.rows() as T[], success: true, meta: {} };
  }

  async run(): Promise<{ results: []; success: true; meta: { changes: number } }> {
    countQueries(this.counter, 1);
    return this.runSync();
  }

  /** The SQL with D1's clock read from the test's, so fake timers move both. */
  private get text(): string {
    return this.sql.replaceAll(D1_NOW_MS, String(Date.now()));
  }

  rows(): Record<string, unknown>[] {
    return this.db
      .prepare(this.text)
      .all(...this.params)
      .map((row) => ({ ...row }));
  }

  runSync(): { results: []; success: true; meta: { changes: number } } {
    const result = this.db.prepare(this.text).run(...this.params);
    return { results: [], success: true, meta: { changes: Number(result.changes) } };
  }
}

/** Runs a batch in one transaction, like D1. */
class TestD1 {
  constructor(
    readonly sqlite: DatabaseSync,
    private readonly counter: QueryCounter,
  ) {}

  prepare(sql: string): TestStatement {
    return new TestStatement(this.sqlite, sql, this.counter);
  }

  async batch(statements: TestStatement[]): Promise<unknown[]> {
    countQueries(this.counter, 1);
    this.sqlite.exec("BEGIN");
    try {
      const results = statements.map((statement) =>
        /^\s*SELECT|\bRETURNING\b/i.test(statement.sql)
          ? { results: statement.rows(), success: true, meta: {} }
          : statement.runSync(),
      );
      this.sqlite.exec("COMMIT");
      return results;
    } catch (error) {
      this.sqlite.exec("ROLLBACK");
      throw error;
    }
  }
}

export function createTestDb(): { db: D1Database; sqlite: DatabaseSync; queries: QueryCounter } {
  // Foreign keys on, as D1 enforces them.
  const sqlite = new DatabaseSync(":memory:", { enableForeignKeyConstraints: true });
  applyMigrations(sqlite, () => true);
  const queries: QueryCounter = { count: 0, limit: Infinity };
  return { db: new TestD1(sqlite, queries) as unknown as D1Database, sqlite, queries };
}

/** Applies the migrations whose file name `include` accepts, in order. */
export function applyMigrations(sqlite: DatabaseSync, include: (file: string) => boolean): void {
  for (const file of readdirSync(MIGRATIONS).sort()) {
    if (include(file)) sqlite.exec(readFileSync(join(MIGRATIONS, file), "utf8"));
  }
}

export type TestContext = {
  env: Env;
  ctx: ExecutionContext;
  /** Work handed to `waitUntil` and not yet awaited by `settle`. */
  background: Promise<unknown>[];
  sqlite: DatabaseSync;
  /** The D1 queries the Worker ran; set `limit` to refuse the ones past it. */
  queries: QueryCounter;
  /** Requests the Worker sent with `fetch`, in order. */
  calls: {
    url: string;
    method: string;
    headers: Headers;
    body: string | null;
    rawBody: unknown;
    redirect?: string;
  }[];
  /** Answers for `fetch`, by `METHOD url`; the first matching prefix wins. */
  routes: Map<string, (body: string | null, url: string) => Response | Promise<Response>>;
  limiter: { allowed: boolean };
  deviceLimiter: { allowed: boolean };
  ownerLimiter: CountingLimiter;
};

/** The test Worker's webhook secret, Standard Webhooks style: `whsec_` and base64. */
export const POLAR_WEBHOOK_SECRET = `whsec_${Buffer.from("a-polar-webhook-secret-for-tests").toString("base64")}`;

export function createContext(overrides: Partial<Env> = {}): TestContext {
  const { db, sqlite, queries } = createTestDb();
  const limiter = { allowed: true };
  const deviceLimiter = { allowed: true };
  const ownerLimiter = { limit: rateLimit("OWNER_LIMITER").limit, counts: new Map() };
  const env = {
    DB: db,
    PASSWORD_LIMITER: { limit: async () => ({ success: limiter.allowed }) },
    DEVICE_LIMITER: { limit: async () => ({ success: deviceLimiter.allowed }) },
    OWNER_LIMITER: countingLimiter(ownerLimiter),
    GITHUB_CLIENT_ID: "client-123",
    SIGNUP: "allowlist",
    ALLOWED_GITHUB_IDS: "1, 2",
    PUBLIC_BASE_DOMAIN: "localify.fyi",
    MACHINE_DOMAIN: "localify.dev",
    CLOUDFLARE_ACCOUNT_ID: "account-1",
    MACHINE_ZONE_ID: "zone-1",
    CLOUDFLARE_API_TOKEN: "cf-token",
    COOKIE_KEY: "cookie-key",
    INGRESS_KEY: "ingress-key",
    EDGE_KEY: "edge-key",
    MAX_TUNNELS: 950,
    POLAR_API_BASE: "https://polar.test",
    POLAR_ACCESS_TOKEN: "polar-token",
    POLAR_WEBHOOK_SECRET: POLAR_WEBHOOK_SECRET,
    POLAR_PRODUCT_PRO_MONTHLY: "prod-monthly",
    POLAR_PRODUCT_PRO_YEARLY: "prod-yearly",
    POLAR_PRODUCT_ADDON_MACHINE: "",
    POLAR_PRODUCT_ADDON_URLS: "",
    ...overrides,
  } as unknown as Env;
  const background: Promise<unknown>[] = [];
  const ctx = {
    waitUntil: (promise: Promise<unknown>) => background.push(promise),
    passThroughOnException: () => {},
  } as unknown as ExecutionContext;
  const context: TestContext = {
    env,
    ctx,
    background,
    sqlite,
    queries,
    calls: [],
    routes: new Map(),
    limiter,
    deviceLimiter,
    ownerLimiter,
  };
  vi.stubGlobal("fetch", async (input: string | URL | Request, init: RequestInit = {}) => {
    const url = String(input instanceof Request ? input.url : input);
    const method = init.method ?? "GET";
    const body = typeof init.body === "string" ? init.body : null;
    context.calls.push({
      url,
      method,
      headers: new Headers(init.headers),
      body,
      rawBody: init.body,
      redirect: init.redirect,
    });
    for (const [key, answer] of context.routes) {
      if (`${method} ${url}`.startsWith(key)) return answer(body, url);
    }
    throw new Error(`No fake answer for ${method} ${url}`);
  });
  return context;
}

/** Waits for every `waitUntil` work, including what that work started. */
export async function settle(context: TestContext): Promise<void> {
  while (context.background.length > 0) {
    await Promise.all(context.background.splice(0));
  }
}

export function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

export function cloudflareOk(result: unknown): Response {
  return jsonResponse({ success: true, errors: [], messages: [], result });
}

const CF = "https://api.cloudflare.com/client/v4";

/** GitHub signs in `login`, and Cloudflare provisions tunnel `tunnel`. */
export function fakeSignIn(
  context: TestContext,
  login: string,
  githubId: number,
  tunnel = "tunnel-1",
): void {
  context.routes.set("POST https://github.com/login/oauth/access_token", () =>
    jsonResponse({ access_token: `gho_${login}`, token_type: "bearer", scope: "" }),
  );
  context.routes.set("GET https://api.github.com/user", () =>
    jsonResponse({ id: githubId, login }),
  );
  context.routes.set(`POST ${CF}/accounts/account-1/cfd_tunnel`, () =>
    cloudflareOk({ id: tunnel }),
  );
  context.routes.set(`PUT ${CF}/accounts/account-1/cfd_tunnel/`, () => cloudflareOk({}));
  // A new tunnel secret, before a tunnel is removed.
  context.routes.set(`PATCH ${CF}/accounts/account-1/cfd_tunnel/`, () => cloudflareOk({}));
  context.routes.set(`POST ${CF}/zones/zone-1/dns_records`, () =>
    cloudflareOk({ id: `dns-${tunnel}` }),
  );
  context.routes.set(`PATCH ${CF}/zones/zone-1/dns_records/`, () => cloudflareOk({}));
  context.routes.set(`GET ${CF}/accounts/account-1/cfd_tunnel/`, () =>
    cloudflareOk(`token-of-${tunnel}`),
  );
  context.routes.set(`DELETE ${CF}/`, () => cloudflareOk(null));
}

/**
 * Signs `login` in on a machine. Unless `entitled` is false, the account
 * holds the override the old allowlist got (5 serving machines, 10 URLs).
 */
export async function signIn(
  context: TestContext,
  login = "teo-goulois",
  githubId = 1,
  machineName = "laptop",
  { entitled = true }: { entitled?: boolean } = {},
): Promise<{ token: string; machineId: string; userId: string }> {
  fakeSignIn(context, login, githubId, `tunnel-${machineName}`);
  const response = await api(context, "POST", "/v1/auth/device/token", {
    body: { deviceCode: "device-1", machineName, ingressPort: 7381 },
  });
  if (response.status !== 200)
    throw new Error(`Sign-in failed: ${response.status} ${await response.text()}`);
  const body = (await response.json()) as { token: string; machine: { id: string } };
  const { id: userId } = context.sqlite
    .prepare("SELECT id FROM users WHERE github_id = ?")
    .get(githubId) as { id: string };
  if (entitled) {
    context.sqlite
      .prepare(
        `INSERT OR IGNORE INTO plan_overrides (user_id, serving_machines, public_urls, reason, created_at)
         VALUES (?, 5, 10, 'team', 1)`,
      )
      .run(userId);
  }
  return { token: body.token, machineId: body.machine.id, userId };
}

/** Moves every owner's last serving stop to just before the quiet period, as if it had elapsed. */
export function endQuietPeriods(context: TestContext): void {
  context.sqlite
    .prepare(`UPDATE users SET serving_stopped_at = ? WHERE serving_stopped_at IS NOT NULL`)
    .run(Date.now() - QUIET_AFTER_STOP_MS - 1);
}

export async function api(
  context: TestContext,
  method: string,
  path: string,
  options: { token?: string; body?: unknown; headers?: Record<string, string> } = {},
): Promise<Response> {
  const { default: worker } = await import("../src/index");
  const headers = new Headers(options.headers);
  if (options.token) headers.set("authorization", `Bearer ${options.token}`);
  if (options.body !== undefined) headers.set("content-type", "application/json");
  const request = new Request(`https://api.localify.dev${path}`, {
    method,
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });
  return worker.fetch(request as never, context.env, context.ctx);
}

/** The v2 runtime of the machine behind `token`, after its background work is done. */
export async function serve(
  context: TestContext,
  token: string,
  generation = 0,
): Promise<{ status: number; body: Record<string, unknown> }> {
  const response = await api(context, "GET", `/v2/machines/self/runtime?generation=${generation}`, {
    token,
  });
  const body = (await response.json()) as Record<string, unknown>;
  await settle(context);
  return { status: response.status, body };
}

export async function visit(
  context: TestContext,
  url: string,
  init: { method?: string; headers?: Record<string, string>; body?: string } = {},
): Promise<Response> {
  const { default: worker } = await import("../src/index");
  return worker.fetch(new Request(url, init) as never, context.env, context.ctx);
}
