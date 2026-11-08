// Reads the Worker's configuration (docs/public-urls.md, "Configuration").

export class ConfigError extends Error {
  constructor(readonly setting: string) {
    super(`${setting} is not set`);
  }
}

type Setting =
  | "GITHUB_CLIENT_ID"
  | "PUBLIC_BASE_DOMAIN"
  | "MACHINE_DOMAIN"
  | "CLOUDFLARE_ACCOUNT_ID"
  | "MACHINE_ZONE_ID"
  | "CLOUDFLARE_API_TOKEN"
  | "COOKIE_KEY"
  | "INGRESS_KEY"
  | "EDGE_KEY"
  | "POLAR_API_BASE"
  | "POLAR_ACCESS_TOKEN"
  | "POLAR_PRODUCT_PRO_MONTHLY"
  | "POLAR_PRODUCT_PRO_YEARLY";

/** A setting's value, or a ConfigError when it is empty: nothing runs on a default. */
export function setting(env: Env, name: Setting): string {
  const value = env[name];
  if (typeof value !== "string" || value.trim() === "") throw new ConfigError(name);
  return value.trim();
}

/** `MAX_TUNNELS`: the most serving slots the whole Cloudflare account holds. */
export function maxTunnels(env: Env): number {
  const value = Number(env.MAX_TUNNELS);
  if (!Number.isInteger(value) || value < 1) throw new ConfigError("MAX_TUNNELS");
  return value;
}

/**
 * `SIGNUP`: who may finish a new sign-in. `allowlist` lets in only the ids
 * in `ALLOWED_GITHUB_IDS`, `open` anyone. It gates the device flow only:
 * entitlements decide what an account may use, in both modes.
 */
export function signup(env: Env): "allowlist" | "open" {
  const value = typeof env.SIGNUP === "string" ? env.SIGNUP.trim() : "";
  if (value !== "allowlist" && value !== "open") throw new ConfigError("SIGNUP");
  return value;
}

/**
 * Whether `list`, comma-separated numeric GitHub user ids, holds `githubId`.
 * Ids, not logins: a login can be renamed and then taken by someone else.
 */
function listsGithubId(list: string | undefined, githubId: number): boolean {
  const wanted = String(githubId);
  return (list ?? "").split(",").some((entry) => entry.trim() === wanted);
}

/** `ALLOWED_GITHUB_IDS`: under `SIGNUP=allowlist`, who may sign in. */
export function isAllowedGithubId(env: Env, githubId: number): boolean {
  return listsGithubId(env.ALLOWED_GITHUB_IDS, githubId);
}

/**
 * `TEAM_GITHUB_IDS`: who gets the team override when they sign in. It
 * outlives the allowlist, which goes away once `SIGNUP` opens.
 */
export function isTeamGithubId(env: Env, githubId: number): boolean {
  return listsGithubId(env.TEAM_GITHUB_IDS, githubId);
}

/** The two Pro product ids, monthly then yearly: the only Pro products a checkout offers. */
export function proProducts(env: Env): { month: string; year: string } {
  return {
    month: setting(env, "POLAR_PRODUCT_PRO_MONTHLY"),
    year: setting(env, "POLAR_PRODUCT_PRO_YEARLY"),
  };
}

/**
 * `POLAR_WEBHOOK_SECRET`, or null while the webhook endpoint does not exist
 * yet: it is created after the first deploy, so a deploy runs without it.
 */
export function webhookSecret(env: Env): string | null {
  const value = (env as Env & { POLAR_WEBHOOK_SECRET?: unknown }).POLAR_WEBHOOK_SECRET;
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}
