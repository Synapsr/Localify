// Plans, sign-up, bans, takedowns and downgrades, through the API, the
// runtime and the public path (docs/public-urls.md, "Plans" and "API").

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { purgeDeletedUrls } from "../src/machines";
import { QUIET_AFTER_STOP_MS, runSweep } from "../src/tunnels";
import {
  api,
  createContext,
  endQuietPeriods,
  fakeSignIn,
  serve,
  settle,
  signIn,
  visit,
  type TestContext,
} from "./support";

let context: TestContext;
const DAY = 24 * 60 * 60 * 1000;

beforeEach(() => {
  context = createContext();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function sql(statement: string, ...params: (string | number | null)[]): void {
  context.sqlite.prepare(statement).run(...params);
}

function removeOverride(userId: string): void {
  sql("DELETE FROM plan_overrides WHERE user_id = ?", userId);
}

function setOverride(userId: string, machines: number, urls: number): void {
  sql(
    "UPDATE plan_overrides SET serving_machines = ?, public_urls = ? WHERE user_id = ?",
    machines,
    urls,
    userId,
  );
}

function setBilling(userId: string, fields: Record<string, string | number | null>): void {
  const columns = Object.keys(fields);
  sql(
    `INSERT INTO billing (user_id, revision, refreshed_at${columns.map((c) => `, ${c}`).join("")})
     VALUES (?, 1, 1${columns.map(() => ", ?").join("")})`,
    userId,
    ...Object.values(fields),
  );
}

let slugCount = 0;
/** A slug no other test used, since the public path's cache outlives a test. */
function freshSlug(): string {
  return `plan-${Date.now().toString(36)}-${slugCount++}`;
}

async function createUrl(token: string, localDomain: string, slug = freshSlug()) {
  const response = await api(context, "POST", "/v1/public-urls", {
    token,
    body: { localDomain, slug, protection: { mode: "public" } },
  });
  const body = (await response.json()) as {
    publicUrl: { id: string; slug: string; state: string };
    error?: string;
  };
  // Two creates can share a millisecond: give each its own, in order.
  if (response.status === 201) {
    sql("UPDATE public_urls SET created_at = ? WHERE id = ?", slugCount, body.publicUrl.id);
  }
  return { status: response.status, body, error: body.error };
}

async function me(token: string) {
  const response = await api(context, "GET", "/v1/me", { token });
  return { status: response.status, body: (await response.json()) as Record<string, unknown> };
}

async function states(token: string): Promise<Record<string, string>> {
  const response = await api(context, "GET", "/v1/public-urls", { token });
  const { publicUrls } = (await response.json()) as {
    publicUrls: { slug: string; state: string }[];
  };
  return Object.fromEntries(publicUrls.map((url) => [url.slug, url.state]));
}

async function errorOf(response: Response): Promise<string> {
  return ((await response.json()) as { error: string }).error;
}

function tunnelState(machineId: string): string {
  return (
    context.sqlite.prepare("SELECT tunnel_state FROM machines WHERE id = ?").get(machineId) as {
      tunnel_state: string;
    }
  ).tunnel_state;
}

describe("/v1/me", () => {
  it("says there is no plan, with zero limits and the trial available", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    expect((await me(token)).body.plan).toEqual({
      status: "none",
      entitled: false,
      interval: null,
      trialEndsAt: null,
      renewsAt: null,
      endsAt: null,
      trialAvailable: true,
      limits: { servingMachines: 0, publicUrls: 0, remoteAccess: false },
      usage: { servingMachines: 0, publicUrls: 0 },
      addons: { machines: 0, urlPacks: 0 },
    });
  });

  it("gives the Pro plan from the billing snapshot, with its add-ons and the usage", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const renews = Date.now() + 30 * DAY;
    setBilling(userId, {
      pro_status: "active",
      pro_interval: "year",
      current_period_end: renews,
      addon_machines: 1,
      addon_url_packs: 1,
    });
    sql("UPDATE users SET trial_used_at = 1 WHERE id = ?", userId);
    const url = await createUrl(token, "journal.localify");
    await createUrl(token, "notes.localify");
    await serve(context, token);
    sql("UPDATE public_urls SET taken_down_at = 1 WHERE id = ?", url.body.publicUrl.id);

    expect((await me(token)).body.plan).toEqual({
      status: "active",
      entitled: true,
      interval: "year",
      trialEndsAt: null,
      renewsAt: new Date(renews).toISOString(),
      endsAt: null,
      trialAvailable: false,
      limits: { servingMachines: 3, publicUrls: 20, remoteAccess: true },
      // A taken-down URL is not live.
      usage: { servingMachines: 1, publicUrls: 1 },
      addons: { machines: 1, urlPacks: 1 },
    });
  });

  it("reports an override as such", async () => {
    const { token } = await signIn(context);
    expect((await me(token)).body.plan).toMatchObject({
      status: "override",
      entitled: true,
      limits: { servingMachines: 5, publicUrls: 10, remoteAccess: true },
      endsAt: null,
    });
  });
});

describe("without an entitlement", () => {
  it("refuses create and claim with 402, and keeps listing, protection, removal and sign-out", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const { body } = await createUrl(laptop.token, "journal.localify");
    const id = body.publicUrl.id;
    removeOverride(laptop.userId);

    const created = await createUrl(laptop.token, "notes.localify");
    expect(created.status).toBe(402);
    expect(created.error).toBe("subscription_required");
    const claim = await api(context, "POST", `/v1/public-urls/${id}/claim`, {
      token: runner.token,
    });
    expect(claim.status).toBe(402);
    expect(await errorOf(claim)).toBe("subscription_required");

    expect(await states(laptop.token)).toEqual({ [body.publicUrl.slug]: "suspended" });
    const protect = await api(context, "PUT", `/v1/public-urls/${id}/protection`, {
      token: laptop.token,
      body: { protection: { mode: "password" } },
    });
    expect(protect.status).toBe(200);
    expect((await me(laptop.token)).status).toBe(200);
    expect(
      (await api(context, "DELETE", `/v1/public-urls/${id}`, { token: laptop.token })).status,
    ).toBe(204);
    expect((await api(context, "POST", "/v1/auth/logout", { token: runner.token })).status).toBe(
      204,
    );
  });

  it("serves a 402 paused page on the public path", async () => {
    const { token, userId } = await signIn(context);
    const { body } = await createUrl(token, "journal.localify");
    await serve(context, token);
    removeOverride(userId);
    const response = await visit(context, `https://${body.publicUrl.slug}.localify.fyi/`);
    expect(response.status).toBe(402);
    expect(await response.text()).toContain("paused");
  });
});

describe("limits", () => {
  it("keeps the URLs within the plan's count, suspended ones included and taken-down ones not", async () => {
    const { token, userId } = await signIn(context);
    setOverride(userId, 1, 2);
    const first = await createUrl(token, "a.localify");
    await createUrl(token, "b.localify");
    const third = await createUrl(token, "c.localify");
    expect(third.status).toBe(403);
    expect(third.error).toBe("quota_exceeded");
    expect((third.body as unknown as { message: string }).message).toContain("2 public URLs");

    // A downgrade below the count keeps refusing, with suspended URLs counted.
    setOverride(userId, 1, 1);
    expect((await createUrl(token, "c.localify")).error).toBe("quota_exceeded");
    sql("UPDATE public_urls SET taken_down_at = 1 WHERE id = ?", first.body.publicUrl.id);
    setOverride(userId, 1, 2);
    expect((await createUrl(token, "c.localify")).status).toBe(201);
  });

  it("serves the oldest URLs within the limit, and the next one once an older one is removed", async () => {
    const { token, userId } = await signIn(context);
    const a = await createUrl(token, "a.localify");
    const b = await createUrl(token, "b.localify");
    const c = await createUrl(token, "c.localify");
    setOverride(userId, 1, 2);
    const slugs = [a, b, c].map((url) => url.body.publicUrl.slug);
    expect(await states(token)).toEqual({
      [slugs[0]!]: "serving",
      [slugs[1]!]: "serving",
      [slugs[2]!]: "suspended",
    });
    expect((await visit(context, `https://${slugs[2]}.localify.fyi/`)).status).toBe(402);

    await api(context, "DELETE", `/v1/public-urls/${a.body.publicUrl.id}`, { token });
    expect(await states(token)).toEqual({ [slugs[1]!]: "serving", [slugs[2]!]: "serving" });
  });

  it("gives Pro 2 serving machines, and blocks a third one", async () => {
    const first = await signIn(context, "teo-goulois", 1, "one", { entitled: false });
    setBilling(first.userId, { pro_status: "trialing", trial_end: Date.now() + DAY });
    for (const name of ["one", "two", "three"]) {
      const machine =
        name === "one" ? first : await signIn(context, "teo-goulois", 1, name, { entitled: false });
      await createUrl(machine.token, `${name}.localify`);
      const runtime = await serve(context, machine.token);
      expect(runtime.body.state, name).toBe(name === "three" ? "blocked" : "provisioning");
    }
  });
});

describe("SIGNUP", () => {
  async function deviceToken(machineName = "laptop") {
    return api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "device-1", machineName, ingressPort: 7381 },
    });
  }

  it("under open, lets anyone sign in, without an entitlement", async () => {
    context.env.SIGNUP = "open";
    const { token } = await signIn(context, "stranger", 9, "laptop", { entitled: false });
    expect((await me(token)).body.plan).toMatchObject({ status: "none", entitled: false });
    expect((await createUrl(token, "journal.localify")).status).toBe(402);
  });

  it("back to allowlist, refuses new sign-ins but keeps signed-in accounts and their plans", async () => {
    context.env.SIGNUP = "open";
    const { token, machineId } = await signIn(context, "stranger", 9, "laptop");
    const { body } = await createUrl(token, "journal.localify");
    await serve(context, token);
    context.routes.set(`GET https://m-${machineId}.localify.dev/`, () => new Response("app"));
    context.env.SIGNUP = "allowlist";

    expect((await me(token)).status).toBe(200);
    expect((await serve(context, token, 1)).body.state).toBe("ready");
    expect((await visit(context, `https://${body.publicUrl.slug}.localify.fyi/`)).status).toBe(200);
    fakeSignIn(context, "stranger", 9);
    const again = await deviceToken("desktop");
    expect(again.status).toBe(403);
    expect(await errorOf(again)).toBe("not_allowed");
  });

  it("refuses to run on any other value", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    context.env.SIGNUP = "everyone";
    fakeSignIn(context, "teo-goulois", 1);
    const response = await deviceToken();
    expect(response.status).toBe(500);
    expect(await errorOf(response)).toBe("not_configured");
  });
});

describe("TEAM_GITHUB_IDS", () => {
  function overrideOf(githubId: number) {
    return context.sqlite
      .prepare(
        `SELECT serving_machines, public_urls, reason, expires_at FROM plan_overrides
         WHERE user_id = (SELECT id FROM users WHERE github_id = ?)`,
      )
      .get(githubId);
  }

  beforeEach(() => {
    context.env.TEAM_GITHUB_IDS = "1, 3";
  });

  it("gives a team member the team override at their first sign-in", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    expect(overrideOf(1)).toEqual({
      serving_machines: 5,
      public_urls: 10,
      reason: "team",
      expires_at: null,
    });
    expect((await me(token)).body.plan).toMatchObject({
      status: "override",
      entitled: true,
      limits: { servingMachines: 5, publicUrls: 10, remoteAccess: true },
      endsAt: null,
    });
  });

  it("keeps an override an operator changed when the member signs in again", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    sql(
      "UPDATE plan_overrides SET serving_machines = 1, public_urls = 3, reason = 'trial', expires_at = 99 WHERE user_id = ?",
      userId,
    );
    await signIn(context, "teo-goulois", 1, "desktop", { entitled: false });
    expect(overrideOf(1)).toEqual({
      serving_machines: 1,
      public_urls: 3,
      reason: "trial",
      expires_at: 99,
    });
  });

  it("gives nothing to an allowlisted id outside the team", async () => {
    const { token } = await signIn(context, "guest", 2, "laptop", { entitled: false });
    expect(overrideOf(2)).toBeUndefined();
    expect((await me(token)).body.plan).toMatchObject({ status: "none", entitled: false });
  });

  it("gives nothing to a banned team member", async () => {
    context.env.TEAM_GITHUB_IDS = "";
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    sql("UPDATE users SET banned_at = 1, banned_reason = 'spam' WHERE id = ?", userId);
    context.env.TEAM_GITHUB_IDS = "1";
    fakeSignIn(context, "teo-goulois", 1);
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "device-1", machineName: "desktop", ingressPort: 7381 },
    });
    expect(response.status).toBe(403);
    expect(await errorOf(response)).toBe("banned");
    expect(overrideOf(1)).toBeUndefined();
  });
});

describe("bans", () => {
  it("answer 403 banned on every route but sign-out, and keep the tokens and URLs", async () => {
    const { token, userId } = await signIn(context);
    const { body } = await createUrl(token, "journal.localify");
    sql("UPDATE users SET banned_at = 1, banned_reason = 'spam' WHERE id = ?", userId);
    const id = body.publicUrl.id;
    const routes: [string, string, unknown?][] = [
      ["GET", "/v1/me"],
      ["GET", "/v1/public-urls"],
      ["POST", "/v1/public-urls", { localDomain: "notes.localify" }],
      ["DELETE", `/v1/public-urls/${id}`],
      ["PUT", `/v1/public-urls/${id}/protection`, { protection: { mode: "public" } }],
      ["POST", `/v1/public-urls/${id}/claim`],
      ["GET", "/v1/slugs?localDomain=notes.localify"],
      ["GET", "/v2/machines/self/runtime?generation=0"],
      ["GET", "/v1/machines/self/runtime"],
    ];
    for (const [method, path, requestBody] of routes) {
      const response = await api(context, method, path, { token, body: requestBody });
      expect(response.status, `${method} ${path}`).toBe(403);
      expect(await errorOf(response)).toBe("banned");
    }
    expect(context.sqlite.prepare("SELECT COUNT(*) AS n FROM public_urls").get()).toEqual({ n: 1 });

    sql("UPDATE users SET banned_at = NULL WHERE id = ?", userId);
    expect((await me(token)).status).toBe(200);
    sql("UPDATE users SET banned_at = 1 WHERE id = ?", userId);
    expect((await api(context, "POST", "/v1/auth/logout", { token })).status).toBe(204);
  });

  it("refuse a banned account's new sign-in, on the allowlist or not", async () => {
    const { userId } = await signIn(context);
    sql("UPDATE users SET banned_at = 1 WHERE id = ?", userId);
    fakeSignIn(context, "teo-goulois", 1);
    const response = await api(context, "POST", "/v1/auth/device/token", {
      body: { deviceCode: "device-1", machineName: "desktop", ingressPort: 7381 },
    });
    expect(response.status).toBe(403);
    expect(await errorOf(response)).toBe("banned");
  });

  it("take the URLs offline with the removal page, and release the tunnels at the next sweep", async () => {
    const { token, userId, machineId } = await signIn(context);
    const { body } = await createUrl(token, "journal.localify");
    await serve(context, token);
    expect(tunnelState(machineId)).toBe("ready");
    sql("UPDATE users SET banned_at = 1 WHERE id = ?", userId);

    const page = await visit(context, `https://${body.publicUrl.slug}.localify.fyi/`);
    expect(page.status).toBe(410);
    expect(await page.text()).toContain("abuse@localify.dev");
    await runSweep(context.env);
    expect(tunnelState(machineId)).toBe("none");
  });
});

describe("takedowns", () => {
  it("refuse a claim, show the removal page, and hold the slug forever", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const ryan = await signIn(context, "ryan-ede", 2, "ryan");
    const { body } = await createUrl(laptop.token, "journal.localify");
    const { id, slug } = body.publicUrl;
    sql(
      "UPDATE public_urls SET taken_down_at = 1, taken_down_reason = 'phishing' WHERE id = ?",
      id,
    );

    expect(await states(laptop.token)).toEqual({ [slug]: "taken_down" });
    const claim = await api(context, "POST", `/v1/public-urls/${id}/claim`, {
      token: runner.token,
    });
    expect(claim.status).toBe(403);
    expect(await errorOf(claim)).toBe("taken_down");
    expect((await visit(context, `https://${slug}.localify.fyi/`)).status).toBe(410);

    // Its owner may delete it; the slug stays held, and its page stays.
    expect(
      (await api(context, "DELETE", `/v1/public-urls/${id}`, { token: laptop.token })).status,
    ).toBe(204);
    sql("UPDATE public_urls SET deleted_at = ? WHERE id = ?", Date.now() - 31 * DAY, id);
    await purgeDeletedUrls(context.env);
    expect(context.sqlite.prepare("SELECT id FROM public_urls").all()).toEqual([{ id }]);
    for (const token of [laptop.token, ryan.token]) {
      expect((await createUrl(token, "other.localify", slug)).error).toBe("slug_taken");
      const check = await api(context, "GET", `/v1/slugs/${slug}`, { token });
      expect(await check.json()).toEqual({ slug, available: false });
    }
    expect((await visit(context, `https://${slug}.localify.fyi/`)).status).toBe(410);
  });
});

describe("claiming beyond the limit", () => {
  it("answers 403 quota_exceeded for a suspended URL, and moves one that serves", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const old = await createUrl(laptop.token, "a.localify");
    const recent = await createUrl(laptop.token, "b.localify");
    setOverride(laptop.userId, 2, 1);
    const machineOf = (id: string) =>
      (
        context.sqlite.prepare("SELECT machine_id FROM public_urls WHERE id = ?").get(id) as {
          machine_id: string;
        }
      ).machine_id;

    const refused = await api(
      context,
      "POST",
      `/v1/public-urls/${recent.body.publicUrl.id}/claim`,
      {
        token: runner.token,
      },
    );
    expect(refused.status).toBe(403);
    expect(await errorOf(refused)).toBe("quota_exceeded");
    expect(machineOf(recent.body.publicUrl.id)).toBe(laptop.machineId);

    const moved = await api(context, "POST", `/v1/public-urls/${old.body.publicUrl.id}/claim`, {
      token: runner.token,
    });
    expect(moved.status).toBe(200);
    expect(machineOf(old.body.publicUrl.id)).toBe(runner.machineId);
  });
});

describe("timed transitions", () => {
  function suspendedAt(id: string): number | null {
    return (
      context.sqlite.prepare("SELECT suspended_at FROM public_urls WHERE id = ?").get(id) as {
        suspended_at: number | null;
      }
    ).suspended_at;
  }

  it("rotate a machine that keeps serving once an expired override suspends one of its URLs", async () => {
    const { token, userId, machineId } = await signIn(context);
    const expiresAt = Date.now() + DAY;
    sql(
      "UPDATE plan_overrides SET public_urls = 11, expires_at = ? WHERE user_id = ?",
      expiresAt,
      userId,
    );
    setBilling(userId, { pro_status: "active", current_period_end: expiresAt + 30 * DAY });
    const ids: string[] = [];
    for (let i = 0; i < 11; i++) {
      ids.push((await createUrl(token, `u${i}.localify`)).body.publicUrl.id);
    }
    await serve(context, token);
    vi.spyOn(console, "error").mockImplementation(() => {});

    // Before the expiry, nothing changes.
    await runSweep(context.env, expiresAt - 1);
    expect(tunnelState(machineId)).toBe("ready");
    expect(suspendedAt(ids[10]!)).toBeNull();

    // After it, Pro serves the 10 oldest: the 11th stops, and its streams with the tunnel.
    await runSweep(context.env, expiresAt + 1);
    expect(suspendedAt(ids[10]!)).toBe(expiresAt + 1);
    expect(suspendedAt(ids[9]!)).toBeNull();
    expect(tunnelState(machineId)).toBe("none");

    // The machine still serves the others: after the quiet period, its next
    // fetch provisions generation 2.
    endQuietPeriods(context);
    const next = await serve(context, token, 1);
    expect(next.body.state).toBe("provisioning");
    expect(
      context.sqlite
        .prepare("SELECT tunnel_state, tunnel_generation FROM machines WHERE id = ?")
        .get(machineId),
    ).toEqual({ tunnel_state: "ready", tunnel_generation: 2 });

    // Recorded once: the next sweep rotates nothing.
    await runSweep(context.env, expiresAt + 2);
    expect(tunnelState(machineId)).toBe("ready");
  });

  it("clear the suspension of a URL that serves again after a removal", async () => {
    const { token, userId } = await signIn(context);
    const a = await createUrl(token, "a.localify");
    const b = await createUrl(token, "b.localify");
    setOverride(userId, 1, 1);
    await runSweep(context.env);
    expect(suspendedAt(b.body.publicUrl.id)).not.toBeNull();

    await api(context, "DELETE", `/v1/public-urls/${a.body.publicUrl.id}`, { token });
    await settle(context);
    expect(suspendedAt(b.body.publicUrl.id)).toBeNull();
  });
});

describe("the quiet period after a stop", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps a URL cached as serving from reaching its machine's next tunnel", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const t0 = Date.parse("2026-11-08T10:00:00Z");
    vi.setSystemTime(t0);
    const { token, userId, machineId } = await signIn(context);
    await createUrl(token, "a.localify");
    const b = await createUrl(token, "b.localify");
    await serve(context, token);
    context.routes.set(`GET https://m-${machineId}.localify.dev/`, () => new Response("app"));
    const forwarded = () =>
      context.calls.filter((call) => call.url.startsWith(`https://m-${machineId}.`)).length;

    // B is cached as serving by this isolate.
    expect((await visit(context, `https://${b.body.publicUrl.slug}.localify.fyi/`)).status).toBe(
      200,
    );
    expect(forwarded()).toBe(1);

    // B stops serving; the sweep marks it and releases generation 1.
    setOverride(userId, 5, 1);
    vi.setSystemTime(t0 + 1_000);
    await runSweep(context.env, Date.now());
    expect(tunnelState(machineId)).toBe("none");

    // A still serves, but generation 2 waits while B's cached entry may live.
    vi.setSystemTime(t0 + 5_000);
    const early = await serve(context, token, 1);
    expect(early.body).toMatchObject({ state: "provisioning" });
    expect(early.body.retryAfter).toBe(Math.ceil((1_000 + QUIET_AFTER_STOP_MS - 5_000) / 1000));
    expect(
      context.sqlite
        .prepare("SELECT tunnel_state, tunnel_generation FROM machines WHERE id = ?")
        .get(machineId),
    ).toEqual({ tunnel_state: "none", tunnel_generation: 1 });

    // After it, generation 2 starts, and B's cached entry has expired.
    vi.setSystemTime(t0 + 1_000 + QUIET_AFTER_STOP_MS + 1);
    expect((await serve(context, token, 1)).body.state).toBe("provisioning");
    expect(
      context.sqlite
        .prepare("SELECT tunnel_state, tunnel_generation FROM machines WHERE id = ?")
        .get(machineId),
    ).toEqual({ tunnel_state: "ready", tunnel_generation: 2 });
    expect((await visit(context, `https://${b.body.publicUrl.slug}.localify.fyi/`)).status).toBe(
      402,
    );
    expect(forwarded()).toBe(1);
  });
});

describe("upgrade hints", () => {
  const PRO = { kind: "pro", command: "localify subscribe" };

  async function body(response: Response): Promise<Record<string, unknown>> {
    return (await response.json()) as Record<string, unknown>;
  }

  it("point an account without a plan to Pro, on every 402", async () => {
    const { token, userId } = await signIn(context);
    await createUrl(token, "journal.localify");
    removeOverride(userId);

    const created = await createUrl(token, "notes.localify");
    expect(created.status).toBe(402);
    expect(created.body).toMatchObject({ error: "subscription_required", upgrade: PRO });
    const runtime = await serve(context, token);
    expect(runtime.body.blocked).toMatchObject({ reason: "subscription_required", upgrade: PRO });
    const lease = await api(context, "GET", "/v1/remote/lease", { token });
    expect(lease.status).toBe(402);
    expect(await body(lease)).toMatchObject({ error: "subscription_required", upgrade: PRO });
  });

  it("point a Pro account at its URL limit to the URL add-on, only while add-ons are on sale", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    setBilling(userId, { pro_status: "active" });
    for (let i = 0; i < 10; i++)
      expect((await createUrl(token, `u${i}.localify`)).status).toBe(201);

    const off = await createUrl(token, "extra.localify");
    expect(off.status).toBe(403);
    expect(off.error).toBe("quota_exceeded");
    expect(off.body).not.toHaveProperty("upgrade");

    context.env.ADDONS = "on";
    const on = await createUrl(token, "extra.localify");
    expect(on.body).toMatchObject({
      error: "quota_exceeded",
      upgrade: { kind: "addon_urls", command: "localify billing" },
    });
  });

  it("point a claim beyond the URL limit to the URL add-on", async () => {
    context.env.ADDONS = "on";
    const laptop = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const runner = await signIn(context, "teo-goulois", 1, "runner", { entitled: false });
    setBilling(laptop.userId, { pro_status: "active", addon_url_packs: 1 });
    let last = "";
    for (let i = 0; i < 11; i++)
      last = (await createUrl(laptop.token, `u${i}.localify`)).body.publicUrl.id;
    sql("UPDATE billing SET addon_url_packs = 0 WHERE user_id = ?", laptop.userId);

    const claim = await api(context, "POST", `/v1/public-urls/${last}/claim`, {
      token: runner.token,
    });
    expect(claim.status).toBe(403);
    expect(await body(claim)).toMatchObject({
      error: "quota_exceeded",
      upgrade: { kind: "addon_urls", command: "localify billing" },
    });
  });

  it("give an override no add-on hint: add-ons do not count under it", async () => {
    context.env.ADDONS = "on";
    const { token, userId } = await signIn(context);
    setOverride(userId, 1, 1);
    await createUrl(token, "a.localify");
    const refused = await createUrl(token, "b.localify");
    expect(refused.error).toBe("quota_exceeded");
    expect(refused.body).not.toHaveProperty("upgrade");
  });

  it("point a Pro account with every serving slot taken to the machine add-on", async () => {
    context.env.ADDONS = "on";
    const first = await signIn(context, "teo-goulois", 1, "one", { entitled: false });
    setBilling(first.userId, { pro_status: "active" });
    for (const name of ["one", "two", "three"]) {
      const machine =
        name === "one" ? first : await signIn(context, "teo-goulois", 1, name, { entitled: false });
      await createUrl(machine.token, `${name}.localify`);
      const runtime = await serve(context, machine.token);
      if (name === "three") {
        expect(runtime.body.blocked).toMatchObject({
          reason: "serving_slots_full",
          upgrade: { kind: "addon_machine", command: "localify billing" },
        });
      }
    }
  });
});
