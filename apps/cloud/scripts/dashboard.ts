/// <reference types="node" />
// `pnpm cloud:dashboard [--local]`: a read-only overview of every Localify
// Cloud account, served on this machine. Start it through Localify, which
// gives it a port and a `.localify` URL (apps/cloud/README.md, "Dashboard").
// It reads the production D1 as `CLOUDFLARE_DEPLOY_TOKEN` from Infisical,
// in one `wrangler d1 execute` call of SELECT statements at most every 30
// seconds; `--local` reads the D1 of `pnpm cloud:dev` instead.

import { createServer } from "node:http";

import { wranglerReader } from "./dashboard/db.ts";
import { cachedLoader, createHandler } from "./dashboard/server.ts";

const args = process.argv.slice(2).filter((arg) => arg !== "--");
const unknown = args.filter((arg) => arg !== "--local");
if (unknown.length > 0) {
  console.error(`Unknown argument: ${unknown.join(" ")}. Usage: pnpm cloud:dashboard [--local]`);
  process.exit(1);
}
const local = args.includes("--local");

const port = Number(process.env.PORT);
if (!Number.isInteger(port) || port <= 0 || port > 65535) {
  console.error(
    'PORT is not set: start the dashboard through Localify, which picks its port (apps/cloud/README.md, "Dashboard").',
  );
  process.exit(1);
}

// Stopping kills a read in progress, so a stalled wrangler never holds the exit.
const reads = new AbortController();
const handler = createHandler(cachedLoader(wranglerReader(local, reads.signal)), {
  source: local ? "Local D1" : "Production D1",
});
const server = createServer((request, response) => {
  handler(request, response).catch((error: unknown) => {
    console.error(error);
    if (!response.headersSent) response.writeHead(500, { "content-type": "text/plain" });
    response.end("Internal error.");
  });
});
// Loopback only: the Localify proxy is the way in.
server.listen(port, "127.0.0.1", () => {
  const url = process.env.LOCALIFY_URL ?? `http://127.0.0.1:${port}`;
  console.log(
    `Localify Cloud dashboard, read-only, on ${local ? "the local" : "the production"} D1: ${url}`,
  );
});
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    reads.abort();
    server.close(() => process.exit(0));
    server.closeAllConnections();
  });
}
