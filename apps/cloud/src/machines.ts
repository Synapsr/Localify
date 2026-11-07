// A machine's life: registered at sign-in without any Cloudflare call,
// revoked at sign-out or when a new sign-in with the same name replaces it.
// A revoked machine's tunnel and CNAME are removed right away, and by the
// 5-minute cleanup when that fails. Its tunnels while it is active are in
// tunnels.ts.

import { CallBudget, CloudflareApi } from "./cloudflare";
import { setting } from "./config";
import { hmacHex, randomHex, sha256Hex } from "./crypto";
import { ApiError } from "./http";

const MAX_SIGNED_IN_MACHINES = 10;
const STUCK_AFTER_MS = 10 * 60 * 1000;
const DELETED_URL_RETENTION_MS = 30 * 24 * 60 * 60 * 1000;
/** A revoked machine costs at most 5 calls: two lookups by name, the connections, the tunnel, the record. */
const CALLS_PER_REVOKED_MACHINE = 5;
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
 * Revokes the machines `?1` names (a JSON array of ids). A tunnel that may
 * exist goes to `releasing` under a new operation, which takes the row from
 * any provisioning still running. When a tunnel or the first CNAME may exist
 * without its id recorded, the cleanup looks them up by name.
 */
const REVOKE = `
  UPDATE machines SET status = 'revoked', revoked_at = ?2,
    reconcile = CASE WHEN tunnel_state != 'none' AND (tunnel_id IS NULL OR dns_record_id IS NULL)
                     THEN 1 ELSE reconcile END,
    tunnel_state = CASE WHEN tunnel_state = 'none' THEN 'none' ELSE 'releasing' END,
    tunnel_op = CASE WHEN tunnel_state = 'none' THEN NULL ELSE ?3 END,
    tunnel_op_at = CASE WHEN tunnel_state = 'none' THEN NULL ELSE ?2 END,
    rotate_requested_at = NULL, cleanup_attempts = 0, next_cleanup_at = NULL
  WHERE id IN (SELECT value FROM json_each(?1)) AND status = 'active'`;

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
  const { results: sameName } = await env.DB.prepare(
    `SELECT id FROM machines WHERE user_id = ?1 AND name = ?2 AND status = 'active'`,
  )
    .bind(userId, name)
    .all<{ id: string }>();
  const replaced = JSON.stringify(sameName.map((machine) => machine.id));
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
      `UPDATE public_urls SET machine_id = ?2
       WHERE machine_id IN (SELECT value FROM json_each(?1)) AND ${exists("?2")}`,
    ).bind(replaced, id),
    env.DB.prepare(
      `DELETE FROM tokens WHERE machine_id IN (SELECT value FROM json_each(?1)) AND ${exists("?2")}`,
    ).bind(replaced, id),
    env.DB.prepare(`${REVOKE} AND ${exists("?4")}`).bind(replaced, now, newOp(), id),
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
  for (const old of sameName) await cleanUpRevokedMachine(env, old.id);
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
    env.DB.prepare(REVOKE).bind(JSON.stringify([machineId]), now, newOp()),
    env.DB.prepare(`UPDATE public_urls SET machine_id = NULL WHERE machine_id = ?1`).bind(
      machineId,
    ),
  ]);
  await cleanUpRevokedMachine(env, machineId);
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

/** Removes what a revoked machine still holds at Cloudflare. Never throws. */
async function cleanUpRevokedMachine(env: Env, machineId: string): Promise<boolean> {
  const machine = await env.DB.prepare(`${SELECT_REVOKED} AND id = ?1`)
    .bind(machineId)
    .first<RevokedMachine>();
  return machine ? removeCloudflareResources(env, machine) : true;
}

/**
 * Looks up by name what an operation may have left unrecorded, then drops
 * the tunnel's connections, deletes the tunnel and the CNAME, recording each
 * step once Cloudflare confirms. The slot is free once the tunnel is gone.
 * Whether everything is gone; failures are logged and retried.
 */
async function removeCloudflareResources(env: Env, machine: RevokedMachine): Promise<boolean> {
  let cleaned = true;
  const failed = (error: unknown) => {
    cleaned = false;
    console.error("Cloudflare cleanup failed; the 5-minute cleanup retries it", machine.id, error);
  };
  let cloudflare: CloudflareApi;
  let tunnelId = machine.tunnel_id;
  let dnsRecordId = machine.dns_record_id;
  try {
    cloudflare = new CloudflareApi(env);
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
    if (tunnelId) {
      await cloudflare.cleanConnections(tunnelId);
      await cloudflare.deleteTunnel(tunnelId);
    }
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
): Promise<void> {
  const pending = await env.DB.prepare(
    `${SELECT_REVOKED}
       AND (tunnel_id IS NOT NULL OR dns_record_id IS NOT NULL OR reconcile = 1 OR tunnel_state != 'none')
       AND (next_cleanup_at IS NULL OR next_cleanup_at <= ?1)
     ORDER BY next_cleanup_at IS NOT NULL, next_cleanup_at, revoked_at LIMIT ?2`,
  )
    .bind(now, Math.floor(budget.left / CALLS_PER_REVOKED_MACHINE))
    .all<RevokedMachine>();
  for (const machine of pending.results) {
    if (!budget.take(CALLS_PER_REVOKED_MACHINE)) break;
    const cleaned = await removeCloudflareResources(env, machine);
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
 * only remains on rows from sign-ins before tunnels became lazy.
 */
export async function revokeStuckSignIns(env: Env, now: number): Promise<void> {
  await env.DB.prepare(
    `UPDATE machines SET status = 'revoked', revoked_at = ?1, reconcile = 1
     WHERE status = 'provisioning' AND created_at <= ?2`,
  )
    .bind(now, now - STUCK_AFTER_MS)
    .run();
}

/** Removes URL rows deleted more than 30 days ago, freeing their slugs. */
export async function purgeDeletedUrls(env: Env, now = Date.now()): Promise<void> {
  await env.DB.prepare(`DELETE FROM public_urls WHERE deleted_at IS NOT NULL AND deleted_at < ?1`)
    .bind(now - DELETED_URL_RETENTION_MS)
    .run();
}
