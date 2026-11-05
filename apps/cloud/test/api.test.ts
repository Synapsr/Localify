import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { hmacHex, sha256Hex } from "../src/crypto";
import { runCleanup } from "../src/machines";
import { WORDS } from "../src/passwords";
import {
  api,
  cloudflareOk,
  createContext,
  fakeSignIn,
  jsonResponse,
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
  protection: { mode: string };
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

  it("checks the GitHub id, not the login, before touching Cloudflare", async () => {
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
  it("registers the machine: tunnel, ingress, CNAME, then an active machine and a hashed token", async () => {
    const { token, machineId } = await signIn(context, "Teo-Goulois", 1, "laptop");
    expect(token).toMatch(/^lfy_[0-9a-f]{64}$/);

    const github = context.calls.find((call) => call.url === "https://api.github.com/user")!;
    expect(github.headers.get("user-agent")).toBeTruthy();
    expect(github.headers.get("authorization")).toBe("Bearer gho_Teo-Goulois");

    const create = context.calls.find(
      (call) => call.method === "POST" && call.url.endsWith("/cfd_tunnel"),
    )!;
    expect(JSON.parse(create.body!)).toEqual({
      name: `localify-${machineId}`,
      config_src: "cloudflare",
    });
    expect(create.headers.get("authorization")).toBe("Bearer cf-token");

    const configure = context.calls.find((call) => call.method === "PUT")!;
    expect(configure.url).toBe(`${CF}/accounts/account-1/cfd_tunnel/tunnel-laptop/configurations`);
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

    expect(machineRow(machineId)).toEqual({
      status: "active",
      tunnel_id: "tunnel-laptop",
      dns_record_id: "dns-tunnel-laptop",
      revoked_at: null,
    });
    expect(context.sqlite.prepare("SELECT hash FROM tokens").all()).toEqual([
      { hash: await sha256Hex(token) },
    ]);

    const me = await api(context, "GET", "/v1/me", { token });
    expect(await me.json()).toEqual({
      user: { login: "Teo-Goulois" },
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

  it("answers 502 provisioning_failed and removes what it created when a step fails", async () => {
    fakeSignIn(context, "teo-goulois", 1, "tunnel-x");
    context.routes.set(`POST ${CF}/zones/zone-1/dns_records`, () =>
      jsonResponse({ success: false, errors: [{ code: 81053, message: "record exists" }] }, 400),
    );
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "laptop", ingressPort: 7381 },
    });
    expect(response.status).toBe(502);
    expect(await errorCode(response)).toBe("provisioning_failed");
    expect(cloudflareCalls().slice(-2)).toEqual([
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-x/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-x",
    ]);
    const machines = context.sqlite
      .prepare("SELECT status, tunnel_id, dns_record_id FROM machines")
      .all();
    expect(machines).toEqual([{ status: "revoked", tunnel_id: null, dns_record_id: null }]);
    expect(context.sqlite.prepare("SELECT COUNT(*) AS n FROM tokens").get()).toEqual({ n: 0 });
  });

  it("keeps a user to 5 active machines", async () => {
    for (let i = 0; i < 5; i++) await signIn(context, "teo-goulois", 1, `machine-${i}`);
    fakeSignIn(context, "teo-goulois", 1, "tunnel-6");
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "d", machineName: "machine-6", ingressPort: 7381 },
    });
    expect(response.status).toBe(403);
    expect(await errorCode(response)).toBe("quota_exceeded");
  });

  it("replaces an active machine with the same name and moves its URLs", async () => {
    const old = await signIn(context, "teo-goulois", 1, "laptop");
    const { body } = await createUrl(old.token, { localDomain: "a.localify" });
    for (let i = 0; i < 4; i++) await signIn(context, "teo-goulois", 1, `machine-${i}`);

    // Five machines, but the same name replaces one instead of counting.
    fakeSignIn(context, "teo-goulois", 1, "tunnel-new");
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
    expect(cloudflareCalls().slice(-3)).toEqual([
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop/connections",
      "DELETE /accounts/account-1/cfd_tunnel/tunnel-laptop",
      "DELETE /zones/zone-1/dns_records/dns-tunnel-laptop",
    ]);
    expect((await api(context, "GET", "/v1/me", { token: old.token })).status).toBe(401);
    const moved = await api(context, "GET", "/v1/public-urls", { token: fresh.token });
    expect(((await moved.json()) as { publicUrls: PublicUrlBody[] }).publicUrls).toEqual([
      { ...body.publicUrl, machine: { id: fresh.machine.id, name: "laptop" } },
    ]);
  });

  it("stops accepting a token once its GitHub id leaves the allowlist", async () => {
    const { token } = await signIn(context);
    context.env.ALLOWED_GITHUB_IDS = "2";
    const response = await api(context, "GET", "/v1/me", { token });
    expect(response.status).toBe(403);
    expect(await errorCode(response)).toBe("not_allowed");
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
      protection: { mode: "password" },
    });
    expect(Object.keys(body.publicUrl.protection)).toEqual(["mode"]);
    expect(body.publicUrl.slug).toMatch(/^journal-[0-9a-f]{4}$/);
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
    expect(((await rotated.json()) as { password?: string }).password).toBeDefined();
    expect(version()).toBe(2);

    const opened = await api(context, "PUT", `/v1/public-urls/${id}/protection`, {
      token,
      body: { protection: { mode: "public" } },
    });
    const openedBody = (await opened.json()) as { publicUrl: PublicUrlBody; password?: string };
    expect(openedBody.publicUrl.protection).toEqual({ mode: "public" });
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
    expect(((await chosen.json()) as { password?: string }).password).toBeUndefined();
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

describe("machine runtime", () => {
  it("returns the tunnel token, the ingress secret and the live URLs this machine serves", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const served = await createUrl(runner.token, { localDomain: "a.localify" });
    const deleted = await createUrl(runner.token, { localDomain: "c.localify" });
    await api(context, "DELETE", `/v1/public-urls/${deleted.body.publicUrl.id}`, {
      token: runner.token,
    });
    await createUrl(laptop.token, { localDomain: "b.localify" });

    const response = await api(context, "GET", "/v1/machines/self/runtime", {
      token: runner.token,
    });
    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      tunnelToken: string;
      ingressSecret: string;
      publicUrls: PublicUrlBody[];
    };
    expect(body.tunnelToken).toBe("token-of-tunnel-runner");
    expect(body.ingressSecret).toBe(await hmacHex("ingress-key", runner.machineId));
    expect(body.publicUrls.map((url) => url.id)).toEqual([served.body.publicUrl.id]);
    expect(cloudflareCalls().at(-1)).toBe("GET /accounts/account-1/cfd_tunnel/tunnel-runner/token");
  });

  it("answers 502 when Cloudflare fails", async () => {
    const { token } = await signIn(context);
    context.routes.clear();
    context.routes.set(`GET ${CF}/`, () => jsonResponse({ success: false, errors: [] }, 500));
    const response = await api(context, "GET", "/v1/machines/self/runtime", { token });
    expect(response.status).toBe(502);
    expect(await errorCode(response)).toBe("cloudflare_unavailable");
  });
});

describe("signing out", () => {
  it("revokes in D1, then removes connections, tunnel and DNS record", async () => {
    const { token, machineId } = await signIn(context);
    const { body } = await createUrl(token, { localDomain: "a.localify" });
    context.calls.length = 0;

    const response = await api(context, "POST", "/v1/auth/logout", { token });
    expect(response.status).toBe(204);
    expect(cloudflareCalls()).toEqual([
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
      context.sqlite
        .prepare("SELECT machine_id FROM public_urls WHERE id = ?")
        .get(body.publicUrl.id),
    ).toEqual({
      machine_id: null,
    });
  });

  it("succeeds when Cloudflare fails, and the hourly cleanup retries what is left", async () => {
    const { token, machineId } = await signIn(context);
    context.routes.clear();
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

    context.routes.set(`DELETE ${CF}/accounts/`, () => cloudflareOk(null));
    await runCleanup(context.env);
    expect(machineRow(machineId)).toMatchObject({
      status: "revoked",
      tunnel_id: null,
      dns_record_id: null,
    });
  });

  it("treats an already deleted tunnel as gone", async () => {
    const { token, machineId } = await signIn(context);
    context.routes.clear();
    context.routes.set(`DELETE ${CF}/`, () => jsonResponse({ success: false, errors: [] }, 404));
    expect((await api(context, "POST", "/v1/auth/logout", { token })).status).toBe(204);
    expect(machineRow(machineId)).toMatchObject({ tunnel_id: null, dns_record_id: null });
  });
});

describe("hourly cleanup", () => {
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

    await runCleanup(context.env);
    expect(cloudflareCalls()).toEqual([
      "GET /accounts/account-1/cfd_tunnel?name=localify-stuck&is_deleted=false",
      "GET /zones/zone-1/dns_records?type=CNAME&name=m-stuck.localify.dev",
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

  it("purges URL rows deleted more than 30 days ago", async () => {
    const { token } = await signIn(context);
    const recent = await createUrl(token, { localDomain: "a.localify" });
    const old = await createUrl(token, { localDomain: "b.localify" });
    await api(context, "DELETE", `/v1/public-urls/${recent.body.publicUrl.id}`, { token });
    await api(context, "DELETE", `/v1/public-urls/${old.body.publicUrl.id}`, { token });
    context.sqlite
      .prepare("UPDATE public_urls SET deleted_at = ? WHERE id = ?")
      .run(Date.now() - 31 * 24 * 60 * 60 * 1000, old.body.publicUrl.id);

    await runCleanup(context.env);
    expect(context.sqlite.prepare("SELECT id FROM public_urls").all()).toEqual([
      { id: recent.body.publicUrl.id },
    ]);
  });
});
