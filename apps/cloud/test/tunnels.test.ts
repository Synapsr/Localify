import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { hmacHex } from "../src/crypto";
import { machineRuntime, runSweep } from "../src/tunnels";
import {
  api,
  applyMigrations,
  cloudflareOk,
  createContext,
  jsonResponse,
  serve,
  settle,
  signIn,
  type TestContext,
} from "./support";

const CF = "https://api.cloudflare.com/client/v4";
const TUNNELS = `${CF}/accounts/account-1/cfd_tunnel`;
const MINUTE = 60 * 1000;
let context: TestContext;

beforeEach(() => {
  context = createContext();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

type Row = {
  tunnel_state: string;
  tunnel_generation: number;
  tunnel_op: string | null;
  tunnel_id: string | null;
  tunnel_ready_at: number | null;
  down_seen_at: number | null;
  rotate_requested_at: number | null;
  dns_record_id: string | null;
  cleanup_attempts: number;
  next_cleanup_at: number | null;
};

function row(machineId: string): Row {
  return context.sqlite
    .prepare(
      `SELECT tunnel_state, tunnel_generation, tunnel_op, tunnel_id, tunnel_ready_at, down_seen_at,
              rotate_requested_at, dns_record_id, cleanup_attempts, next_cleanup_at
       FROM machines WHERE id = ?`,
    )
    .get(machineId) as Row;
}

function update(sql: string, ...params: (string | number | null)[]): void {
  context.sqlite.prepare(sql).run(...params);
}

function cloudflareCalls(): string[] {
  return context.calls
    .filter((call) => call.url.startsWith(CF))
    .map((call) => `${call.method} ${call.url.slice(CF.length)}`);
}

function count(prefix: string): number {
  return cloudflareCalls().filter((call) => call.startsWith(prefix)).length;
}

async function createUrl(token: string, localDomain: string): Promise<string> {
  const response = await api(context, "POST", "/v1/public-urls", {
    token,
    body: { localDomain, protection: { mode: "public" } },
  });
  expect(response.status).toBe(201);
  return ((await response.json()) as { publicUrl: { id: string } }).publicUrl.id;
}

/** A signed-in machine with one URL, served through a ready tunnel. */
async function servingMachine(login = "teo-goulois", githubId = 1, name = "laptop") {
  const machine = await signIn(context, login, githubId, name);
  const urlId = await createUrl(machine.token, `${name}.localify`);
  const first = await serve(context, machine.token);
  expect(first.body.state).toBe("provisioning");
  expect(row(machine.machineId).tunnel_state).toBe("ready");
  return { ...machine, urlId };
}

function listTunnels(tunnels: { id: string; name: string; status: string; ageMs: number }[]) {
  context.routes.set(`GET ${TUNNELS}?is_deleted=false`, () =>
    cloudflareOk(
      tunnels.map((tunnel) => ({
        id: tunnel.id,
        name: tunnel.name,
        status: tunnel.status,
        created_at: new Date(Date.now() - tunnel.ageMs).toISOString(),
      })),
    ),
  );
}

function listRecords(records: { id: string; name: string; content: string }[]) {
  context.routes.set(`GET ${CF}/zones/zone-1/dns_records?type=CNAME&per_page`, () =>
    cloudflareOk(records),
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

describe("migration", () => {
  it("makes existing tunnels generation 1, counts the ones still being removed, and gives the allowlist an override", () => {
    const sqlite = new DatabaseSync(":memory:", { enableForeignKeyConstraints: true });
    applyMigrations(sqlite, (file) => file < "0003");
    sqlite.exec(`
      INSERT INTO users (id, github_id, login, created_at) VALUES ('team', 77238381, 'teo', 1), ('other', 9, 'x', 1);
      INSERT INTO machines (id, user_id, name, status, tunnel_id, dns_record_id, ingress_port, created_at, reconcile)
      VALUES ('served', 'team', 'a', 'active', 't-1', 'd-1', 7381, 5, 0),
             ('gone', 'team', 'b', 'revoked', 't-2', NULL, 7381, 6, 0),
             ('failed', 'team', 'c', 'revoked', NULL, NULL, 7381, 7, 1),
             ('signing-in', 'team', 'd', 'provisioning', NULL, NULL, 7381, 8, 0),
             ('cleaned', 'team', 'e', 'revoked', NULL, NULL, 7381, 9, 0),
             ('bare', 'other', 'f', 'active', NULL, NULL, 7381, 10, 0);`);
    applyMigrations(sqlite, (file) => file >= "0003");

    expect(
      sqlite
        .prepare(
          "SELECT id, tunnel_state, tunnel_generation, tunnel_ready_at FROM machines ORDER BY id",
        )
        .all(),
    ).toEqual([
      { id: "bare", tunnel_state: "none", tunnel_generation: 0, tunnel_ready_at: null },
      { id: "cleaned", tunnel_state: "none", tunnel_generation: 0, tunnel_ready_at: null },
      // A revoked or unreconciled machine may still hold a tunnel: it holds a slot until the cleanup.
      { id: "failed", tunnel_state: "releasing", tunnel_generation: 0, tunnel_ready_at: null },
      { id: "gone", tunnel_state: "releasing", tunnel_generation: 0, tunnel_ready_at: null },
      { id: "served", tunnel_state: "ready", tunnel_generation: 1, tunnel_ready_at: 5 },
      { id: "signing-in", tunnel_state: "releasing", tunnel_generation: 0, tunnel_ready_at: null },
    ]);
    expect(
      sqlite.prepare("SELECT COUNT(*) AS n FROM machines WHERE tunnel_state != 'none'").get(),
    ).toEqual({ n: 4 });
    expect(
      sqlite
        .prepare(
          "SELECT user_id, serving_machines, public_urls, reason, expires_at FROM plan_overrides",
        )
        .all(),
    ).toEqual([
      { user_id: "team", serving_machines: 5, public_urls: 10, reason: "team", expires_at: null },
    ]);
  });

  it("refuses an unknown tunnel state", () => {
    expect(() =>
      context.sqlite.exec(`
        INSERT INTO users (id, github_id, login, created_at) VALUES ('u', 1, 'u', 1);
        INSERT INTO machines (id, user_id, name, status, ingress_port, created_at, tunnel_state)
        VALUES ('m', 'u', 'm', 'active', 7381, 1, 'up');`),
    ).toThrow(/CHECK constraint failed/);
  });
});

describe("runtime v2", () => {
  it("is idle, with no Cloudflare call, while the machine has nothing to serve", async () => {
    const { token, machineId } = await signIn(context);
    context.calls.length = 0;
    const { status, body } = await serve(context, token);
    expect(status).toBe(200);
    expect(body).toEqual({
      state: "idle",
      blocked: null,
      tunnel: null,
      ingressSecret: null,
      publicUrls: [],
      retryAfter: null,
    });
    expect(cloudflareCalls()).toEqual([]);
    expect(row(machineId).tunnel_state).toBe("none");
  });

  it("reserves a slot, provisions generation 1 in the background, then serves it", async () => {
    const { token, machineId } = await signIn(context);
    const urlId = await createUrl(token, "journal.localify");
    context.calls.length = 0;

    const response = await api(context, "GET", "/v2/machines/self/runtime?generation=0", { token });
    const first = (await response.json()) as Record<string, unknown>;
    expect(first).toMatchObject({ state: "provisioning", retryAfter: 3, tunnel: null });
    expect(row(machineId)).toMatchObject({ tunnel_state: "provisioning", tunnel_generation: 1 });
    await settle(context);

    const create = context.calls.find((call) => call.method === "POST" && call.url === TUNNELS)!;
    expect(JSON.parse(create.body!)).toEqual({
      name: `localify-${machineId}-1`,
      config_src: "cloudflare",
    });
    const configure = context.calls.find((call) => call.method === "PUT")!;
    expect(configure.url).toBe(`${TUNNELS}/tunnel-laptop/configurations`);
    expect(JSON.parse(configure.body!)).toEqual({
      config: {
        ingress: [
          { hostname: `m-${machineId}.localify.dev`, service: "http://127.0.0.1:7381" },
          { service: "http_status:404" },
        ],
      },
    });
    const dns = context.calls.find((call) => call.url.endsWith("/zones/zone-1/dns_records"))!;
    expect(JSON.parse(dns.body!)).toMatchObject({
      type: "CNAME",
      name: `m-${machineId}.localify.dev`,
      content: "tunnel-laptop.cfargotunnel.com",
      proxied: true,
    });
    expect(row(machineId)).toMatchObject({
      tunnel_state: "ready",
      tunnel_generation: 1,
      tunnel_op: null,
      tunnel_id: "tunnel-laptop",
      dns_record_id: "dns-tunnel-laptop",
    });

    const ready = await serve(context, token, 0);
    expect(ready.body).toMatchObject({
      state: "ready",
      blocked: null,
      tunnel: { generation: 1, token: "token-of-tunnel-laptop" },
      ingressSecret: await hmacHex("ingress-key", machineId),
      retryAfter: null,
    });
    expect(ready.body.publicUrls as { id: string; state: string }[]).toMatchObject([
      { id: urlId, state: "serving" },
    ]);

    // The daemon already runs generation 1: no token, no Cloudflare call.
    context.calls.length = 0;
    const polled = await serve(context, token, 1);
    expect(polled.body).toMatchObject({ state: "ready", tunnel: { generation: 1, token: null } });
    expect(cloudflareCalls()).toEqual([]);
  });

  it("refuses a missing or malformed generation", async () => {
    const { token } = await signIn(context);
    for (const query of ["", "?generation=", "?generation=-1", "?generation=one"]) {
      const response = await api(context, "GET", `/v2/machines/self/runtime${query}`, { token });
      expect(response.status, query).toBe(422);
    }
  });

  it("reserves one slot and makes one tunnel for concurrent fetches", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, "journal.localify");
    context.calls.length = 0;
    // Cloudflare answers only once all three fetches have answered.
    const created = deferred<Response>();
    context.routes.set(`POST ${TUNNELS}`, () => created.promise);
    const answers = await Promise.all(
      [1, 2, 3].map(() =>
        api(context, "GET", "/v2/machines/self/runtime?generation=0", { token }).then(
          (response) => response.json() as Promise<{ state: string }>,
        ),
      ),
    );
    expect(answers.map((answer) => answer.state)).toEqual([
      "provisioning",
      "provisioning",
      "provisioning",
    ]);
    created.resolve(cloudflareOk({ id: "tunnel-laptop" }));
    await settle(context);
    expect(count("POST /accounts/account-1/cfd_tunnel")).toBe(1);
    expect(row(machineId)).toMatchObject({ tunnel_state: "ready", tunnel_generation: 1 });
  });

  it("is blocked with serving_slots_full beyond the owner's serving machines, naming them", async () => {
    for (let i = 0; i < 5; i++) await servingMachine("teo-goulois", 1, `machine-${i}`);
    const sixth = await signIn(context, "teo-goulois", 1, "sixth");
    await createUrl(sixth.token, "sixth.localify");
    const { body } = await serve(context, sixth.token);
    expect(body).toMatchObject({
      state: "blocked",
      blocked: { reason: "serving_slots_full" },
      retryAfter: null,
      tunnel: null,
    });
    expect((body.blocked as { message: string }).message).toContain("machine-0");
    expect(row(sixth.machineId).tunnel_state).toBe("none");
  });

  it("is blocked with capacity once the account holds MAX_TUNNELS slots, and retries in 60 s", async () => {
    context = createContext({ MAX_TUNNELS: 1 } as Partial<Env>);
    await servingMachine();
    const ryan = await signIn(context, "ryan-ede", 2, "ryan");
    await createUrl(ryan.token, "ryan.localify");
    const { body } = await serve(context, ryan.token);
    expect(body).toMatchObject({
      state: "blocked",
      blocked: { reason: "capacity" },
      retryAfter: 60,
    });
    expect(row(ryan.machineId).tunnel_state).toBe("none");
  });

  it("is blocked with subscription_required without an entitlement, and releases the tunnel", async () => {
    const { machineId } = await servingMachine();
    const userId = (
      context.sqlite.prepare("SELECT user_id FROM machines WHERE id = ?").get(machineId) as {
        user_id: string;
      }
    ).user_id;
    // An account off the allowlist: the API refuses it before, so call the runtime itself.
    const runtime = await machineRuntime(
      context.env,
      context.ctx,
      { userId, githubId: 9, machineId },
      1,
    );
    expect(runtime).toMatchObject({
      state: "blocked",
      blocked: { reason: "subscription_required" },
      retryAfter: null,
    });
    expect(runtime.publicUrls.map((url) => url.state)).toEqual(["suspended"]);
    await settle(context);
    expect(row(machineId)).toMatchObject({ tunnel_state: "none", tunnel_id: null });
  });

  it("releases the tunnel and answers idle once the machine has nothing to serve", async () => {
    const { token, machineId, urlId } = await servingMachine();
    // Deleted behind the API's back, so only the runtime can notice.
    update("UPDATE public_urls SET deleted_at = 1 WHERE id = ?", urlId);
    context.calls.length = 0;
    const { body } = await serve(context, token, 1);
    expect(body.state).toBe("idle");
    expect(cloudflareCalls()).toEqual([
      "PATCH /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop",
    ]);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "none",
      tunnel_id: null,
      dns_record_id: "dns-tunnel-laptop",
    });
  });

  it("rotates right away when a rotation is requested, then serves the next generation", async () => {
    const { token, machineId } = await servingMachine();
    update("UPDATE machines SET rotate_requested_at = ? WHERE id = ?", Date.now(), machineId);
    const rotating = await serve(context, token, 1);
    expect(rotating.body.state).toBe("provisioning");
    expect(row(machineId)).toMatchObject({ tunnel_state: "none", rotate_requested_at: null });

    await serve(context, token, 1);
    const next = await serve(context, token, 1);
    expect(next.body).toMatchObject({
      state: "ready",
      tunnel: { generation: 2, token: "token-of-tunnel-laptop" },
    });
  });

  it("leaves a failed provisioning until it is abandoned, then provisions the next generation", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, "journal.localify");
    context.routes.set(`PUT ${TUNNELS}/`, () => jsonResponse({ success: false, errors: [] }, 500));
    await serve(context, token);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "provisioning",
      tunnel_generation: 1,
      tunnel_id: "tunnel-laptop",
    });

    // Not yet abandoned: nothing changes.
    expect((await serve(context, token)).body.state).toBe("provisioning");
    expect(row(machineId).tunnel_state).toBe("provisioning");

    update("UPDATE machines SET tunnel_op_at = ? WHERE id = ?", Date.now() - 3 * MINUTE, machineId);
    context.routes.set(`PUT ${TUNNELS}/`, () => cloudflareOk({}));
    context.calls.length = 0;
    expect((await serve(context, token)).body.state).toBe("provisioning");
    expect(cloudflareCalls()).toEqual([
      "PATCH /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop",
    ]);
    expect(row(machineId).tunnel_state).toBe("none");

    await serve(context, token);
    expect(row(machineId)).toMatchObject({ tunnel_state: "ready", tunnel_generation: 2 });
  });
});

describe("operations", () => {
  it("lets a stale provisioning lose to a newer generation, deleting only its own tunnel", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, "journal.localify");
    const stalled = deferred<Response>();
    let creates = 0;
    context.routes.set(`POST ${TUNNELS}`, () =>
      ++creates === 1 ? stalled.promise : cloudflareOk({ id: "tunnel-gen-2" }),
    );
    context.routes.set(`GET ${TUNNELS}?name=`, () => cloudflareOk([]));

    await api(context, "GET", "/v2/machines/self/runtime?generation=0", { token });
    expect(row(machineId)).toMatchObject({ tunnel_state: "provisioning", tunnel_generation: 1 });

    // Two minutes later the operation counts as abandoned: the sweep releases it.
    await runSweep(context.env, Date.now() + 3 * MINUTE);
    expect(row(machineId)).toMatchObject({ tunnel_state: "none", tunnel_op: null });

    // The next fetch provisions generation 2 while generation 1's call is still on its way.
    const fetched = api(context, "GET", "/v2/machines/self/runtime?generation=0", { token });
    stalled.resolve(cloudflareOk({ id: "tunnel-gen-1" }));
    await fetched;
    await settle(context);

    expect(row(machineId)).toMatchObject({
      tunnel_state: "ready",
      tunnel_generation: 2,
      tunnel_id: "tunnel-gen-2",
    });
    expect(cloudflareCalls()).toContain("DELETE /accounts/account-1/cfd_tunnel/tunnel-gen-1");
    expect(cloudflareCalls()).not.toContain("DELETE /accounts/account-1/cfd_tunnel/tunnel-gen-2");
    const configured = context.calls
      .filter((call) => call.method === "PUT")
      .map((call) => call.url.slice(TUNNELS.length));
    expect(configured).toEqual(["/tunnel-gen-2/configurations"]);
  });

  it("keeps the CNAME across tunnels: the next tunnel only moves it", async () => {
    const { token, machineId, urlId } = await servingMachine();
    await api(context, "DELETE", `/v1/public-urls/${urlId}`, { token });
    await settle(context);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "none",
      dns_record_id: "dns-tunnel-laptop",
    });
    expect(count("DELETE /zones/")).toBe(0);

    let creates = 0;
    context.routes.set(`POST ${TUNNELS}`, () => cloudflareOk({ id: `tunnel-${++creates + 1}` }));
    await createUrl(token, "again.localify");
    await serve(context, token);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "ready",
      tunnel_generation: 2,
      tunnel_id: "tunnel-2",
      dns_record_id: "dns-tunnel-laptop",
    });
    expect(count("POST /zones/zone-1/dns_records")).toBe(1);
    const moved = context.calls.find(
      (call) => call.method === "PATCH" && call.url.includes("/dns_records/"),
    )!;
    expect(moved.url).toBe(`${CF}/zones/zone-1/dns_records/dns-tunnel-laptop`);
    expect(JSON.parse(moved.body!)).toEqual({
      content: "tunnel-2.cfargotunnel.com",
      proxied: true,
    });
  });

  it("releases a machine whose URL is claimed elsewhere", async () => {
    const laptop = await servingMachine();
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    await api(context, "POST", `/v1/public-urls/${laptop.urlId}/claim`, { token: runner.token });
    await settle(context);
    expect(row(laptop.machineId).tunnel_state).toBe("none");
    expect(row(runner.machineId).tunnel_state).toBe("none");
    await serve(context, runner.token);
    expect(row(runner.machineId).tunnel_state).toBe("ready");
  });

  it("keeps serving a machine that still has a URL after one is deleted", async () => {
    const { token, machineId } = await servingMachine();
    const second = await createUrl(token, "second.localify");
    await api(context, "DELETE", `/v1/public-urls/${second}`, { token });
    await settle(context);
    expect(row(machineId).tunnel_state).toBe("ready");
  });
});

describe("runtime v1", () => {
  it("answers 409 runtime_not_ready until the tunnel is ready, then the current token every time", async () => {
    const { token, machineId } = await signIn(context);
    const idle = await api(context, "GET", "/v1/machines/self/runtime", { token });
    expect(idle.status).toBe(409);
    expect(await idle.json()).toMatchObject({ error: "runtime_not_ready", state: "idle" });

    const urlId = await createUrl(token, "journal.localify");
    const pending = await api(context, "GET", "/v1/machines/self/runtime", { token });
    expect(pending.status).toBe(409);
    expect(await pending.json()).toMatchObject({
      error: "runtime_not_ready",
      state: "provisioning",
    });
    await settle(context);

    context.calls.length = 0;
    for (let i = 0; i < 2; i++) {
      const ready = await api(context, "GET", "/v1/machines/self/runtime", { token });
      expect(ready.status).toBe(200);
      const body = (await ready.json()) as {
        tunnelToken: string;
        ingressSecret: string;
        publicUrls: { id: string }[];
      };
      expect(body.tunnelToken).toBe("token-of-tunnel-laptop");
      expect(body.ingressSecret).toBe(await hmacHex("ingress-key", machineId));
      expect(body.publicUrls.map((url) => url.id)).toEqual([urlId]);
    }
    expect(cloudflareCalls()).toEqual([
      "GET /accounts/account-1/cfd_tunnel/tunnel-laptop/token",
      "GET /accounts/account-1/cfd_tunnel/tunnel-laptop/token",
    ]);
  });

  it("serves the tunnels made before generations", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, "journal.localify");
    update(
      `UPDATE machines SET tunnel_state = 'ready', tunnel_generation = 1, tunnel_id = 'legacy',
              tunnel_ready_at = 1 WHERE id = ?`,
      machineId,
    );
    const ready = await api(context, "GET", "/v1/machines/self/runtime", { token });
    expect(((await ready.json()) as { tunnelToken: string }).tunnelToken).toBe(
      "token-of-tunnel-laptop",
    );
    expect(cloudflareCalls().at(-1)).toBe("GET /accounts/account-1/cfd_tunnel/legacy/token");
  });

  it("answers 502 when Cloudflare fails", async () => {
    const { token } = await servingMachine();
    context.routes.clear();
    context.routes.set(`GET ${CF}/`, () => jsonResponse({ success: false, errors: [] }, 500));
    const response = await api(context, "GET", "/v1/machines/self/runtime", { token });
    expect(response.status).toBe(502);
    expect(((await response.json()) as { error: string }).error).toBe("cloudflare_unavailable");
  });
});

describe("the sweep", () => {
  it("reaps a tunnel only after two down readings 15 minutes apart", async () => {
    const { machineId } = await servingMachine();
    listTunnels([
      { id: "tunnel-laptop", name: `localify-${machineId}-1`, status: "down", ageMs: 0 },
    ]);
    const start = Date.now() + 16 * MINUTE;

    await runSweep(context.env, start);
    expect(row(machineId)).toMatchObject({ tunnel_state: "ready", down_seen_at: start });
    await runSweep(context.env, start + 10 * MINUTE);
    expect(row(machineId)).toMatchObject({ tunnel_state: "ready", down_seen_at: start });
    await runSweep(context.env, start + 16 * MINUTE);
    expect(row(machineId)).toMatchObject({ tunnel_state: "none", down_seen_at: null });
    expect(count("DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop")).toBe(2);
  });

  it("gives a new tunnel 15 minutes of grace", async () => {
    const { machineId } = await servingMachine();
    listTunnels([
      { id: "tunnel-laptop", name: `localify-${machineId}-1`, status: "inactive", ageMs: 0 },
    ]);
    const old = Date.now() - 20 * MINUTE;
    update("UPDATE machines SET down_seen_at = ? WHERE id = ?", old, machineId);
    await runSweep(context.env);
    expect(row(machineId)).toMatchObject({ tunnel_state: "ready", down_seen_at: old });
  });

  it("clears a down reading when the tunnel is healthy again, and reads a missing tunnel as down", async () => {
    const laptop = await servingMachine();
    const runner = await servingMachine("teo-goulois", 1, "runner");
    listTunnels([
      { id: "tunnel-laptop", name: `localify-${laptop.machineId}-1`, status: "healthy", ageMs: 0 },
    ]);
    update("UPDATE machines SET down_seen_at = 1 WHERE id = ?", laptop.machineId);
    const now = Date.now();
    await runSweep(context.env, now);
    expect(row(laptop.machineId).down_seen_at).toBeNull();
    expect(row(runner.machineId).down_seen_at).toBe(now);
  });

  it("changes nothing when the status reads fail", async () => {
    const { machineId } = await servingMachine();
    context.routes.set(`GET ${TUNNELS}?is_deleted=false`, () =>
      jsonResponse({ success: false, errors: [] }, 500),
    );
    const old = Date.now() - 30 * MINUTE;
    update(
      "UPDATE machines SET down_seen_at = ?, tunnel_ready_at = ? WHERE id = ?",
      old,
      old,
      machineId,
    );
    await runSweep(context.env);
    expect(row(machineId)).toMatchObject({ tunnel_state: "ready", down_seen_at: old });
  });

  it("rotates a machine whose rotation was requested, first, and logs a late one", async () => {
    const { token, machineId } = await servingMachine();
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    update(
      "UPDATE machines SET rotate_requested_at = ? WHERE id = ?",
      Date.now() - 11 * MINUTE,
      machineId,
    );
    // No listing answers: revocations do not need one.
    await runSweep(context.env);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "none",
      rotate_requested_at: null,
      tunnel_id: null,
    });
    expect(
      errors.mock.calls.some((call) => String(call[0]).includes("waited more than 10 minutes")),
    ).toBe(true);

    await serve(context, token, 1);
    const next = await serve(context, token, 1);
    expect(next.body).toMatchObject({ state: "ready", tunnel: { generation: 2 } });
  });

  it("releases every slot of an owner who lost the entitlement", async () => {
    const { machineId } = await servingMachine();
    context.env.ALLOWED_GITHUB_IDS = "2";
    await runSweep(context.env);
    expect(row(machineId).tunnel_state).toBe("none");
  });

  it("releases the slots provisioned most recently beyond the owner's serving machines", async () => {
    const machines = [];
    for (let i = 0; i < 5; i++) machines.push(await servingMachine("teo-goulois", 1, `m-${i}`));
    // A sixth slot, as a downgrade would leave it.
    const extra = await signIn(context, "teo-goulois", 1, "extra");
    await createUrl(extra.token, "extra.localify");
    update(
      `UPDATE machines SET tunnel_state = 'ready', tunnel_generation = 1, tunnel_id = 'tunnel-extra',
              tunnel_ready_at = ? WHERE id = ?`,
      Date.now() + 1,
      extra.machineId,
    );
    await runSweep(context.env);
    expect(row(extra.machineId).tunnel_state).toBe("none");
    for (const machine of machines) expect(row(machine.machineId).tunnel_state).toBe("ready");
  });

  it("does at least 75 revocations a run within 300 calls, and the rest at the next one", async () => {
    const { machineId } = await signIn(context);
    const { user_id } = context.sqlite
      .prepare("SELECT user_id FROM machines WHERE id = ?")
      .get(machineId) as { user_id: string };
    const insert = context.sqlite.prepare(
      `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at, tunnel_state,
                             tunnel_generation, tunnel_id, tunnel_ready_at, rotate_requested_at)
       VALUES (?, ?, ?, 'active', 7381, 1, 'ready', 1, ?, 1, ?)`,
    );
    for (let i = 0; i < 101; i++) insert.run(`r-${i}`, user_id, `r-${i}`, `t-${i}`, i + 1);
    vi.spyOn(console, "error").mockImplementation(() => {});
    const ready = () =>
      (
        context.sqlite
          .prepare("SELECT COUNT(*) AS n FROM machines WHERE tunnel_state != 'none'")
          .get() as { n: number }
      ).n;

    context.calls.length = 0;
    await runSweep(context.env);
    // 3 calls each, started while 4 remain: 99 of them.
    expect(ready()).toBe(2);
    expect(row("r-100").tunnel_state).toBe("ready");
    expect(cloudflareCalls().length).toBeLessThanOrEqual(300);
    await runSweep(context.env);
    expect(ready()).toBe(0);
  });

  it("releases an abandoned operation, finding its tunnel by name", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, "journal.localify");
    update(
      `UPDATE machines SET tunnel_state = 'provisioning', tunnel_generation = 4, tunnel_op = 'old',
              tunnel_op_at = ? WHERE id = ?`,
      Date.now() - 3 * MINUTE,
      machineId,
    );
    context.routes.set(`GET ${TUNNELS}?name=`, () => cloudflareOk([{ id: "tunnel-4" }]));
    await runSweep(context.env);
    expect(cloudflareCalls()).toContain(
      `GET /accounts/account-1/cfd_tunnel?name=localify-${machineId}-4&is_deleted=false`,
    );
    expect(cloudflareCalls()).toContain("DELETE /accounts/account-1/cfd_tunnel/tunnel-4");
    expect(row(machineId)).toMatchObject({ tunnel_state: "none", tunnel_op: null });
  });

  it("leaves an operation that is still running", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, "journal.localify");
    update(
      `UPDATE machines SET tunnel_state = 'provisioning', tunnel_generation = 1, tunnel_op = 'live',
              tunnel_op_at = ? WHERE id = ?`,
      Date.now(),
      machineId,
    );
    await runSweep(context.env);
    expect(row(machineId)).toMatchObject({ tunnel_state: "provisioning", tunnel_op: "live" });
  });

  it("releases a machine with nothing left to serve, backing off when that fails", async () => {
    const { machineId, urlId } = await servingMachine();
    update("UPDATE public_urls SET deleted_at = 1 WHERE id = ?", urlId);
    context.routes.delete(`DELETE ${CF}/`);
    context.routes.set(`DELETE ${TUNNELS}/`, () =>
      jsonResponse({ success: false, errors: [] }, 500),
    );
    const now = Date.now();
    await runSweep(context.env, now);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "releasing",
      cleanup_attempts: 1,
      next_cleanup_at: now + 10 * MINUTE,
    });

    context.routes.set(`DELETE ${TUNNELS}/`, () => cloudflareOk(null));
    context.calls.length = 0;
    await runSweep(context.env, now + 5 * MINUTE);
    expect(count("DELETE")).toBe(0);
    await runSweep(context.env, now + 11 * MINUTE);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "none",
      cleanup_attempts: 0,
      next_cleanup_at: null,
    });
  });

  it("points a drifted CNAME back at the machine's tunnel", async () => {
    const { machineId } = await servingMachine();
    listTunnels([
      { id: "tunnel-laptop", name: `localify-${machineId}-1`, status: "healthy", ageMs: 0 },
    ]);
    listRecords([
      {
        id: "dns-tunnel-laptop",
        name: `m-${machineId}.localify.dev`,
        content: "older-tunnel.cfargotunnel.com",
      },
    ]);
    context.calls.length = 0;
    await runSweep(context.env);
    const patches = context.calls.filter((call) => call.method === "PATCH");
    expect(patches.map((call) => [call.url, JSON.parse(call.body!)])).toEqual([
      [
        `${CF}/zones/zone-1/dns_records/dns-tunnel-laptop`,
        { content: "tunnel-laptop.cfargotunnel.com", proxied: true },
      ],
    ]);
    expect(row(machineId).tunnel_op).toBeNull();

    listRecords([
      {
        id: "dns-tunnel-laptop",
        name: `m-${machineId}.localify.dev`,
        content: "tunnel-laptop.cfargotunnel.com",
      },
    ]);
    context.calls.length = 0;
    await runSweep(context.env);
    expect(count("PATCH")).toBe(0);
  });

  it("deletes our tunnels that no machine holds once they are an hour old", async () => {
    const { machineId } = await servingMachine();
    const orphan = `localify-${"a".repeat(24)}-3`;
    listTunnels([
      {
        id: "tunnel-laptop",
        name: `localify-${machineId}-1`,
        status: "healthy",
        ageMs: 2 * 60 * MINUTE,
      },
      { id: "orphan", name: orphan, status: "inactive", ageMs: 2 * 60 * MINUTE },
      { id: "young", name: `localify-${"b".repeat(24)}-1`, status: "inactive", ageMs: 10 * MINUTE },
      { id: "foreign", name: "someone-elses-tunnel", status: "inactive", ageMs: 2 * 60 * MINUTE },
    ]);
    context.calls.length = 0;
    await runSweep(context.env);
    expect(cloudflareCalls().filter((call) => call.startsWith("DELETE"))).toEqual([
      "DELETE /accounts/account-1/cfd_tunnel/orphan/connections",
      "DELETE /accounts/account-1/cfd_tunnel/orphan",
    ]);
  });

  it("logs an error above 800 slots", async () => {
    const { machineId } = await signIn(context);
    const { user_id } = context.sqlite
      .prepare("SELECT user_id FROM machines WHERE id = ?")
      .get(machineId) as { user_id: string };
    const insert = context.sqlite.prepare(
      `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at, tunnel_state,
                             tunnel_generation, tunnel_op, tunnel_op_at)
       VALUES (?, ?, ?, 'active', 7381, 1, 'releasing', 1, 'op', ?)`,
    );
    // Releases still running: the sweep leaves them be.
    for (let i = 0; i < 801; i++) insert.run(`s-${i}`, user_id, `s-${i}`, Date.now());
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    await runSweep(context.env);
    expect(
      errors.mock.calls.some((call) => String(call[0]).includes("holds 801 serving slots")),
    ).toBe(true);
  });
});

describe("revocations that fail", () => {
  function failDeletes() {
    context.routes.delete(`DELETE ${CF}/`);
    context.routes.set(`DELETE ${TUNNELS}/`, () =>
      jsonResponse({ success: false, errors: [] }, 500),
    );
  }

  it("keep their mark until Cloudflare confirms, and are retried every run without backoff", async () => {
    const { machineId } = await servingMachine();
    vi.spyOn(console, "error").mockImplementation(() => {});
    const requested = Date.now();
    update("UPDATE machines SET rotate_requested_at = ? WHERE id = ?", requested, machineId);
    failDeletes();
    for (let run = 0; run < 3; run++) {
      context.calls.length = 0;
      await runSweep(context.env, requested + run * 5 * MINUTE);
      expect(count("DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop"), `run ${run}`).toBe(1);
      expect(row(machineId)).toMatchObject({
        tunnel_state: "releasing",
        rotate_requested_at: requested,
      });
    }
    context.routes.set(`DELETE ${TUNNELS}/`, () => cloudflareOk(null));
    await runSweep(context.env, requested + 15 * MINUTE);
    expect(row(machineId)).toMatchObject({
      tunnel_state: "none",
      rotate_requested_at: null,
      cleanup_attempts: 0,
    });
  });

  it("are retried every run for an owner who lost the entitlement", async () => {
    const { machineId } = await servingMachine();
    vi.spyOn(console, "error").mockImplementation(() => {});
    context.env.ALLOWED_GITHUB_IDS = "2";
    failDeletes();
    const start = Date.now();
    for (let run = 0; run < 3; run++) {
      context.calls.length = 0;
      await runSweep(context.env, start + run * 5 * MINUTE);
      expect(count("DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop"), `run ${run}`).toBe(1);
    }
    expect(row(machineId)).toMatchObject({ tunnel_state: "releasing", next_cleanup_at: null });
    context.routes.set(`DELETE ${TUNNELS}/`, () => cloudflareOk(null));
    await runSweep(context.env, start + 15 * MINUTE);
    expect(row(machineId)).toMatchObject({ tunnel_state: "none", rotate_requested_at: null });
  });
});

describe("removing a tunnel", () => {
  it("gives it a new secret before dropping its connections and deleting it", async () => {
    const { token, urlId } = await servingMachine();
    context.calls.length = 0;
    await api(context, "DELETE", `/v1/public-urls/${urlId}`, { token });
    await settle(context);
    expect(cloudflareCalls()).toEqual([
      "PATCH /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop",
    ]);
    const { tunnel_secret } = JSON.parse(context.calls[0]!.body!) as { tunnel_secret: string };
    expect(Buffer.from(tunnel_secret, "base64")).toHaveLength(32);
  });
});

describe("the sweep's budget", () => {
  it("counts every Cloudflare request, fallbacks included", async () => {
    const githubIds = Array.from({ length: 20 }, (_, i) => 100 + i);
    context = createContext({ ALLOWED_GITHUB_IDS: githubIds.join(",") } as Partial<Env>);
    const insertUser = context.sqlite.prepare(
      "INSERT INTO users (id, github_id, login, created_at) VALUES (?, ?, ?, 1)",
    );
    const insertMachine = context.sqlite.prepare(
      `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at, tunnel_state,
                             tunnel_generation, tunnel_id, tunnel_ready_at, dns_record_id)
       VALUES (?, ?, ?, 'active', 7381, 1, 'ready', 1, ?, 1, 'missing')`,
    );
    const insertUrl = context.sqlite.prepare(
      `INSERT INTO public_urls (id, user_id, slug, local_domain, wildcard, machine_id, protected,
                                password_version, created_at)
       VALUES (?, ?, ?, ?, 0, ?, 0, 1, 1)`,
    );
    // 20 owners serving from 5 machines each, every CNAME gone from the zone.
    const tunnels = [];
    for (const [u, githubId] of githubIds.entries()) {
      insertUser.run(`u-${u}`, githubId, `user-${u}`);
      for (let m = 0; m < 5; m++) {
        const id = `m-${u}-${m}`;
        insertMachine.run(id, `u-${u}`, id, `t-${id}`);
        insertUrl.run(`url-${id}`, `u-${u}`, `slug-${id}`, `${id}.localify`, id);
        tunnels.push({ id: `t-${id}`, name: `localify-${id}-1`, status: "healthy", ageMs: 0 });
      }
    }
    listTunnels(tunnels);
    listRecords([]);
    // Each correction takes the longest path: moved, created, looked up, moved.
    context.routes.set(`PATCH ${CF}/zones/zone-1/dns_records/missing`, () =>
      jsonResponse({ success: false, errors: [] }, 404),
    );
    context.routes.set(`POST ${CF}/zones/zone-1/dns_records`, () =>
      jsonResponse({ success: false, errors: [{ code: 81053, message: "exists" }] }, 400),
    );
    context.routes.set(`GET ${CF}/zones/zone-1/dns_records?type=CNAME&name=`, () =>
      cloudflareOk([{ id: "found" }]),
    );
    context.routes.set(`PATCH ${CF}/zones/zone-1/dns_records/found`, () => cloudflareOk({}));
    vi.spyOn(console, "warn").mockImplementation(() => {});

    await runSweep(context.env);
    expect(cloudflareCalls().length).toBeLessThanOrEqual(300);
    expect(count("PATCH /zones/zone-1/dns_records/found")).toBeGreaterThan(0);
  });
});
