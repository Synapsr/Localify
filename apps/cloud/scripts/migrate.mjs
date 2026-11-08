// Applies the D1 migrations to the remote database: `localify-cloud`, or
// with `--env sandbox`, `localify-cloud-sandbox`. Migrations need no Worker
// secret, only wrangler's identity, chosen in infisical.mjs from `prod`: the
// sandbox lives in the same Cloudflare account.

import { readCloudValues, readTarget, runWrangler } from "./infisical.mjs";

const { sandbox, rest } = readTarget(process.argv.slice(2));

process.exitCode = runWrangler(readCloudValues("prod"), [
  "d1",
  "migrations",
  "apply",
  sandbox ? "localify-cloud-sandbox" : "localify-cloud",
  "--remote",
  // Always named, production as "": wrangler warns when none is.
  ...(sandbox ? ["--env", "sandbox"] : ["--env", ""]),
  ...rest,
]);
