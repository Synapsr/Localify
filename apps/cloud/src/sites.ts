// The public path's view of a public URL, cached in the isolate for 10
// seconds, so a deleted URL, a new password, a claim, a lost entitlement or
// a released tunnel reaches every location within that time. These changes
// apply to new requests: streams already open close when the machine's
// tunnel is released.

import { entitlementOf } from "./entitlement";
import { ingressSecret } from "./machines";

export type Site = {
  id: string;
  slug: string;
  localDomain: string;
  wildcard: boolean;
  /** Null when no machine serves the URL, or its tunnel is not `ready`. */
  machineId: string | null;
  ingressSecret: string | null;
  protected: boolean;
  passwordHash: string | null;
  passwordSalt: string | null;
  passwordVersion: number;
};

type SiteRow = {
  id: string;
  slug: string;
  local_domain: string;
  wildcard: number;
  machine_id: string | null;
  protected: number;
  password_hash: string | null;
  password_salt: string | null;
  password_version: number;
  user_id: string;
  github_id: number;
  tunnel_state: string | null;
};

const TTL_MS = 10_000;
const MAX_ENTRIES = 1_000;
const cache = new Map<string, { site: Site | null; expires: number }>();

/** The live public URL for a slug, or null when there is none or its owner has no entitlement. */
export async function findSite(env: Env, slug: string, now = Date.now()): Promise<Site | null> {
  const cached = cache.get(slug);
  if (cached && cached.expires > now) return cached.site;
  const site = await loadSite(env, slug);
  if (cache.size >= MAX_ENTRIES) cache.clear();
  cache.set(slug, { site, expires: now + TTL_MS });
  return site;
}

/** Drops a slug from this isolate's cache after the API changed it. */
export function forgetSite(slug: string): void {
  cache.delete(slug);
}

async function loadSite(env: Env, slug: string): Promise<Site | null> {
  const row = await env.DB.prepare(
    `SELECT p.id, p.slug, p.local_domain, p.wildcard, p.machine_id, p.protected,
            p.password_hash, p.password_salt, p.password_version, p.user_id, u.github_id,
            m.tunnel_state
     FROM public_urls p JOIN users u ON u.id = p.user_id
       LEFT JOIN machines m ON m.id = p.machine_id AND m.status = 'active'
     WHERE p.slug = ?1 AND p.deleted_at IS NULL`,
  )
    .bind(slug)
    .first<SiteRow>();
  if (!row) return null;
  const entitlement = await entitlementOf(env, { id: row.user_id, githubId: row.github_id });
  if (entitlement.publicUrls === 0) return null;
  const machineId = row.tunnel_state === "ready" ? row.machine_id : null;
  return {
    id: row.id,
    slug: row.slug,
    localDomain: row.local_domain,
    wildcard: row.wildcard === 1,
    machineId,
    ingressSecret: machineId === null ? null : await ingressSecret(env, machineId),
    protected: row.protected === 1,
    passwordHash: row.password_hash,
    passwordSalt: row.password_salt,
    passwordVersion: row.password_version,
  };
}
