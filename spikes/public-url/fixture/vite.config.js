import { createHash } from "node:crypto";
import { defineConfig } from "vite";

// Local domains the connector sends in `Host` (ADR 0017).
const ALLOWED_HOSTS = [".app.localify"];

export default defineConfig({
  root: import.meta.dirname,
  logLevel: "warn",
  server: { host: "127.0.0.1", port: 5199, strictPort: true, allowedHosts: ALLOWED_HOSTS },
  plugins: [spikeRoutes()],
});

function spikeRoutes() {
  return {
    name: "public-url-spike-routes",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const url = new URL(req.url, "http://fixture");
        if (url.pathname === "/sse") return sse(res, Number(url.searchParams.get("count") ?? 10));
        if (url.pathname === "/big") return big(res, Number(url.searchParams.get("mb") ?? 20));
        if (url.pathname === "/echo" && req.method === "POST") return echo(req, res);
        if (url.pathname === "/headers") return json(res, req.headers);
        next();
      });
    },
  };
}

function sse(res, count) {
  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
  let sent = 0;
  const timer = setInterval(() => {
    res.write(`data: ${JSON.stringify({ n: sent, at: Date.now() })}\n\n`);
    if (++sent >= count) {
      clearInterval(timer);
      res.end();
    }
  }, 200);
  res.on("close", () => clearInterval(timer));
}

/** Deterministic bytes (i % 251) so the client can verify the download. */
function big(res, megabytes) {
  const total = megabytes * 1024 * 1024;
  const chunkSize = 64 * 1024;
  res.writeHead(200, { "content-type": "application/octet-stream", "content-length": total });
  let offset = 0;
  const writeMore = () => {
    while (offset < total) {
      const size = Math.min(chunkSize, total - offset);
      const chunk = Buffer.allocUnsafe(size);
      for (let i = 0; i < size; i++) chunk[i] = (offset + i) % 251;
      offset += size;
      if (!res.write(chunk)) return res.once("drain", writeMore);
    }
    res.end();
  };
  writeMore();
}

function echo(req, res) {
  const hash = createHash("sha256");
  let bytes = 0;
  req.on("data", (chunk) => {
    bytes += chunk.length;
    hash.update(chunk);
  });
  req.on("end", () => json(res, { bytes, sha256: hash.digest("hex") }));
}

function json(res, value) {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}
