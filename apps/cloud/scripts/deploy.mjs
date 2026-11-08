// Deploys the Worker with its secrets read from Infisical (project
// `localify`, folder `/cloud`): environment `prod`, or with `--env sandbox`,
// the sandbox Worker with the secrets of `dev`, which holds the sandbox
// POLAR_ACCESS_TOKEN. wrangler's identity is chosen in infisical.mjs, from
// `prod` in both cases: the sandbox lives in the same Cloudflare account.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { readCloudValues, readTarget, runWrangler } from "./infisical.mjs";

const { sandbox, rest } = readTarget(process.argv.slice(2));
const environment = sandbox ? "dev" : "prod";

// The sandbox never gets CLOUDFLARE_API_TOKEN: it must not touch the
// account's tunnels or DNS records, which belong to production.
const SECRETS = [
  ...(sandbox ? [] : ["CLOUDFLARE_API_TOKEN"]),
  "COOKIE_KEY",
  "INGRESS_KEY",
  "EDGE_KEY",
  "POLAR_ACCESS_TOKEN",
  "POLAR_WEBHOOK_SECRET",
];
// Polar gives this one when the webhook endpoint is created, after the first
// deploy. Until then the Worker answers webhooks with 503.
const OPTIONAL = new Set(["POLAR_WEBHOOK_SECRET"]);

const values = readCloudValues(environment);
const identity = sandbox ? readCloudValues("prod") : values;
const missing = SECRETS.filter((name) => !values[name] && !OPTIONAL.has(name));
if (missing.length > 0) {
  console.error(`Missing in Infisical (localify, /cloud, ${environment}): ${missing.join(", ")}`);
  process.exit(1);
}
const passed = SECRETS.filter((name) => values[name]);
for (const name of SECRETS.filter((name) => !values[name])) {
  console.warn(`${name} is not in Infisical (${environment}) yet: the Worker runs without it.`);
}

const dir = mkdtempSync(join(tmpdir(), "localify-cloud-"));
const file = join(dir, "secrets.json");
try {
  writeFileSync(
    file,
    JSON.stringify(Object.fromEntries(passed.map((name) => [name, values[name]]))),
    { mode: 0o600 },
  );
  process.exitCode = runWrangler(identity, [
    "deploy",
    "--secrets-file",
    file,
    // Always named, production as "": wrangler warns when none is.
    ...(sandbox ? ["--env", "sandbox"] : ["--env", ""]),
    ...rest,
  ]);
} finally {
  rmSync(dir, { recursive: true, force: true });
}
