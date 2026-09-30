# Public URL transport spike

Throwaway spike for the native connector transport chosen in ADR 0003: a Worker, one `PublicEndpoint` Durable Object per endpoint, and a Rust connector multiplexing public requests over one WebSocket. It answers two questions before any product code: do Vite HMR and SSE survive the tunnel, and what does the protocol cost on a page that loads hundreds of modules?

Everything runs locally with `wrangler dev`. Nothing is deployed.

## Layout

- `edge/`: Worker and Durable Object (TypeScript). `src/protocol.ts` defines the frame format.
- `connector/`: standalone Rust connector. `src/protocol.rs` mirrors the frame format.
- `fixture/`: Vite 7 app with 331 generated modules (a wide tree plus a 10-deep import chain), an HMR target, and `/sse`, `/big`, `/echo`, `/headers` routes.
- `bench/`: Playwright script driving the installed Chrome.

## Run it

```bash
# 1. Fixture on 127.0.0.1:5199
node fixture/generate.mjs
../../node_modules/.bin/vite --config fixture/vite.config.js

# 2. Edge on 127.0.0.1:8787
cd edge && ../../../apps/web/node_modules/.bin/wrangler dev --ip 127.0.0.1 --port 8787

# 3. Connector (defaults: slug app-a8f3, local domain app.localify, upstream 127.0.0.1:5199)
cd connector && cargo run --release

# 4. Bench
cd bench && npm install && node run.mjs
```

Public URLs use Chrome's built-in `*.localhost` resolution: `http://feature--app-a8f3.localhost:8787` maps to label `feature`, slug `app-a8f3`, and local host `feature.app.localify`.

## Protocol

One binary WebSocket message per frame: a type byte, a big-endian `u32` stream id, then the payload. Request and response heads are JSON; bodies are raw chunks of at most 64 KiB.

| Type | Direction | Payload |
|---|---|---|
| `RequestHead` | edge → connector | method, path, headers, public host, label, `hasBody`, `websocket` |
| `RequestBody`, `RequestEnd` | edge → connector | body chunk, nothing |
| `ResponseHead` | connector → edge | status, headers |
| `ResponseBody`, `ResponseEnd` | connector → edge | body chunk, nothing |
| `Reset` | both | reason |
| `WsText`, `WsBinary`, `WsClose` | both | message, or close code and reason |

Browser WebSockets are accepted with the Hibernation API and tagged `stream:<id>`, so the mapping survives hibernation without in-memory state. HTTP streams live in memory only while a request is in flight.

## Results

Two bench runs on 2026-09-29, M-series Mac, Chrome headless, HTTP/1.1 everywhere. Page load is the median of 7 cold loads (336 requests each).

| Path | Page load | Sequential request |
|---|---|---|
| Direct to Vite | 70–71 ms | 0.3 ms |
| Worker only, `fetch` to Vite | 262–293 ms | 1.2–2.1 ms |
| Worker + Durable Object, `fetch` to Vite | 364–388 ms | 1.4–2.8 ms |
| Full tunnel (Worker + DO + connector) | 282–298 ms | 1.1–2.1 ms |

The tunnel costs about the same as a Worker that does nothing but `fetch` the app: 5 to 20 ms more on a 336-request page. Almost all of the gap with direct access comes from the local `wrangler dev` stack, not from the connector protocol. Multiplexing over the connector socket was even faster than the Durable Object's own outbound `fetch` locally.

Functional checks, all passing on both runs:

- 336-module page loads with correct module values.
- Vite HMR connects through the tunnel (the `vite-hmr` subprotocol is negotiated end to end) and applies a hot update in 120–195 ms without a full reload.
- SSE: 10 events arrive about 200 ms apart, so responses stream rather than buffer.
- 20 MB download intact (about 190 MB/s locally), 8 MB upload intact (SHA-256 checked).
- Aborting a streaming response mid-flight does not break the next request.
- The upstream app sees `Host: feature.app.localify` and `X-Forwarded-Host` set to the public host (ADR 0017). Client-supplied `X-Localify-*` and `X-Forwarded-*` headers are overwritten.
- With the connector stopped, the public URL returns a 502 "offline" page. After a restart, or after `wrangler dev` reloads the Durable Object, the connector reconnects on its own.

## Not covered

- **Real network latency.** Locally every hop is free. In production each Vite import level pays browser → edge → Durable Object → connector and back, and this fixture has about 13 levels. It has to be measured with the connector on the remote machine and the browser on the laptop, on the real `localify.fyi` zone. `workers.dev` cannot host wildcard labels.
- **Flow control.** The Durable Object queues response chunks without backpressure, and the connector buffers request bodies without limit. A fast upstream feeding a slow browser could fill the Durable Object's memory. The product needs per-stream credit windows.
- **Hibernation.** Not observed under `wrangler dev`.
- **Daemon integration.** The spike connector talks to Vite directly. Looping through the daemon would lose the public host, because `set_forwarding_headers` in `src/daemon/proxy.rs` overwrites `X-Forwarded-*` from the daemon's own client. That argues for dispatching public requests straight into the router.
- **Connector auth.** A static token, instead of short-lived scoped tokens (ADR 0003).
- **Access protection**, custom hostnames, and quotas.
- **Stream id reuse.** Ids restart at 1 after hibernation. The connector clears finished streams through a `done` channel, so a reused id could in theory be cleared by a stale notification.
