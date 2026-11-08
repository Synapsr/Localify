// Deploys the Worker with its secrets read from Infisical (project
// `localify`, folder `/cloud`, environment `prod`). wrangler's identity is
// chosen in infisical.mjs.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { readCloudValues, runWrangler } from "./infisical.mjs";

const SECRETS = [
  "CLOUDFLARE_API_TOKEN",
  "COOKIE_KEY",
  "INGRESS_KEY",
  "EDGE_KEY",
  "POLAR_ACCESS_TOKEN",
  "POLAR_WEBHOOK_SECRET",
];
// Polar gives this one when the webhook endpoint is created, after the first
// deploy. Until then the Worker answers webhooks with 503.
const OPTIONAL = new Set(["POLAR_WEBHOOK_SECRET"]);

const values = readCloudValues();
const missing = SECRETS.filter((name) => !values[name] && !OPTIONAL.has(name));
if (missing.length > 0) {
  console.error(`Missing in Infisical (localify, /cloud, prod): ${missing.join(", ")}`);
  process.exit(1);
}
const passed = SECRETS.filter((name) => values[name]);
for (const name of SECRETS.filter((name) => !values[name])) {
  console.warn(`${name} is not in Infisical yet: the Worker runs without it.`);
}

const dir = mkdtempSync(join(tmpdir(), "localify-cloud-"));
const file = join(dir, "secrets.json");
try {
  writeFileSync(
    file,
    JSON.stringify(Object.fromEntries(passed.map((name) => [name, values[name]]))),
    { mode: 0o600 },
  );
  process.exitCode = runWrangler(values, [
    "deploy",
    "--secrets-file",
    file,
    ...process.argv.slice(2),
  ]);
} finally {
  rmSync(dir, { recursive: true, force: true });
}
