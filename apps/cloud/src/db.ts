// D1 reads of public URLs, and the API's PublicUrl shape. Deleted URLs are
// never returned.

export type PublicUrlRow = {
  id: string;
  slug: string;
  local_domain: string;
  wildcard: number;
  machine_id: string | null;
  machine_name: string | null;
  protected: number;
  created_at: number;
};

export type PublicUrl = {
  id: string;
  slug: string;
  url: string;
  localDomain: string;
  wildcard: boolean;
  machine: { id: string; name: string } | null;
  protection: { mode: "password" | "public" };
  createdAt: string;
};

const SELECT_URLS = `
  SELECT p.id, p.slug, p.local_domain, p.wildcard, p.machine_id, m.name AS machine_name, p.protected, p.created_at
  FROM public_urls p LEFT JOIN machines m ON m.id = p.machine_id`;

/** The user's public URLs, oldest first. */
export async function urlsOfUser(
  db: D1Database,
  baseDomain: string,
  userId: string,
): Promise<PublicUrl[]> {
  const { results } = await db
    .prepare(
      `${SELECT_URLS} WHERE p.user_id = ?1 AND p.deleted_at IS NULL ORDER BY p.created_at, p.id`,
    )
    .bind(userId)
    .all<PublicUrlRow>();
  return results.map((row) => toPublicUrl(row, baseDomain));
}

/** The public URLs a machine serves, oldest first. */
export async function urlsOfMachine(
  db: D1Database,
  baseDomain: string,
  machineId: string,
): Promise<PublicUrl[]> {
  const { results } = await db
    .prepare(
      `${SELECT_URLS} WHERE p.machine_id = ?1 AND p.deleted_at IS NULL ORDER BY p.created_at, p.id`,
    )
    .bind(machineId)
    .all<PublicUrlRow>();
  return results.map((row) => toPublicUrl(row, baseDomain));
}

/** One live public URL of the user, or null for an unknown id and for someone else's. */
export async function urlOfUser(
  db: D1Database,
  baseDomain: string,
  userId: string,
  urlId: string,
): Promise<PublicUrl | null> {
  const row = await db
    .prepare(`${SELECT_URLS} WHERE p.id = ?1 AND p.user_id = ?2 AND p.deleted_at IS NULL`)
    .bind(urlId, userId)
    .first<PublicUrlRow>();
  return row ? toPublicUrl(row, baseDomain) : null;
}

export function toPublicUrl(row: PublicUrlRow, baseDomain: string): PublicUrl {
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
    protection: { mode: row.protected === 1 ? "password" : "public" },
    createdAt: new Date(row.created_at).toISOString(),
  };
}

/** True when a D1 error is a UNIQUE constraint failure that names `table.column`. */
export function isUniqueViolation(error: unknown, column: string): boolean {
  const text = String(error);
  return text.includes("UNIQUE constraint failed") && text.includes(column);
}
