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
  | "EDGE_KEY";

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
 * `ALLOWED_GITHUB_IDS` holds comma-separated numeric GitHub user ids. Ids,
 * not logins: a login can be renamed and then taken by someone else.
 */
export function isAllowedGithubId(env: Env, githubId: number): boolean {
  const wanted = String(githubId);
  return (env.ALLOWED_GITHUB_IDS ?? "").split(",").some((entry) => entry.trim() === wanted);
}
