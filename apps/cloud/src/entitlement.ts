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
  user_id: string;
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

const SELECT_ACCOUNTS = `
  SELECT u.id AS user_id, u.banned_at, u.trial_used_at,
         o.serving_machines AS override_machines, o.public_urls AS override_urls,
         o.expires_at AS override_expires_at,
         b.pro_status, b.pro_interval, b.cancel_at_period_end, b.current_period_end, b.trial_end,
         b.ends_at, b.ended_at, b.past_due_at, b.addon_machines, b.addon_url_packs
  FROM users u
    LEFT JOIN plan_overrides o ON o.user_id = u.id
    LEFT JOIN billing b ON b.user_id = u.id`;

/** `?1` is the user id. No row for an unknown user. */
export const ACCOUNT_SQL = `${SELECT_ACCOUNTS} WHERE u.id = ?1`;

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

export type ProPlan = {
  entitled: boolean;
  plan: Pick<PlanState, "status" | "interval" | "trialEndsAt" | "renewsAt" | "endsAt">;
};

/** The fields of one Pro subscription, as the `billing` snapshot keeps them. */
export type ProFields = Pick<
  AccountRow,
  | "pro_status"
  | "pro_interval"
  | "cancel_at_period_end"
  | "current_period_end"
  | "trial_end"
  | "ends_at"
  | "ended_at"
  | "past_due_at"
>;

/**
 * The lifecycle table: Polar's status, and the dates that bound it. The
 * refresh reads each subscription through it too, so the subscription it
 * keeps and the plan it reports agree.
 */
export function proPlan(row: ProFields, now: number): ProPlan {
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

/**
 * A public URL of one owner that is not deleted, oldest first. `suspended_at`
 * is null while it served at the last reconcile.
 */
export type OwnedUrl = {
  id: string;
  user_id: string;
  machine_id: string | null;
  taken_down_at: number | null;
  suspended_at: number | null;
};

const SELECT_OWNED_URLS = `
  SELECT id, user_id, machine_id, taken_down_at, suspended_at FROM public_urls
  WHERE deleted_at IS NULL`;

/** `?1` is a JSON array of user ids: their URLs that are not deleted, in serving order. */
export const OWNED_URLS_SQL = `${SELECT_OWNED_URLS}
  AND user_id IN (SELECT value FROM json_each(?1)) ORDER BY user_id, created_at, id`;
/** Every URL that is not deleted, by owner, in serving order. */
export const ALL_OWNED_URLS_SQL = `${SELECT_OWNED_URLS} ORDER BY user_id, created_at, id`;

/** `?1` is a JSON array of user ids: their accounts. */
export const ACCOUNTS_SQL = `${SELECT_ACCOUNTS} WHERE u.id IN (SELECT value FROM json_each(?1))`;
/** The accounts of every owner of a URL that is not deleted, or of a serving slot. */
export const ALL_ACCOUNTS_SQL = `${SELECT_ACCOUNTS}
  WHERE u.id IN (SELECT user_id FROM public_urls WHERE deleted_at IS NULL
                 UNION SELECT user_id FROM machines WHERE tunnel_state != 'none')`;

/**
 * D1's clock in Unix milliseconds, read as the statement runs: the quiet
 * period after a stop is stamped and checked on it, never on a Worker's.
 * `julianday('now')` is core SQLite, in D1 and in `node:sqlite` alike.
 */
export const D1_NOW_MS = "CAST(ROUND((julianday('now') - 2440587.5) * 86400000) AS INTEGER)";

/**
 * Moves an owner's last serving stop to now, on D1's clock as the stamping
 * statement runs, and never backwards: an older operation cannot shorten a
 * newer quiet period. Each batch places it as late as its guard allows.
 */
export const STAMP_STOP = `serving_stopped_at = MAX(COALESCE(serving_stopped_at, 0), ${D1_NOW_MS})`;

/**
 * The quiet period's margin assumes a stamping batch, a few indexed
 * writes, commits within milliseconds of its stamp: one slower than this is
 * logged as an error.
 */
export const SLOW_STAMP_MS = 2_000;

/** A statement with its values, for D1 or for `wrangler d1 execute`. */
export type Statement = { sql: string; params: (string | number | null)[] };

/**
 * `?1` a JSON array of URL ids of owner `?3`, `?2` now. Marks for a rotation
 * every machine of the owner that holds a serving slot and has none
 * pending, when one of these URLs has not recorded that it stopped serving.
 * Every machine, not the URL's own: the set is resolved here, at write time,
 * so a claim that moved the URL since it was read cannot escape it. Runs
 * before `SUSPEND_SQL`, which records the stop, so a retry or a second
 * reconcile finds nothing left to rotate (ADR 0027).
 */
const ROTATE_OWNER_SQL = `
  UPDATE machines SET rotate_requested_at = ?2
  WHERE user_id = ?3 AND status = 'active'
    AND (tunnel_state != 'none' OR tunnel_id IS NOT NULL)
    AND rotate_requested_at IS NULL
    AND EXISTS (SELECT 1 FROM public_urls
                WHERE user_id = ?3 AND id IN (SELECT value FROM json_each(?1))
                  AND suspended_at IS NULL)
  RETURNING id, name`;
/**
 * Stamps the owner `?2` with a serving stop, when one of the URLs `?1` has
 * not recorded its own yet: no new tunnel generation of theirs starts
 * within the quiet period after it. Guarded like `ROTATE_OWNER_SQL`, and
 * before `SUSPEND_SQL`.
 */
const STOPPED_SQL = `
  UPDATE users SET ${STAMP_STOP}
  WHERE id = ?2
    AND EXISTS (SELECT 1 FROM public_urls
                WHERE user_id = ?2 AND id IN (SELECT value FROM json_each(?1))
                  AND suspended_at IS NULL)`;
/** `?1` a JSON array of URL ids, `?2` now: records that they stopped serving. */
const SUSPEND_SQL = `
  UPDATE public_urls SET suspended_at = ?2
  WHERE id IN (SELECT value FROM json_each(?1)) AND suspended_at IS NULL`;
/** `?1` a JSON array of URL ids: records that they serve again. */
const RESUME_SQL = `
  UPDATE public_urls SET suspended_at = NULL
  WHERE id IN (SELECT value FROM json_each(?1)) AND suspended_at IS NOT NULL`;

/**
 * The writes for URLs of one owner that stopped serving, deleted ones
 * included: the owner-wide rotation, the owner's quiet period, then the
 * record of the stop. All are guarded at write time, so they are no-ops
 * once the stop is recorded.
 */
export function stopStatements(userId: string, urlIds: string[], now: number): Statement[] {
  if (urlIds.length === 0) return [];
  const ids = JSON.stringify(urlIds);
  return [
    { sql: ROTATE_OWNER_SQL, params: [ids, now, userId] },
    { sql: STOPPED_SQL, params: [ids, userId] },
    { sql: SUSPEND_SQL, params: [ids, now] },
  ];
}

/**
 * The writes that bring one owner's URLs in line with an entitlement: the
 * oldest live URLs within the limit serve. URLs that served at the last
 * reconcile and no longer do, taken down or suspended, get `stopStatements`;
 * URLs that serve again get `suspended_at` cleared. `urls` are the owner's,
 * in serving order. From a stale read, the worst they do is an extra
 * rotation or a clear, which the next reconcile corrects.
 */
export function reconcileStatements(
  userId: string,
  entitlement: Entitlement,
  urls: OwnedUrl[],
  now: number,
  guard: Statement | null = null,
): Statement[] {
  const within = withinLimitIds(entitlement, urls);
  const stopped = urls.filter((url) => !within.has(url.id) && url.suspended_at === null);
  const resumed = urls.filter((url) => within.has(url.id) && url.suspended_at !== null);
  const statements = stopStatements(
    userId,
    stopped.map((url) => url.id),
    now,
  );
  if (resumed.length > 0) {
    statements.push({ sql: RESUME_SQL, params: [JSON.stringify(resumed.map((url) => url.id))] });
  }
  return guard === null ? statements : statements.map((statement) => guarded(statement, guard));
}

/**
 * `statement` with `guard` added to its WHERE clause: a condition whose
 * `?1`, `?2`… are the guard's own params, as the entitlement read before
 * the batch is valid only while it holds. A guarded rotation is no longer
 * recognized by `isRotation`.
 */
function guarded(statement: Statement, guard: Statement): Statement {
  const offset = statement.params.length;
  const condition = guard.sql.replace(/\?(\d+)/g, (_, n: string) => `?${Number(n) + offset}`);
  const returning = statement.sql.search(/\bRETURNING\b/);
  const sql =
    returning < 0
      ? `${statement.sql} AND ${condition}`
      : `${statement.sql.slice(0, returning)} AND ${condition}\n  ${statement.sql.slice(returning)}`;
  return { sql, params: [...statement.params, ...guard.params] };
}

/** The ids of the oldest live URLs within the limit. */
function withinLimitIds(entitlement: Entitlement, urls: OwnedUrl[]): Set<string> {
  return new Set(
    urls
      .filter((url) => url.taken_down_at === null)
      .slice(0, servingUrlLimit(entitlement))
      .map((url) => url.id),
  );
}

/**
 * The ids of the URLs that serve: within the limit, and without
 * `suspended_at`. Only a reconcile clears that mark, so a URL that served
 * at no point since its mark, whose machines rotated when it was marked,
 * never serves before the reconcile has seen it again.
 */
export function servingUrlIds(entitlement: Entitlement, urls: OwnedUrl[]): Set<string> {
  const within = withinLimitIds(entitlement, urls);
  return new Set(
    urls.filter((url) => within.has(url.id) && url.suspended_at === null).map((url) => url.id),
  );
}

/** Whether a statement is a rotation, whose rows are the machines it marked. */
export function isRotation(statement: Statement): boolean {
  return statement.sql === ROTATE_OWNER_SQL;
}
