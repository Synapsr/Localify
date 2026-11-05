// Deploys the Worker with its secrets read from Infisical (project
// `localify`, folder `/cloud`, environment `prod`).
//
// wrangler deploys with its own login. The CLOUDFLARE_API_TOKEN kept in
// Infisical only has Tunnel and DNS rights: it becomes a secret of the
// Worker, never wrangler's identity, so it is kept out of wrangler's
// environment.

import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SECRETS = ["CLOUDFLARE_API_TOKEN", "COOKIE_KEY", "INGRESS_KEY"];

const exported = JSON.parse(
  execFileSync(
    "infisical",
    ["export", "--env", "prod", "--path", "/cloud", "--format", "json", "--silent"],
    { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] },
  ),
);
const values = Object.fromEntries(exported.map((secret) => [secret.key, secret.value]));
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
    {
      mode: 0o600,
    },
  );
  const env = { ...process.env };
  delete env.CLOUDFLARE_API_TOKEN;
  delete env.CLOUDFLARE_ACCOUNT_ID;
  const result = spawnSync(
    "wrangler",
    ["deploy", "--secrets-file", file, ...process.argv.slice(2)],
    {
      stdio: "inherit",
      env,
    },
  );
  process.exitCode = result.status ?? 1;
} finally {
  rmSync(dir, { recursive: true, force: true });
}
