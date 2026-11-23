// What the dashboard reads from D1, and the pure aggregation of those rows
// into what it shows. Plans, overrides and add-ons come from the Worker's own
// rules (`src/entitlement.ts`), never from a copy of them.
//
// No statement selects a token, a password hash or salt, a tunnel or DNS
// id, a remote key or an ingress port: what is not read cannot be served.

import type { ReadDb, Row } from "./db.ts";

import {
  EVERY_ACCOUNT_SQL,
  accountOf,
  servingUrlLimit,
  signedInMachineCap,
  type AccountRow,
  type Entitlement,
  type PlanStatus,
} from "../../src/entitlement.ts";

/** The reads of one load, sent together. Each is a SELECT without parameters. */
export const QUERIES = {
  accounts: EVERY_ACCOUNT_SQL,
  users: `SELECT id, github_id, login, created_at, banned_reason FROM users`,
  overrides: `SELECT user_id, reason FROM plan_overrides`,
  billing: `SELECT user_id, refreshed_at FROM billing`,
  machines: `
    SELECT id, user_id, name, status, tunnel_state, created_at, tunnel_ready_at, remote_key_at
    FROM machines WHERE status != 'revoked' OR tunnel_state != 'none'
    ORDER BY user_id, created_at, id`,
  urls: `
    SELECT p.id, p.user_id, p.slug, p.wildcard, p.protected, m.name AS machine_name,
           p.created_at, p.deleted_at, p.taken_down_at, p.taken_down_reason, p.suspended_at
    FROM public_urls p LEFT JOIN machines m ON m.id = p.machine_id
    WHERE p.deleted_at IS NULL OR p.taken_down_at IS NOT NULL
    ORDER BY p.user_id, p.created_at, p.id`,
  // D1 records no visits: the latest sign-in, tunnel start, remote key,
  // URL created or deleted, and billing event stand in for activity, 0 for
  // none. Joins, not a UNION: D1 caps the terms of a compound SELECT.
  activity: `
    SELECT u.id AS user_id, MAX(COALESCE(m.at, 0), COALESCE(p.at, 0), COALESCE(w.at, 0)) AS at
    FROM users u
      LEFT JOIN (SELECT user_id, MAX(MAX(created_at), COALESCE(MAX(tunnel_ready_at), 0),
                                     COALESCE(MAX(remote_key_at), 0)) AS at
                 FROM machines GROUP BY user_id) m ON m.user_id = u.id
      LEFT JOIN (SELECT user_id, MAX(MAX(created_at), COALESCE(MAX(deleted_at), 0)) AS at
                 FROM public_urls GROUP BY user_id) p ON p.user_id = u.id
      LEFT JOIN (SELECT user_id, MAX(received_at) AS at
                 FROM webhook_events WHERE user_id IS NOT NULL GROUP BY user_id) w ON w.user_id = u.id`,
} as const;

export type Raw = { [K in keyof typeof QUERIES]: Row[] };

/** Every query in one call. */
export async function readRaw(db: ReadDb): Promise<Raw> {
  const keys = Object.keys(QUERIES) as (keyof typeof QUERIES)[];
  const results = await db(keys.map((key) => QUERIES[key]));
  return Object.fromEntries(keys.map((key, index) => [key, results[index] ?? []])) as Raw;
}

const DAY_MS = 24 * 60 * 60 * 1000;
/** Share of a limit from which an account is listed for attention. */
export const NEAR_LIMIT = 0.8;
/** How far ahead an expiring override is listed. */
export const EXPIRY_WINDOW_MS = 30 * DAY_MS;

export const PLAN_STATUSES: PlanStatus[] = [
  "override",
  "trialing",
  "active",
  "past_due",
  "canceling",
  "none",
];

export type MachineView = {
  id: string;
  name: string;
  status: string;
  /** Holds a serving slot: its tunnel state is anything but `none`. */
  serving: boolean;
  tunnelState: string;
  linked: boolean;
  createdAt: number;
};

export type UrlState = "serving" | "pending" | "suspended" | "taken_down";

export type UrlView = {
  slug: string;
  state: UrlState;
  deleted: boolean;
  wildcard: boolean;
  protected: boolean;
  machineName: string | null;
  createdAt: number;
  takenDownAt: number | null;
  takenDownReason: string | null;
};

export type OverrideView = {
  servingMachines: number;
  publicUrls: number;
  remoteAccess: boolean;
  reason: string;
  expiresAt: number | null;
  /** Past its expiry: the plan applies again. */
  expired: boolean;
};

export type UserView = {
  id: string;
  githubId: number;
  login: string;
  createdAt: number;
  bannedAt: number | null;
  bannedReason: string | null;
  status: PlanStatus;
  interval: string | null;
  proStatus: string | null;
  renewsAt: string | null;
  endsAt: string | null;
  trialEndsAt: string | null;
  billingRefreshedAt: number | null;
  override: OverrideView | null;
  addons: { machines: number; urlPacks: number };
  entitlement: Entitlement;
  /** Machines holding a serving slot, against `entitlement.servingMachines`. */
  servingSlots: number;
  /** Active machines, against `signedInCap`. */
  signedIn: number;
  signedInCap: number;
  /** Neither deleted nor taken down, against `entitlement.publicUrls`. */
  liveUrls: number;
  servingUrls: number;
  lastActivityAt: number | null;
  machines: MachineView[];
  urls: UrlView[];
};

export type AttentionKind = "banned" | "past_due" | "override_expiring" | "urls" | "machines";

export type Attention = { userId: string; login: string; kind: AttentionKind; detail: string };

export type Dashboard = {
  readAt: number;
  totals: {
    users: number;
    newLast7Days: number;
    newLast30Days: number;
    activeMachines: number;
    servingMachines: number;
    liveUrls: number;
    servingUrls: number;
    banned: number;
    takedowns: number;
  };
  statuses: Record<PlanStatus, number>;
  /** Add-on units that count right now, across accounts. */
  addonUnits: { machines: number; urlPacks: number };
  /** New sign-ups per week, oldest first, from the first sign-up's week to now's. */
  signupsByWeek: { weekStart: number; count: number }[];
  attention: Attention[];
  /** Newest sign-up first. */
  users: UserView[];
};

function num(value: unknown): number {
  return Number(value);
}

function numOrNull(value: unknown): number | null {
  return value === null || value === undefined ? null : Number(value);
}

function strOrNull(value: unknown): string | null {
  return value === null || value === undefined ? null : String(value);
}

function groupBy(rows: Row[]): Map<string, Row[]> {
  const groups = new Map<string, Row[]>();
  for (const row of rows) {
    const key = String(row.user_id);
    const group = groups.get(key);
    if (group) group.push(row);
    else groups.set(key, [row]);
  }
  return groups;
}

function byUser(rows: Row[]): Map<string, Row> {
  return new Map(rows.map((row) => [String(row.user_id), row]));
}

/** Monday 00:00 UTC of the week holding `ms`. */
export function weekStart(ms: number): number {
  const day = Math.floor(ms / DAY_MS);
  // 1970-01-01 was a Thursday: day 0 is 3 days after a Monday.
  return (day - ((day + 3) % 7)) * DAY_MS;
}

/**
 * The URLs of one owner with their state, as `cloud:admin show` gives it:
 * the oldest live URLs within the limit serve unless `suspended_at` is set,
 * which a reconcile clears ("pending"); the rest are suspended.
 */
function urlViews(urls: Row[], entitlement: Entitlement): UrlView[] {
  const limit = servingUrlLimit(entitlement);
  let rank = 0;
  return urls.map((url) => {
    const deleted = url.deleted_at !== null;
    const takenDown = url.taken_down_at !== null;
    if (!deleted && !takenDown) rank++;
    const state: UrlState = takenDown
      ? "taken_down"
      : rank <= limit && url.suspended_at === null
        ? "serving"
        : rank <= limit
          ? "pending"
          : "suspended";
    return {
      slug: String(url.slug),
      state,
      deleted,
      wildcard: num(url.wildcard) === 1,
      protected: num(url.protected) === 1,
      machineName: strOrNull(url.machine_name),
      createdAt: num(url.created_at),
      takenDownAt: numOrNull(url.taken_down_at),
      takenDownReason: strOrNull(url.taken_down_reason),
    };
  });
}

/** Whether `used` reaches `NEAR_LIMIT` of a non-zero `limit`. */
export function nearLimit(used: number, limit: number): boolean {
  return limit > 0 && used >= NEAR_LIMIT * limit;
}

/** The dashboard of the rows of one load, as of `now`. */
export function buildDashboard(raw: Raw, now: number): Dashboard {
  const accounts = byUser(raw.accounts);
  const overrides = byUser(raw.overrides);
  const billing = byUser(raw.billing);
  const activity = byUser(raw.activity);
  const machines = groupBy(raw.machines);
  const urls = groupBy(raw.urls);

  const users: UserView[] = raw.users.map((user) => {
    const id = String(user.id);
    const row = accounts.get(id) as AccountRow | undefined;
    if (!row) throw new Error(`User ${id} has no account row`);
    const account = accountOf(row, now);
    const overrideRow = overrides.get(id);
    const override: OverrideView | null =
      row.override_machines === null || row.override_urls === null
        ? null
        : {
            servingMachines: num(row.override_machines),
            publicUrls: num(row.override_urls),
            remoteAccess: num(row.override_remote_access) === 1,
            reason: String(overrideRow?.reason ?? ""),
            expiresAt: numOrNull(row.override_expires_at),
            expired: row.override_expires_at !== null && num(row.override_expires_at) <= now,
          };
    const machineViews: MachineView[] = (machines.get(id) ?? []).map((machine) => ({
      id: String(machine.id),
      name: String(machine.name),
      status: String(machine.status),
      serving: machine.tunnel_state !== "none",
      tunnelState: String(machine.tunnel_state),
      linked: machine.remote_key_at !== null,
      createdAt: num(machine.created_at),
    }));
    const views = urlViews(urls.get(id) ?? [], account.entitlement);
    return {
      id,
      githubId: num(user.github_id),
      login: String(user.login),
      createdAt: num(user.created_at),
      bannedAt: numOrNull(row.banned_at),
      bannedReason: strOrNull(user.banned_reason),
      status: account.plan.status,
      interval: account.plan.interval,
      proStatus: row.pro_status,
      renewsAt: account.plan.renewsAt,
      endsAt: account.plan.endsAt,
      trialEndsAt: account.plan.trialEndsAt,
      billingRefreshedAt: numOrNull(billing.get(id)?.refreshed_at),
      override,
      addons: account.plan.addons,
      entitlement: account.entitlement,
      servingSlots: machineViews.filter((machine) => machine.serving).length,
      signedIn: machineViews.filter((machine) => machine.status === "active").length,
      signedInCap: signedInMachineCap(account.entitlement),
      liveUrls: views.filter((url) => !url.deleted && url.state !== "taken_down").length,
      servingUrls: views.filter((url) => url.state === "serving").length,
      lastActivityAt: num(activity.get(id)?.at ?? 0) || null,
      machines: machineViews,
      urls: views,
    };
  });
  users.sort((a, b) => b.createdAt - a.createdAt || a.login.localeCompare(b.login));

  const statuses = Object.fromEntries(PLAN_STATUSES.map((status) => [status, 0])) as Record<
    PlanStatus,
    number
  >;
  for (const user of users) statuses[user.status]++;

  const sum = (pick: (user: UserView) => number) =>
    users.reduce((total, user) => total + pick(user), 0);

  return {
    readAt: now,
    totals: {
      users: users.length,
      newLast7Days: users.filter((user) => user.createdAt > now - 7 * DAY_MS).length,
      newLast30Days: users.filter((user) => user.createdAt > now - 30 * DAY_MS).length,
      activeMachines: sum((user) => user.signedIn),
      servingMachines: sum((user) => user.servingSlots),
      liveUrls: sum((user) => user.liveUrls),
      servingUrls: sum((user) => user.servingUrls),
      banned: users.filter((user) => user.bannedAt !== null).length,
      takedowns: sum((user) => user.urls.filter((url) => url.state === "taken_down").length),
    },
    statuses,
    addonUnits: {
      machines: sum((user) => user.addons.machines),
      urlPacks: sum((user) => user.addons.urlPacks),
    },
    signupsByWeek: signupsByWeek(users, now),
    attention: attentionList(users, now),
    users,
  };
}

function signupsByWeek(users: UserView[], now: number): Dashboard["signupsByWeek"] {
  if (users.length === 0) return [];
  const first = weekStart(Math.min(...users.map((user) => user.createdAt)));
  const last = weekStart(now);
  const weeks: Dashboard["signupsByWeek"] = [];
  for (let start = first; start <= last; start += 7 * DAY_MS) {
    weeks.push({ weekStart: start, count: 0 });
  }
  for (const user of users) {
    const index = Math.round((weekStart(user.createdAt) - first) / (7 * DAY_MS));
    const week = weeks[index];
    if (week) week.count++;
  }
  return weeks;
}

/** Banned and past_due accounts, overrides ending within 30 days, and accounts near a limit. */
function attentionList(users: UserView[], now: number): Attention[] {
  const list: Attention[] = [];
  const add = (user: UserView, kind: AttentionKind, detail: string) =>
    list.push({ userId: user.id, login: user.login, kind, detail });
  for (const user of users) {
    if (user.bannedAt !== null) add(user, "banned", user.bannedReason ?? "(no reason)");
    // Under an override too: the card still fails.
    if (user.proStatus === "past_due") {
      add(
        user,
        "past_due",
        user.status === "override" ? "Pro past_due, under an override" : "Pro past_due",
      );
    }
    const expiresAt = user.override?.expiresAt ?? null;
    if (expiresAt !== null && expiresAt > now && expiresAt <= now + EXPIRY_WINDOW_MS) {
      const days = Math.ceil((expiresAt - now) / DAY_MS);
      add(
        user,
        "override_expiring",
        `override "${user.override!.reason}" ends in ${days} ${days === 1 ? "day" : "days"}`,
      );
    }
    if (user.bannedAt !== null) continue;
    if (nearLimit(user.liveUrls, user.entitlement.publicUrls)) {
      add(user, "urls", `${user.liveUrls} of ${user.entitlement.publicUrls} URLs`);
    }
    if (nearLimit(user.servingSlots, user.entitlement.servingMachines)) {
      add(
        user,
        "machines",
        `${user.servingSlots} of ${user.entitlement.servingMachines} serving machines`,
      );
    }
    if (nearLimit(user.signedIn, user.signedInCap)) {
      add(user, "machines", `${user.signedIn} of ${user.signedInCap} signed-in machines`);
    }
  }
  const order: AttentionKind[] = ["banned", "past_due", "override_expiring", "urls", "machines"];
  return list.sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind));
}
