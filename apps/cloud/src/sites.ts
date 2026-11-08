// The public path's view of a public URL, cached in the isolate for 10
// seconds, so a deleted URL, a new password, a claim, a plan change, a ban
// or a takedown reaches every location within that time. These changes
// apply to new requests: streams already open close when the machine's
// tunnel is released.

import { loadAccount, servingUrlLimit } from "./entitlement";
import { ingressSecret } from "./machines";

export type Site = {
  id: string;
  slug: string;
  localDomain: string;
  wildcard: boolean;
  /**
   * `removed` when the URL was taken down or its owner is banned;
   * `suspended` when its owner's entitlement does not cover it
   * (docs/public-urls.md, "Downgrades").
   */
  state: "serving" | "suspended" | "removed";
  /** Null when no machine serves the URL, or its tunnel is not `ready`. */
  machineId: string | null;
  ingressSecret: string | null;
  protected: boolean;
  passwordHash: string | null;
  passwordSalt: string | null;
  passwordVersion: number;
  /**
   * When the read behind this decision started, on this Worker's clock. A
   * decision older than `SITE_CACHE_TTL_MS` is not forwarded on.
   */
  readAt: number;
  /** The owner, whose traffic limit the URL shares, and their GitHub login. */
  userId: string;
  ownerLogin: string;
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
  login: string;
  taken_down_at: number | null;
  suspended_at: number | null;
  /** The URL's rank among its owner's live URLs, oldest first. */
  live_rank: number;
  tunnel_state: string | null;
};

/**
 * How long an isolate trusts a lookup. A tunnel generation starts no sooner
 * than this, plus a margin, after its owner's URL stopped serving
 * (`QUIET_AFTER_STOP_MS` in tunnels.ts).
 */
export const SITE_CACHE_TTL_MS = 10_000;
const MAX_ENTRIES = 1_000;
const cache = new Map<string, { site: Site | null; expires: number }>();

/** The public URL for a slug, or null when there is none. */
export async function findSite(env: Env, slug: string, now = Date.now()): Promise<Site | null> {
  const cached = cache.get(slug);
  if (cached && cached.expires > now) return cached.site;
  const site = await loadSite(env, slug, now);
  if (cache.size >= MAX_ENTRIES) cache.clear();
  cache.set(slug, { site, expires: now + SITE_CACHE_TTL_MS });
  return site;
}

/** The public URL for a slug, read now whatever the cache holds, and cached. */
export async function reloadSite(env: Env, slug: string, now = Date.now()): Promise<Site | null> {
  const site = await loadSite(env, slug, now);
  if (cache.size >= MAX_ENTRIES) cache.clear();
  cache.set(slug, { site, expires: now + SITE_CACHE_TTL_MS });
  return site;
}

/** Drops a slug from this isolate's cache after the API changed it. */
export function forgetSite(slug: string): void {
  cache.delete(slug);
}

/**
 * A taken-down URL keeps its slug forever, deleted or not, so it keeps
 * showing the removal page.
 */
async function loadSite(env: Env, slug: string, now: number): Promise<Site | null> {
  const row = await env.DB.prepare(
    `SELECT p.id, p.slug, p.local_domain, p.wildcard, p.machine_id, p.protected,
            p.password_hash, p.password_salt, p.password_version, p.user_id, u.login, p.taken_down_at, p.suspended_at,
            (SELECT COUNT(*) FROM public_urls q
             WHERE q.user_id = p.user_id AND q.deleted_at IS NULL AND q.taken_down_at IS NULL
               AND (q.created_at < p.created_at OR (q.created_at = p.created_at AND q.id <= p.id)))
              AS live_rank,
            m.tunnel_state
     FROM public_urls p JOIN users u ON u.id = p.user_id
       LEFT JOIN machines m ON m.id = p.machine_id AND m.status = 'active'
     WHERE p.slug = ?1 AND (p.deleted_at IS NULL OR p.taken_down_at IS NOT NULL)`,
  )
    .bind(slug)
    .first<SiteRow>();
  if (!row) return null;
  const account = await loadAccount(env.DB, row.user_id, now);
  const state =
    row.taken_down_at !== null || !account || account.banned
      ? "removed"
      : row.live_rank <= servingUrlLimit(account.entitlement) && row.suspended_at === null
        ? "serving"
        : "suspended";
  const machineId = state === "serving" && row.tunnel_state === "ready" ? row.machine_id : null;
  return {
    id: row.id,
    slug: row.slug,
    localDomain: row.local_domain,
    wildcard: row.wildcard === 1,
    state,
    machineId,
    ingressSecret: machineId === null ? null : await ingressSecret(env, machineId),
    protected: row.protected === 1,
    passwordHash: row.password_hash,
    passwordSalt: row.password_salt,
    passwordVersion: row.password_version,
    readAt: now,
    userId: row.user_id,
    ownerLogin: row.login,
  };
}
