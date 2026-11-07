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
import { entitlementOf, type Entitlement } from "./entitlement";
import {
  cleanUpRevokedMachines,
  ingressSecret,
  machineHostname,
  newOp,
  nextAttemptAt,
  tunnelName,
} from "./machines";

/** An operation older than this is abandoned. */
const OP_STALE_MS = 2 * 60 * 1000;
/** Two down readings this far apart, on a tunnel ready this long, reap it. */
const DOWN_GRACE_MS = 15 * 60 * 1000;
const ORPHAN_AGE_MS = 60 * 60 * 1000;
const LATE_REVOCATION_MS = 10 * 60 * 1000;
const SWEEP_CALL_BUDGET = 300;
/** A release looks the tunnel up by name at most once, then removes it. */
const RELEASE_CALLS = 1 + REMOVE_TUNNEL_CALLS;
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
  } | null;
  tunnel: { generation: number; token: string | null } | null;
  ingressSecret: string | null;
  publicUrls: PublicUrl[];
  retryAfter: number | null;
};

type RuntimeCaller = {
  userId: string;
  githubId: number;
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
  const entitlement = await entitlementOf(env, { id: caller.userId, githubId: caller.githubId });
  const publicUrls = await urlsOfMachine(
    env.DB,
    setting(env, "PUBLIC_BASE_DOMAIN"),
    caller.userId,
    caller.machineId,
    entitlement.publicUrls,
  );
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
        },
      };
    }
    return { ...base, state: "idle" };
  }

  if (row.tunnel_state === "none") {
    const reserved = await reserveSlot(env, caller, entitlement.servingMachines, now);
    if (reserved) {
      background(ctx, provisionTunnel(env, reserved));
      return provisioning;
    }
    row = await tunnelRow(env, caller.machineId);
    if (row.tunnel_state === "none") return blockedRuntime(env, caller, entitlement, base);
  }

  if (row.tunnel_state === "ready" && row.tunnel_id !== null && row.rotate_requested_at === null) {
    const generation = row.tunnel_generation;
    const [token, secret] = await Promise.all([
      daemonGeneration === generation ? null : new CloudflareApi(env).tunnelToken(row.tunnel_id),
      ingressSecret(env, caller.machineId),
    ]);
    return { ...base, state: "ready", tunnel: { generation, token }, ingressSecret: secret };
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
  return entitlement.servingMachines > 0 && publicUrls.some((url) => url.state === "serving");
}

function isEntitled(entitlement: Entitlement): boolean {
  return entitlement.servingMachines > 0 && entitlement.publicUrls > 0;
}

function isStale(opAt: number | null, now: number): boolean {
  return opAt === null || opAt <= now - OP_STALE_MS;
}

/** Why a machine that wants a tunnel could not reserve a slot. */
async function blockedRuntime(
  env: Env,
  caller: RuntimeCaller,
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
    return {
      ...base,
      state: "blocked",
      blocked: {
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

/**
 * Reserves a serving slot in one statement, so concurrent requests can pass
 * neither the owner's limit nor the account's cap. Null when the row is no
 * longer `none` or a limit stops it.
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
     RETURNING tunnel_generation, ingress_port, dns_record_id`,
  )
    .bind(caller.machineId, op, now, caller.userId, servingMachines, maxTunnels(env))
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
  { now = Date.now(), urgent = false, budget = null }: ReleaseOptions = {},
): Promise<ReleaseResult> {
  const op = newOp();
  const taken = await env.DB.prepare(
    `UPDATE machines SET tunnel_state = 'releasing', tunnel_op = ?3, tunnel_op_at = ?4,
            rotate_requested_at = CASE WHEN ?6 THEN COALESCE(rotate_requested_at, ?4)
                                       ELSE rotate_requested_at END
     WHERE id = ?1 AND status = 'active' AND tunnel_generation = ?2
       AND (tunnel_state = 'ready'
            OR (tunnel_state IN ('provisioning', 'releasing') AND tunnel_op_at <= ?5))
     RETURNING tunnel_id`,
  )
    .bind(machineId, generation, op, now, now - OP_STALE_MS, urgent ? 1 : 0)
    .first<{ tunnel_id: string | null }>();
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
      `SELECT m.user_id, u.github_id, m.tunnel_state, m.tunnel_generation
       FROM machines m JOIN users u ON u.id = m.user_id
       WHERE m.id = ?1 AND m.status = 'active'`,
    )
      .bind(machineId)
      .first<{
        user_id: string;
        github_id: number;
        tunnel_state: string;
        tunnel_generation: number;
      }>();
    if (!machine || machine.tunnel_state === "none") return;
    const entitlement = await entitlementOf(env, {
      id: machine.user_id,
      githubId: machine.github_id,
    });
    const publicUrls = await urlsOfMachine(
      env.DB,
      setting(env, "PUBLIC_BASE_DOMAIN"),
      machine.user_id,
      machineId,
      entitlement.publicUrls,
    );
    if (!wantsTunnel(entitlement, publicUrls)) {
      await releaseTunnel(env, machineId, machine.tunnel_generation);
    }
  } catch (error) {
    console.error("Releasing an unwanted tunnel failed; the sweep retries it", machineId, error);
  }
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
  github_id: number;
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
 * The 5-minute sweep, within a budget of Cloudflare calls. Revocations come
 * first and may use the whole budget: rotations, and the slots of owners
 * who lost their entitlement or hold more than it allows. Then the revoked
 * machines' cleanup, the status readings and reaps, abandoned operations,
 * machines with nothing to serve, CNAME drift and orphan tunnels, with what
 * the budget leaves.
 */
export async function runSweep(env: Env, now = Date.now()): Promise<void> {
  const budget = new CallBudget(SWEEP_CALL_BUDGET);
  const { results: slots } = await env.DB.prepare(
    `SELECT m.id, m.user_id, u.github_id, m.tunnel_state, m.tunnel_generation, m.tunnel_op_at,
            m.tunnel_id, m.tunnel_ready_at, m.down_seen_at, m.rotate_requested_at, m.dns_record_id,
            m.cleanup_attempts, m.next_cleanup_at
     FROM machines m JOIN users u ON u.id = m.user_id
     WHERE m.status = 'active' AND m.tunnel_state != 'none'`,
  ).all<SlotRow>();
  const entitlements = new Map<string, Entitlement>();
  for (const slot of slots) {
    if (!entitlements.has(slot.user_id)) {
      entitlements.set(
        slot.user_id,
        await entitlementOf(env, { id: slot.user_id, githubId: slot.github_id }),
      );
    }
  }
  const done = new Set<string>();

  // 1. Revocations, never backed off: one that fails keeps its mark and
  // comes first again in the next run.
  for (const slot of revocations(slots, entitlements)) {
    if (slot.rotate_requested_at !== null && slot.rotate_requested_at <= now - LATE_REVOCATION_MS) {
      console.error("A tunnel revocation has waited more than 10 minutes", slot.id);
    }
    done.add(slot.id);
    if (!budget.has(RELEASE_CALLS)) continue;
    await releaseTunnel(env, slot.id, slot.tunnel_generation, { now, urgent: true, budget });
  }

  // 2. Revoked machines.
  await cleanUpRevokedMachines(env, now, budget);

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
  const serving = await machinesWithServingUrls(env, entitlements);
  const regular = [
    ...reaps,
    ...slots.filter((slot) => slot.tunnel_state !== "ready" && isStale(slot.tunnel_op_at, now)),
    ...slots.filter((slot) => slot.tunnel_state === "ready" && !serving.has(slot.id)),
  ];
  for (const slot of regular) {
    if (done.has(slot.id)) continue;
    done.add(slot.id);
    if (slot.next_cleanup_at !== null && slot.next_cleanup_at > now) continue;
    if (!budget.has(RELEASE_CALLS)) continue;
    if ((await releaseTunnel(env, slot.id, slot.tunnel_generation, { now, budget })) === "failed") {
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
        if (!budget.has(CNAME_CALLS)) break;
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
 * Slots to release before anything else. First the marked ones, oldest mark
 * first: rotations, and revocations whose release has not finished. Then
 * every slot of an owner without entitlement, and an owner's slots
 * provisioned most recently beyond `servingMachines`. A release still
 * running is skipped by `releaseTunnel` until it counts as abandoned.
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
    const allowed = entitlements.get(owner)?.servingMachines ?? 0;
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
  return [...marked, ...beyond.filter((slot) => !marked.includes(slot))];
}

/** Machines with at least one URL their owner's entitlement serves. */
async function machinesWithServingUrls(
  env: Env,
  entitlements: Map<string, Entitlement>,
): Promise<Set<string>> {
  const { results } = await env.DB.prepare(
    `SELECT machine_id, user_id,
            ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY created_at, id) AS live_rank
     FROM public_urls
     WHERE deleted_at IS NULL AND taken_down_at IS NULL
       AND user_id IN (SELECT value FROM json_each(?1))`,
  )
    .bind(JSON.stringify([...entitlements.keys()]))
    .all<{ machine_id: string | null; user_id: string; live_rank: number }>();
  const serving = new Set<string>();
  for (const url of results) {
    const entitlement = entitlements.get(url.user_id);
    if (
      url.machine_id !== null &&
      entitlement &&
      entitlement.servingMachines > 0 &&
      url.live_rank <= entitlement.publicUrls
    ) {
      serving.add(url.machine_id);
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
