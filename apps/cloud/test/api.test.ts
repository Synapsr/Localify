import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CallBudget } from "../src/cloudflare";
import { sha256Hex } from "../src/crypto";
import { cleanUpRevokedMachines, purgeDeletedUrls, revokeStuckSignIns } from "../src/machines";
import { WORDS } from "../src/passwords";
import {
  api,
  cloudflareOk,
  createContext,
  fakeSignIn,
  jsonResponse,
  serve,
  signIn,
  type TestContext,
} from "./support";

type PublicUrlBody = {
  id: string;
  slug: string;
  url: string;
  localDomain: string;
  wildcard: boolean;
  machine: { id: string; name: string } | null;
  protection: { mode: string; version?: number };
  state: string;
  createdAt: string;
};

const CF = "https://api.cloudflare.com/client/v4";
let context: TestContext;

beforeEach(() => {
  context = createContext();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function createUrl(token: string, body: Record<string, unknown>) {
  const response = await api(context, "POST", "/v1/public-urls", { token, body });
  return {
    response,
    body: (await response.json()) as {
      publicUrl: PublicUrlBody;
      password?: string;
      error?: string;
    },
  };
}

async function errorCode(response: Response): Promise<string> {
  return ((await response.json()) as { error: string }).error;
}

/** The cleanup's bookkeeping for one machine. */
function cleanupRow(id: string) {
  return context.sqlite
    .prepare(
      `SELECT user_id, tunnel_id, dns_record_id, reconcile, cleanup_attempts, next_cleanup_at
       FROM machines WHERE id = ?`,
    )
    .get(id) as Record<string, unknown>;
}

function machineRow(id: string) {
  return context.sqlite
    .prepare("SELECT status, tunnel_id, dns_record_id, revoked_at FROM machines WHERE id = ?")
    .get(id) as
    | {
        status: string;
        tunnel_id: string | null;
        dns_record_id: string | null;
        revoked_at: number | null;
      }
    | undefined;
}

function cloudflareCalls(): string[] {
  return context.calls
    .filter((call) => call.url.startsWith(CF))
    .map((call) => `${call.method} ${call.url.slice(CF.length)}`);
}

describe("errors", () => {
  it("answers 401 JSON without a token", async () => {
    const response = await api(context, "GET", "/v1/public-urls");
    expect(response.status).toBe(401);
    expect(response.headers.get("content-type")).toContain("application/json");
    const body = (await response.json()) as { error: string; message: string };
    expect(body.error).toBe("unauthorized");
    expect(body.message).toContain("localify login");
  });

  it("answers 401 for an unknown token", async () => {
    expect((await api(context, "GET", "/v1/me", { token: `lfy_${"0".repeat(64)}` })).status).toBe(
      401,
    );
  });

  it("answers 404 and 405 as JSON", async () => {
    expect((await api(context, "GET", "/v1/nothing")).status).toBe(404);
    const response = await api(context, "GET", "/v1/auth/device");
    expect(response.status).toBe(405);
    expect(await errorCode(response)).toBe("method_not_allowed");
  });

  it("refuses a body that is not JSON", async () => {
    const { token } = await signIn(context);
    const { default: worker } = await import("../src/index");
    const response = await worker.fetch(
      new Request("https://api.localify.dev/v1/public-urls", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: "{",
      }) as never,
      context.env,
      context.ctx,
    );
    expect(response.status).toBe(400);
  });

  it("says when a setting is missing", async () => {
    context = createContext({ GITHUB_CLIENT_ID: "" });
    const response = await api(context, "POST", "/v1/auth/device");
    expect(response.status).toBe(500);
    expect(await errorCode(response)).toBe("not_configured");
  });
});

describe("device flow", () => {
  it("starts GitHub's device flow with the client ID and no scope", async () => {
    context.routes.set("POST https://github.com/login/device/code", () =>
      jsonResponse({
        device_code: "dev",
        user_code: "ABCD-1234",
        verification_uri: "https://github.com/login/device",
        expires_in: 899,
        interval: 5,
      }),
    );
    const response = await api(context, "POST", "/v1/auth/device");
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      deviceCode: "dev",
      userCode: "ABCD-1234",
      verificationUri: "https://github.com/login/device",
      interval: 5,
      expiresIn: 899,
    });
    const call = context.calls[0]!;
    expect(call.headers.get("accept")).toBe("application/json");
    expect(Object.fromEntries(new URLSearchParams(call.body ?? ""))).toEqual({
      client_id: "client-123",
    });
  });

  it("polls with the device grant type", async () => {
    context.routes.set("POST https://github.com/login/oauth/access_token", () =>
      jsonResponse({ error: "authorization_pending" }),
    );
    await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
    });
    expect(Object.fromEntries(new URLSearchParams(context.calls[0]!.body ?? ""))).toEqual({
      client_id: "client-123",
      device_code: "d",
      grant_type: "urn:ietf:params:oauth:grant-type:device_code",
    });
  });

  it.each([
    ["authorization_pending", 428],
    ["slow_down", 429],
    ["expired_token", 410],
    ["access_denied", 403],
  ])("maps GitHub's %s to %i", async (error, status) => {
    context.routes.set("POST https://github.com/login/oauth/access_token", () =>
      jsonResponse({ error, interval: 10 }),
    );
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
    });
    expect(response.status).toBe(status);
    expect(await errorCode(response)).toBe(error);
  });

  it("answers slow_down with GitHub's interval plus 5 seconds", async () => {
    context.routes.set("POST https://github.com/login/oauth/access_token", () =>
      jsonResponse({ error: "slow_down", interval: 10 }),
    );
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
    });
    expect(await response.json()).toMatchObject({ error: "slow_down", interval: 15 });
  });

  it("rate limits both device routes per client IP", async () => {
    context.deviceLimiter.allowed = false;
    for (const path of ["/v1/auth/device", "/v1/auth/device/token"]) {
      const response = await api(context, "POST", path, {
        body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
      });
      expect(response.status, path).toBe(429);
      expect(await errorCode(response)).toBe("rate_limited");
    }
    expect(context.calls).toHaveLength(0);
  });

  it("validates the sign-in body", async () => {
    for (const body of [
      { machineName: "laptop", ingressPort: 7381 },
      { deviceCode: "d", machineName: " ", ingressPort: 7381 },
      { deviceCode: "d", machineName: "laptop", ingressPort: 0 },
      { deviceCode: "d", machineName: "laptop", ingressPort: "7381" },
    ]) {
      expect(
        (await api(context, "POST", "/v1/auth/device/token", { body })).status,
        JSON.stringify(body),
      ).toBe(422);
    }
  });

  it("checks the GitHub id, not the login", async () => {
    context.routes.set("POST https://github.com/login/oauth/access_token", () =>
      jsonResponse({ access_token: "gho" }),
    );
    // An allowed login, renamed and then taken by someone else, gives no access.
    context.routes.set("GET https://api.github.com/user", () =>
      jsonResponse({ id: 9, login: "teo-goulois" }),
    );
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
    });
    expect(response.status).toBe(403);
    expect(await errorCode(response)).toBe("not_allowed");
    expect(cloudflareCalls()).toEqual([]);
  });
});

describe("signing in", () => {
  it("registers an active machine with no tunnel and a hashed token, without any Cloudflare call", async () => {
    const { token, machineId } = await signIn(context, "Teo-Goulois", 1, "laptop");
    expect(token).toMatch(/^lfy_[0-9a-f]{64}$/);

    const github = context.calls.find((call) => call.url === "https://api.github.com/user")!;
    expect(github.headers.get("user-agent")).toBeTruthy();
    expect(github.headers.get("authorization")).toBe("Bearer gho_Teo-Goulois");
    expect(cloudflareCalls()).toEqual([]);

    expect(machineRow(machineId)).toEqual({
      status: "active",
      tunnel_id: null,
      dns_record_id: null,
      revoked_at: null,
    });
    expect(
      context.sqlite
        .prepare("SELECT tunnel_state, tunnel_generation FROM machines WHERE id = ?")
        .get(machineId),
    ).toEqual({ tunnel_state: "none", tunnel_generation: 0 });
    expect(context.sqlite.prepare("SELECT hash FROM tokens").all()).toEqual([
      { hash: await sha256Hex(token) },
    ]);

    const me = await api(context, "GET", "/v1/me", { token });
    expect(await me.json()).toMatchObject({
      user: { id: expect.any(String), login: "Teo-Goulois" },
      machine: { id: machineId, name: "laptop" },
    });
  });

  it("keeps one user per GitHub account and follows login renames", async () => {
    await signIn(context, "teo-goulois", 1, "laptop");
    await signIn(context, "teo-renamed", 1, "runner");
    expect(context.sqlite.prepare("SELECT login FROM users").all()).toEqual([
      { login: "teo-renamed" },
    ]);
    expect(context.sqlite.prepare("SELECT COUNT(*) AS n FROM machines").get()).toEqual({ n: 2 });
  });

  it("keeps a user without a plan to 10 signed-in machines", async () => {
    for (let i = 0; i < 10; i++) {
      await signIn(context, "teo-goulois", 1, `machine-${i}`, { entitled: false });
    }
    fakeSignIn(context, "teo-goulois", 1, "tunnel-11");
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "machine-11", ingressPort: 7381 },
    });
    expect(response.status).toBe(403);
    expect(await errorCode(response)).toBe("quota_exceeded");
    expect(context.sqlite.prepare("SELECT COUNT(*) AS n FROM machines").get()).toEqual({ n: 10 });
    expect(context.sqlite.prepare("SELECT COUNT(*) AS n FROM tokens").get()).toEqual({ n: 10 });
  });

  it("keeps the quota when sign-ins with the same name race", async () => {
    for (let i = 0; i < 9; i++) await signIn(context, "teo-goulois", 1, `machine-${i}`);
    await signIn(context, "teo-goulois", 1, "laptop");
    fakeSignIn(context, "teo-goulois", 1, "tunnel-new");
    const responses = await Promise.all(
      [1, 2, 3].map(() =>
        api(context, "POST", "/v1/auth/device/token", {
          body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
        }),
      ),
    );
    expect(responses.map((response) => response.status)).toEqual([200, 200, 200]);
    const active = (where: string) =>
      context.sqlite
        .prepare(`SELECT COUNT(*) AS n FROM machines WHERE status = 'active' ${where}`)
        .get();
    expect(active("")).toEqual({ n: 10 });
    expect(active("AND name = 'laptop'")).toEqual({ n: 1 });
    expect(context.sqlite.prepare("SELECT COUNT(*) AS n FROM tokens").get()).toEqual({ n: 10 });
  });

  it("replaces an active machine with the same name, moves its URLs and releases its tunnel", async () => {
    const old = await signIn(context, "teo-goulois", 1, "laptop");
    const { body } = await createUrl(old.token, { localDomain: "a.localify" });
    await serve(context, old.token);
    for (let i = 0; i < 9; i++) await signIn(context, "teo-goulois", 1, `machine-${i}`);

    // Ten machines, but the same name replaces one instead of counting.
    fakeSignIn(context, "teo-goulois", 1, "tunnel-new");
    context.calls.length = 0;
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
    });
    expect(response.status).toBe(200);
    const fresh = (await response.json()) as { token: string; machine: { id: string } };

    expect(machineRow(old.machineId)).toMatchObject({
      status: "revoked",
      tunnel_id: null,
      dns_record_id: null,
    });
    expect(cloudflareCalls()).toEqual([
      "PATCH /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /zones/zone-1/dns_records/dns-tunnel-laptop",
    ]);
    expect(
      context.sqlite.prepare("SELECT tunnel_state FROM machines WHERE id = ?").get(old.machineId),
    ).toEqual({ tunnel_state: "none" });
    expect((await api(context, "GET", "/v1/me", { token: old.token })).status).toBe(401);
    const moved = await api(context, "GET", "/v1/public-urls", { token: fresh.token });
    expect(((await moved.json()) as { publicUrls: PublicUrlBody[] }).publicUrls).toEqual([
      { ...body.publicUrl, machine: { id: fresh.machine.id, name: "laptop" } },
    ]);
  });

  it("keeps accepting a token once its GitHub id leaves the allowlist: it only gates sign-in", async () => {
    const { token } = await signIn(context);
    context.env.ALLOWED_GITHUB_IDS = "2";
    expect((await api(context, "GET", "/v1/me", { token })).status).toBe(200);
    expect((await createUrl(token, { localDomain: "journal.localify" })).response.status).toBe(201);
  });
});

describe("public URLs", () => {
  it("creates a protected URL with a generated password shown once", async () => {
    const { token, machineId } = await signIn(context);
    const { response, body } = await createUrl(token, {
      localDomain: "journal.localify",
      wildcard: true,
      protection: { mode: "password" },
    });
    expect(response.status).toBe(201);
    expect(body.publicUrl).toMatchObject({
      localDomain: "journal.localify",
      wildcard: true,
      machine: { id: machineId, name: "laptop" },
    });
    expect(body.publicUrl.protection).toEqual({ mode: "password", version: 1 });
    expect(body.publicUrl.slug).toBe("journal");
    expect(body.publicUrl.url).toBe(`https://${body.publicUrl.slug}.localify.fyi`);
    expect(new Date(body.publicUrl.createdAt).toISOString()).toBe(body.publicUrl.createdAt);
    const words = body.password!.split("-");
    expect(words).toHaveLength(4);
    for (const word of words) expect(WORDS).toContain(word);

    const list = await api(context, "GET", "/v1/public-urls", { token });
    expect(((await list.json()) as { publicUrls: PublicUrlBody[] }).publicUrls).toEqual([
      body.publicUrl,
    ]);
  });

  it("protects a URL by default and never returns a chosen password", async () => {
    const { token } = await signIn(context);
    const defaulted = await createUrl(token, { localDomain: "a.localify" });
    expect(defaulted.body.publicUrl.protection.mode).toBe("password");
    expect(defaulted.body.password).toBeDefined();

    const chosen = await createUrl(token, {
      localDomain: "b.localify",
      slug: "my-site",
      protection: { mode: "password", password: "a long password" },
    });
    expect(chosen.response.status).toBe(201);
    expect(chosen.body.publicUrl.slug).toBe("my-site");
    expect(chosen.body.password).toBeUndefined();

    const open = await createUrl(token, {
      localDomain: "c.localify",
      protection: { mode: "public" },
    });
    expect(open.body.publicUrl.protection.mode).toBe("public");
    expect(open.body.password).toBeUndefined();
  });

  it("refuses invalid and taken slugs and domains, a second URL for a domain, and short passwords", async () => {
    const { token } = await signIn(context);
    expect((await createUrl(token, { localDomain: "a.localify", slug: "ab--c" })).body.error).toBe(
      "invalid_slug",
    );
    expect((await createUrl(token, { localDomain: "a.localify", slug: "x" })).response.status).toBe(
      422,
    );
    expect((await createUrl(token, { localDomain: "Not a domain" })).body.error).toBe(
      "invalid_local_domain",
    );

    await createUrl(token, { localDomain: "a.localify", slug: "taken" });
    const other = await signIn(context, "ryan-ede", 2, "ryan");
    const taken = await createUrl(other.token, { localDomain: "b.localify", slug: "taken" });
    expect(taken.response.status).toBe(409);
    expect(taken.body.error).toBe("slug_taken");

    const again = await createUrl(token, { localDomain: "a.localify" });
    expect(again.response.status).toBe(409);
    expect(again.body.error).toBe("local_domain_taken");

    const short = await createUrl(token, {
      localDomain: "c.localify",
      protection: { mode: "password", password: "short" },
    });
    expect(short.response.status).toBe(422);
  });

  it("keeps a user to 10 live URLs", async () => {
    const { token } = await signIn(context);
    const ids: string[] = [];
    for (let i = 0; i < 10; i++) {
      const created = await createUrl(token, { localDomain: `app${i}.localify` });
      expect(created.response.status).toBe(201);
      ids.push(created.body.publicUrl.id);
    }
    const over = await createUrl(token, { localDomain: "app10.localify" });
    expect(over.response.status).toBe(403);
    expect(over.body.error).toBe("quota_exceeded");

    await api(context, "DELETE", `/v1/public-urls/${ids[0]}`, { token });
    expect((await createUrl(token, { localDomain: "app10.localify" })).response.status).toBe(201);
  });

  it("answers 404 for someone else's URL and for an unknown id alike", async () => {
    const { token } = await signIn(context);
    const { body } = await createUrl(token, { localDomain: "a.localify" });
    const other = await signIn(context, "ryan-ede", 2, "ryan");
    const list = await api(context, "GET", "/v1/public-urls", { token: other.token });
    expect(((await list.json()) as { publicUrls: unknown[] }).publicUrls).toEqual([]);
    for (const id of [body.publicUrl.id, "unknown"]) {
      for (const [method, path, payload] of [
        ["DELETE", `/v1/public-urls/${id}`, undefined],
        ["POST", `/v1/public-urls/${id}/claim`, undefined],
        ["PUT", `/v1/public-urls/${id}/protection`, { protection: { mode: "public" } }],
      ] as const) {
        const response = await api(context, method, path, { token: other.token, body: payload });
        expect(response.status, `${method} ${path}`).toBe(404);
        expect(await errorCode(response)).toBe("not_found");
      }
    }
    const row = context.sqlite
      .prepare("SELECT protected, deleted_at FROM public_urls WHERE id = ?")
      .get(body.publicUrl.id);
    expect(row).toEqual({ protected: 1, deleted_at: null });
  });

  it("keeps a deleted URL's row and reserves its slug for its owner", async () => {
    const { token } = await signIn(context);
    const { body } = await createUrl(token, { localDomain: "a.localify", slug: "kept-slug" });
    expect(
      (await api(context, "DELETE", `/v1/public-urls/${body.publicUrl.id}`, { token })).status,
    ).toBe(204);
    expect(
      (await api(context, "DELETE", `/v1/public-urls/${body.publicUrl.id}`, { token })).status,
    ).toBe(404);
    const list = await api(context, "GET", "/v1/public-urls", { token });
    expect(((await list.json()) as { publicUrls: unknown[] }).publicUrls).toEqual([]);
    expect(
      context.sqlite.prepare("SELECT deleted_at IS NOT NULL AS deleted FROM public_urls").all(),
    ).toEqual([{ deleted: 1 }]);

    const other = await signIn(context, "ryan-ede", 2, "ryan");
    expect(
      (await createUrl(other.token, { localDomain: "b.localify", slug: "kept-slug" })).body.error,
    ).toBe("slug_taken");

    const back = await createUrl(token, { localDomain: "b.localify", slug: "kept-slug" });
    expect(back.response.status).toBe(201);
    expect(context.sqlite.prepare("SELECT COUNT(*) AS n FROM public_urls").get()).toEqual({ n: 1 });
  });

  it("names a new URL after its local domain, then after its owner", async () => {
    const teo = await signIn(context);
    const ryan = await signIn(context, "ryan-ede", 2, "ryan");
    const first = await createUrl(teo.token, { localDomain: "louez.localify" });
    expect(first.body.publicUrl.slug).toBe("louez");
    const second = await createUrl(ryan.token, { localDomain: "louez.localify" });
    expect(second.body.publicUrl.slug).toBe("louez-ryan-ede");
    const nested = await createUrl(teo.token, {
      localDomain: "louez-customer-profiles.louez.localify",
    });
    expect(nested.body.publicUrl.slug).toBe("louez-customer-profiles-louez");
  });

  it("lets a deleted URL's local domain have a new URL", async () => {
    const { token } = await signIn(context);
    const { body } = await createUrl(token, { localDomain: "a.localify" });
    await api(context, "DELETE", `/v1/public-urls/${body.publicUrl.id}`, { token });
    expect((await createUrl(token, { localDomain: "a.localify" })).response.status).toBe(201);
  });

  it("bumps the password version on every protection change", async () => {
    const { token } = await signIn(context);
    const { body } = await createUrl(token, { localDomain: "a.localify" });
    const id = body.publicUrl.id;
    const version = () =>
      (
        context.sqlite
          .prepare("SELECT password_version AS v FROM public_urls WHERE id = ?")
          .get(id) as { v: number }
      ).v;
    expect(version()).toBe(1);

    const rotated = await api(context, "PUT", `/v1/public-urls/${id}/protection`, {
      token,
      body: { protection: { mode: "password" } },
    });
    const rotatedBody = (await rotated.json()) as { publicUrl: PublicUrlBody; password?: string };
    expect(rotatedBody.password).toBeDefined();
    expect(rotatedBody.publicUrl.protection).toEqual({ mode: "password", version: 2 });
    expect(version()).toBe(2);

    const opened = await api(context, "PUT", `/v1/public-urls/${id}/protection`, {
      token,
      body: { protection: { mode: "public" } },
    });
    const openedBody = (await opened.json()) as { publicUrl: PublicUrlBody; password?: string };
    expect(openedBody.publicUrl.protection).toEqual({ mode: "public", version: 3 });
    expect(openedBody.password).toBeUndefined();
    expect(version()).toBe(3);
    const row = context.sqlite
      .prepare("SELECT password_hash, password_salt FROM public_urls WHERE id = ?")
      .get(id);
    expect(row).toEqual({ password_hash: null, password_salt: null });

    const chosen = await api(context, "PUT", `/v1/public-urls/${id}/protection`, {
      token,
      body: { protection: { mode: "password", password: "chosen password" } },
    });
    const chosenBody = (await chosen.json()) as { publicUrl: PublicUrlBody; password?: string };
    expect(chosenBody.password).toBeUndefined();
    expect(chosenBody.publicUrl.protection).toEqual({ mode: "password", version: 4 });
    expect(version()).toBe(4);

    expect(
      (await api(context, "PUT", `/v1/public-urls/${id}/protection`, { token, body: {} })).status,
    ).toBe(422);
  });

  it("has a schema that refuses a protected URL without a password hash", async () => {
    const { token } = await signIn(context);
    const { body } = await createUrl(token, { localDomain: "a.localify" });
    expect(() =>
      context.sqlite
        .prepare("UPDATE public_urls SET password_hash = NULL WHERE id = ?")
        .run(body.publicUrl.id),
    ).toThrow(/CHECK constraint failed/);
  });

  it("moves a URL to the machine that claims it", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const { body } = await createUrl(laptop.token, { localDomain: "a.localify" });
    const claimed = await api(context, "POST", `/v1/public-urls/${body.publicUrl.id}/claim`, {
      token: runner.token,
    });
    expect(((await claimed.json()) as { publicUrl: PublicUrlBody }).publicUrl.machine).toEqual({
      id: runner.machineId,
      name: "runner",
    });
  });
});

describe("slugs", () => {
  async function suggestion(token: string, localDomain: string) {
    const query = new URLSearchParams({ localDomain });
    const response = await api(context, "GET", `/v1/slugs?${query}`, { token });
    return {
      status: response.status,
      body: (await response.json()) as { slug?: string; url?: string },
    };
  }

  async function check(token: string, slug: string) {
    const response = await api(context, "GET", `/v1/slugs/${encodeURIComponent(slug)}`, { token });
    return {
      status: response.status,
      body: (await response.json()) as { slug?: string; available?: boolean; error?: string },
    };
  }

  it("suggests the local name, then with the login, then random characters", async () => {
    const teo = await signIn(context);
    const ryan = await signIn(context, "ryan-ede", 2, "ryan");
    expect((await suggestion(teo.token, "louez.localify")).body).toEqual({
      slug: "louez",
      url: "https://louez.localify.fyi",
    });
    expect((await suggestion(teo.token, "web.louez.localify")).body.slug).toBe("web-louez");

    await createUrl(teo.token, { localDomain: "louez.localify" });
    expect((await suggestion(ryan.token, "louez.localify")).body.slug).toBe("louez-ryan-ede");
    await createUrl(ryan.token, { localDomain: "other.localify", slug: "louez-ryan-ede" });
    expect((await suggestion(ryan.token, "louez.localify")).body.slug).toMatch(
      /^louez-[0-9a-f]{4}$/,
    );

    const invalid = await suggestion(teo.token, "Not a domain");
    expect(invalid.status).toBe(422);
    const missing = await api(context, "GET", "/v1/slugs", { token: teo.token });
    expect(await errorCode(missing)).toBe("invalid_local_domain");
    expect((await api(context, "GET", "/v1/slugs?localDomain=a.localify")).status).toBe(
      401,
    );
  });

  it("says whether a slug is free, keeping a deleted one for its owner", async () => {
    const teo = await signIn(context);
    const ryan = await signIn(context, "ryan-ede", 2, "ryan");
    expect((await check(teo.token, "Free-One")).body).toEqual({ slug: "free-one", available: true });

    const { body } = await createUrl(teo.token, { localDomain: "a.localify", slug: "kept" });
    expect((await check(teo.token, "kept")).body.available).toBe(false);
    expect((await check(ryan.token, "kept")).body.available).toBe(false);

    await api(context, "DELETE", `/v1/public-urls/${body.publicUrl.id}`, { token: teo.token });
    expect((await check(teo.token, "kept")).body.available).toBe(true);
    expect((await check(ryan.token, "kept")).body.available).toBe(false);
    expect((await suggestion(ryan.token, "kept.localify")).body.slug).toBe("kept-ryan-ede");

    for (const slug of ["ab--c", "x", "-abc", "%E0%A4%A"]) {
      const invalid = await check(teo.token, slug);
      expect(invalid.status, slug).toBe(422);
      expect(invalid.body.error, slug).toBe("invalid_slug");
    }
  });
});
describe("signing out", () => {
  it("revokes in D1, then removes connections, tunnel and DNS record", async () => {
    const { token, machineId } = await signIn(context);
    const { body } = await createUrl(token, { localDomain: "a.localify" });
    await serve(context, token);
    context.calls.length = 0;

    const response = await api(context, "POST", "/v1/auth/logout", { token });
    expect(response.status).toBe(204);
    expect(cloudflareCalls()).toEqual([
      "PATCH /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /zones/zone-1/dns_records/dns-tunnel-laptop",
    ]);
    expect((await api(context, "GET", "/v1/me", { token })).status).toBe(401);
    expect(machineRow(machineId)).toMatchObject({
      status: "revoked",
      tunnel_id: null,
      dns_record_id: null,
    });
    expect(
      context.sqlite.prepare("SELECT tunnel_state FROM machines WHERE id = ?").get(machineId),
    ).toEqual({ tunnel_state: "none" });
    expect(
      context.sqlite
        .prepare("SELECT machine_id FROM public_urls WHERE id = ?")
        .get(body.publicUrl.id),
    ).toEqual({
      machine_id: null,
    });
  });

  it("makes no Cloudflare call for a machine that never served", async () => {
    const { token } = await signIn(context);
    context.calls.length = 0;
    expect((await api(context, "POST", "/v1/auth/logout", { token })).status).toBe(204);
    expect(cloudflareCalls()).toEqual([]);
  });

  it("succeeds when Cloudflare fails, holding the slot until the cleanup removes the tunnel", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, { localDomain: "a.localify" });
    await serve(context, token);
    context.routes.clear();
    context.routes.set(`PATCH ${CF}/accounts/`, () => cloudflareOk({}));
    context.routes.set(`DELETE ${CF}/accounts/`, () =>
      jsonResponse({ success: false, errors: [] }, 500),
    );
    context.routes.set(`DELETE ${CF}/zones/`, () => cloudflareOk(null));

    expect((await api(context, "POST", "/v1/auth/logout", { token })).status).toBe(204);
    expect((await api(context, "GET", "/v1/me", { token })).status).toBe(401);
    expect(machineRow(machineId)).toMatchObject({
      status: "revoked",
      tunnel_id: "tunnel-laptop",
      dns_record_id: null,
    });
    expect(
      context.sqlite.prepare("SELECT tunnel_state FROM machines WHERE id = ?").get(machineId),
    ).toEqual({ tunnel_state: "releasing" });

    context.routes.set(`DELETE ${CF}/accounts/`, () => cloudflareOk(null));
    await cleanUpRevokedMachines(context.env, Date.now(), new CallBudget(300));
    expect(machineRow(machineId)).toMatchObject({
      status: "revoked",
      tunnel_id: null,
      dns_record_id: null,
    });
    expect(
      context.sqlite.prepare("SELECT tunnel_state FROM machines WHERE id = ?").get(machineId),
    ).toEqual({ tunnel_state: "none" });
  });

  it("treats an already deleted tunnel as gone", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, { localDomain: "a.localify" });
    await serve(context, token);
    context.routes.clear();
    context.routes.set(`PATCH ${CF}/`, () => jsonResponse({ success: false, errors: [] }, 404));
    context.routes.set(`DELETE ${CF}/`, () => jsonResponse({ success: false, errors: [] }, 404));
    expect((await api(context, "POST", "/v1/auth/logout", { token })).status).toBe(204);
    expect(machineRow(machineId)).toMatchObject({ tunnel_id: null, dns_record_id: null });
  });
});

describe("revoked machines' cleanup", () => {
  it("revokes machines stuck in provisioning, finding unrecorded resources by name", async () => {
    const { machineId } = await signIn(context);
    const { user_id } = context.sqlite
      .prepare("SELECT user_id FROM machines WHERE id = ?")
      .get(machineId) as {
      user_id: string;
    };
    const insert = context.sqlite.prepare(
      `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at) VALUES (?, ?, ?, 'provisioning', 7381, ?)`,
    );
    insert.run("stuck", user_id, "stuck", Date.now() - 11 * 60 * 1000);
    insert.run("young", user_id, "young", Date.now());
    context.routes.set(`GET ${CF}/accounts/account-1/cfd_tunnel?`, () =>
      cloudflareOk([{ id: "tunnel-stuck" }]),
    );
    context.routes.set(`GET ${CF}/zones/zone-1/dns_records?`, () => cloudflareOk([]));
    context.calls.length = 0;

    await revokeStuckSignIns(context.env, Date.now());
    // It may hold a tunnel, so it holds a slot until the cleanup.
    expect(
      context.sqlite.prepare("SELECT tunnel_state FROM machines WHERE id = 'stuck'").get(),
    ).toEqual({ tunnel_state: "releasing" });
    await cleanUpRevokedMachines(context.env, Date.now(), new CallBudget(300));
    expect(cloudflareCalls()).toEqual([
      "GET /accounts/account-1/cfd_tunnel?name=localify-stuck&is_deleted=false",
      "GET /zones/zone-1/dns_records?type=CNAME&name=m-stuck.localify.dev",
      "PATCH /accounts/account-1/cfd_tunnel/tunnel-stuck",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-stuck/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-stuck",
    ]);
    expect(machineRow("stuck")).toMatchObject({
      status: "revoked",
      tunnel_id: null,
      dns_record_id: null,
    });
    expect(machineRow("young")).toMatchObject({ status: "provisioning" });
  });

  it("finds by name what a failed sign-in created without recording it", async () => {
    const { machineId } = await signIn(context);
    const { user_id } = cleanupRow(machineId) as { user_id: string };
    context.sqlite
      .prepare(
        `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at, revoked_at, reconcile)
         VALUES ('failed', ?, 'failed', 'revoked', 7381, ?, ?, 1)`,
      )
      .run(user_id, Date.now(), Date.now());
    context.routes.set(`GET ${CF}/accounts/account-1/cfd_tunnel?`, () =>
      cloudflareOk([{ id: "tunnel-failed" }]),
    );
    context.routes.set(`GET ${CF}/zones/zone-1/dns_records?`, () =>
      cloudflareOk([{ id: "record-failed" }]),
    );
    context.calls.length = 0;

    await cleanUpRevokedMachines(context.env, Date.now(), new CallBudget(300));
    expect(cloudflareCalls()).toEqual([
      "GET /accounts/account-1/cfd_tunnel?name=localify-failed&is_deleted=false",
      "GET /zones/zone-1/dns_records?type=CNAME&name=m-failed.localify.dev",
      "PATCH /accounts/account-1/cfd_tunnel/tunnel-failed",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-failed/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-failed",
      "DELETE /zones/zone-1/dns_records/record-failed",
    ]);
    expect(cleanupRow("failed")).toMatchObject({
      reconcile: 0,
      tunnel_id: null,
      dns_record_id: null,
      next_cleanup_at: null,
    });
  });

  it("keeps machines whose cleanup fails from holding back the others", async () => {
    const { machineId } = await signIn(context);
    const { user_id } = cleanupRow(machineId) as { user_id: string };
    const insert = context.sqlite.prepare(
      `INSERT INTO machines (id, user_id, name, status, tunnel_id, ingress_port, created_at, revoked_at)
       VALUES (?, ?, ?, 'revoked', ?, 7381, ?, ?)`,
    );
    const old = Date.now() - 60 * 60 * 1000;
    for (let i = 0; i < 9; i++) {
      insert.run(`stale-${i}`, user_id, `stale-${i}`, `tunnel-stale-${i}`, old, old + i);
    }
    context.routes.clear();
    context.routes.set(`PATCH ${CF}/accounts/`, () => cloudflareOk({}));
    context.routes.set(`DELETE ${CF}/accounts/account-1/cfd_tunnel/tunnel-stale`, () =>
      jsonResponse({ success: false, errors: [] }, 500),
    );
    context.routes.set(`DELETE ${CF}/accounts/`, () => cloudflareOk(null));

    const now = Date.now();
    await cleanUpRevokedMachines(context.env, now, new CallBudget(40));
    // A machine may cost 6 calls, so a budget of 40 starts 6 machines: the 7th waits.
    expect(cleanupRow("stale-5")).toMatchObject({ cleanup_attempts: 1 });
    expect(cleanupRow("stale-6")).toMatchObject({ cleanup_attempts: 0, next_cleanup_at: null });

    insert.run("fresh", user_id, "fresh", "tunnel-fresh", now, now);
    await cleanUpRevokedMachines(context.env, now + 60 * 1000, new CallBudget(40));
    expect(cleanupRow("stale-8")).toMatchObject({ cleanup_attempts: 1 });
    expect(cleanupRow("fresh")).toMatchObject({ tunnel_id: null, cleanup_attempts: 0 });
    expect(cleanupRow("stale-0")).toMatchObject({ cleanup_attempts: 1 });
  });

  it("purges URL rows deleted more than 30 days ago", async () => {
    const { token } = await signIn(context);
    const recent = await createUrl(token, { localDomain: "a.localify" });
    const old = await createUrl(token, { localDomain: "b.localify" });
    await api(context, "DELETE", `/v1/public-urls/${recent.body.publicUrl.id}`, { token });
    await api(context, "DELETE", `/v1/public-urls/${old.body.publicUrl.id}`, { token });
    context.sqlite
      .prepare("UPDATE public_urls SET deleted_at = ? WHERE id = ?")
      .run(Date.now() - 31 * 24 * 60 * 60 * 1000, old.body.publicUrl.id);

    await purgeDeletedUrls(context.env);
    expect(context.sqlite.prepare("SELECT id FROM public_urls").all()).toEqual([
      { id: recent.body.publicUrl.id },
    ]);
  });
});
