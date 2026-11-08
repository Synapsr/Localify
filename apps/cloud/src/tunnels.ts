// An active machine's tunnel (docs/public-urls.md, "Tunnels"; ADRs 0025 and
// 0027): reserved with a serving slot when the machine first has something
// to serve, provisioned and released by operations that hold the row, and
// swept every 5 minutes.
//
// Every write of an operation is conditioned on its `tunnel_op` and its
// `tunnel_generation`, so a stalled operation can neither overwrite a newer
// one nor delete its tunnel. Only the holder of an operation changes a CNAME,
// and the sweep corrects the one change a condition cannot stop: a DNS call
// already on its way.

import {
  CallBudget,
  CloudflareApi,
  CloudflareError,
  LIST_PAGE_SIZE,
  REMOVE_TUNNEL_CALLS,
  type ListedRecord,
  type ListedTunnel,
} from "./cloudflare";
import { maxTunnels, setting } from "./config";
import { urlsOfMachine, type PublicUrl } from "./db";
import {
  accountOf,
  ACCOUNTS_SQL,
  ALL_ACCOUNTS_SQL,
  ALL_OWNED_URLS_SQL,
  entitlementOf,
  isEntitled,
  loadAccount,
  NO_ENTITLEMENT,
  OWNED_URLS_SQL,
  reconcileStatements,
  servingUrlIds,
  servingUrlLimit,
  D1_NOW_MS,
  SLOW_STAMP_MS,
  STAMP_STOP,
  type Account,
  type AccountRow,
  type Entitlement,
  type OwnedUrl,
  type Statement,
} from "./entitlement";
import {
  cleanUpRevokedMachines,
  ingressSecret,
  machineHostname,
  newOp,
  nextAttemptAt,
  tunnelName,
} from "./machines";
import { meteredDb } from "./queries";
import { SITE_CACHE_TTL_MS } from "./sites";
import { addonUpgrade, PRO_UPGRADE, type Upgrade } from "./upgrade";

/**
 * No new tunnel generation of an owner starts until this long after their
 * latest serving stop or tunnel release, on D1's clock. A routing decision
 * lives at most `SITE_CACHE_TTL_MS` from the start of its read, so one
 * made before the stop has ended by then. The 5 seconds cover the skew
 * between a Worker's clock and D1's.
 */
export const QUIET_AFTER_STOP_MS = SITE_CACHE_TTL_MS + 5_000;
/** Runs a batch that stamps a quiet period, logging it as an error when it is slow. */
async function stampingBatch<T>(
  env: Env,
  name: string,
  statements: D1PreparedStatement[],
): Promise<D1Result<T>[]> {
  const started = Date.now();
  const results = await env.DB.batch<T>(statements);
  const took = Date.now() - started;
  if (took > SLOW_STAMP_MS) {
    console.error(
      `The ${name} batch took ${took} ms; the quiet period's margin assumes milliseconds`,
    );
  }
  return results;
}
/** An operation older than this is abandoned. */
const OP_STALE_MS = 2 * 60 * 1000;
/** Two down readings this far apart, on a tunnel ready this long, reap it. */
const DOWN_GRACE_MS = 15 * 60 * 1000;
const ORPHAN_AGE_MS = 60 * 60 * 1000;
const LATE_REVOCATION_MS = 10 * 60 * 1000;
const SWEEP_CALL_BUDGET = 300;
/**
 * D1 queries a sweep may make, below the 1,000 a Worker invocation may send
 * on the paid plan, leaving room for the cron's other work. A batch counts
 * as one.
 */
const SWEEP_QUERY_BUDGET = 900;
/** A release looks the tunnel up by name at most once, then removes it. */
const RELEASE_CALLS = 1 + REMOVE_TUNNEL_CALLS;
/** A release takes the row, then sets `none`. */
const RELEASE_QUERIES = 2;
/** A release that failed outside the revocations records its backoff. */
const BACKOFF_QUERIES = 1;
/** Reading the slots, and the owners of slots reserved since the owners were read. */
const SLOT_QUERIES = 3;
/** The orphan listing and the slot count, at the end of a run. */
const FINAL_QUERIES = 2;
/** The two writes of the status readings: healthy again, and first seen down. */
const STATUS_QUERIES = 2;
/**
 * The revocations every run attempts first, whatever came before them: the
 * reconcile stops while their queries are left, and they take at most
 * 60 × 4 = 240 of the 300 calls.
 */
const REVOCATIONS_PER_RUN = 60;
/**
 * What maintenance keeps after those 60 (reaps, CNAME repairs, the
 * revoked-machine cleanup, orphan tunnels), however many revocations are
 * queued: 240 + 40 fits in 300. Revocations get what it leaves.
 */
const MAINTENANCE_CALLS = 40;
/** The queries 40 calls of maintenance can need: at most 4 per call, and a few reads. */
const MAINTENANCE_QUERIES = 60;
/** What the reconcile leaves: the slot reads, 60 revocations, maintenance, the end of the run. */
const QUERIES_AFTER_RECONCILE =
  SLOT_QUERIES + REVOCATIONS_PER_RUN * RELEASE_QUERIES + MAINTENANCE_QUERIES + FINAL_QUERIES;
/**
 * Statements per reconcile batch. D1 limits a batch to 30 seconds and
 * rolls all of it back on a failure, so one owner's failure, or a slow
 * batch, costs at most its chunk.
 */
const RECONCILE_BATCH = 150;
/** A revoked machine's cleanup records its lookups, the tunnel, the record and its attempts. */
const CLEANUP_QUERIES_PER_MACHINE = 4;
/** Correcting a CNAME takes the row, records the record, and lets the row go. */
const CNAME_QUERIES = 3;
/** Moving a CNAME may find it gone, fail to create it, then look it up and move it. */
const CNAME_CALLS = 4;
const SLOT_ALERT = 800;
const PROVISIONING_RETRY_SECONDS = 3;
const CAPACITY_RETRY_SECONDS = 60;
/** The names of the tunnels this Worker creates: `localify-<machine-id>[-<generation>]`. */
const OWN_TUNNEL = /^localify-[0-9a-f]{24}(-\d+)?$/;

export type MachineRuntime = {
  state: "idle" | "provisioning" | "ready" | "blocked";
  blocked: {
    reason: "subscription_required" | "serving_slots_full" | "capacity";
    message: string;
    /** What lifts the block, when something the account can buy does. */
    upgrade?: Upgrade;
  } | null;
  tunnel: { generation: number; token: string | null } | null;
  ingressSecret: string | null;
  publicUrls: PublicUrl[];
  retryAfter: number | null;
};

type RuntimeCaller = {
  userId: string;
  machineId: string;
};

type TunnelRow = {
  tunnel_state: "none" | "provisioning" | "ready" | "releasing";
  tunnel_generation: number;
  tunnel_op_at: number | null;
  tunnel_id: string | null;
  rotate_requested_at: number | null;
};

/**
 * The runtime of the calling machine. Reserves a slot and starts the
 * provisioning when the machine wants a tunnel and has none, and releases
 * the tunnel when it no longer wants one. The token comes only when
 * `daemonGeneration` is not the current one: `null` always asks for it.
 */
export async function machineRuntime(
  env: Env,
  ctx: ExecutionContext,
  caller: RuntimeCaller,
  daemonGeneration: number | null,
  now = Date.now(),
): Promise<MachineRuntime> {
  const account = await loadAccount(env.DB, caller.userId, now);
  const entitlement = account?.entitlement ?? NO_ENTITLEMENT;
  const publicUrls = await urlsOfMachine(
    env.DB,
    setting(env, "PUBLIC_BASE_DOMAIN"),
    caller.userId,
    caller.machineId,
    servingUrlLimit(entitlement),
  );
  await adoptUntrackedTunnels(env, now, caller.machineId);
  let row = await tunnelRow(env, caller.machineId);
  const base = { blocked: null, tunnel: null, ingressSecret: null, publicUrls, retryAfter: null };
  const provisioning = {
    ...base,
    state: "provisioning",
    retryAfter: PROVISIONING_RETRY_SECONDS,
  } as const;

  if (!wantsTunnel(entitlement, publicUrls)) {
    if (row.tunnel_state !== "none") {
      background(ctx, releaseTunnel(env, caller.machineId, row.tunnel_generation, { now }));
    }
    if (!isEntitled(entitlement) && publicUrls.some((url) => url.state !== "taken_down")) {
      return {
        ...base,
        state: "blocked",
        blocked: {
          reason: "subscription_required",
          message:
            "Public URLs need Localify Cloud Pro. Start a 7-day trial with `localify subscribe`.",
          upgrade: PRO_UPGRADE,
        },
      };
    }
    return { ...base, state: "idle" };
  }

  if (row.tunnel_state === "none") {
    const quiet = await quietLeft(env, caller.userId);
    if (quiet > 0) return { ...provisioning, retryAfter: Math.ceil(quiet / 1000) };
    const reserved = await reserveSlot(env, caller, entitlement.servingMachines, now);
    if (reserved) {
      background(ctx, provisionTunnel(env, reserved));
      return provisioning;
    }
    row = await tunnelRow(env, caller.machineId);
    if (row.tunnel_state === "none") {
      // A stop that landed since the check above.
      const late = await quietLeft(env, caller.userId);
      if (late > 0) return { ...provisioning, retryAfter: Math.ceil(late / 1000) };
      return blockedRuntime(env, caller, account, entitlement, base);
    }
  }

  if (row.tunnel_state === "ready" && row.tunnel_id !== null && row.rotate_requested_at === null) {
    const generation = row.tunnel_generation;
    const [token, secret] = await Promise.all([
      daemonGeneration === generation ? null : new CloudflareApi(env).tunnelToken(row.tunnel_id),
      ingressSecret(env, caller.machineId),
    ]);
    // A revocation may have taken the row while the token was on its way,
    // and its new secret makes Cloudflare hand out a token that works again.
    // Only a row still ready, on the same tunnel, unmarked, may have it.
    const after = await tunnelRow(env, caller.machineId);
    if (
      after.tunnel_state === "ready" &&
      after.tunnel_generation === generation &&
      after.tunnel_id === row.tunnel_id &&
      after.rotate_requested_at === null
    ) {
      return { ...base, state: "ready", tunnel: { generation, token }, ingressSecret: secret };
    }
    row = after;
    if (row.tunnel_state === "none") return provisioning;
  }
  // A rotation, or an operation that stopped: release, and the next fetch
  // provisions the next generation.
  if (row.tunnel_state === "ready" || isStale(row.tunnel_op_at, now)) {
    background(
      ctx,
      releaseTunnel(env, caller.machineId, row.tunnel_generation, {
        now,
        urgent: row.rotate_requested_at !== null,
      }),
    );
  }
  return provisioning;
}

/** A machine wants a tunnel when its owner may serve and one of its URLs serves. */
function wantsTunnel(entitlement: Entitlement, publicUrls: PublicUrl[]): boolean {
  return isEntitled(entitlement) && publicUrls.some((url) => url.state === "serving");
}

function isStale(opAt: number | null, now: number): boolean {
  return opAt === null || opAt <= now - OP_STALE_MS;
}

/** Why a machine that wants a tunnel could not reserve a slot. */
async function blockedRuntime(
  env: Env,
  caller: RuntimeCaller,
  account: Account | null,
  entitlement: Entitlement,
  base: Omit<MachineRuntime, "state">,
): Promise<MachineRuntime> {
  const { results: holders } = await env.DB.prepare(
    `SELECT name FROM machines WHERE user_id = ?1 AND tunnel_state != 'none' ORDER BY name`,
  )
    .bind(caller.userId)
    .all<{ name: string }>();
  if (holders.length >= entitlement.servingMachines) {
    const count = entitlement.servingMachines;
    const upgrade = addonUpgrade(env, account, "addon_machine");
    return {
      ...base,
      state: "blocked",
      blocked: {
        ...(upgrade === null ? {} : { upgrade }),
        reason: "serving_slots_full",
        message: `Your plan serves public URLs from ${count} ${count === 1 ? "machine" : "machines"} at a time, and ${holders
          .map((holder) => holder.name)
          .join(
            ", ",
          )} already ${holders.length === 1 ? "serves" : "serve"}. Remove their URLs, or claim them from this machine.`,
      },
    };
  }
  return {
    ...base,
    state: "blocked",
    blocked: {
      reason: "capacity",
      message:
        "Localify Cloud is serving as many machines as it can right now. Serving resumes by itself as slots free up.",
    },
    retryAfter: CAPACITY_RETRY_SECONDS,
  };
}

async function tunnelRow(env: Env, machineId: string): Promise<TunnelRow> {
  const row = await env.DB.prepare(
    `SELECT tunnel_state, tunnel_generation, tunnel_op_at, tunnel_id, rotate_requested_at
     FROM machines WHERE id = ?1`,
  )
    .bind(machineId)
    .first<TunnelRow>();
  if (!row) throw new Error(`Machine ${machineId} has no row`);
  return row;
}

type Reservation = {
  machineId: string;
  generation: number;
  op: string;
  ingressPort: number;
  dnsRecordId: string | null;
};

/** How long the owner's quiet period still lasts, in milliseconds of D1's clock. */
async function quietLeft(env: Env, userId: string): Promise<number> {
  const row = await env.DB.prepare(
    `SELECT serving_stopped_at + ?2 - ${D1_NOW_MS} AS left FROM users WHERE id = ?1`,
  )
    .bind(userId, QUIET_AFTER_STOP_MS)
    .first<{ left: number | null }>();
  return Math.max(0, row?.left ?? 0);
}

/**
 * Reserves a serving slot in one statement, so concurrent requests can pass
 * neither the owner's limit, nor the account's cap, nor the owner's quiet
 * period. Null when the row is no longer `none` or one of them stops it.
 */
async function reserveSlot(
  env: Env,
  caller: RuntimeCaller,
  servingMachines: number,
  now: number,
): Promise<Reservation | null> {
  const op = newOp();
  const row = await env.DB.prepare(
    `UPDATE machines
     SET tunnel_state = 'provisioning', tunnel_generation = tunnel_generation + 1,
         tunnel_op = ?2, tunnel_op_at = ?3, rotate_requested_at = NULL, down_seen_at = NULL
     WHERE id = ?1 AND status = 'active' AND tunnel_state = 'none'
       AND (SELECT COUNT(*) FROM machines WHERE user_id = ?4 AND tunnel_state != 'none') < ?5
       AND (SELECT COUNT(*) FROM machines WHERE tunnel_state != 'none') < ?6
       AND NOT EXISTS (SELECT 1 FROM users
                       WHERE id = ?4 AND serving_stopped_at > ${D1_NOW_MS} - ?7)
     RETURNING tunnel_generation, ingress_port, dns_record_id`,
  )
    .bind(
      caller.machineId,
      op,
      now,
      caller.userId,
      servingMachines,
      maxTunnels(env),
      QUIET_AFTER_STOP_MS,
    )
    .first<{ tunnel_generation: number; ingress_port: number; dns_record_id: string | null }>();
  return row
    ? {
        machineId: caller.machineId,
        generation: row.tunnel_generation,
        op,
        ingressPort: row.ingress_port,
        dnsRecordId: row.dns_record_id,
      }
    : null;
}

/** The condition every write of a provisioning carries: `?1` id, `?2` op, `?3` generation. */
const HOLDS_PROVISIONING = `id = ?1 AND tunnel_op = ?2 AND tunnel_generation = ?3 AND tunnel_state = 'provisioning'`;

/**
 * Creates the generation's tunnel, sets its ingress, points the CNAME at it,
 * then marks the machine `ready`. An operation that finds the row taken
 * over deletes the tunnel it created and stops. One that fails stays
 * `provisioning` until it counts as abandoned and is released.
 */
async function provisionTunnel(env: Env, reservation: Reservation): Promise<void> {
  const { machineId, op, generation } = reservation;
  const holds = (sql: string, ...values: unknown[]) =>
    env.DB.prepare(sql)
      .bind(machineId, op, generation, ...values)
      .run()
      .then((result) => result.meta.changes > 0);
  let cloudflare: CloudflareApi | null = null;
  let tunnelId: string | null = null;
  try {
    cloudflare = new CloudflareApi(env);
    const hostname = machineHostname(env, machineId);
    tunnelId = await createOrFindTunnel(cloudflare, tunnelName(machineId, generation));
    if (!(await holds(`UPDATE machines SET tunnel_id = ?4 WHERE ${HOLDS_PROVISIONING}`, tunnelId)))
      return await abandon(cloudflare, machineId, tunnelId);
    await cloudflare.configureTunnel(tunnelId, hostname, reservation.ingressPort);
    // Checked right before the DNS call: only the holder moves the CNAME.
    if (
      !(await holds(`UPDATE machines SET tunnel_op_at = tunnel_op_at WHERE ${HOLDS_PROVISIONING}`))
    )
      return await abandon(cloudflare, machineId, tunnelId);
    const recordId = await pointCname(
      env,
      cloudflare,
      machineId,
      tunnelId,
      reservation.dnsRecordId,
    );
    // The CNAME belongs to the machine, not to the generation: recorded even
    // when the row was taken over meanwhile, so a sign-out removes it.
    await env.DB.prepare(`UPDATE machines SET dns_record_id = ?2 WHERE id = ?1`)
      .bind(machineId, recordId)
      .run();
    const ready = await holds(
      `UPDATE machines SET tunnel_state = 'ready', tunnel_ready_at = ?4, down_seen_at = NULL,
              tunnel_op = NULL, tunnel_op_at = NULL, cleanup_attempts = 0, next_cleanup_at = NULL
       WHERE ${HOLDS_PROVISIONING}`,
      Date.now(),
    );
    if (!ready) await abandon(cloudflare, machineId, tunnelId);
  } catch (error) {
    console.error("Provisioning a tunnel failed; the sweep releases it", machineId, error);
  }
}

async function createOrFindTunnel(cloudflare: CloudflareApi, name: string): Promise<string> {
  try {
    return await cloudflare.createTunnel(name);
  } catch (error) {
    // A retry may find the tunnel an earlier call made without answering.
    if (!(error instanceof CloudflareError)) throw error;
    const found = await cloudflare.findTunnelId(name);
    if (found) return found;
    throw error;
  }
}

/** Points the machine's CNAME at the tunnel: moves it when it exists, creates it the first time. */
async function pointCname(
  env: Env,
  cloudflare: CloudflareApi,
  machineId: string,
  tunnelId: string,
  recordId: string | null,
): Promise<string> {
  if (recordId) {
    try {
      await cloudflare.pointRecord(recordId, tunnelId);
      return recordId;
    } catch (error) {
      // Removed by hand: created again below.
      if (!(error instanceof CloudflareError && error.status === 404)) throw error;
    }
  }
  const hostname = machineHostname(env, machineId);
  try {
    return await cloudflare.createTunnelRecord(
      hostname,
      tunnelId,
      `Localify Cloud machine ${machineId}`,
    );
  } catch (error) {
    // An earlier operation created it without recording its id.
    if (!(error instanceof CloudflareError)) throw error;
    const found = await cloudflare.findDnsRecordId(hostname);
    if (!found) throw error;
    await cloudflare.pointRecord(found, tunnelId);
    return found;
  }
}

/** A provisioning that lost its row deletes the tunnel it created. */
async function abandon(cloudflare: CloudflareApi, machineId: string, tunnelId: string) {
  console.warn("A provisioning lost its machine row; deleting its tunnel", machineId);
  await cloudflare.removeTunnel(tunnelId);
}

type ReleaseResult = "released" | "skipped" | "failed";

type ReleaseOptions = {
  now?: number;
  /**
   * A revocation: the release must not wait. `rotate_requested_at` marks it
   * until Cloudflare confirms the deletion, so a failed one stays first in
   * the next sweep, without backoff.
   */
  urgent?: boolean;
  /** The sweep's budget; API requests have none. */
  budget?: CallBudget | null;
  /**
   * Releases only while no live URL is on the machine, checked as the
   * release takes the row: one created since the caller read the URLs
   * keeps its new tunnel.
   */
  ifIdle?: boolean;
};

/**
 * Releases generation `generation` of an active machine, as the holder of a
 * new operation: from `ready`, or from a provisioning or a release that
 * stopped. Removes the tunnel, which closes every open stream, then sets
 * `none` and clears a rotation requested before it started. The CNAME
 * stays: the next tunnel only moves it.
 */
async function releaseTunnel(
  env: Env,
  machineId: string,
  generation: number,
  { now = Date.now(), urgent = false, budget = null, ifIdle = false }: ReleaseOptions = {},
): Promise<ReleaseResult> {
  const op = newOp();
  // Claiming the release stamps the owner's quiet period in the same batch:
  // a request routed before it cannot reach the generation that follows.
  const [claim] = await stampingBatch<{ tunnel_id: string | null }>(env, "release", [
    env.DB.prepare(
      `UPDATE machines SET tunnel_state = 'releasing', tunnel_op = ?3, tunnel_op_at = ?4,
            rotate_requested_at = CASE WHEN ?6 THEN COALESCE(rotate_requested_at, ?4)
                                       ELSE rotate_requested_at END
     WHERE id = ?1 AND status = 'active' AND tunnel_generation = ?2
       AND (tunnel_state = 'ready'
            OR (tunnel_state IN ('provisioning', 'releasing') AND tunnel_op_at <= ?5))
       AND (?7 = 0 OR NOT EXISTS (SELECT 1 FROM public_urls p WHERE p.machine_id = machines.id
                                  AND p.deleted_at IS NULL AND p.taken_down_at IS NULL))
     RETURNING tunnel_id`,
    ).bind(machineId, generation, op, now, now - OP_STALE_MS, urgent ? 1 : 0, ifIdle ? 1 : 0),
    env.DB.prepare(
      `UPDATE users SET ${STAMP_STOP}
       WHERE id = (SELECT user_id FROM machines WHERE id = ?1 AND tunnel_op = ?2)`,
    ).bind(machineId, op),
  ]);
  const taken = claim?.results[0];
  if (!taken) return "skipped";
  try {
    const cloudflare = new CloudflareApi(env, budget);
    const tunnelId =
      taken.tunnel_id ?? (await cloudflare.findTunnelId(tunnelName(machineId, generation)));
    if (tunnelId) await cloudflare.removeTunnel(tunnelId);
  } catch (error) {
    console.error("Releasing a tunnel failed; the sweep retries it", machineId, error);
    return "failed";
  }
  await env.DB.prepare(
    `UPDATE machines SET tunnel_state = 'none', tunnel_id = NULL, tunnel_op = NULL,
            tunnel_op_at = NULL, tunnel_ready_at = NULL, down_seen_at = NULL,
            rotate_requested_at = CASE WHEN rotate_requested_at <= ?3 THEN NULL
                                       ELSE rotate_requested_at END,
            cleanup_attempts = 0, next_cleanup_at = NULL
     WHERE id = ?1 AND tunnel_op = ?2`,
  )
    .bind(machineId, op, now)
    .run();
  return "released";
}

/**
 * Releases a machine's tunnel when the machine no longer wants one: after
 * its URL was deleted or claimed elsewhere. Never throws.
 */
export async function releaseIfUnwanted(env: Env, machineId: string): Promise<void> {
  try {
    const machine = await env.DB.prepare(
      `SELECT user_id, tunnel_state, tunnel_generation FROM machines
       WHERE id = ?1 AND status = 'active'`,
    )
      .bind(machineId)
      .first<{ user_id: string; tunnel_state: string; tunnel_generation: number }>();
    if (!machine || machine.tunnel_state === "none") return;
    const entitlement = await entitlementOf(env, machine.user_id);
    const publicUrls = await urlsOfMachine(
      env.DB,
      setting(env, "PUBLIC_BASE_DOMAIN"),
      machine.user_id,
      machineId,
      servingUrlLimit(entitlement),
    );
    if (!wantsTunnel(entitlement, publicUrls)) {
      await releaseTunnel(env, machineId, machine.tunnel_generation);
    }
  } catch (error) {
    console.error("Releasing an unwanted tunnel failed; the sweep retries it", machineId, error);
  }
}

/** What decides which URLs of some owners serve: their entitlements and their URLs. */
export type Owners = { entitlements: Map<string, Entitlement>; urls: Map<string, OwnedUrl[]> };

/**
 * The owners' accounts and URLs, in two queries whatever their number.
 * `userIds` null means every owner of a URL that is not deleted or of a
 * serving slot.
 */
export async function loadOwners(
  env: Env,
  userIds: string[] | null,
  now = Date.now(),
): Promise<Owners> {
  const list = JSON.stringify(userIds ?? []);
  const [accounts, urls] = await Promise.all([
    userIds === null
      ? env.DB.prepare(ALL_ACCOUNTS_SQL).all<AccountRow>()
      : env.DB.prepare(ACCOUNTS_SQL).bind(list).all<AccountRow>(),
    userIds === null
      ? env.DB.prepare(ALL_OWNED_URLS_SQL).all<OwnedUrl>()
      : env.DB.prepare(OWNED_URLS_SQL).bind(list).all<OwnedUrl>(),
  ]);
  const owners: Owners = { entitlements: new Map(), urls: new Map() };
  for (const row of accounts.results) {
    owners.entitlements.set(row.user_id, accountOf(row, now).entitlement);
  }
  for (const url of urls.results) {
    owners.urls.set(url.user_id, [...(owners.urls.get(url.user_id) ?? []), url]);
  }
  return owners;
}

/**
 * Brings the serving state of the owners' URLs in line with their
 * entitlements. A URL that stopped serving since the last reconcile,
 * through a downgrade, an expiry, a ban or a takedown, gets `suspended_at`,
 * and every serving machine of its owner a rotation, so its open streams
 * close at the next release (ADR 0027). The statements go in batches of
 * about `RECONCILE_BATCH`, each owner's in one batch, and each batch counts
 * as one query. A batch that fails is logged and the others still run; the
 * next reconcile retries it. With a budget, the batches stop while
 * `reserve` queries are left.
 */
export async function reconcileServing(
  env: Env,
  owners: Owners,
  now = Date.now(),
  queries: { budget: CallBudget; reserve: number } | null = null,
): Promise<void> {
  const batches: Statement[][] = [];
  for (const [userId, entitlement] of owners.entitlements) {
    const statements = reconcileStatements(userId, entitlement, owners.urls.get(userId) ?? [], now);
    if (statements.length === 0) continue;
    const last = batches.at(-1);
    // An owner's rotation and the record of its stop always share a batch.
    if (last && last.length + statements.length <= RECONCILE_BATCH) last.push(...statements);
    else batches.push(statements);
  }
  for (const batch of batches) {
    if (queries && !queries.budget.has(1 + queries.reserve)) return;
    try {
      await stampingBatch(
        env,
        "reconcile",
        batch.map(({ sql, params }) => env.DB.prepare(sql).bind(...params)),
      );
    } catch (error) {
      console.error("A reconcile batch failed; the next sweep retries it", error);
    }
  }
}

/** Reconciles one owner, after a change of their URLs. */
export async function reconcileOwner(env: Env, userId: string, now = Date.now()): Promise<void> {
  await reconcileServing(env, await loadOwners(env, [userId], now), now);
}

/**
 * Reconciles one owner in one batch in which every write also meets
 * `guard`, followed by `after`. A billing refresh passes its revision as
 * the guard, so a refresh overtaken by a newer one changes nothing. Unlike
 * `reconcileServing`, a batch that fails throws, so the caller keeps what
 * it owes.
 */
export async function reconcileOwnerIf(
  env: Env,
  userId: string,
  guard: Statement,
  after: Statement[],
  now = Date.now(),
): Promise<void> {
  const owners = await loadOwners(env, [userId], now);
  const statements = [
    ...reconcileStatements(
      userId,
      owners.entitlements.get(userId) ?? NO_ENTITLEMENT,
      owners.urls.get(userId) ?? [],
      now,
      guard,
    ),
    ...after,
  ];
  await stampingBatch(
    env,
    "reconcile",
    statements.map(({ sql, params }) => env.DB.prepare(sql).bind(...params)),
  );
}

/** Runs work after the response, logging what fails. */
export function background(ctx: ExecutionContext, work: Promise<unknown>): void {
  ctx.waitUntil(
    work.catch((error: unknown) => console.error("Background tunnel work failed", error)),
  );
}

// --- The sweep ---

type SlotRow = {
  id: string;
  user_id: string;
  tunnel_state: "provisioning" | "ready" | "releasing";
  tunnel_generation: number;
  tunnel_op_at: number | null;
  tunnel_id: string | null;
  tunnel_ready_at: number | null;
  down_seen_at: number | null;
  rotate_requested_at: number | null;
  dns_record_id: string | null;
  cleanup_attempts: number;
  next_cleanup_at: number | null;
};

/**
 * The 5-minute sweep, within a budget of Cloudflare calls and one of D1
 * queries. It first marks for a rotation the machines of URLs that stopped
 * serving. Revocations come next: rotations, and the slots of owners who
 * lost their entitlement or hold more than it allows. The first 60 are
 * attempted whatever else waits, then more while maintenance keeps its
 * share: the revoked machines' cleanup, the status readings and reaps,
 * abandoned operations, machines with nothing to serve, CNAME drift and
 * orphan tunnels. What maintenance leaves goes to the revocations still
 * queued.
 */
export async function runSweep(sweptEnv: Env, now = Date.now()): Promise<void> {
  const budget = new CallBudget(SWEEP_CALL_BUDGET);
  const queries = new CallBudget(SWEEP_QUERY_BUDGET);
  const env: Env = { ...sweptEnv, DB: meteredDb(sweptEnv.DB, queries) };
  await adoptUntrackedTunnels(env, now, null);
  // Every owner's entitlement and URLs in two queries, for the reconcile and
  // the rest of the run. The reconcile goes before the slots are read, so
  // the rotations it marks are revoked in this run.
  const owners = await loadOwners(env, null, now);
  try {
    await reconcileServing(env, owners, now, { budget: queries, reserve: QUERIES_AFTER_RECONCILE });
  } catch (error) {
    console.error("Reconciling the serving URLs failed; the next sweep retries it", error);
  }
  // Serving is off (the sandbox): no slot is ever reserved, so there is no
  // tunnel of ours to sweep, and the account's tunnels and records belong to
  // another environment. Nothing below may list or delete them.
  if (maxTunnels(env) === 0) return;
  const { results: slots } = await env.DB.prepare(
    `SELECT id, user_id, tunnel_state, tunnel_generation, tunnel_op_at, tunnel_id,
            tunnel_ready_at, down_seen_at, rotate_requested_at, dns_record_id, cleanup_attempts,
            next_cleanup_at
     FROM machines WHERE status = 'active' AND tunnel_state != 'none'`,
  ).all<SlotRow>();
  // A slot reserved since the owners were read.
  const unread = [...new Set(slots.map((slot) => slot.user_id))].filter(
    (userId) => !owners.entitlements.has(userId),
  );
  if (unread.length > 0) {
    const late = await loadOwners(env, unread, now);
    for (const [userId, entitlement] of late.entitlements)
      owners.entitlements.set(userId, entitlement);
    for (const [userId, urls] of late.urls) owners.urls.set(userId, urls);
  }
  const entitlements = owners.entitlements;
  const done = new Set<string>();

  // 1. Revocations, never backed off: one that fails keeps its mark, and
  // goes behind the ones not tried since, so none of them starves.
  const queue = revocations(slots, entitlements);
  for (const slot of queue) {
    if (slot.rotate_requested_at !== null && slot.rotate_requested_at <= now - LATE_REVOCATION_MS) {
      console.error("A tunnel revocation has waited more than 10 minutes", slot.id);
    }
    done.add(slot.id);
  }
  const revoke = async (most: number, callReserve: number, queryReserve: number) => {
    for (
      let tried = 0;
      tried < most &&
      queue.length > 0 &&
      budget.has(RELEASE_CALLS + callReserve) &&
      queries.has(RELEASE_QUERIES + queryReserve + FINAL_QUERIES);
      tried++
    ) {
      const slot = queue.shift()!;
      await releaseTunnel(env, slot.id, slot.tunnel_generation, { now, urgent: true, budget });
    }
  };
  // The 60 every run attempts, then all but maintenance's share. What
  // maintenance leaves comes back to them at the end of the run.
  await revoke(REVOCATIONS_PER_RUN, 0, MAINTENANCE_QUERIES);
  await revoke(Number.POSITIVE_INFINITY, MAINTENANCE_CALLS, MAINTENANCE_QUERIES);

  // 2. Revoked machines, as many as the queries left allow after their own
  // read, the status readings and the end of the run.
  const cleanups = Math.floor(
    (queries.left - 1 - STATUS_QUERIES - FINAL_QUERIES) / CLEANUP_QUERIES_PER_MACHINE,
  );
  if (cleanups > 0) await cleanUpRevokedMachines(env, now, budget, cleanups);

  // 3. and 4. Status readings, from the account's tunnel listing.
  const tunnels = await listAll(budget, (cloudflare, page) => cloudflare.listTunnels(page), env);
  const reaps: SlotRow[] = [];
  if (tunnels) {
    const byId = new Map(tunnels.map((tunnel) => [tunnel.id, tunnel]));
    const healthy: string[] = [];
    const firstDown: string[] = [];
    for (const slot of slots) {
      if (done.has(slot.id) || slot.tunnel_state !== "ready" || slot.tunnel_id === null) continue;
      // A tunnel missing from the whole listing no longer exists: that reads as down.
      const status = byId.get(slot.tunnel_id)?.status ?? "down";
      if (status !== "down" && status !== "inactive") {
        if (slot.down_seen_at !== null) healthy.push(slot.tunnel_id);
      } else if (slot.down_seen_at === null) {
        firstDown.push(slot.tunnel_id);
      } else if (
        slot.down_seen_at <= now - DOWN_GRACE_MS &&
        (slot.tunnel_ready_at ?? now) <= now - DOWN_GRACE_MS
      ) {
        reaps.push(slot);
      }
    }
    await env.DB.prepare(
      `UPDATE machines SET down_seen_at = NULL
       WHERE tunnel_state = 'ready' AND tunnel_id IN (SELECT value FROM json_each(?1))`,
    )
      .bind(JSON.stringify(healthy))
      .run();
    await env.DB.prepare(
      `UPDATE machines SET down_seen_at = ?2
       WHERE tunnel_state = 'ready' AND down_seen_at IS NULL
         AND tunnel_id IN (SELECT value FROM json_each(?1))`,
    )
      .bind(JSON.stringify(firstDown), now)
      .run();
  }

  // 5. Reaps, seen down the longest first, abandoned operations, and
  // machines with nothing left to serve. A release that keeps failing backs off.
  reaps.sort((a, b) => (a.down_seen_at ?? 0) - (b.down_seen_at ?? 0));
  const serving = machinesWithServingUrls(owners);
  // The run read the URLs at its start: an idle machine is released only
  // while it still has no live URL, so a URL created since keeps its tunnel.
  const idle = slots.filter((slot) => slot.tunnel_state === "ready" && !serving.has(slot.id));
  const regular = [
    ...reaps,
    ...slots.filter((slot) => slot.tunnel_state !== "ready" && isStale(slot.tunnel_op_at, now)),
    ...idle,
  ];
  for (const slot of regular) {
    if (done.has(slot.id)) continue;
    done.add(slot.id);
    if (slot.next_cleanup_at !== null && slot.next_cleanup_at > now) continue;
    if (
      !budget.has(RELEASE_CALLS) ||
      !queries.has(RELEASE_QUERIES + BACKOFF_QUERIES + FINAL_QUERIES)
    )
      continue;
    const ifIdle = !reaps.includes(slot) && idle.includes(slot);
    const released = await releaseTunnel(env, slot.id, slot.tunnel_generation, {
      now,
      budget,
      ifIdle,
    });
    if (released === "failed") {
      const attempts = slot.cleanup_attempts + 1;
      await env.DB.prepare(
        `UPDATE machines SET cleanup_attempts = ?2, next_cleanup_at = ?3 WHERE id = ?1`,
      )
        .bind(slot.id, attempts, nextAttemptAt(now, attempts))
        .run();
    }
  }

  // 6. CNAME drift: a DNS call of an older operation may have landed late.
  const ready = slots.filter((slot) => !done.has(slot.id) && slot.tunnel_state === "ready");
  if (ready.length > 0) {
    const records = await listAll(budget, (cloudflare, page) => cloudflare.listRecords(page), env);
    if (records) {
      const byName = new Map(records.map((record) => [record.name, record]));
      for (const slot of ready) {
        if (slot.tunnel_id === null) continue;
        const record = byName.get(machineHostname(env, slot.id));
        if (record?.content === `${slot.tunnel_id}.cfargotunnel.com`) continue;
        if (!budget.has(CNAME_CALLS) || !queries.has(CNAME_QUERIES + FINAL_QUERIES)) break;
        await correctCname(env, slot, record ?? null, now, budget);
      }
    }
  }

  // 7. Orphan tunnels: ours by name, held by no machine row, an hour old.
  if (tunnels) {
    const { results: held } = await env.DB.prepare(
      `SELECT id, tunnel_id, tunnel_generation, tunnel_state, reconcile FROM machines
       WHERE tunnel_id IS NOT NULL OR tunnel_state != 'none' OR reconcile = 1`,
    ).all<{
      id: string;
      tunnel_id: string | null;
      tunnel_generation: number;
      tunnel_state: string;
      reconcile: number;
    }>();
    const heldIds = new Set(held.map((row) => row.tunnel_id));
    const heldNames = new Set(held.map((row) => tunnelName(row.id, row.tunnel_generation)));
    const cloudflare = new CloudflareApi(env, budget);
    for (const tunnel of tunnels) {
      if (
        !OWN_TUNNEL.test(tunnel.name) ||
        tunnel.createdAt > now - ORPHAN_AGE_MS ||
        heldIds.has(tunnel.id) ||
        heldNames.has(tunnel.name)
      )
        continue;
      if (!budget.has(REMOVE_TUNNEL_CALLS)) break;
      try {
        await cloudflare.removeTunnel(tunnel.id);
      } catch (error) {
        console.error("Deleting an orphan tunnel failed", tunnel.name, error);
      }
    }
  }

  // The revocations still queued get what maintenance left.
  await revoke(Number.POSITIVE_INFINITY, 0, 0);

  // 8. The account's slots.
  const total = await env.DB.prepare(
    `SELECT COUNT(*) AS n FROM machines WHERE tunnel_state != 'none'`,
  ).first<{ n: number }>();
  if (total && total.n > SLOT_ALERT) {
    console.error(
      `Localify Cloud holds ${total.n} serving slots, above ${SLOT_ALERT}; the cap is ${env.MAX_TUNNELS}`,
    );
  }
}

/**
 * Slots to release before anything else: the marked ones (rotations, and
 * revocations whose release has not finished), every slot of an owner
 * without entitlement, and an owner's slots provisioned most recently beyond
 * `servingMachines`. Those never tried come first, then the others by their
 * last attempt, so a release that keeps failing goes to the back of the
 * queue without losing its urgency; then by their mark, oldest first. A
 * release still running is skipped by `releaseTunnel` until it counts as
 * abandoned.
 */
function revocations(slots: SlotRow[], entitlements: Map<string, Entitlement>): SlotRow[] {
  const marked = slots
    .filter((slot) => slot.rotate_requested_at !== null)
    .sort((a, b) => (a.rotate_requested_at ?? 0) - (b.rotate_requested_at ?? 0));
  const beyond: SlotRow[] = [];
  const byOwner = new Map<string, SlotRow[]>();
  for (const slot of slots) {
    byOwner.set(slot.user_id, [...(byOwner.get(slot.user_id) ?? []), slot]);
  }
  for (const [owner, held] of byOwner) {
    const entitlement = entitlements.get(owner);
    const allowed = entitlement && isEntitled(entitlement) ? entitlement.servingMachines : 0;
    if (allowed === 0) {
      beyond.push(...held);
      continue;
    }
    const kept = held
      .filter((slot) => slot.tunnel_state !== "releasing")
      .sort(
        (a, b) =>
          (a.tunnel_ready_at ?? Infinity) - (b.tunnel_ready_at ?? Infinity) ||
          a.id.localeCompare(b.id),
      );
    beyond.push(...kept.slice(allowed));
  }
  const lastAttempt = (slot: SlotRow) =>
    slot.tunnel_state === "releasing" ? (slot.tunnel_op_at ?? 0) : 0;
  return [...marked, ...beyond.filter((slot) => !marked.includes(slot))].sort(
    (a, b) =>
      lastAttempt(a) - lastAttempt(b) ||
      (a.rotate_requested_at ?? Infinity) - (b.rotate_requested_at ?? Infinity),
  );
}

/**
 * A Worker from before tunnel states can still sign a machine in while a
 * deploy rolls out: the machine is active with a real tunnel and
 * `tunnel_state` `none`, so it holds no slot. Takes it over at generation 1
 * in one statement: `ready` when it has a live URL, `releasing` otherwise,
 * already abandoned, so the next release removes it. The runtime runs it for
 * its own machine before it reserves anything, so the old tunnel is reused,
 * never replaced; the sweep runs it for every machine. `machineId` null
 * means every machine.
 */
async function adoptUntrackedTunnels(
  env: Env,
  now: number,
  machineId: string | null,
): Promise<void> {
  const live = `EXISTS (SELECT 1 FROM public_urls p WHERE p.machine_id = machines.id
                        AND p.deleted_at IS NULL AND p.taken_down_at IS NULL)`;
  await env.DB.prepare(
    `UPDATE machines SET tunnel_generation = 1, tunnel_op = NULL, down_seen_at = NULL,
            tunnel_state = CASE WHEN ${live} THEN 'ready' ELSE 'releasing' END,
            tunnel_ready_at = CASE WHEN ${live} THEN ?1 ELSE NULL END,
            tunnel_op_at = CASE WHEN ${live} THEN NULL ELSE 0 END
     WHERE status = 'active' AND tunnel_state = 'none' AND tunnel_id IS NOT NULL
       AND (?2 IS NULL OR id = ?2)`,
  )
    .bind(now, machineId)
    .run();
}

/** Machines with at least one URL their owner's entitlement serves. */
function machinesWithServingUrls(owners: Owners): Set<string> {
  const serving = new Set<string>();
  for (const [userId, entitlement] of owners.entitlements) {
    const urls = owners.urls.get(userId) ?? [];
    const ids = servingUrlIds(entitlement, urls);
    for (const url of urls) {
      if (ids.has(url.id) && url.machine_id !== null) serving.add(url.machine_id);
    }
  }
  return serving;
}

/** Points a ready machine's CNAME back at its tunnel, as the holder of an operation. */
async function correctCname(
  env: Env,
  slot: SlotRow,
  record: ListedRecord | null,
  now: number,
  budget: CallBudget,
): Promise<void> {
  const op = newOp();
  const taken = await env.DB.prepare(
    `UPDATE machines SET tunnel_op = ?4, tunnel_op_at = ?5
     WHERE id = ?1 AND status = 'active' AND tunnel_state = 'ready' AND tunnel_generation = ?2
       AND tunnel_id = ?3 AND (tunnel_op IS NULL OR tunnel_op_at <= ?6)`,
  )
    .bind(slot.id, slot.tunnel_generation, slot.tunnel_id, op, now, now - OP_STALE_MS)
    .run();
  if (taken.meta.changes === 0 || slot.tunnel_id === null) return;
  try {
    console.warn("Correcting a machine's CNAME", slot.id);
    const recordId = await pointCname(
      env,
      new CloudflareApi(env, budget),
      slot.id,
      slot.tunnel_id,
      record?.id ?? slot.dns_record_id,
    );
    await env.DB.prepare(`UPDATE machines SET dns_record_id = ?2 WHERE id = ?1`)
      .bind(slot.id, recordId)
      .run();
  } catch (error) {
    console.error("Correcting a machine's CNAME failed", slot.id, error);
  } finally {
    await env.DB.prepare(
      `UPDATE machines SET tunnel_op = NULL, tunnel_op_at = NULL WHERE id = ?1 AND tunnel_op = ?2`,
    )
      .bind(slot.id, op)
      .run();
  }
}

/** Every page of a listing, one call each, or null when one fails or the budget runs out. */
async function listAll<T extends ListedTunnel | ListedRecord>(
  budget: CallBudget,
  page: (cloudflare: CloudflareApi, page: number) => Promise<T[]>,
  env: Env,
): Promise<T[] | null> {
  const all: T[] = [];
  try {
    const cloudflare = new CloudflareApi(env, budget);
    for (let number = 1; ; number++) {
      if (!budget.has(1)) return null;
      const results = await page(cloudflare, number);
      all.push(...results);
      if (results.length < LIST_PAGE_SIZE) return all;
    }
  } catch (error) {
    // A failed reading counts as no reading.
    console.error("A Cloudflare listing failed; the sweep skips what needs it", error);
    return null;
  }
}
