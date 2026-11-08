// Plans, sign-up, bans, takedowns and downgrades, through the API, the
// runtime and the public path (docs/public-urls.md, "Plans" and "API").

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { purgeDeletedUrls } from "../src/machines";
import { runSweep } from "../src/tunnels";
import { api, createContext, fakeSignIn, serve, signIn, visit, type TestContext } from "./support";

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
      limits: { servingMachines: 0, publicUrls: 0 },
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
      limits: { servingMachines: 3, publicUrls: 20 },
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
      limits: { servingMachines: 5, publicUrls: 10 },
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
