// The public path's view of a public URL, cached in the isolate for 10
// seconds, so a deleted URL, a new password, a claim or a removal from the
// allowlist reaches every location within that time. These changes apply to
// new requests: a WebSocket or SSE stream already open stays open until it
// closes.

import { isAllowedGithubId } from "./config";
import { ingressSecret } from "./machines";

export type Site = {
  id: string;
  slug: string;
  localDomain: string;
  wildcard: boolean;
  machineId: string | null;
  /** Null when no machine serves the URL. */
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
  github_id: number;
};

const TTL_MS = 10_000;
const MAX_ENTRIES = 1_000;
const cache = new Map<string, { site: Site | null; expires: number }>();

/** The live public URL for a slug, or null when there is none or its owner is off the allowlist. */
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
            p.password_hash, p.password_salt, p.password_version, u.github_id
     FROM public_urls p JOIN users u ON u.id = p.user_id
     WHERE p.slug = ?1 AND p.deleted_at IS NULL`,
  )
    .bind(slug)
    .first<SiteRow>();
  if (!row || !isAllowedGithubId(env, row.github_id)) return null;
  return {
    id: row.id,
    slug: row.slug,
    localDomain: row.local_domain,
    wildcard: row.wildcard === 1,
    machineId: row.machine_id,
    ingressSecret: row.machine_id === null ? null : await ingressSecret(env, row.machine_id),
    protected: row.protected === 1,
    passwordHash: row.password_hash,
    passwordSalt: row.password_salt,
    passwordVersion: row.password_version,
  };
}
