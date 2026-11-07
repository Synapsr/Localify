// Deploys the Worker with its secrets read from Infisical (project
// `localify`, folder `/cloud`, environment `prod`). wrangler's identity is
// chosen in infisical.mjs.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { readCloudValues, runWrangler } from "./infisical.mjs";

const SECRETS = ["CLOUDFLARE_API_TOKEN", "COOKIE_KEY", "INGRESS_KEY", "EDGE_KEY"];

const values = readCloudValues();
const missing = SECRETS.filter((name) => !values[name]);
if (missing.length > 0) {
  console.error(`Missing in Infisical (localify, /cloud, prod): ${missing.join(", ")}`);
  process.exit(1);
}

const dir = mkdtempSync(join(tmpdir(), "localify-cloud-"));
const file = join(dir, "secrets.json");
try {
  writeFileSync(
    file,
    JSON.stringify(Object.fromEntries(SECRETS.map((name) => [name, values[name]]))),
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
