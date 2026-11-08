// Localify Cloud Pro and its add-ons, billed through Polar
// (docs/public-urls.md, "Billing"; ADR 0024): the checkouts, the customer
// portal, the webhooks, the refresh that keeps the `billing` snapshot, the
// revocation of duplicate subscriptions, the cancellation of the add-ons of
// a lapsed plan, and the daily reconciliation.
//
// Polar is never the source of truth through a webhook payload: an event
// only names a user, whose subscriptions a refresh then reads from Polar.
// Every refresh takes the next per-user revision, and only the latest one
// that wrote may mark its events processed, so a late, repeated or
// out-of-order delivery changes nothing.

import { CallBudget } from "./cloudflare";
import {
  addonProduct,
  addonsOnSale,
  ConfigError,
  proProducts,
  webhookSecret,
  type AddonKind,
} from "./config";
import { constantTimeEqual } from "./crypto";
import {
  ACCOUNT_SQL,
  addonsOf,
  loadAccount,
  proLapsed,
  proPlan,
  type AccountRow,
  type AddonGrant,
  type ProFields,
  type Statement,
} from "./entitlement";
import { ApiError, json, readLimitedBytes } from "./http";
import { newOp } from "./machines";
import {
  isRateLimited,
  PolarApi,
  PolarError,
  type Checkout,
  type PolarSubscription,
} from "./polar";
import { meteredDb } from "./queries";
import { pushToRelay } from "./relay";
import { forgetSite } from "./sites";
import { reconcileOwnerIf } from "./tunnels";
import { PRO_UPGRADE } from "./upgrade";

const MINUTE = 60 * 1000;
const DAY = 24 * 60 * MINUTE;

/** A webhook more than this far from our clock is refused. */
const WEBHOOK_TOLERANCE_S = 5 * 60;
/** Polar's subscription payloads embed the product and its prices: room enough. */
const MAX_WEBHOOK_BYTES = 1024 * 1024;
/** A failed refresh's events are retried after 1 minute, doubling, up to 6 hours. */
const RETRY_BASE_MS = MINUTE;
const MAX_RETRY_MS = 6 * 60 * MINUTE;
/** A pending event retried this many times is logged as an error. */
const RETRY_ALERT_ATTEMPTS = 10;
/** Processed events are kept this long. */
const EVENT_RETENTION_MS = 30 * DAY;
/** A customer with more subscription pages than this is refused rather than half read. */
const MAX_PAGES_PER_USER = 5;
/** Tries of a refresh that keeps meeting another one's duplicates. */
const REFRESH_ATTEMPTS = 3;
/** A failed revocation or add-on cancellation waits this long, doubling up to a day. */
const POLAR_WRITE_RETRY_BASE_MS = 5 * MINUTE;
/** Canceled add-ons are kept on record this long. */
const CANCELLATION_RETENTION_MS = 30 * DAY;

/** An open checkout is reused while it expires more than this from now. */
const CHECKOUT_REUSE_MARGIN_MS = 5 * MINUTE;
/** A checkout still `creating` after this is taken over. */
const CHECKOUT_CREATING_STALE_MS = 30 * 1000;
const CHECKOUT_SUCCESS_URL = "https://localify.dev/cloud/subscribed?checkout_id={CHECKOUT_ID}";
const CHECKOUT_RETURN_URL = "https://localify.dev/pricing";
const PORTAL_RETURN_URL = "https://localify.dev/cloud";

/**
 * What one 5-minute run may spend: D1 queries, within the 1,000 an
 * invocation may send next to the sweep's 900, and Polar calls.
 */
const JOB_QUERY_BUDGET = 80;
const JOB_CALL_BUDGET = 40;
/** The most queries one refresh makes: its reads, its commit, a reconcile, the slugs, the mark. */
const REFRESH_QUERIES = 8;
const REFRESHES_PER_RUN = 25;
/** An owed serving reconcile: the owner's two reads, its batch, the slugs. */
const OWED_QUERIES = 4;
const OWED_PER_RUN = 10;
/** Revocations a run attempts, each one Polar call and one query, on a Polar budget of their own. */
const REVOCATIONS_PER_RUN = 10;
/** Add-on cancellations a run attempts, after the refreshes, on the run's budgets. */
const CANCELLATIONS_PER_RUN = 10;
/** The most one cancellation makes: its reads and writes, and a restoration after it. */
const CANCELLATION_QUERIES = 6;
const CANCELLATION_CALLS = 2;
/** What the refreshes leave the cancellations: two of them, and their listing. */
const CANCELLATION_RESERVE_QUERIES = 1 + 2 * CANCELLATION_QUERIES;
const CANCELLATION_RESERVE_CALLS = 2 * CANCELLATION_CALLS;
/** Pages of the organization's subscriptions the daily reconciliation reads. */
const DAILY_PAGES = 20;

// --- Webhooks ---

/** `POST /v1/webhooks/polar`: verifies, records, answers 202, then refreshes the user. */
export async function handlePolarWebhook(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
): Promise<Response> {
  const secret = webhookSecret(env);
  if (secret === null) {
    console.error("A Polar webhook arrived, but POLAR_WEBHOOK_SECRET is not set");
    return json(
      { error: "not_configured", message: "Webhooks are not set up yet. Polar retries later." },
      503,
    );
  }
  const body = await readLimitedBytes(request, MAX_WEBHOOK_BYTES);
  if (body === null) {
    return json({ error: "body_too_large", message: "The webhook body is too large." }, 413);
  }
  const id = request.headers.get("webhook-id") ?? "";
  if (!(await verifyWebhook(secret, request.headers, body, Date.now()))) {
    return json(
      { error: "invalid_signature", message: "The webhook signature is not valid." },
      403,
    );
  }

  const { type, externalId } = readEvent(body);
  const now = Date.now();
  // Only an existing user is recorded as the event's user. Any other event
  // is recorded as processed, so a repeat is still recognized.
  const inserted = await env.DB.prepare(
    `INSERT INTO webhook_events (id, type, user_id, received_at, processed_at, next_attempt_at)
     VALUES (?1, ?2, (SELECT id FROM users WHERE id = ?3), ?4,
             CASE WHEN EXISTS (SELECT 1 FROM users WHERE id = ?3) THEN NULL ELSE ?4 END,
             CASE WHEN EXISTS (SELECT 1 FROM users WHERE id = ?3) THEN ?5 ELSE NULL END)
     ON CONFLICT (id) DO NOTHING
     RETURNING user_id`,
  )
    .bind(id, type, externalId, now, now + RETRY_BASE_MS)
    .first<{ user_id: string | null }>();
  const userId = inserted?.user_id ?? null;
  if (userId !== null) {
    ctx.waitUntil(
      refreshBilling(env, userId, new PolarApi(env), now).catch((error: unknown) =>
        console.error(
          "A billing refresh after a webhook failed; the 5-minute job retries it",
          error,
        ),
      ),
    );
  }
  return new Response(null, { status: 202, headers: { "cache-control": "no-store" } });
}

/**
 * Standard Webhooks: `webhook-signature` holds space-separated `v1,<base64>`
 * values, each an HMAC-SHA-256 of `<id>.<timestamp>.<raw body>` keyed with
 * the base64 part of `whsec_<base64>`. The body is checked byte for byte, as
 * it came.
 */
export async function verifyWebhook(
  secret: string,
  headers: Headers,
  body: Uint8Array,
  now: number,
): Promise<boolean> {
  const id = headers.get("webhook-id") ?? "";
  const timestamp = headers.get("webhook-timestamp") ?? "";
  const signatures = headers.get("webhook-signature") ?? "";
  if (id === "" || id.length > 256 || !/^\d{1,12}$/.test(timestamp) || signatures === "") {
    return false;
  }
  if (Math.abs(now / 1000 - Number(timestamp)) > WEBHOOK_TOLERANCE_S) return false;
  const key = fromBase64(secret.startsWith("whsec_") ? secret.slice("whsec_".length) : secret);
  if (key === null || key.length === 0) return false;
  const prefix = new TextEncoder().encode(`${id}.${timestamp}.`);
  const signed = new Uint8Array(prefix.length + body.length);
  signed.set(prefix);
  signed.set(body, prefix.length);
  const cryptoKey = await crypto.subtle.importKey(
    "raw",
    key,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const expected = toBase64(new Uint8Array(await crypto.subtle.sign("HMAC", cryptoKey, signed)));
  return signatures.split(" ").some((entry) => {
    const comma = entry.indexOf(",");
    return (
      comma > 0 &&
      entry.slice(0, comma) === "v1" &&
      constantTimeEqual(entry.slice(comma + 1), expected)
    );
  });
}

/**
 * The event's type, and the user it names: the customer's external id, on
 * `subscription.*` in `data.customer.external_id` and on
 * `customer.state_changed` in `data.external_id`. Nothing else is read.
 */
function readEvent(body: Uint8Array): { type: string; externalId: string | null } {
  type Event = { type?: unknown; data?: Record<string, unknown> } | null;
  let event: Event;
  try {
    event = JSON.parse(new TextDecoder().decode(body)) as Event;
  } catch {
    event = null;
  }
  const type = typeof event?.type === "string" ? event.type.slice(0, 100) : "unknown";
  const data = event?.data;
  let externalId: unknown = null;
  if (type.startsWith("subscription.")) {
    externalId = (data?.customer as { external_id?: unknown } | null | undefined)?.external_id;
  } else if (type === "customer.state_changed") {
    externalId = data?.external_id;
  }
  return { type, externalId: typeof externalId === "string" ? externalId : null };
}

// --- Refresh ---

export type RefreshResult = "committed" | "superseded";

/** What the refresh keeps of the user's subscriptions. */
type Snapshot = {
  customerId: string | null;
  pro: PolarSubscription | null;
  /** The add-on subscriptions that count now, each with its own lifecycle, by id. */
  addonGrants: AddonGrant[];
  trialUsed: boolean;
  /** Entitled Pro subscriptions besides the one kept, not recorded yet. */
  duplicates: string[];
  /** Add-on subscriptions that still renew although their Pro plan lapsed. */
  lapsedAddons: string[];
};

/**
 * Refreshes one existing user from Polar's subscription list. It takes the
 * next revision and notes the user's highest stored event first; its
 * snapshot and the work after it commit only while that revision is the
 * latest written. A refresh that loses to a newer one is not a failure:
 * that one read Polar later, and marks the events. One that fails leaves
 * its events pending, with a backoff, and the user queued.
 */
export async function refreshBilling(
  env: Env,
  userId: string,
  polar: PolarApi,
  now = Date.now(),
): Promise<RefreshResult> {
  for (let attempt = 1; ; attempt++) {
    const result = await refreshOnce(env, userId, polar, now, attempt === REFRESH_ATTEMPTS);
    if (result !== "conflict") return result;
  }
}

/**
 * One try. `conflict` when the duplicates it worked out no longer match
 * what D1 holds: another refresh recorded the subscription this one keeps,
 * or keeps one this one would record. It wrote nothing, and the caller
 * starts again from Polar; on the `last` try, that is a failure.
 */
async function refreshOnce(
  env: Env,
  userId: string,
  polar: PolarApi,
  now: number,
  last: boolean,
): Promise<RefreshResult | "conflict"> {
  const [taken, noted, recorded, kept] = await env.DB.batch<Record<string, unknown>>([
    env.DB.prepare(
      `INSERT INTO billing_revisions (user_id, last) VALUES (?1, 1)
       ON CONFLICT (user_id) DO UPDATE SET last = last + 1
       RETURNING last`,
    ).bind(userId),
    env.DB.prepare(`SELECT MAX(seq) AS seq FROM webhook_events WHERE user_id = ?1`).bind(userId),
    env.DB.prepare(`SELECT subscription_id FROM duplicate_subscriptions WHERE user_id = ?1`).bind(
      userId,
    ),
    env.DB.prepare(`SELECT pro_subscription_id FROM billing WHERE user_id = ?1`).bind(userId),
  ]);
  const revision = Number(taken?.results[0]?.last);
  const watermark = (noted?.results[0]?.seq as number | null | undefined) ?? null;
  if (!Number.isInteger(revision)) throw new Error(`No billing revision for ${userId}`);
  try {
    const subscriptions = await subscriptionsOf(polar, userId);
    const known = new Set((recorded?.results ?? []).map((row) => String(row.subscription_id)));
    const keptId = (kept?.results[0]?.pro_subscription_id as string | null | undefined) ?? null;
    const snapshot = snapshotOf(env, subscriptions, known, keptId, now);
    const result = await commit(env, userId, revision, watermark, snapshot, now);
    if (result === "conflict" && last) {
      throw new Error(`The billing refresh of ${userId} kept meeting another one`);
    }
    return result;
  } catch (error) {
    await recordFailure(env, userId, watermark, error, now);
    throw error;
  }
}

/** Every subscription of the customer, every status, page by page. */
async function subscriptionsOf(polar: PolarApi, userId: string): Promise<PolarSubscription[]> {
  const all: PolarSubscription[] = [];
  for (let page = 1; ; page++) {
    const { items, maxPage } = await polar.listSubscriptions(userId, page);
    all.push(...items.filter((item) => item.externalCustomerId === userId));
    if (page >= maxPage) return all;
    if (page >= MAX_PAGES_PER_USER) {
      throw new Error(`${userId} has more than ${MAX_PAGES_PER_USER} pages of subscriptions`);
    }
  }
}

/** A subscription read through the lifecycle table, as the snapshot would keep it. */
function fieldsOf(subscription: PolarSubscription | null): ProFields {
  const interval = subscription?.recurringInterval;
  return {
    pro_status: subscription?.status ?? null,
    pro_interval: interval === "month" || interval === "year" ? interval : null,
    cancel_at_period_end: subscription?.cancelAtPeriodEnd ? 1 : 0,
    current_period_end: subscription?.currentPeriodEnd ?? null,
    trial_end: subscription?.trialEnd ?? null,
    ends_at: subscription?.endsAt ?? null,
    ended_at: subscription?.endedAt ?? null,
    past_due_at: subscription?.pastDueAt ?? null,
  };
}

/** When a subscription stops on its own: never, while it renews. */
function endOf(subscription: PolarSubscription): number {
  if (!subscription.cancelAtPeriodEnd && subscription.endsAt === null) return Infinity;
  return subscription.endsAt ?? subscription.currentPeriodEnd ?? Infinity;
}

/** Ending latest first, then the later period, the later start, and the id. */
function endingLatestFirst(a: PolarSubscription, b: PolarSubscription): number {
  return (
    compare(endOf(b), endOf(a)) ||
    compare(b.currentPeriodEnd ?? 0, a.currentPeriodEnd ?? 0) ||
    compare(b.startedAt ?? 0, a.startedAt ?? 0) ||
    (a.id < b.id ? 1 : a.id > b.id ? -1 : 0)
  );
}

function compare(a: number, b: number): number {
  return a === b ? 0 : a < b ? -1 : 1;
}

/**
 * The Pro plan is an entitled Pro subscription: the one `billing` already
 * keeps while it is entitled, otherwise the one ending latest; and without
 * any, the most recently started one. The other entitled ones are
 * duplicates. A subscription already recorded as a duplicate is never kept:
 * it is on its way to being revoked.
 */
function snapshotOf(
  env: Env,
  subscriptions: PolarSubscription[],
  recordedDuplicates: Set<string>,
  keptId: string | null,
  now: number,
): Snapshot {
  const { month, year } = proProducts(env);
  const pros = subscriptions.filter((s) => s.productId === month || s.productId === year);
  const candidates = pros.filter((s) => !recordedDuplicates.has(s.id));
  const entitled = candidates
    .filter((s) => proPlan(fieldsOf(s), now).entitled)
    .sort(endingLatestFirst);
  const pro =
    entitled.find((s) => s.id === keptId) ??
    entitled[0] ??
    [...candidates].sort((a, b) => compare(b.startedAt ?? 0, a.startedAt ?? 0))[0] ??
    null;

  // Each add-on that counts now is kept with its own lifecycle, which the
  // entitlement reads again at every use: one set to cancel stops counting
  // when its period ends, and a past_due one when the grace does, with no
  // refresh in between.
  const machine = env.POLAR_PRODUCT_ADDON_MACHINE?.trim() ?? "";
  const urls = env.POLAR_PRODUCT_ADDON_URLS?.trim() ?? "";
  const addons = subscriptions.filter(
    (s) => s.productId !== "" && (s.productId === machine || s.productId === urls),
  );
  const addonGrants = addons
    .filter((s) => proPlan(fieldsOf(s), now).entitled)
    .map(
      (s): AddonGrant => ({
        id: s.id,
        kind: s.productId === machine ? "machine" : "urls",
        units: s.units ?? 1,
        status: s.status,
        cancel_at_period_end: s.cancelAtPeriodEnd,
        current_period_end: s.currentPeriodEnd,
        trial_end: s.trialEnd,
        ends_at: s.endsAt,
        ended_at: s.endedAt,
        past_due_at: s.pastDueAt,
      }),
    )
    .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));

  return {
    customerId: pro?.customerId ?? subscriptions.find((s) => s.customerId)?.customerId ?? null,
    pro,
    addonGrants,
    trialUsed: pros.some((s) => s.trialStart !== null),
    duplicates: entitled.filter((s) => s !== pro).map((s) => s.id),
    lapsedAddons: proLapsed(fieldsOf(pro), now) ? addons.filter(renews).map((s) => s.id) : [],
  };
}

/** Whether a subscription will charge again: not ended, and not set to cancel. */
function renews(subscription: PolarSubscription): boolean {
  return (
    (subscription.status === "active" ||
      subscription.status === "trialing" ||
      subscription.status === "past_due") &&
    !subscription.cancelAtPeriodEnd &&
    subscription.endedAt === null
  );
}

/** `?1` the user, `?2` the revision: whether that revision is the one `billing` holds. */
const HOLDS_REVISION = `EXISTS (SELECT 1 FROM billing WHERE user_id = ?1 AND revision = ?2)`;

/**
 * `?1` the user, `?2` the revision, `?3` the subscription kept, `?4` a JSON
 * array of the duplicates found: whether this snapshot may be written. Its
 * revision is newer than the one `billing` holds, the subscription it keeps
 * is not recorded as a duplicate, and the one `billing` keeps is not among
 * its duplicates. Every write of the commit checks it before the snapshot
 * changes, so they all see the same answer.
 */
const MAY_WRITE = `?2 > COALESCE((SELECT revision FROM billing WHERE user_id = ?1), 0)
  AND NOT EXISTS (SELECT 1 FROM duplicate_subscriptions WHERE subscription_id = ?3)
  AND NOT EXISTS (SELECT 1 FROM billing WHERE user_id = ?1
                  AND pro_subscription_id IN (SELECT value FROM json_each(?4)))`;

/**
 * Commits the snapshot in one batch: the serving reconcile owed when the
 * snapshot changes, the duplicates found, the add-ons of a lapsed plan, and
 * the snapshot itself, each under `MAY_WRITE`; then, while the revision
 * holds, the trial history and the end of the user's turn in the
 * reconciliation queue. Once committed,
 * the owed reconcile runs, guarded by the revision; the events at or below
 * the watermark are processed only after it.
 */
async function commit(
  env: Env,
  userId: string,
  revision: number,
  watermark: number | null,
  snapshot: Snapshot,
  now: number,
): Promise<RefreshResult | "conflict"> {
  const pro = fieldsOf(snapshot.pro);
  const decides = [userId, revision, snapshot.pro?.id ?? null, JSON.stringify(snapshot.duplicates)];
  const fields = [
    pro.pro_status,
    pro.pro_interval,
    pro.cancel_at_period_end,
    pro.current_period_end,
    pro.trial_end,
    pro.ends_at,
    pro.ended_at,
    pro.past_due_at,
    JSON.stringify(snapshot.addonGrants),
  ];
  const [, duplicates, , , , , held, owed] = await env.DB.batch<Record<string, unknown>>([
    // Any change of the snapshot may change what serves.
    env.DB.prepare(
      `UPDATE billing_revisions SET serving_due_at = COALESCE(serving_due_at, ?5)
       WHERE user_id = ?1 AND ${MAY_WRITE}
         AND NOT EXISTS (SELECT 1 FROM billing WHERE user_id = ?1
           AND pro_status IS ?6 AND pro_interval IS ?7 AND cancel_at_period_end = ?8
           AND current_period_end IS ?9 AND trial_end IS ?10 AND ends_at IS ?11
           AND ended_at IS ?12 AND past_due_at IS ?13 AND addon_grants = ?14)`,
    ).bind(...decides, now, ...fields),
    // Recorded before any call to Polar: a revoked subscription no longer
    // looks like a duplicate, and the operator refunds from this record.
    env.DB.prepare(
      `INSERT INTO duplicate_subscriptions (subscription_id, user_id, created_at)
       SELECT value, ?1, ?5 FROM json_each(?4) WHERE ${MAY_WRITE}
       ON CONFLICT (subscription_id) DO NOTHING
       RETURNING subscription_id`,
    ).bind(...decides, now),
    // At this snapshot's revision: the 5-minute job cancels them at period
    // end only while `billing` still holds it. One canceled before that
    // renews again, set back in the portal while Pro is still lapsed: it is
    // canceled again. A pending one keeps its backoff.
    env.DB.prepare(
      `INSERT INTO addon_cancellations (subscription_id, user_id, revision, created_at)
       SELECT value, ?1, ?2, ?5 FROM json_each(?6) WHERE ${MAY_WRITE}
       ON CONFLICT (subscription_id) DO UPDATE SET revision = excluded.revision,
         attempts = CASE WHEN state = 'pending' THEN attempts ELSE 0 END,
         next_attempt_at = CASE WHEN state = 'pending' THEN next_attempt_at END,
         state = 'pending', canceled_at = NULL`,
    ).bind(...decides, now, JSON.stringify(snapshot.lapsedAddons)),
    env.DB.prepare(
      `INSERT INTO billing (user_id, revision, pro_subscription_id, refreshed_at, pro_status,
                            pro_interval, cancel_at_period_end, current_period_end, trial_end,
                            ends_at, ended_at, past_due_at, addon_grants, polar_customer_id)
       SELECT ?1, ?2, ?3, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
       WHERE ${MAY_WRITE}
       ON CONFLICT (user_id) DO UPDATE SET
         polar_customer_id = excluded.polar_customer_id,
         pro_subscription_id = excluded.pro_subscription_id,
         pro_status = excluded.pro_status, pro_interval = excluded.pro_interval,
         cancel_at_period_end = excluded.cancel_at_period_end,
         current_period_end = excluded.current_period_end, trial_end = excluded.trial_end,
         ends_at = excluded.ends_at, ended_at = excluded.ended_at,
         past_due_at = excluded.past_due_at, addon_grants = excluded.addon_grants,
         revision = excluded.revision,
         refreshed_at = excluded.refreshed_at`,
    ).bind(...decides, now, ...fields, snapshot.customerId),
    env.DB.prepare(
      `UPDATE users SET trial_used_at = ?3
       WHERE id = ?1 AND trial_used_at IS NULL AND ?4 = 1 AND ${HOLDS_REVISION}`,
    ).bind(userId, revision, now, snapshot.trialUsed ? 1 : 0),
    // A turn queued during this refresh stays queued.
    env.DB.prepare(
      `UPDATE billing_revisions SET check_due_at = NULL
       WHERE user_id = ?1 AND check_due_at <= ?3 AND ${HOLDS_REVISION}`,
    ).bind(userId, revision, now),
    env.DB.prepare(`SELECT revision FROM billing WHERE user_id = ?1`).bind(userId),
    env.DB.prepare(`SELECT serving_due_at FROM billing_revisions WHERE user_id = ?1`).bind(userId),
  ]);
  const holds = (held?.results[0]?.revision as number | undefined) ?? 0;
  if (holds > revision) return "superseded";
  if (holds !== revision) return "conflict";

  for (const row of duplicates?.results ?? []) {
    console.error(
      `Duplicate Pro subscription ${String(row.subscription_id)} of user ${userId}: it is revoked, and an operator refunds it (pnpm cloud:admin duplicates)`,
    );
  }
  if (owed?.results[0]?.serving_due_at != null) {
    await pushLostRemoteAccess(env, userId, revision, now);
    await applyEntitlement(env, userId, revision, now);
  }
  if (watermark !== null) {
    await env.DB.prepare(
      // Only once no serving reconcile is owed: a reconcile this refresh
      // was overtaken in, or a newer one's that failed, keeps them pending.
      `UPDATE webhook_events SET processed_at = ?3, next_attempt_at = NULL
       WHERE user_id = ?1 AND seq <= ?2 AND processed_at IS NULL
         AND NOT EXISTS (SELECT 1 FROM billing_revisions
                         WHERE user_id = ?1 AND serving_due_at IS NOT NULL)`,
    )
      .bind(userId, watermark, now)
      .run();
  }
  return "committed";
}

/**
 * The serving reconcile owed for the user (docs/public-urls.md,
 * "Downgrades"): URLs suspended or resumed and machines rotated, in one
 * batch whose every write holds only while `billing` still holds
 * `revision`, with the debt cleared in it. A refresh overtaken by a newer
 * one changes nothing and leaves the debt to that one. A batch that fails
 * throws, and the debt stays for the 5-minute job.
 */
async function applyEntitlement(
  env: Env,
  userId: string,
  revision: number,
  now: number,
): Promise<void> {
  const guard: Statement = { sql: HOLDS_REVISION, params: [userId, revision] };
  await reconcileOwnerIf(
    env,
    userId,
    guard,
    [
      {
        sql: `UPDATE billing_revisions SET serving_due_at = NULL WHERE user_id = ?1 AND ${HOLDS_REVISION}`,
        params: [userId, revision],
      },
    ],
    now,
  );
  const { results } = await env.DB.prepare(
    `SELECT slug FROM public_urls WHERE user_id = ?1 AND deleted_at IS NULL`,
  )
    .bind(userId)
    .all<{ slug: string }>();
  for (const { slug } of results) forgetSite(slug);
}

/**
 * A committed change of the snapshot that leaves the account without
 * remote access closes its relay sockets at once (docs/remote-machines.md,
 * "Entitlement"). The relay reads the account again before closing
 * anything, so a push that arrives after a newer refresh gave access back
 * closes nothing. It does nothing when no socket is open, and its lazy
 * checks and the leases cover a push that fails.
 */
async function pushLostRemoteAccess(
  env: Env,
  userId: string,
  revision: number,
  now: number,
): Promise<void> {
  const account = await loadAccount(env.DB, userId, now);
  if (!account || account.entitlement.remoteAccess) return;
  await pushToRelay(env, userId, (relay) => relay.revokeAccount(revision));
}

/**
 * A failed refresh: its events wait 1 minute, doubling up to 6 hours, and
 * the user's turn in the reconciliation queue goes to the back. Polar's
 * rate limit is no failure: the events and the user's turn wait what it
 * asks, and the events keep their count.
 */
async function recordFailure(
  env: Env,
  userId: string,
  watermark: number | null,
  error: unknown,
  now: number,
): Promise<void> {
  const limited = isRateLimited(error);
  try {
    const [events] = await env.DB.batch<{ id: string; attempts: number }>([
      limited
        ? env.DB.prepare(
            `UPDATE webhook_events SET next_attempt_at = ?3
             WHERE user_id = ?1 AND seq <= ?2 AND processed_at IS NULL
             RETURNING id, attempts`,
          ).bind(userId, watermark ?? 0, now + Math.max(error.retryAfterMs, RETRY_BASE_MS))
        : env.DB.prepare(
            `UPDATE webhook_events
             SET attempts = attempts + 1,
                 next_attempt_at = ?3 + MIN(${RETRY_BASE_MS} * (1 << MIN(attempts, 12)), ${MAX_RETRY_MS})
             WHERE user_id = ?1 AND seq <= ?2 AND processed_at IS NULL
             RETURNING id, attempts`,
          ).bind(userId, watermark ?? 0, now),
      env.DB.prepare(
        `UPDATE billing_revisions SET check_due_at = ?2
         WHERE user_id = ?1 AND check_due_at IS NOT NULL`,
      ).bind(userId, limited ? now + Math.max(error.retryAfterMs, RETRY_BASE_MS) : now),
    ]);
    for (const event of events?.results ?? []) {
      if (!limited && event.attempts >= RETRY_ALERT_ATTEMPTS) {
        console.error(
          `Polar webhook ${event.id} of user ${userId} still fails after ${event.attempts} refreshes`,
        );
      }
    }
  } catch (failure) {
    console.error("Recording a failed billing refresh failed", failure);
  }
}

// --- The 5-minute job ---

/**
 * Within its budgets: revokes duplicate subscriptions; refreshes the users
 * with a webhook retry due, then those due for the reconciliation, oldest
 * first; cancels the add-ons of lapsed plans against the snapshots those
 * refreshes left; then runs the serving reconciles still owed. Nothing reaches Polar when nothing is due, and
 * Polar's rate limit ends that part of the run.
 */
export async function runBillingJobs(jobEnv: Env, now = Date.now()): Promise<void> {
  const queries = new CallBudget(JOB_QUERY_BUDGET);
  const env: Env = { ...jobEnv, DB: meteredDb(jobEnv.DB, queries) };
  try {
    // First, and on their own Polar budget: refresh work never holds them back.
    await revokeDuplicates(env, now, queries);
    const calls = new CallBudget(JOB_CALL_BUDGET);
    let polar: PolarApi | null = null;
    const api = () => (polar ??= new PolarApi(env, calls));
    const { results: due } = await env.DB.prepare(
      `SELECT user_id FROM (
         SELECT user_id, 0 AS turn, next_attempt_at AS due FROM webhook_events
         WHERE processed_at IS NULL AND user_id IS NOT NULL AND next_attempt_at <= ?1
         UNION ALL
         SELECT user_id, 1, check_due_at FROM billing_revisions WHERE check_due_at <= ?1)
       GROUP BY user_id ORDER BY MIN(turn), MIN(due), user_id LIMIT ?2`,
    )
      .bind(now, REFRESHES_PER_RUN)
      .all<{ user_id: string }>();
    for (const { user_id: userId } of due) {
      if (
        !queries.has(REFRESH_QUERIES + OWED_QUERIES + CANCELLATION_RESERVE_QUERIES + 1) ||
        !calls.has(1 + CANCELLATION_RESERVE_CALLS)
      )
        break;
      try {
        await refreshBilling(env, userId, api(), now);
      } catch (error) {
        if (error instanceof ConfigError) throw error;
        if (isRateLimited(error)) {
          console.warn("Polar's rate limit stops this run's billing refreshes");
          break;
        }
        console.error("A billing refresh failed; it is retried", userId, error);
      }
    }
    await cancelLapsedAddons(env, now, queries, calls, api);
    await runOwedReconciles(env, now, queries);
  } catch (error) {
    if (!(error instanceof ConfigError)) throw error;
    console.error("Billing jobs need Polar's settings:", error.message);
  }
}

/**
 * The serving reconciles a refresh committed but could not finish, each
 * guarded by the revision `billing` holds now, every run. The ones owed
 * longest go first, and one that fails goes to the back, so a few that
 * keep failing cannot hold back the others.
 */
async function runOwedReconciles(env: Env, now: number, queries: CallBudget): Promise<void> {
  if (!queries.has(1 + OWED_QUERIES + 1)) return;
  const { results } = await env.DB.prepare(
    `SELECT r.user_id, b.revision FROM billing_revisions r JOIN billing b ON b.user_id = r.user_id
     WHERE r.serving_due_at IS NOT NULL ORDER BY r.serving_due_at, r.user_id LIMIT ?1`,
  )
    .bind(OWED_PER_RUN)
    .all<{ user_id: string; revision: number }>();
  for (const { user_id: userId, revision } of results) {
    if (!queries.has(OWED_QUERIES + 1)) return;
    try {
      await applyEntitlement(env, userId, revision, now);
    } catch (error) {
      console.error("An owed serving reconcile failed; it is retried", userId, error);
      await env.DB.prepare(
        `UPDATE billing_revisions SET serving_due_at = ?2
         WHERE user_id = ?1 AND serving_due_at IS NOT NULL`,
      )
        .bind(userId, now)
        .run();
    }
  }
}

type DuplicateRow = { subscription_id: string; user_id: string; attempts: number };

/**
 * Revokes recorded duplicates on a Polar budget of their own: those never
 * tried first, then the others by their next attempt, so one that keeps
 * failing waits its backoff (5 minutes × 2^(attempts − 1), at most a day)
 * behind the rest. Refunds are an operator's: `pnpm cloud:admin
 * duplicates` lists the orders of each duplicate, live from Polar.
 */
async function revokeDuplicates(env: Env, now: number, queries: CallBudget): Promise<void> {
  if (!queries.has(1 + REVOCATIONS_PER_RUN)) return;
  const { results } = await env.DB.prepare(
    `SELECT subscription_id, user_id, attempts FROM duplicate_subscriptions
     WHERE revoked_at IS NULL AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
     ORDER BY next_attempt_at IS NOT NULL, next_attempt_at, created_at, subscription_id
     LIMIT ?2`,
  )
    .bind(now, REVOCATIONS_PER_RUN)
    .all<DuplicateRow>();
  if (results.length === 0) return;
  const polar = new PolarApi(env, new CallBudget(REVOCATIONS_PER_RUN));
  for (const row of results) {
    try {
      await polar.revokeSubscription(row.subscription_id);
      await env.DB.prepare(
        `UPDATE duplicate_subscriptions SET revoked_at = ?2, attempts = 0, next_attempt_at = NULL
         WHERE subscription_id = ?1`,
      )
        .bind(row.subscription_id, now)
        .run();
      console.error(
        `Duplicate Pro subscription ${row.subscription_id} of user ${row.user_id} is revoked: refund it in Polar (pnpm cloud:admin duplicates)`,
      );
    } catch (error) {
      if (error instanceof ConfigError) throw error;
      if (isRateLimited(error)) {
        await env.DB.prepare(
          `UPDATE duplicate_subscriptions SET next_attempt_at = ?2 WHERE subscription_id = ?1`,
        )
          .bind(row.subscription_id, now + error.retryAfterMs)
          .run();
        console.warn("Polar's rate limit stops this run's revocations");
        return;
      }
      const attempts = row.attempts + 1;
      await env.DB.prepare(
        `UPDATE duplicate_subscriptions SET attempts = ?2, next_attempt_at = ?3
         WHERE subscription_id = ?1`,
      )
        .bind(row.subscription_id, attempts, retryAt(now, attempts))
        .run();
      console.error(
        `Revoking duplicate Pro subscription ${row.subscription_id} of user ${row.user_id} failed; it is retried`,
        error,
      );
    }
  }
}

/** A failed revocation or cancellation waits 5 minutes × 2^(attempts − 1), at most a day. */
function retryAt(now: number, attempts: number): number {
  return now + Math.min(POLAR_WRITE_RETRY_BASE_MS * 2 ** (attempts - 1), DAY);
}

type CancellationRow = {
  subscription_id: string;
  user_id: string;
  revision: number;
  state: "pending" | "restoring";
  attempts: number;
};

/** The user's snapshot as D1 holds it now: its revision and its Pro plan. */
type BillingState = ProFields & { revision: number };

async function billingState(env: Env, userId: string): Promise<BillingState | null> {
  return env.DB.prepare(
    `SELECT revision, pro_status, pro_interval, cancel_at_period_end, current_period_end,
            trial_end, ends_at, ended_at, past_due_at
     FROM billing WHERE user_id = ?1`,
  )
    .bind(userId)
    .first<BillingState>();
}

/**
 * Cancels at period end the add-ons a refresh found renewing while their
 * Pro plan had lapsed, after the run's refreshes and within its budgets,
 * without a refund: an add-on needs Pro, so it stops charging for nothing.
 * Each is fenced by the snapshot's revision (`cancelAddon`), and one
 * canceled while the plan came back is set to renew again
 * (`restoreAddon`). Those never tried go first, then the others by their
 * next attempt; one that fails waits like a failed revocation. Polar's rate
 * limit ends them.
 */
async function cancelLapsedAddons(
  env: Env,
  now: number,
  queries: CallBudget,
  calls: CallBudget,
  api: () => PolarApi,
): Promise<void> {
  if (!queries.has(1 + CANCELLATION_QUERIES + OWED_QUERIES + 1)) return;
  if (!calls.has(CANCELLATION_CALLS)) return;
  const { results } = await env.DB.prepare(
    `SELECT subscription_id, user_id, revision, state, attempts FROM addon_cancellations
     WHERE state != 'canceled' AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
     ORDER BY next_attempt_at IS NOT NULL, next_attempt_at, created_at, subscription_id
     LIMIT ?2`,
  )
    .bind(now, CANCELLATIONS_PER_RUN)
    .all<CancellationRow>();
  for (const row of results) {
    if (!queries.has(CANCELLATION_QUERIES + OWED_QUERIES + 1)) return;
    if (!calls.has(CANCELLATION_CALLS)) return;
    try {
      if (row.state === "pending") await cancelAddon(env, row, api, now);
      else await restoreAddon(env, row, api, now);
    } catch (error) {
      if (error instanceof ConfigError) throw error;
      if (isRateLimited(error)) {
        await env.DB.prepare(
          `UPDATE addon_cancellations SET next_attempt_at = ?2 WHERE subscription_id = ?1`,
        )
          .bind(row.subscription_id, now + error.retryAfterMs)
          .run();
        console.warn("Polar's rate limit stops this run's add-on cancellations");
        return;
      }
      const attempts = row.attempts + 1;
      await env.DB.prepare(
        `UPDATE addon_cancellations SET attempts = ?2, next_attempt_at = ?3
         WHERE subscription_id = ?1`,
      )
        .bind(row.subscription_id, attempts, retryAt(now, attempts))
        .run();
      console.error(
        `Canceling or restoring add-on subscription ${row.subscription_id} of user ${row.user_id} failed; it is retried`,
        error,
      );
    }
  }
}

/**
 * Cancels one add-on, fenced by the revision of the refresh that recorded
 * it. Right before the call, D1 must still hold that revision with the plan
 * lapsed; otherwise a newer refresh overtook it, and the record goes (a
 * newer refresh that still finds the plan lapsed records it again, at its
 * own revision). After the call, it is marked canceled only while that
 * still holds. If a refresh committed while Polar answered: still lapsed,
 * the cancellation stands; Pro back, the add-on is set to renew again.
 */
async function cancelAddon(
  env: Env,
  row: CancellationRow,
  api: () => PolarApi,
  now: number,
): Promise<void> {
  const before = await billingState(env, row.user_id);
  if (before?.revision !== row.revision || !proLapsed(before, now)) {
    await env.DB.prepare(
      `DELETE FROM addon_cancellations
       WHERE subscription_id = ?1 AND state = 'pending' AND revision = ?2`,
    )
      .bind(row.subscription_id, row.revision)
      .run();
    return;
  }
  await api().setCancelAtPeriodEnd(row.subscription_id, true);
  const fenced = await env.DB.prepare(
    `UPDATE addon_cancellations SET state = 'canceled', canceled_at = ?3, attempts = 0,
       next_attempt_at = NULL
     WHERE subscription_id = ?1 AND revision = ?2
       AND EXISTS (SELECT 1 FROM billing WHERE user_id = ?4 AND revision = ?2)`,
  )
    .bind(row.subscription_id, row.revision, now, row.user_id)
    .run();
  if (fenced.meta.changes > 0) {
    console.warn(
      `Add-on subscription ${row.subscription_id} of user ${row.user_id} ends with its period: its Pro plan lapsed`,
    );
    return;
  }
  const after = await billingState(env, row.user_id);
  if (after !== null && proLapsed(after, now)) {
    await env.DB.prepare(
      `UPDATE addon_cancellations SET state = 'canceled', canceled_at = ?3, revision = ?2,
         attempts = 0, next_attempt_at = NULL
       WHERE subscription_id = ?1`,
    )
      .bind(row.subscription_id, after.revision, now)
      .run();
    return;
  }
  // Pro came back while the cancellation was on its way.
  await env.DB.prepare(
    `UPDATE addon_cancellations SET state = 'restoring', revision = ?2, attempts = 0,
       next_attempt_at = NULL
     WHERE subscription_id = ?1`,
  )
    .bind(row.subscription_id, after?.revision ?? row.revision)
    .run();
  await restoreAddon(env, { ...row, state: "restoring" }, api, now);
}

/**
 * Sets an add-on canceled while its plan came back to renew again, then
 * forgets it. A plan lapsed again since keeps the cancellation instead.
 * Setting it twice is harmless, so a retry is too.
 */
async function restoreAddon(
  env: Env,
  row: CancellationRow,
  api: () => PolarApi,
  now: number,
): Promise<void> {
  const state = await billingState(env, row.user_id);
  if (state !== null && proLapsed(state, now)) {
    await env.DB.prepare(
      `UPDATE addon_cancellations SET state = 'canceled', canceled_at = ?3, revision = ?2,
         attempts = 0, next_attempt_at = NULL
       WHERE subscription_id = ?1 AND state = 'restoring'`,
    )
      .bind(row.subscription_id, state.revision, now)
      .run();
    return;
  }
  await api().setCancelAtPeriodEnd(row.subscription_id, false);
  await env.DB.prepare(
    `DELETE FROM addon_cancellations WHERE subscription_id = ?1 AND state = 'restoring'`,
  )
    .bind(row.subscription_id)
    .run();
  console.warn(
    `Add-on subscription ${row.subscription_id} of user ${row.user_id} renews again: its Pro plan came back`,
  );
}

// --- The daily reconciliation ---

/**
 * Queues for a refresh every user with a snapshot or a checkout, and every
 * customer the organization's subscriptions name, so a delivery that never
 * came costs at most a day. The listing reads `DAILY_PAGES` pages: the
 * newest one, then a window that moves each day, so every page is read
 * within a few days however many there are. The 5-minute job does the
 * refreshes. Processed events older than 30 days go, and so do add-ons
 * canceled more than 30 days ago.
 */
export async function reconcileBilling(env: Env, now = Date.now()): Promise<void> {
  await env.DB.prepare(
    `INSERT INTO billing_revisions (user_id, last, check_due_at)
     SELECT user_id, 0, ?1 FROM (SELECT user_id FROM billing UNION SELECT user_id FROM checkouts)
     WHERE true
     ON CONFLICT (user_id) DO UPDATE SET
       check_due_at = COALESCE(billing_revisions.check_due_at, excluded.check_due_at)`,
  )
    .bind(now)
    .run();

  const found = new Set<string>();
  try {
    const polar = new PolarApi(env, new CallBudget(DAILY_PAGES));
    const read = async (page: number) => {
      const { items, maxPage } = await polar.listSubscriptions(null, page);
      for (const item of items) if (item.externalCustomerId) found.add(item.externalCustomerId);
      return maxPage;
    };
    const others = (await read(1)) - 1;
    const pages = Math.min(others, DAILY_PAGES - 1);
    const start = others > 0 ? (Math.floor(now / DAY) * pages) % others : 0;
    for (let i = 0; i < pages; i++) await read(2 + ((start + i) % others));
  } catch (error) {
    console.error("Listing Polar's subscriptions failed; the users found so far are queued", error);
  }
  if (found.size > 0) {
    await env.DB.prepare(
      `INSERT INTO billing_revisions (user_id, last, check_due_at)
       SELECT id, 0, ?2 FROM users WHERE id IN (SELECT value FROM json_each(?1))
       ON CONFLICT (user_id) DO UPDATE SET
         check_due_at = COALESCE(billing_revisions.check_due_at, excluded.check_due_at)`,
    )
      .bind(JSON.stringify([...found]), now)
      .run();
  }

  await env.DB.prepare(
    `DELETE FROM webhook_events WHERE processed_at IS NOT NULL AND processed_at < ?1`,
  )
    .bind(now - EVENT_RETENTION_MS)
    .run();
  await env.DB.prepare(
    `DELETE FROM addon_cancellations WHERE state = 'canceled' AND canceled_at < ?1`,
  )
    .bind(now - CANCELLATION_RETENTION_MS)
    .run();
}

// --- Checkout and portal ---

export type BillingPage = { kind: "checkout" | "portal"; url: string };

/** What a checkout sells: Pro, with the interval to show first, or a quantity of an add-on. */
export type CheckoutProduct =
  | { kind: "pro"; interval: "month" | "year" }
  | { kind: AddonKind; units: number };

/**
 * A checkout for the user, or the portal when they already have what they
 * ask for: an entitled Pro plan for Pro, where the interval is switched; an
 * add-on subscription of that kind for an add-on, where its quantity is
 * changed. One checkout per user and kind exists at a time, for both Pro
 * intervals, and creating it is serialized through its `checkouts` row. An
 * add-on needs an entitled Pro plan, and add-ons on sale (`ADDONS=on`).
 */
export async function createCheckout(
  env: Env,
  userId: string,
  product: CheckoutProduct,
  clientIp: string | null,
  now = Date.now(),
): Promise<BillingPage> {
  if (product.kind !== "pro" && !addonsOnSale(env)) throw addonsUnavailable();
  const products =
    product.kind === "pro"
      ? product.interval === "month"
        ? // The requested interval first: Polar selects it by default.
          [proProducts(env).month, proProducts(env).year]
        : [proProducts(env).year, proProducts(env).month]
      : [addonProduct(env, product.kind)];
  const row = await env.DB.prepare(ACCOUNT_SQL).bind(userId).first<AccountRow>();
  if (!row) throw new Error(`User ${userId} has no row`);
  const proEntitled = proPlan(row, now).entitled;
  if (product.kind === "pro") {
    if (proEntitled) return { kind: "portal", url: await portalUrl(env, userId) };
  } else {
    if (!proEntitled) {
      throw new ApiError(
        402,
        "subscription_required",
        "Add-ons extend Localify Cloud Pro. Start a 7-day trial with `localify subscribe`, then add what you need.",
        { upgrade: PRO_UPGRADE },
      );
    }
    if (await hasAddon(env, userId, row, product.kind, products[0]!, now)) {
      return { kind: "portal", url: await portalUrl(env, userId) };
    }
  }
  const units = product.kind === "pro" ? 1 : product.units;

  const op = newOp();
  const claimed = await env.DB.prepare(
    `INSERT INTO checkouts (user_id, kind, state, op, units, created_at)
     VALUES (?1, ?6, 'creating', ?2, ?7, ?3)
     ON CONFLICT (user_id, kind) DO UPDATE SET
       state = 'creating', op = excluded.op, units = excluded.units,
       created_at = excluded.created_at, checkout_id = NULL, url = NULL, expires_at = NULL
     WHERE (checkouts.state = 'open'
            AND (COALESCE(checkouts.expires_at, 0) <= ?4 OR checkouts.units != excluded.units))
        OR (checkouts.state = 'creating' AND checkouts.created_at <= ?5)
     RETURNING op`,
  )
    .bind(
      userId,
      op,
      now,
      now + CHECKOUT_REUSE_MARGIN_MS,
      now - CHECKOUT_CREATING_STALE_MS,
      product.kind,
      units,
    )
    .first<{ op: string }>();
  if (claimed?.op !== op) return storedCheckout(env, userId, product.kind);

  let checkout: Checkout;
  try {
    checkout = await new PolarApi(env).createCheckout({
      products,
      externalCustomerId: userId,
      // Add-ons have no trial.
      allowTrial: product.kind === "pro" && row.trial_used_at === null,
      userId,
      successUrl: CHECKOUT_SUCCESS_URL,
      returnUrl: CHECKOUT_RETURN_URL,
      customerIpAddress: clientIp,
      units: product.kind === "pro" ? null : units,
    });
  } catch (error) {
    // Lets the next request try again at once.
    await env.DB.prepare(`DELETE FROM checkouts WHERE user_id = ?1 AND kind = ?2 AND op = ?3`)
      .bind(userId, product.kind, op)
      .run();
    throw error;
  }
  const stored = await env.DB.prepare(
    `UPDATE checkouts SET state = 'open', checkout_id = ?4, url = ?5, expires_at = ?6
     WHERE user_id = ?1 AND kind = ?2 AND op = ?3
     RETURNING url`,
  )
    .bind(userId, product.kind, op, checkout.id, checkout.url, checkout.expiresAt)
    .first<{ url: string }>();
  if (stored) return { kind: "checkout", url: stored.url };
  // Another request took the row over while Polar answered: its checkout
  // is the one, and this one's link is never given out. A Pro subscription
  // made through it anyway would be a duplicate, and revoked.
  console.warn(`Checkout ${checkout.id} of user ${userId} lost its row to another request`);
  return storedCheckout(env, userId, product.kind);
}

/** `409 addons_unavailable`: add-ons are not on sale (`ADDONS` is not `on`). */
export function addonsUnavailable(): ApiError {
  return new ApiError(
    409,
    "addons_unavailable",
    "Add-ons are not on sale yet. Your Pro plan includes 2 serving machines and 10 public URLs.",
  );
}

/**
 * Whether the user already has an add-on subscription of this kind that has
 * not ended: one a quantity change in the portal serves. The snapshot says
 * so for one that counts; otherwise Polar is asked, so a subscription whose
 * webhook has not arrived yet, or a past_due one, is found too.
 */
async function hasAddon(
  env: Env,
  userId: string,
  row: AccountRow,
  kind: AddonKind,
  productId: string,
  now: number,
): Promise<boolean> {
  const counted = addonsOf(row.addon_grants, now);
  if ((kind === "addon_machine" ? counted.machines : counted.urlPacks) > 0) return true;
  const { items } = await new PolarApi(env).listSubscriptions(userId, 1);
  return items.some(
    (s) =>
      s.externalCustomerId === userId &&
      s.productId === productId &&
      (proPlan(fieldsOf(s), now).entitled || s.status === "past_due"),
  );
}

/** The checkout the row holds, or `409 checkout_in_progress` while it has none ready. */
async function storedCheckout(
  env: Env,
  userId: string,
  kind: CheckoutProduct["kind"],
): Promise<BillingPage> {
  const current = await env.DB.prepare(
    `SELECT state, url FROM checkouts WHERE user_id = ?1 AND kind = ?2`,
  )
    .bind(userId, kind)
    .first<{ state: string; url: string | null }>();
  if (current?.state === "open" && current.url) return { kind: "checkout", url: current.url };
  throw new ApiError(
    409,
    "checkout_in_progress",
    "Another checkout for this account is being created.",
  );
}

/** A customer portal session, or `404 no_billing` for a user Polar does not know. */
export async function portalUrl(env: Env, userId: string): Promise<string> {
  const known = await env.DB.prepare(
    `SELECT EXISTS (SELECT 1 FROM billing WHERE user_id = ?1 AND polar_customer_id IS NOT NULL)
         OR EXISTS (SELECT 1 FROM checkouts WHERE user_id = ?1) AS known`,
  )
    .bind(userId)
    .first<{ known: number }>();
  if (!known?.known) throw noBilling();
  try {
    return await new PolarApi(env).createCustomerSession(userId, PORTAL_RETURN_URL);
  } catch (error) {
    // A checkout that was never completed made no customer.
    if (error instanceof PolarError && (error.status === 404 || error.status === 422)) {
      throw noBilling();
    }
    throw error;
  }
}

function noBilling(): ApiError {
  return new ApiError(
    404,
    "no_billing",
    "This account has no subscription yet. Start one with `localify subscribe`.",
  );
}

// --- Base64 ---

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function fromBase64(text: string): Uint8Array | null {
  try {
    const binary = atob(text);
    return Uint8Array.from(binary, (char) => char.charCodeAt(0));
  } catch {
    return null;
  }
}
