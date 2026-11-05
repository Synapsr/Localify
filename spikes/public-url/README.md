# Public URL transport spike

Throwaway spike for the native connector transport chosen in ADR 0003: a Worker, one `PublicEndpoint` Durable Object per endpoint, and a Rust connector multiplexing public requests over one WebSocket. It answers two questions before any product code: do Vite HMR and SSE survive the tunnel, and what does the protocol cost on a page that loads hundreds of modules?

Everything runs locally with `wrangler dev`, except the [real-network run](#real-network), which was deployed for one session and then deleted.

## Layout

- `edge/`: Worker and Durable Object (TypeScript). `src/protocol.ts` defines the frame format.
- `connector/`: standalone Rust connector. `src/protocol.rs` mirrors the frame format.
- `fixture/`: Vite 7 app with 331 generated modules (a wide tree plus a 10-deep import chain), an HMR target, and `/sse`, `/big`, `/echo`, `/headers` routes.
- `tunnel-edge/`: the alternative measured afterwards, a Worker that checks an access cookie and forwards to a named Cloudflare Tunnel (no Durable Object).
- `bench/`: Playwright scripts driving the installed Chrome. `run.mjs` is the full bench; `diag.mjs` repeats page loads and lists failed or pending requests.

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

## Real network

Run on 2026-11-05. The Worker was deployed on `localify.fyi` with exact hostnames as Worker custom domains (`edge/wrangler.measure.jsonc`), then deleted. The fixture and connector ran on one Mac at home, the browser on a laptop on another ISP. Both reached Cloudflare in Paris (CDG), about 20 ms away. A `cloudflared` Quick Tunnel to the same fixture served as the comparison.

```bash
# Laptop: deploy the edge (custom domains need the localify.fyi zone)
cd edge && wrangler deploy -c wrangler.measure.jsonc --secrets-file <file with CONNECTOR_TOKEN>
# Remote machine: fixture, then the connector pointed at the deployed edge
LOCALIFY_EDGE_URL=wss://localify.fyi LOCALIFY_SLUG=bench-a8f3 LOCALIFY_CONNECTOR_TOKEN=<secret> cargo run --release
# Laptop: local fixture as the reference, then the bench through the tunnel
SPIKE_TUNNEL=https://feature--bench-a8f3.localify.fyi node bench/run.mjs
node bench/diag.mjs https://feature--bench-a8f3.localify.fyi 15
```

Remote mode triggers HMR through the tunnel with `POST /__spike/hmr`, since the HMR target lives on the other machine.

| Measure, from the laptop | Localify tunnel | `cloudflared` | Local Vite |
|---|---|---|---|
| One request, median | 84–104 ms | 65–67 ms | 0.3 ms |
| 336-module page, median of 15 | 2.15 s | 1.48 s | 0.1 s |

Per-request median from the laptop, by segment: Worker alone 32 ms, Worker and Durable Object 45 ms, full tunnel 104 ms. The connector leg is the round trip to the remote machine's network.

Functional checks through the real tunnel all pass: the page loads with correct module values, HMR applies in 613 ms without a reload (including the trigger request), SSE streams, a 20 MB download (23 MB/s) and an 8 MB upload arrive intact, and aborting a stream does not break the next request.

Findings:

- **Upstream connections must be capped.** Over HTTP/2 the browser sends the whole module graph at once. The connector opened one connection per request to Vite, overflowed its accept backlog (128 on macOS), and up to 145 of 300 requests failed with 502 or stalled until the 75 s connect timeout, mostly after a few idle seconds when Vite had closed its keep-alive connections. A semaphore of 32 in-flight upstream requests fixed it: 4 of 4 bursts and 15 of 15 page loads succeeded. The local `wrangler dev` runs never hit this because the browser spoke HTTP/1.1, limited to 6 connections. The daemon today is not affected either, since its TLS listener does not offer HTTP/2.
- **Nagle is not the cost.** Disabling it on the connector socket changed the per-request median from 89.5 to 93.6 ms, within noise. It stays disabled.
- **The Durable Object design costs about 20 ms per request and 45% on this page compared with `cloudflared`.** The extra hops are the Worker and the Durable Object. Both designs are dominated by round trips times import depth: a dev page over a real network takes seconds either way.

## Named tunnel

Measured the same day, after a review by Codex recommended replacing the native connector with one named Cloudflare Tunnel per machine. `tunnel-edge/` checks a signed `__Host-` access cookie, strips Localify, `X-Forwarded-*`, `CF-*` and `Forwarded` headers and the access cookie, then fetches the machine's tunnel hostname with a secret header that the machine's ingress would check. The tunnel was created through the API with `config_src: "cloudflare"`, its ingress pointing at the fixture with `httpHostHeader: feature.app.localify`, and `cloudflared tunnel run` read its token from `TUNNEL_TOKEN`. It opened four QUIC connections, in Paris and Marseille.

| Measure, from the laptop | Worker + named tunnel | Named tunnel alone | Quick Tunnel | Worker + DO + connector |
|---|---|---|---|---|
| One request, median | 59–64 ms | 54–55 ms | 66 ms | 84–104 ms |
| 336-module page, median of 15 | 1.01 s | | 1.23 s | 2.15 s |
| 300-request burst after 12 s idle | 300/300, 0.3–0.4 s | | | 300/300, 0.8–1.1 s with the cap |

The full bench through the Worker and the named tunnel: page load 970 ms, 61 ms per sequential request, HMR in 495 ms without a reload over the proxied WebSocket, SSE streamed, 20 MB down and 8 MB up intact, abort then reuse fine.

Findings:

- **The named tunnel wins on every measure,** and the Worker in front costs 4 to 10 ms. Localify drops its own multiplexing protocol, flow control and stream lifecycle, and supervises `cloudflared` instead.
- **Cloudflare rewrites `X-Forwarded-Host`** to the tunnel hostname by the time the request reaches the origin. The public host has to travel in a Localify header that the machine's ingress trusts only with the right secret.
- **The tunnel hostname is public.** Without the secret check at the machine's ingress, anyone who learns it skips the password.
- **The cap still matters behind the tunnel.** `cloudflared` pooled its connections to the fixture, but the daemon's proxy opens one upstream connection per request, so the ingress must bound concurrent requests.

## Not covered

- **Flow control.** The Durable Object queues response chunks without backpressure, and the connector buffers request bodies without limit. A fast upstream feeding a slow browser could fill the Durable Object's memory. The product needs per-stream credit windows.
- **Hibernation.** Not observed under `wrangler dev`.
- **Daemon integration.** The spike connector talks to Vite directly. Looping through the daemon would lose the public host, because `set_forwarding_headers` in `src/daemon/proxy.rs` overwrites `X-Forwarded-*` from the daemon's own client. That argues for dispatching public requests straight into the router.
- **Connector auth.** A static token, instead of short-lived scoped tokens (ADR 0003).
- **Access protection**, custom hostnames, and quotas.
- **Stream id reuse.** Ids restart at 1 after hibernation. The connector clears finished streams through a `done` channel, so a reused id could in theory be cleared by a stale notification.
