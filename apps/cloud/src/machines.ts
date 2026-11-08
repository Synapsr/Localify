// A machine's life: registered at sign-in without any Cloudflare call,
// revoked at sign-out or when a new sign-in with the same name replaces it.
// A revoked machine's tunnel and CNAME are removed right away, and by the
// 5-minute cleanup when that fails. Its tunnels while it is active are in
// tunnels.ts.

import { CallBudget, CloudflareApi, REMOVE_TUNNEL_CALLS } from "./cloudflare";
import { setting } from "./config";
import { hmacHex, randomHex, sha256Hex } from "./crypto";
import { ApiError } from "./http";

const MAX_SIGNED_IN_MACHINES = 10;
const STUCK_AFTER_MS = 10 * 60 * 1000;
const DELETED_URL_RETENTION_MS = 30 * 24 * 60 * 60 * 1000;
/** A revoked machine costs at most two lookups by name, the tunnel's removal, and the record. */
const CALLS_PER_REVOKED_MACHINE = 2 + REMOVE_TUNNEL_CALLS + 1;
/** A sign-in cleans up at most what one replaced machine costs, twice over. */
const SIGN_IN_CLEANUP_CALLS = 2 * CALLS_PER_REVOKED_MACHINE;
const RETRY_BASE_MS = 5 * 60 * 1000;
const MAX_RETRY_DELAY_MS = 24 * 60 * 60 * 1000;

export function newId(): string {
  return randomHex(12);
}

/** A random id for an operation that holds a machine row. */
export function newOp(): string {
  return randomHex(12);
}

export function machineHostname(env: Env, machineId: string): string {
  return `m-${machineId}.${setting(env, "MACHINE_DOMAIN")}`;
}

/**
 * The tunnel of a generation. Generation 0 names the tunnel a sign-in made
 * before tunnels had generations; those that became generation 1 keep that
 * name too, but are always known by id.
 */
export function tunnelName(machineId: string, generation: number): string {
  return generation === 0 ? `localify-${machineId}` : `localify-${machineId}-${generation}`;
}

/** HMAC-SHA-256(INGRESS_KEY, machine id) in hex. Derived on demand, never stored. */
export function ingressSecret(env: Env, machineId: string): Promise<string> {
  return hmacHex(setting(env, "INGRESS_KEY"), machineId);
}

/** When a cleanup or a release that failed `attempts` times in a row is tried again. */
export function nextAttemptAt(now: number, attempts: number): number {
  return now + Math.min(2 ** attempts * RETRY_BASE_MS, MAX_RETRY_DELAY_MS);
}

/**
 * Revokes machines, with the parameters holding the time and a new
 * operation. A tunnel that may exist goes to `releasing` under that
 * operation, which takes the row from any provisioning still running, and
 * keeps its slot until the cleanup confirms it is gone. When a tunnel or the
 * first CNAME may exist without its id recorded, the cleanup looks them up
 * by name.
 */
function revoke(now: string, op: string): string {
  return `UPDATE machines SET status = 'revoked', revoked_at = ${now},
    reconcile = CASE WHEN tunnel_state != 'none' AND (tunnel_id IS NULL OR dns_record_id IS NULL)
                     THEN 1 ELSE reconcile END,
    tunnel_state = CASE WHEN tunnel_state = 'none' THEN 'none' ELSE 'releasing' END,
    tunnel_op = CASE WHEN tunnel_state = 'none' THEN NULL ELSE ${op} END,
    tunnel_op_at = CASE WHEN tunnel_state = 'none' THEN NULL ELSE ${now} END,
    rotate_requested_at = NULL, cleanup_attempts = 0, next_cleanup_at = NULL`;
}

/**
 * Signs a machine in, in one batch: inserts it as `active` within the quota,
 * issues the token, and revokes any active machine of the user with the
 * same name, moving its URLs here. No Cloudflare call: the tunnel comes when
 * the machine first has something to serve. Returns the new machine's id
 * and token.
 */
export async function signInMachine(
  env: Env,
  userId: string,
  name: string,
  ingressPort: number,
): Promise<{ machineId: string; token: string }> {
  const id = newId();
  const now = Date.now();
  const token = `lfy_${randomHex(32)}`;
  // `?1` user, `?2` name, `?3` the new machine. The replaced machines are
  // found inside the batch, so a concurrent sign-in with the same name is
  // replaced too, and at most one machine per name stays active.
  const replaced = `SELECT id FROM machines
    WHERE user_id = ?1 AND name = ?2 AND status = 'active' AND id != ?3`;
  // The quota and the insert are one statement, so concurrent sign-ins cannot
  // both pass it. A same-name active machine does not count: this one replaces
  // it. Every other statement runs only when the insert did.
  const [insert] = await env.DB.batch([
    env.DB.prepare(
      `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at)
       SELECT ?1, ?2, ?3, 'active', ?4, ?5
       WHERE (SELECT COUNT(*) FROM machines
              WHERE user_id = ?2 AND status != 'revoked' AND NOT (status = 'active' AND name = ?3)) < ?6`,
    ).bind(id, userId, name, ingressPort, now, MAX_SIGNED_IN_MACHINES),
    env.DB.prepare(
      `UPDATE public_urls SET machine_id = ?3 WHERE machine_id IN (${replaced}) AND ${exists("?3")}`,
    ).bind(userId, name, id),
    env.DB.prepare(`DELETE FROM tokens WHERE machine_id IN (${replaced}) AND ${exists("?3")}`).bind(
      userId,
      name,
      id,
    ),
    env.DB.prepare(`${revoke("?4", "?5")} WHERE id IN (${replaced}) AND ${exists("?3")}`).bind(
      userId,
      name,
      id,
      now,
      newOp(),
    ),
    env.DB.prepare(
      `INSERT INTO tokens (hash, user_id, machine_id, created_at)
       SELECT ?1, ?2, ?3, ?4 WHERE ${exists("?3")}`,
    ).bind(await sha256Hex(token), userId, id, now),
  ]);
  if (!insert || insert.meta.changes === 0) {
    throw new ApiError(
      403,
      "quota_exceeded",
      `Your account already has ${MAX_SIGNED_IN_MACHINES} machines signed in, the most it can have. Run \`localify logout\` on one of them, or sign in with the name of a machine you no longer use to replace it.`,
    );
  }
  // Only what this batch replaced, within a small budget: older cleanups,
  // and anything left over, are the sweep's.
  const { results: replacedNow } = await env.DB.prepare(
    `${SELECT_REVOKED} AND user_id = ?1 AND name = ?2 AND revoked_at = ?3 AND ${HOLDS_RESOURCES}`,
  )
    .bind(userId, name, now)
    .all<RevokedMachine>();
  const budget = new CallBudget(SIGN_IN_CLEANUP_CALLS);
  for (const machine of replacedNow) {
    if (!budget.has(CALLS_PER_REVOKED_MACHINE)) break;
    await removeCloudflareResources(env, machine, budget);
  }
  return { machineId: id, token };
}

/** A condition that holds once the machine whose id is `param` exists. */
function exists(param: string): string {
  return `EXISTS (SELECT 1 FROM machines WHERE id = ${param})`;
}

/**
 * Signs a machine out in one batch: deletes its token, marks it `revoked`
 * and detaches its URLs. Then removes its tunnel and its CNAME.
 */
export async function signOutMachine(env: Env, machineId: string): Promise<void> {
  const now = Date.now();
  await env.DB.batch([
    env.DB.prepare(`DELETE FROM tokens WHERE machine_id = ?1`).bind(machineId),
    env.DB.prepare(`${revoke("?2", "?3")} WHERE id = ?1 AND status = 'active'`).bind(
      machineId,
      now,
      newOp(),
    ),
    env.DB.prepare(`UPDATE public_urls SET machine_id = NULL WHERE machine_id = ?1`).bind(
      machineId,
    ),
  ]);
  const machine = await env.DB.prepare(`${SELECT_REVOKED} AND id = ?1`)
    .bind(machineId)
    .first<RevokedMachine>();
  if (machine) await removeCloudflareResources(env, machine, null);
}

type RevokedMachine = {
  id: string;
  tunnel_id: string | null;
  dns_record_id: string | null;
  tunnel_generation: number;
  reconcile: number;
  cleanup_attempts: number;
};

const SELECT_REVOKED = `SELECT id, tunnel_id, dns_record_id, tunnel_generation, reconcile, cleanup_attempts
  FROM machines WHERE status = 'revoked'`;
/** A revoked machine that may still hold something at Cloudflare. */
const HOLDS_RESOURCES = `(tunnel_id IS NOT NULL OR dns_record_id IS NOT NULL OR reconcile = 1
  OR tunnel_state != 'none')`;

/**
 * Looks up by name what an operation may have left unrecorded, then removes
 * the tunnel and the CNAME, recording each step once Cloudflare confirms.
 * The slot is free once the tunnel is gone. Whether everything is gone;
 * failures are logged and retried. Never throws.
 */
async function removeCloudflareResources(
  env: Env,
  machine: RevokedMachine,
  budget: CallBudget | null,
): Promise<boolean> {
  let cleaned = true;
  const failed = (error: unknown) => {
    cleaned = false;
    console.error("Cloudflare cleanup failed; the 5-minute cleanup retries it", machine.id, error);
  };
  let cloudflare: CloudflareApi;
  let tunnelId = machine.tunnel_id;
  let dnsRecordId = machine.dns_record_id;
  try {
    cloudflare = new CloudflareApi(env, budget);
    if (machine.reconcile === 1) {
      tunnelId ??= await cloudflare.findTunnelId(tunnelName(machine.id, machine.tunnel_generation));
      dnsRecordId ??= await cloudflare.findDnsRecordId(machineHostname(env, machine.id));
      await env.DB.prepare(
        `UPDATE machines SET tunnel_id = ?2, dns_record_id = ?3, reconcile = 0 WHERE id = ?1`,
      )
        .bind(machine.id, tunnelId, dnsRecordId)
        .run();
    }
  } catch (error) {
    failed(error);
    return cleaned;
  }
  try {
    if (tunnelId) await cloudflare.removeTunnel(tunnelId);
    await env.DB.prepare(
      `UPDATE machines SET tunnel_id = NULL, tunnel_state = 'none', tunnel_op = NULL,
              tunnel_op_at = NULL, tunnel_ready_at = NULL, down_seen_at = NULL
       WHERE id = ?1 AND status = 'revoked'`,
    )
      .bind(machine.id)
      .run();
  } catch (error) {
    failed(error);
  }
  if (dnsRecordId) {
    try {
      await cloudflare.deleteDnsRecord(dnsRecordId);
      await env.DB.prepare(`UPDATE machines SET dns_record_id = NULL WHERE id = ?1`)
        .bind(machine.id)
        .run();
    } catch (error) {
      failed(error);
    }
  }
  return cleaned;
}

/**
 * Retries the Cloudflare cleanup of revoked machines within the run's
 * budget, due machines first and oldest first. One whose cleanup keeps
 * failing waits longer each time, so it cannot hold back the others.
 */
export async function cleanUpRevokedMachines(
  env: Env,
  now: number,
  budget: CallBudget,
  maxMachines = Number.POSITIVE_INFINITY,
): Promise<void> {
  const pending = await env.DB.prepare(
    `${SELECT_REVOKED} AND ${HOLDS_RESOURCES}
       AND (next_cleanup_at IS NULL OR next_cleanup_at <= ?1)
     ORDER BY next_cleanup_at IS NOT NULL, next_cleanup_at, revoked_at LIMIT ?2`,
  )
    .bind(now, Math.min(maxMachines, Math.floor(budget.left / CALLS_PER_REVOKED_MACHINE)))
    .all<RevokedMachine>();
  for (const machine of pending.results) {
    if (!budget.has(CALLS_PER_REVOKED_MACHINE)) break;
    const cleaned = await removeCloudflareResources(env, machine, budget);
    const attempts = cleaned ? 0 : machine.cleanup_attempts + 1;
    await env.DB.prepare(
      `UPDATE machines SET cleanup_attempts = ?2, next_cleanup_at = ?3 WHERE id = ?1`,
    )
      .bind(machine.id, attempts, cleaned ? null : nextAttemptAt(now, attempts))
      .run();
  }
}

/**
 * Revokes machines left `provisioning` for 10 minutes or more. That status
 * only remains on rows from sign-ins before tunnels became lazy. Such a
 * sign-in may have made a tunnel: it holds a slot until the cleanup finds
 * it by name and removes it.
 */
export async function revokeStuckSignIns(env: Env, now: number): Promise<void> {
  await env.DB.prepare(
    `UPDATE machines SET status = 'revoked', revoked_at = ?1, reconcile = 1,
            tunnel_state = 'releasing', tunnel_op_at = ?1
     WHERE status = 'provisioning' AND created_at <= ?2`,
  )
    .bind(now, now - STUCK_AFTER_MS)
    .run();
}

/**
 * Removes URL rows deleted more than 30 days ago, freeing their slugs. A
 * taken-down URL keeps its row, so its slug is never given out again.
 */
export async function purgeDeletedUrls(env: Env, now = Date.now()): Promise<void> {
  await env.DB.prepare(
    `DELETE FROM public_urls
     WHERE deleted_at IS NOT NULL AND deleted_at < ?1 AND taken_down_at IS NULL`,
  )
    .bind(now - DELETED_URL_RETENTION_MS)
    .run();
}
