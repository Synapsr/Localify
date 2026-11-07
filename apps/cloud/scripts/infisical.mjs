// Runs wrangler against the production account with the values kept in
// Infisical (project `localify`, folder `/cloud`, environment `prod`).
//
// wrangler's identity is CLOUDFLARE_DEPLOY_TOKEN when Infisical has one, and
// the person's own `wrangler login` otherwise. The Worker's own
// CLOUDFLARE_API_TOKEN (Tunnel and DNS rights) never becomes wrangler's
// identity: it is a secret of the Worker only.

import { execFileSync, spawnSync } from "node:child_process";

/** The `/cloud` values for `prod`, by key. */
export function readCloudValues() {
  const exported = JSON.parse(
    execFileSync(
      "infisical",
      ["export", "--env", "prod", "--path", "/cloud", "--format", "json", "--silent"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] },
    ),
  );
  return Object.fromEntries(exported.map((secret) => [secret.key, secret.value]));
}

/** Runs `wrangler <args>` with the deploy identity; returns its exit status. */
export function runWrangler(values, args) {
  const env = { ...process.env };
  delete env.CLOUDFLARE_API_TOKEN;
  delete env.CLOUDFLARE_ACCOUNT_ID;
  if (values.CLOUDFLARE_DEPLOY_TOKEN) {
    env.CLOUDFLARE_API_TOKEN = values.CLOUDFLARE_DEPLOY_TOKEN;
    env.CLOUDFLARE_ACCOUNT_ID = values.CLOUDFLARE_ACCOUNT_ID;
  }
  const result = spawnSync("wrangler", args, { stdio: "inherit", env });
  return result.status ?? 1;
}

// `node scripts/infisical.mjs <wrangler args>`, for the migration scripts.
if (import.meta.url === `file://${process.argv[1]}`) {
  process.exitCode = runWrangler(readCloudValues(), process.argv.slice(2));
}
