import { describe, expect, it } from "vitest";

import {
  isRotation,
  loadAccount,
  PAST_DUE_GRACE_MS,
  reconcileStatements,
  type OwnedUrl,
} from "../src/entitlement";
import { createTestDb } from "./support";

const NOW = Date.parse("2026-11-07T12:00:00Z");
const DAY = 24 * 60 * 60 * 1000;

type Billing = Partial<{
  pro_status: string | null;
  pro_interval: "month" | "year" | null;
  cancel_at_period_end: number;
  current_period_end: number | null;
  trial_end: number | null;
  ends_at: number | null;
  ended_at: number | null;
  past_due_at: number | null;
  addon_machines: number;
  addon_url_packs: number;
}>;

type Setup = {
  billing?: Billing;
  override?: { machines: number; urls: number; expiresAt?: number | null };
  bannedAt?: number;
  trialUsedAt?: number;
};

/** The account of a user whose D1 rows are `setup`, read the way the Worker reads it. */
async function account(setup: Setup = {}, now = NOW) {
  const { db, sqlite } = createTestDb();
  sqlite
    .prepare(
      `INSERT INTO users (id, github_id, login, created_at, banned_at, trial_used_at)
       VALUES ('u', 1, 'teo', 1, ?, ?)`,
    )
    .run(setup.bannedAt ?? null, setup.trialUsedAt ?? null);
  if (setup.billing) {
    const billing = {
      pro_status: null,
      pro_interval: "month",
      cancel_at_period_end: 0,
      current_period_end: null,
      trial_end: null,
      ends_at: null,
      ended_at: null,
      past_due_at: null,
      addon_machines: 0,
      addon_url_packs: 0,
      ...setup.billing,
    };
    sqlite
      .prepare(
        `INSERT INTO billing (user_id, pro_subscription_id, pro_status, pro_interval,
                              cancel_at_period_end, current_period_end, trial_end, ends_at, ended_at,
                              past_due_at, addon_machines, addon_url_packs, revision, refreshed_at)
         VALUES ('u', 'sub', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 1)`,
      )
      .run(
        billing.pro_status,
        billing.pro_interval,
        billing.cancel_at_period_end,
        billing.current_period_end,
        billing.trial_end,
        billing.ends_at,
        billing.ended_at,
        billing.past_due_at,
        billing.addon_machines,
        billing.addon_url_packs,
      );
  }
  if (setup.override) {
    sqlite
      .prepare(
        `INSERT INTO plan_overrides (user_id, serving_machines, public_urls, reason, expires_at, created_at)
         VALUES ('u', ?, ?, 'team', ?, 1)`,
      )
      .run(setup.override.machines, setup.override.urls, setup.override.expiresAt ?? null);
  }
  const result = await loadAccount(db, "u", now);
  if (!result) throw new Error("No account");
  return result;
}

const PRO = { servingMachines: 2, publicUrls: 10 };
const NONE = { servingMachines: 0, publicUrls: 0 };
const iso = (ms: number) => new Date(ms).toISOString();

describe("the lifecycle table", () => {
  it("gives nothing without a subscription", async () => {
    expect(await account()).toEqual({
      banned: false,
      entitlement: NONE,
      plan: {
        status: "none",
        entitled: false,
        interval: null,
        trialEndsAt: null,
        renewsAt: null,
        endsAt: null,
        trialAvailable: true,
        limits: NONE,
        addons: { machines: 0, urlPacks: 0 },
      },
    });
  });

  it("entitles a trial, with its end and its first charge", async () => {
    const { entitlement, plan } = await account({
      billing: { pro_status: "trialing", trial_end: NOW + DAY, current_period_end: NOW + DAY },
      trialUsedAt: NOW - DAY,
    });
    expect(entitlement).toEqual(PRO);
    expect(plan).toMatchObject({
      status: "trialing",
      entitled: true,
      interval: "month",
      trialEndsAt: iso(NOW + DAY),
      renewsAt: iso(NOW + DAY),
      endsAt: null,
      trialAvailable: false,
      limits: PRO,
    });
  });

  it("entitles an active plan, renewing at the end of its period", async () => {
    const { plan } = await account({
      billing: { pro_status: "active", pro_interval: "year", current_period_end: NOW + 30 * DAY },
    });
    expect(plan).toMatchObject({
      status: "active",
      entitled: true,
      interval: "year",
      renewsAt: iso(NOW + 30 * DAY),
      endsAt: null,
    });
  });

  it("entitles an active plan set to cancel until its period ends, then nothing", async () => {
    const billing = {
      pro_status: "active",
      cancel_at_period_end: 1,
      current_period_end: NOW + DAY,
    };
    expect((await account({ billing })).plan).toMatchObject({
      status: "canceling",
      entitled: true,
      renewsAt: null,
      endsAt: iso(NOW + DAY),
    });
    const ended = await account({ billing }, NOW + DAY);
    expect(ended.entitlement).toEqual(NONE);
    expect(ended.plan).toMatchObject({ status: "none", entitled: false, interval: null });
  });

  it("keeps a past_due plan for 7 days after Polar's past_due_at", async () => {
    const billing = { pro_status: "past_due", past_due_at: NOW };
    const grace = await account({ billing }, NOW + PAST_DUE_GRACE_MS - 1);
    expect(grace.entitlement).toEqual(PRO);
    expect(grace.plan).toMatchObject({ status: "past_due", entitled: true, renewsAt: null });
    const after = await account({ billing }, NOW + PAST_DUE_GRACE_MS);
    expect(after.entitlement).toEqual(NONE);
    expect(after.plan).toMatchObject({ status: "past_due", entitled: false });
  });

  it("gives nothing to a past_due plan without past_due_at", async () => {
    expect((await account({ billing: { pro_status: "past_due" } })).entitlement).toEqual(NONE);
  });

  it("keeps a canceled plan until ends_at, and stops once it ended", async () => {
    const canceling = await account({ billing: { pro_status: "canceled", ends_at: NOW + DAY } });
    expect(canceling.entitlement).toEqual(PRO);
    expect(canceling.plan).toMatchObject({ status: "canceling", endsAt: iso(NOW + DAY) });
    for (const billing of [
      { pro_status: "canceled", ends_at: NOW },
      { pro_status: "canceled", ends_at: NOW + DAY, ended_at: NOW - 1 },
      { pro_status: "canceled" },
    ]) {
      const ended = await account({ billing });
      expect(ended.entitlement, JSON.stringify(billing)).toEqual(NONE);
      expect(ended.plan.status).toBe("none");
    }
  });

  it("gives nothing to unpaid, paused or incomplete plans", async () => {
    for (const status of ["unpaid", "paused", "incomplete", "incomplete_expired", null]) {
      const { entitlement, plan } = await account({
        billing: { pro_status: status, current_period_end: NOW + DAY },
      });
      expect(entitlement, String(status)).toEqual(NONE);
      expect(plan).toMatchObject({ status: "none", entitled: false, interval: null });
    }
  });

  it("adds the add-ons to an entitled plan only", async () => {
    const addons = { addon_machines: 2, addon_url_packs: 3 };
    const entitled = await account({ billing: { pro_status: "active", ...addons } });
    expect(entitled.entitlement).toEqual({ servingMachines: 4, publicUrls: 40 });
    expect(entitled.plan.addons).toEqual({ machines: 2, urlPacks: 3 });
    const lapsed = await account({ billing: { pro_status: "unpaid", ...addons } });
    expect(lapsed.entitlement).toEqual(NONE);
    expect(lapsed.plan.addons).toEqual({ machines: 2, urlPacks: 3 });
  });
});

describe("overrides and bans", () => {
  it("lets a live override replace the plan entirely", async () => {
    const { entitlement, plan } = await account({
      billing: { pro_status: "active", addon_machines: 3 },
      override: { machines: 1, urls: 3, expiresAt: NOW + DAY },
    });
    expect(entitlement).toEqual({ servingMachines: 1, publicUrls: 3 });
    expect(plan).toMatchObject({
      status: "override",
      entitled: true,
      interval: null,
      renewsAt: null,
      endsAt: iso(NOW + DAY),
      limits: { servingMachines: 1, publicUrls: 3 },
    });
  });

  it("falls back to the plan once the override expired", async () => {
    const expired = await account({
      billing: { pro_status: "active" },
      override: { machines: 5, urls: 10, expiresAt: NOW },
    });
    expect(expired.entitlement).toEqual(PRO);
    expect(expired.plan.status).toBe("active");
    const none = await account({ override: { machines: 5, urls: 10, expiresAt: NOW - 1 } });
    expect(none.entitlement).toEqual(NONE);
  });

  it("counts an override that serves nothing as no entitlement", async () => {
    const { plan } = await account({ override: { machines: 0, urls: 10 } });
    expect(plan).toMatchObject({ status: "override", entitled: false });
  });

  it("lets a ban beat the plan and the override", async () => {
    const banned = await account({
      billing: { pro_status: "active" },
      override: { machines: 5, urls: 10 },
      bannedAt: NOW - 1,
    });
    expect(banned.banned).toBe(true);
    expect(banned.entitlement).toEqual(NONE);
    expect(banned.plan).toMatchObject({ entitled: false, limits: NONE });
  });

  it("is null for an unknown user", async () => {
    expect(await loadAccount(createTestDb().db, "nobody", NOW)).toBeNull();
  });
});

describe("reconciling the serving URLs", () => {
  const url = (id: string, machine: string | null, extra: Partial<OwnedUrl> = {}): OwnedUrl => ({
    id,
    user_id: "u",
    machine_id: machine,
    taken_down_at: null,
    suspended_at: null,
    ...extra,
  });
  const urls = [url("a", "m1"), url("b", "m2"), url("c", "m2"), url("d", null)];
  const two = { servingMachines: 2, publicUrls: 2 };
  /** What the statements write, by kind, with the URL ids each one covers. */
  const writes = (entitlement: typeof two, owned: OwnedUrl[]) =>
    reconcileStatements("u", entitlement, owned, NOW).map((statement) => [
      isRotation(statement)
        ? "rotate"
        : statement.sql.includes("UPDATE users")
          ? "quiet"
          : statement.sql.includes("= NULL")
            ? "resume"
            : "suspend",
      JSON.parse(String(statement.params[0])) as string[],
    ]);

  it("rotates the owner's machines for the URLs beyond the limit, then records them", () => {
    expect(writes(two, urls)).toEqual([
      ["rotate", ["c", "d"]],
      ["quiet", ["c", "d"]],
      ["suspend", ["c", "d"]],
    ]);
    const [rotation] = reconcileStatements("u", two, urls, NOW);
    expect(rotation!.params).toEqual(['["c","d"]', NOW, "u"]);
  });

  it("writes nothing once the state is recorded", () => {
    const recorded = urls.map((owned) =>
      ["c", "d"].includes(owned.id) ? { ...owned, suspended_at: 1 } : owned,
    );
    expect(writes(two, recorded)).toEqual([]);
  });

  it("stops everything without an entitlement, and a taken-down URL whatever the limit", () => {
    expect(writes(NONE, urls)).toEqual([
      ["rotate", ["a", "b", "c", "d"]],
      ["quiet", ["a", "b", "c", "d"]],
      ["suspend", ["a", "b", "c", "d"]],
    ]);
    const takenDown = [url("a", "m1", { taken_down_at: 1 }), url("b", "m2")];
    expect(writes(two, takenDown)).toEqual([
      ["rotate", ["a"]],
      ["quiet", ["a"]],
      ["suspend", ["a"]],
    ]);
  });

  it("resumes a suspended URL that serves again, rotating nothing", () => {
    const suspended = [url("a", "m1"), url("b", "m2", { suspended_at: 1 })];
    expect(writes(two, suspended)).toEqual([["resume", ["b"]]]);
  });
});
