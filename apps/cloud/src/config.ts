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
  | "INGRESS_KEY";

/** A setting's value, or a ConfigError when it is empty: nothing runs on a default. */
export function setting(env: Env, name: Setting): string {
  const value = env[name];
  if (typeof value !== "string" || value.trim() === "") throw new ConfigError(name);
  return value.trim();
}

/**
 * `ALLOWED_GITHUB_IDS` holds comma-separated numeric GitHub user ids. Ids,
 * not logins: a login can be renamed and then taken by someone else.
 */
export function isAllowedGithubId(env: Env, githubId: number): boolean {
  const wanted = String(githubId);
  return (env.ALLOWED_GITHUB_IDS ?? "").split(",").some((entry) => entry.trim() === wanted);
}
