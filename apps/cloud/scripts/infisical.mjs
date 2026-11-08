// Runs wrangler against the production account with the values kept in
// Infisical (project `localify`, folder `/cloud`, environment `prod`, or
// `dev` for the sandbox's Worker secrets).
//
// wrangler's identity is CLOUDFLARE_DEPLOY_TOKEN when Infisical has one, and
// the person's own `wrangler login` otherwise. The Worker's own
// CLOUDFLARE_API_TOKEN (Tunnel and DNS rights) never becomes wrangler's
// identity: it is a secret of the Worker only.

import { execFileSync, spawnSync } from "node:child_process";

/** The `/cloud` values for an Infisical environment, `prod` by default, by key. */
export function readCloudValues(environment = "prod") {
  const exported = JSON.parse(
    execFileSync(
      "infisical",
      ["export", "--env", environment, "--path", "/cloud", "--format", "json", "--silent"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] },
    ),
  );
  return Object.fromEntries(exported.map((secret) => [secret.key, secret.value]));
}

/**
 * Takes `--env sandbox` (or `--env=sandbox`) out of a script's arguments:
 * the wrangler environment `sandbox` of wrangler.jsonc. Any other `--env`
 * is refused, so a typo never falls back to production.
 */
export function readTarget(args) {
  const rest = [];
  let sandbox = false;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    // pnpm passes a `--` through as it is.
    if (arg === "--") continue;
    const value = arg === "--env" ? args[++i] : arg.startsWith("--env=") ? arg.slice(6) : null;
    if (value === null) {
      rest.push(arg);
    } else if (value === "sandbox") {
      sandbox = true;
    } else {
      console.error(`Unknown --env ${value ?? ""}: the only one is --env sandbox.`);
      process.exit(1);
    }
  }
  return { sandbox, rest };
}

/** The environment wrangler runs with: the deploy identity, never the Worker's token. */
function wranglerEnv(values) {
  const env = { ...process.env };
  delete env.CLOUDFLARE_API_TOKEN;
  delete env.CLOUDFLARE_ACCOUNT_ID;
  if (values.CLOUDFLARE_DEPLOY_TOKEN) {
    env.CLOUDFLARE_API_TOKEN = values.CLOUDFLARE_DEPLOY_TOKEN;
    env.CLOUDFLARE_ACCOUNT_ID = values.CLOUDFLARE_ACCOUNT_ID;
  }
  return env;
}

/** Runs `wrangler <args>` with the deploy identity; returns its exit status. */
export function runWrangler(values, args) {
  const result = spawnSync("wrangler", args, { stdio: "inherit", env: wranglerEnv(values) });
  return result.status ?? 1;
}

/**
 * Runs `wrangler <args>` with the deploy identity and returns what it printed
 * on stdout. Its stderr goes to the terminal; a failure throws.
 */
export function wranglerOutput(values, args) {
  const result = spawnSync("wrangler", args, {
    stdio: ["ignore", "pipe", "inherit"],
    env: wranglerEnv(values),
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`wrangler ${args[0] ?? ""} failed:\n${result.stdout ?? ""}`);
  }
  return result.stdout;
}

// `node scripts/infisical.mjs <wrangler args>`, for the migration scripts.
if (import.meta.url === `file://${process.argv[1]}`) {
  process.exitCode = runWrangler(readCloudValues(), process.argv.slice(2));
}
