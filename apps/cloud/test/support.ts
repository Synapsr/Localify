/// <reference types="node" />
// A D1 stand-in over node:sqlite with the real migrations, a test Env, and a
// fake `fetch` for GitHub, Cloudflare and the machines' tunnels.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync, type SQLInputValue } from "node:sqlite";
import { vi } from "vitest";

const MIGRATIONS = join(import.meta.dirname, "..", "migrations");

class TestStatement {
  constructor(
    private readonly db: DatabaseSync,
    readonly sql: string,
    private readonly params: SQLInputValue[] = [],
  ) {}

  bind(...params: SQLInputValue[]): TestStatement {
    return new TestStatement(this.db, this.sql, params);
  }

  async first<T>(): Promise<T | null> {
    const row = this.db.prepare(this.sql).get(...this.params);
    return row ? ({ ...row } as T) : null;
  }

  async all<T>(): Promise<{ results: T[]; success: true; meta: Record<string, never> }> {
    return { results: this.rows() as T[], success: true, meta: {} };
  }

  async run(): Promise<{ results: []; success: true; meta: { changes: number } }> {
    return this.runSync();
  }

  rows(): Record<string, unknown>[] {
    return this.db
      .prepare(this.sql)
      .all(...this.params)
      .map((row) => ({ ...row }));
  }

  runSync(): { results: []; success: true; meta: { changes: number } } {
    const result = this.db.prepare(this.sql).run(...this.params);
    return { results: [], success: true, meta: { changes: Number(result.changes) } };
  }
}

/** Runs a batch in one transaction, like D1. */
class TestD1 {
  constructor(readonly sqlite: DatabaseSync) {}

  prepare(sql: string): TestStatement {
    return new TestStatement(this.sqlite, sql);
  }

  async batch(statements: TestStatement[]): Promise<unknown[]> {
    this.sqlite.exec("BEGIN");
    try {
      const results = statements.map((statement) =>
        /^\s*SELECT/i.test(statement.sql)
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

export function createTestDb(): { db: D1Database; sqlite: DatabaseSync } {
  // Foreign keys on, as D1 enforces them.
  const sqlite = new DatabaseSync(":memory:", { enableForeignKeyConstraints: true });
  for (const file of readdirSync(MIGRATIONS).sort()) {
    sqlite.exec(readFileSync(join(MIGRATIONS, file), "utf8"));
  }
  return { db: new TestD1(sqlite) as unknown as D1Database, sqlite };
}

export type TestContext = {
  env: Env;
  sqlite: DatabaseSync;
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
  routes: Map<string, (body: string | null) => Response>;
  limiter: { allowed: boolean };
  deviceLimiter: { allowed: boolean };
};

export function createContext(overrides: Partial<Env> = {}): TestContext {
  const { db, sqlite } = createTestDb();
  const limiter = { allowed: true };
  const deviceLimiter = { allowed: true };
  const env = {
    DB: db,
    PASSWORD_LIMITER: { limit: async () => ({ success: limiter.allowed }) },
    DEVICE_LIMITER: { limit: async () => ({ success: deviceLimiter.allowed }) },
    GITHUB_CLIENT_ID: "client-123",
    ALLOWED_GITHUB_IDS: "1, 2",
    PUBLIC_BASE_DOMAIN: "localify.fyi",
    MACHINE_DOMAIN: "localify.dev",
    CLOUDFLARE_ACCOUNT_ID: "account-1",
    MACHINE_ZONE_ID: "zone-1",
    CLOUDFLARE_API_TOKEN: "cf-token",
    COOKIE_KEY: "cookie-key",
    INGRESS_KEY: "ingress-key",
    ...overrides,
  } as unknown as Env;
  const context: TestContext = {
    env,
    sqlite,
    calls: [],
    routes: new Map(),
    limiter,
    deviceLimiter,
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
      if (`${method} ${url}`.startsWith(key)) return answer(body);
    }
    throw new Error(`No fake answer for ${method} ${url}`);
  });
  return context;
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

/** GitHub signs in `login`, and Cloudflare provisions tunnel `tunnel-<n>`. */
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
  context.routes.set(`POST ${CF}/zones/zone-1/dns_records`, () =>
    cloudflareOk({ id: `dns-${tunnel}` }),
  );
  context.routes.set(`GET ${CF}/accounts/account-1/cfd_tunnel/`, () =>
    cloudflareOk(`token-of-${tunnel}`),
  );
  context.routes.set(`DELETE ${CF}/`, () => cloudflareOk(null));
}

export async function signIn(
  context: TestContext,
  login = "teo-goulois",
  githubId = 1,
  machineName = "laptop",
): Promise<{ token: string; machineId: string }> {
  fakeSignIn(context, login, githubId, `tunnel-${machineName}`);
  const response = await api(context, "POST", "/v1/auth/device/token", {
    body: { deviceCode: "device-1", machineName, ingressPort: 7381 },
  });
  if (response.status !== 200)
    throw new Error(`Sign-in failed: ${response.status} ${await response.text()}`);
  const body = (await response.json()) as { token: string; machine: { id: string } };
  return { token: body.token, machineId: body.machine.id };
}

export async function api(
  context: TestContext,
  method: string,
  path: string,
  options: { token?: string; body?: unknown } = {},
): Promise<Response> {
  const { default: worker } = await import("../src/index");
  const headers = new Headers();
  if (options.token) headers.set("authorization", `Bearer ${options.token}`);
  if (options.body !== undefined) headers.set("content-type", "application/json");
  const request = new Request(`https://api.localify.dev${path}`, {
    method,
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });
  return worker.fetch(request as never, context.env);
}

export async function visit(
  context: TestContext,
  url: string,
  init: { method?: string; headers?: Record<string, string>; body?: string } = {},
): Promise<Response> {
  const { default: worker } = await import("../src/index");
  return worker.fetch(new Request(url, init) as never, context.env);
}
