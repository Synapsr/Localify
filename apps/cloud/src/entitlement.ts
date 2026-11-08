// What an account may use right now (docs/public-urls.md, "Plans",
// "Lifecycle" and "Downgrades"): its ban, its override, or its Pro plan from
// the `billing` snapshot, plus its add-ons.
//
// No value imports: `scripts/admin.ts` runs this file in Node as it is.

export type Entitlement = { servingMachines: number; publicUrls: number };

export type PlanStatus = "none" | "trialing" | "active" | "past_due" | "canceling" | "override";

/** `Plan` in `/v1/me`, without `usage`, which the API counts. */
export type PlanState = {
  status: PlanStatus;
  entitled: boolean;
  interval: "month" | "year" | null;
  trialEndsAt: string | null;
  renewsAt: string | null;
  endsAt: string | null;
  trialAvailable: boolean;
  limits: Entitlement;
  addons: { machines: number; urlPacks: number };
};

export type Account = { banned: boolean; entitlement: Entitlement; plan: PlanState };

/** The row `ACCOUNT_SQL` returns. Times are Unix milliseconds. */
export type AccountRow = {
  banned_at: number | null;
  trial_used_at: number | null;
  override_machines: number | null;
  override_urls: number | null;
  override_expires_at: number | null;
  pro_status: string | null;
  pro_interval: string | null;
  cancel_at_period_end: number | null;
  current_period_end: number | null;
  trial_end: number | null;
  ends_at: number | null;
  ended_at: number | null;
  past_due_at: number | null;
  addon_machines: number | null;
  addon_url_packs: number | null;
};

/** `?1` is the user id. No row for an unknown user. */
export const ACCOUNT_SQL = `
  SELECT u.banned_at, u.trial_used_at,
         o.serving_machines AS override_machines, o.public_urls AS override_urls,
         o.expires_at AS override_expires_at,
         b.pro_status, b.pro_interval, b.cancel_at_period_end, b.current_period_end, b.trial_end,
         b.ends_at, b.ended_at, b.past_due_at, b.addon_machines, b.addon_url_packs
  FROM users u
    LEFT JOIN plan_overrides o ON o.user_id = u.id
    LEFT JOIN billing b ON b.user_id = u.id
  WHERE u.id = ?1`;

export const NO_ENTITLEMENT: Entitlement = { servingMachines: 0, publicUrls: 0 };
/** What Pro includes before add-ons. */
export const PRO: Entitlement = { servingMachines: 2, publicUrls: 10 };
/** Public URLs per URL add-on unit. */
export const URLS_PER_PACK = 10;
/** A `past_due` plan stays entitled this long after Polar's `past_due_at`. */
export const PAST_DUE_GRACE_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * The account behind a row. A ban beats everything; then an override that
 * has not expired; then the Pro plan when the lifecycle table makes it
 * entitled, plus the add-ons; otherwise nothing.
 */
export function accountOf(row: AccountRow, now: number): Account {
  const addons = { machines: row.addon_machines ?? 0, urlPacks: row.addon_url_packs ?? 0 };
  const trialAvailable = row.trial_used_at === null;
  const pro = proPlan(row, now);

  let entitlement: Entitlement;
  let plan: Omit<PlanState, "entitled" | "limits">;
  if (
    row.override_machines !== null &&
    row.override_urls !== null &&
    (row.override_expires_at === null || row.override_expires_at > now)
  ) {
    entitlement = { servingMachines: row.override_machines, publicUrls: row.override_urls };
    plan = {
      status: "override",
      interval: null,
      trialEndsAt: null,
      renewsAt: null,
      endsAt: time(row.override_expires_at),
      trialAvailable,
      addons,
    };
  } else {
    entitlement = pro.entitled
      ? {
          servingMachines: PRO.servingMachines + addons.machines,
          publicUrls: PRO.publicUrls + URLS_PER_PACK * addons.urlPacks,
        }
      : NO_ENTITLEMENT;
    plan = { ...pro.plan, trialAvailable, addons };
  }
  if (row.banned_at !== null) entitlement = NO_ENTITLEMENT;
  return {
    banned: row.banned_at !== null,
    entitlement,
    plan: { ...plan, entitled: isEntitled(entitlement), limits: entitlement },
  };
}

type ProPlan = {
  entitled: boolean;
  plan: Pick<PlanState, "status" | "interval" | "trialEndsAt" | "renewsAt" | "endsAt">;
};

/** The lifecycle table: Polar's status, and the dates that bound it. */
function proPlan(row: AccountRow, now: number): ProPlan {
  const interval =
    row.pro_interval === "month" || row.pro_interval === "year" ? row.pro_interval : null;
  const plan = (
    status: PlanStatus,
    dates: { trialEndsAt?: number | null; renewsAt?: number | null; endsAt?: number | null } = {},
  ): ProPlan["plan"] => ({
    status,
    interval: status === "none" ? null : interval,
    trialEndsAt: time(dates.trialEndsAt ?? null),
    renewsAt: time(dates.renewsAt ?? null),
    endsAt: time(dates.endsAt ?? null),
  });
  const none: ProPlan = { entitled: false, plan: plan("none") };
  const cancelAtPeriodEnd = row.cancel_at_period_end === 1;

  switch (row.pro_status) {
    case "trialing":
      return {
        entitled: true,
        plan: plan("trialing", {
          trialEndsAt: row.trial_end,
          renewsAt: cancelAtPeriodEnd ? null : row.current_period_end,
          endsAt: cancelAtPeriodEnd ? row.current_period_end : null,
        }),
      };
    case "active":
      if (!cancelAtPeriodEnd) {
        return { entitled: true, plan: plan("active", { renewsAt: row.current_period_end }) };
      }
      // Entitled until the period ends; after that, it has ended.
      return before(row.current_period_end, now)
        ? { entitled: true, plan: plan("canceling", { endsAt: row.current_period_end }) }
        : none;
    case "past_due":
      // Polar retries the card. Without `past_due_at`, the grace has no
      // start, so it gives nothing.
      return {
        entitled: row.past_due_at !== null && now < row.past_due_at + PAST_DUE_GRACE_MS,
        plan: plan("past_due"),
      };
    case "canceled":
      return row.ended_at === null && row.ends_at !== null && row.ends_at > now
        ? { entitled: true, plan: plan("canceling", { endsAt: row.ends_at }) }
        : none;
    default:
      // unpaid, paused, incomplete, incomplete_expired, or no subscription.
      return none;
  }
}

/** Whether `end` is still ahead; a missing end bounds nothing. */
function before(end: number | null, now: number): boolean {
  return end === null || end > now;
}

function time(ms: number | null): string | null {
  return ms === null ? null : new Date(ms).toISOString();
}

/** An entitlement lets something serve: a zero one answers `402 subscription_required`. */
export function isEntitled(entitlement: Entitlement): boolean {
  return entitlement.servingMachines > 0 && entitlement.publicUrls > 0;
}

/** How many of the owner's live URLs serve, oldest first: none without an entitlement. */
export function servingUrlLimit(entitlement: Entitlement): number {
  return isEntitled(entitlement) ? entitlement.publicUrls : 0;
}

/** The account of a user, or null for an unknown one. */
export async function loadAccount(
  db: D1Database,
  userId: string,
  now = Date.now(),
): Promise<Account | null> {
  const row = await db.prepare(ACCOUNT_SQL).bind(userId).first<AccountRow>();
  return row ? accountOf(row, now) : null;
}

/** What the user may use right now: nothing for an unknown or banned user. */
export async function entitlementOf(
  env: Env,
  userId: string,
  now = Date.now(),
): Promise<Entitlement> {
  return (await loadAccount(env.DB, userId, now))?.entitlement ?? NO_ENTITLEMENT;
}

// --- Downgrades ---

/** A live URL of one owner, oldest first: neither deleted nor taken down. */
export type LiveUrl = { id: string; machine_id: string | null };

/** `?1` is the owner: their live URLs in the order that decides which serve. */
export const LIVE_URLS_SQL = `
  SELECT id, machine_id FROM public_urls
  WHERE user_id = ?1 AND deleted_at IS NULL AND taken_down_at IS NULL
  ORDER BY created_at, id`;

/** The URLs that serve: the oldest live ones, up to the limit (docs/public-urls.md, "Downgrades"). */
export function servingUrls(entitlement: Entitlement, liveUrls: LiveUrl[]): LiveUrl[] {
  return liveUrls.slice(0, servingUrlLimit(entitlement));
}

/**
 * The machines to rotate after a change of one owner's entitlement or URLs:
 * those that served a URL which no longer serves. Their open streams ride
 * the tunnel, so only a new generation closes them (ADR 0027).
 */
export function machinesThatLostUrls(
  before: { entitlement: Entitlement; liveUrls: LiveUrl[] },
  after: { entitlement: Entitlement; liveUrls: LiveUrl[] },
): string[] {
  const still = new Set(servingUrls(after.entitlement, after.liveUrls).map((url) => url.id));
  const machines = new Set<string>();
  for (const url of servingUrls(before.entitlement, before.liveUrls)) {
    if (!still.has(url.id) && url.machine_id !== null) machines.add(url.machine_id);
  }
  return [...machines].sort();
}

/**
 * `?1` is a JSON array of machine ids, `?2` now. Marks the ones that hold a
 * serving slot for a rotation; the next sweep, or their next runtime fetch,
 * releases their current generation.
 */
export const ROTATE_SQL = `
  UPDATE machines SET rotate_requested_at = COALESCE(rotate_requested_at, ?2)
  WHERE id IN (SELECT value FROM json_each(?1)) AND status = 'active' AND tunnel_state != 'none'
  RETURNING id, name`;
