// D1 reads of public URLs, and the API's PublicUrl shape. Deleted URLs are
// never returned.

type PublicUrlRow = {
  id: string;
  slug: string;
  local_domain: string;
  wildcard: number;
  machine_id: string | null;
  machine_name: string | null;
  protected: number;
  password_version: number;
  created_at: number;
  taken_down_at: number | null;
  /** The URL's rank among its owner's live URLs, oldest first; null when taken down. */
  live_rank: number | null;
};

export type PublicUrl = {
  id: string;
  slug: string;
  url: string;
  localDomain: string;
  wildcard: boolean;
  machine: { id: string; name: string } | null;
  /** `version` changes with every protection change: a password saved before is stale. */
  protection: { mode: "password" | "public"; version: number };
  /** `serving` while the owner's `publicUrls` covers it (docs/public-urls.md, "Downgrades"). */
  state: "serving" | "suspended" | "taken_down";
  createdAt: string;
};

/**
 * `?1` is the owner. A live URL is neither deleted nor taken down; the
 * oldest ones, by `created_at` then `id`, are the ones that serve.
 */
const SELECT_URLS = `
  WITH ranked AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY created_at, id) AS live_rank
    FROM public_urls WHERE user_id = ?1 AND deleted_at IS NULL AND taken_down_at IS NULL)
  SELECT p.id, p.slug, p.local_domain, p.wildcard, p.machine_id, m.name AS machine_name, p.protected,
         p.password_version, p.created_at, p.taken_down_at, r.live_rank
  FROM public_urls p LEFT JOIN machines m ON m.id = p.machine_id LEFT JOIN ranked r ON r.id = p.id`;

/** The user's public URLs, oldest first. `servingLimit` is the owner's `publicUrls`. */
export async function urlsOfUser(
  db: D1Database,
  baseDomain: string,
  userId: string,
  servingLimit: number,
): Promise<PublicUrl[]> {
  const { results } = await db
    .prepare(
      `${SELECT_URLS} WHERE p.user_id = ?1 AND p.deleted_at IS NULL ORDER BY p.created_at, p.id`,
    )
    .bind(userId)
    .all<PublicUrlRow>();
  return results.map((row) => toPublicUrl(row, baseDomain, servingLimit));
}

/** The public URLs a machine of `userId` serves, oldest first. */
export async function urlsOfMachine(
  db: D1Database,
  baseDomain: string,
  userId: string,
  machineId: string,
  servingLimit: number,
): Promise<PublicUrl[]> {
  const { results } = await db
    .prepare(
      `${SELECT_URLS} WHERE p.user_id = ?1 AND p.machine_id = ?2 AND p.deleted_at IS NULL
       ORDER BY p.created_at, p.id`,
    )
    .bind(userId, machineId)
    .all<PublicUrlRow>();
  return results.map((row) => toPublicUrl(row, baseDomain, servingLimit));
}

/** One live public URL of the user, or null for an unknown id and for someone else's. */
export async function urlOfUser(
  db: D1Database,
  baseDomain: string,
  userId: string,
  urlId: string,
  servingLimit: number,
): Promise<PublicUrl | null> {
  const row = await db
    .prepare(`${SELECT_URLS} WHERE p.id = ?2 AND p.user_id = ?1 AND p.deleted_at IS NULL`)
    .bind(userId, urlId)
    .first<PublicUrlRow>();
  return row ? toPublicUrl(row, baseDomain, servingLimit) : null;
}

function toPublicUrl(row: PublicUrlRow, baseDomain: string, servingLimit: number): PublicUrl {
  return {
    id: row.id,
    slug: row.slug,
    url: `https://${row.slug}.${baseDomain}`,
    localDomain: row.local_domain,
    wildcard: row.wildcard === 1,
    machine:
      row.machine_id !== null && row.machine_name !== null
        ? { id: row.machine_id, name: row.machine_name }
        : null,
    protection: {
      mode: row.protected === 1 ? "password" : "public",
      version: row.password_version,
    },
    state:
      row.taken_down_at !== null
        ? "taken_down"
        : row.live_rank !== null && row.live_rank <= servingLimit
          ? "serving"
          : "suspended",
    createdAt: new Date(row.created_at).toISOString(),
  };
}

/** True when a D1 error is a UNIQUE constraint failure that names `table.column`. */
export function isUniqueViolation(error: unknown, column: string): boolean {
  const text = String(error);
  return text.includes("UNIQUE constraint failed") && text.includes(column);
}
