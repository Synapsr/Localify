// A machine's life: provisioned at sign-in, revoked at sign-out or when a
// new sign-in with the same name replaces it, and its Cloudflare tunnel and
// DNS record removed, by the hourly cleanup when the first try fails.

import { CloudflareApi } from "./cloudflare";
import { setting } from "./config";
import { hmacHex, randomHex, sha256Hex } from "./crypto";
import { ApiError } from "./http";

export const MAX_ACTIVE_MACHINES = 5;
const STUCK_AFTER_MS = 10 * 60 * 1000;
const DELETED_URL_RETENTION_MS = 30 * 24 * 60 * 60 * 1000;
/** Machines per cleanup run, to stay well within a run's subrequest limit. */
const CLEANUP_BATCH = 25;

export type CloudflareResources = {
  id: string;
  tunnelId: string | null;
  dnsRecordId: string | null;
};

export function newId(): string {
  return randomHex(12);
}

export function machineHostname(env: Env, machineId: string): string {
  return `m-${machineId}.${setting(env, "MACHINE_DOMAIN")}`;
}

/** HMAC-SHA-256(INGRESS_KEY, machine id) in hex. Derived on demand, never stored. */
export function ingressSecret(env: Env, machineId: string): Promise<string> {
  return hmacHex(setting(env, "INGRESS_KEY"), machineId);
}

/**
 * Signs a machine in: inserts it as `provisioning` within the quota, creates
 * its tunnel, ingress and CNAME, then in one batch marks it `active`, issues
 * the token, and revokes any active machine of the user with the same name,
 * moving its URLs here. Returns the new machine's id and token.
 */
export async function signInMachine(
  env: Env,
  userId: string,
  name: string,
  ingressPort: number,
): Promise<{ machineId: string; token: string }> {
  const id = newId();
  const now = Date.now();
  // The quota and the insert are one statement, so concurrent sign-ins cannot
  // both pass it. A same-name active machine does not count: this one replaces it.
  const inserted = await env.DB.prepare(
    `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at)
     SELECT ?1, ?2, ?3, 'provisioning', ?4, ?5
     WHERE (SELECT COUNT(*) FROM machines
            WHERE user_id = ?2 AND status != 'revoked' AND NOT (status = 'active' AND name = ?3)) < ?6`,
  )
    .bind(id, userId, name, ingressPort, now, MAX_ACTIVE_MACHINES)
    .run();
  if (inserted.meta.changes === 0) {
    throw new ApiError(
      403,
      "quota_exceeded",
      `Your account already has ${MAX_ACTIVE_MACHINES} machines signed in, the most it can have. Run \`localify logout\` on one of them, or sign in with the name of a machine you no longer use to replace it.`,
    );
  }

  try {
    await provision(env, id, ingressPort);
  } catch (error) {
    console.error("Provisioning failed", id, error);
    await revokeAndClean(env, id);
    throw new ApiError(
      502,
      "provisioning_failed",
      "Localify Cloud could not set up this machine's tunnel. Run `localify login` again in a minute.",
    );
  }

  const replaced = await env.DB.prepare(
    `SELECT id, tunnel_id, dns_record_id FROM machines
     WHERE user_id = ?1 AND name = ?2 AND status = 'active' AND id != ?3`,
  )
    .bind(userId, name, id)
    .all<{ id: string; tunnel_id: string | null; dns_record_id: string | null }>();
  const token = `lfy_${randomHex(32)}`;
  const sameName = `SELECT id FROM machines WHERE user_id = ?1 AND name = ?2 AND status = 'active' AND id != ?3`;
  try {
    await env.DB.batch([
      env.DB.prepare(
        `UPDATE public_urls SET machine_id = ?3 WHERE machine_id IN (${sameName})`,
      ).bind(userId, name, id),
      env.DB.prepare(`DELETE FROM tokens WHERE machine_id IN (${sameName})`).bind(userId, name, id),
      env.DB.prepare(
        `UPDATE machines SET status = 'revoked', revoked_at = ?4
         WHERE user_id = ?1 AND name = ?2 AND status = 'active' AND id != ?3`,
      ).bind(userId, name, id, Date.now()),
      env.DB.prepare(
        `UPDATE machines SET status = 'active' WHERE id = ?1 AND status = 'provisioning'`,
      ).bind(id),
      env.DB.prepare(
        `INSERT INTO tokens (hash, user_id, machine_id, created_at) VALUES (?1, ?2, ?3, ?4)`,
      ).bind(await sha256Hex(token), userId, id, Date.now()),
    ]);
  } catch (error) {
    console.error("Activating the machine failed", id, error);
    await revokeAndClean(env, id);
    throw new ApiError(
      502,
      "provisioning_failed",
      "Localify Cloud could not finish signing this machine in. Run `localify login` again in a minute.",
    );
  }

  for (const old of replaced.results) {
    await removeCloudflareResources(env, {
      id: old.id,
      tunnelId: old.tunnel_id,
      dnsRecordId: old.dns_record_id,
    });
  }
  return { machineId: id, token };
}

/** Creates the tunnel, its ingress and the CNAME, recording each id as soon as it exists. */
async function provision(env: Env, id: string, ingressPort: number): Promise<void> {
  const cloudflare = new CloudflareApi(env);
  const hostname = machineHostname(env, id);
  const tunnelId = await cloudflare.createTunnel(`localify-${id}`);
  await env.DB.prepare(`UPDATE machines SET tunnel_id = ?2 WHERE id = ?1`).bind(id, tunnelId).run();
  await cloudflare.configureTunnel(tunnelId, hostname, ingressPort);
  const dnsRecordId = await cloudflare.createTunnelRecord(
    hostname,
    tunnelId,
    `Localify Cloud machine ${id}`,
  );
  await env.DB.prepare(`UPDATE machines SET dns_record_id = ?2 WHERE id = ?1`)
    .bind(id, dnsRecordId)
    .run();
}

/** A machine that will never become active: revoked, and its Cloudflare resources removed. */
async function revokeAndClean(env: Env, id: string): Promise<void> {
  const row = await env.DB.prepare(
    `UPDATE machines SET status = 'revoked', revoked_at = ?2 WHERE id = ?1
     RETURNING tunnel_id, dns_record_id`,
  )
    .bind(id, Date.now())
    .first<{ tunnel_id: string | null; dns_record_id: string | null }>();
  if (row)
    await removeCloudflareResources(env, {
      id,
      tunnelId: row.tunnel_id,
      dnsRecordId: row.dns_record_id,
    });
}

/**
 * Signs a machine out in one batch: deletes its token, marks it `revoked` and
 * detaches its URLs. Then removes its Cloudflare resources.
 */
export async function signOutMachine(env: Env, machine: CloudflareResources): Promise<void> {
  await env.DB.batch([
    env.DB.prepare(`DELETE FROM tokens WHERE machine_id = ?1`).bind(machine.id),
    env.DB.prepare(`UPDATE machines SET status = 'revoked', revoked_at = ?2 WHERE id = ?1`).bind(
      machine.id,
      Date.now(),
    ),
    env.DB.prepare(`UPDATE public_urls SET machine_id = NULL WHERE machine_id = ?1`).bind(
      machine.id,
    ),
  ]);
  await removeCloudflareResources(env, machine);
}

/**
 * Drops the tunnel's connections, deletes the tunnel, and deletes the DNS
 * record, clearing each id once Cloudflare confirms. Never throws: what fails
 * stays recorded on the revoked machine for the hourly cleanup.
 */
export async function removeCloudflareResources(
  env: Env,
  machine: CloudflareResources,
): Promise<void> {
  const failed = (error: unknown) =>
    console.error("Cloudflare cleanup failed; the hourly cleanup retries it", machine.id, error);
  let cloudflare: CloudflareApi;
  try {
    cloudflare = new CloudflareApi(env);
  } catch (error) {
    return failed(error);
  }
  if (machine.tunnelId) {
    try {
      await cloudflare.cleanConnections(machine.tunnelId);
      await cloudflare.deleteTunnel(machine.tunnelId);
      await env.DB.prepare(`UPDATE machines SET tunnel_id = NULL WHERE id = ?1`)
        .bind(machine.id)
        .run();
    } catch (error) {
      failed(error);
    }
  }
  if (machine.dnsRecordId) {
    try {
      await cloudflare.deleteDnsRecord(machine.dnsRecordId);
      await env.DB.prepare(`UPDATE machines SET dns_record_id = NULL WHERE id = ?1`)
        .bind(machine.id)
        .run();
    } catch (error) {
      failed(error);
    }
  }
}

/**
 * The hourly cleanup: purges URL rows deleted more than 30 days ago, revokes
 * machines stuck in `provisioning` for 10 minutes or more, and retries the
 * Cloudflare cleanup of revoked machines. Each step runs whatever happens to
 * the others.
 */
export async function runCleanup(env: Env, now = Date.now()): Promise<void> {
  await env.DB.prepare(`DELETE FROM public_urls WHERE deleted_at IS NOT NULL AND deleted_at < ?1`)
    .bind(now - DELETED_URL_RETENTION_MS)
    .run();
  await revokeStuckMachines(env, now);

  const pending = await env.DB.prepare(
    `SELECT id, tunnel_id, dns_record_id FROM machines
     WHERE status = 'revoked' AND (tunnel_id IS NOT NULL OR dns_record_id IS NOT NULL)
     ORDER BY revoked_at LIMIT ?1`,
  )
    .bind(CLEANUP_BATCH)
    .all<{ id: string; tunnel_id: string | null; dns_record_id: string | null }>();
  for (const machine of pending.results) {
    await removeCloudflareResources(env, {
      id: machine.id,
      tunnelId: machine.tunnel_id,
      dnsRecordId: machine.dns_record_id,
    });
  }
}

/** Marks stuck sign-ins `revoked` with their resources, for the retry that follows. */
async function revokeStuckMachines(env: Env, now: number): Promise<void> {
  const stuck = await env.DB.prepare(
    `SELECT id, tunnel_id, dns_record_id FROM machines
     WHERE status = 'provisioning' AND created_at <= ?1 ORDER BY created_at LIMIT ?2`,
  )
    .bind(now - STUCK_AFTER_MS, CLEANUP_BATCH)
    .all<{ id: string; tunnel_id: string | null; dns_record_id: string | null }>();
  for (const machine of stuck.results) {
    try {
      const cloudflare = new CloudflareApi(env);
      // A sign-in that stopped between creating something and recording it
      // left no id: find it by name.
      const tunnelId =
        machine.tunnel_id ?? (await cloudflare.findTunnelId(`localify-${machine.id}`));
      const dnsRecordId =
        machine.dns_record_id ??
        (await cloudflare.findDnsRecordId(machineHostname(env, machine.id)));
      await env.DB.prepare(
        `UPDATE machines SET status = 'revoked', revoked_at = ?2, tunnel_id = ?3, dns_record_id = ?4
         WHERE id = ?1 AND status = 'provisioning'`,
      )
        .bind(machine.id, now, tunnelId, dnsRecordId)
        .run();
    } catch (error) {
      console.error("Could not revoke a machine stuck in provisioning", machine.id, error);
    }
  }
}
