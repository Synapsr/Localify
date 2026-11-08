# Remote machines

The contract for remote machines: what the cloud, the relay, the daemon, the desktop app, the CLI and the MCP server each do, and the messages between them. The decisions behind it are ADRs 0028 to 0032; ADR 0031 amends ADR 0012. It builds on [public-urls.md](public-urls.md) for accounts, machines, plans and the Worker, and on the entitlement model of PR #56.

## What it does

A person signed in to Localify Cloud on two or more computers sees the others from any of them: their projects and services, whether each one runs, a service's live output, its public URL, and buttons to start, stop and restart it. A laptop shows what an agent runs on a desktop in the next room, and restarts the API server it crashed. The goal is the experience of T3 Code's remote environments: pick a machine, and the app shows that machine.

```text
 viewer machine                  Localify Cloud                          target machine
┌──────────────────────┐        ┌──────────────────────────────┐        ┌──────────────────────┐
│ app · CLI · MCP      │        │ Worker (api.localify.dev)    │        │ desktop app          │
│   │ mgmt socket      │  wss   │ token, ban, remoteAccess     │  wss   │   ▲ app bridge       │
│   ▼                  │───────▶│            │                 │◀───────│   │                  │
│ daemon               │        │            ▼                 │        │ daemon               │
│ Noise session ═══════│════════│ RemoteRelay, one per account │════════│ Noise session        │
│                      │        │ routes frames by machine id, │        │ inventory, output,   │
│                      │        │ stores no payload            │        │ controls             │
└──────────────────────┘        └──────────────────────────────┘        └──────────────────────┘
```

Every daemon connects out to the relay; nothing listens on a machine for remote access. The relay forwards opaque frames between machines of one account. What the machines say to each other is end-to-end encrypted between paired machines, so Localify Cloud routes it without reading it.

Goals:

- see every machine of the account, with its connection state, in one switcher;
- see a target's projects, environments and services, with four separate states: process running, port reachable, output available, public URL serving;
- read and follow a service's output, as plain lines, with search;
- start, stop and restart a service on a target that allows it, through whoever owns that service there;
- the same from the CLI and, within limits, from an agent over MCP;
- nothing streams and nothing costs while nobody looks.

Not covered here: typing into a service, attaching to a terminal, resizing one; opening a target's `.localify` URL from the viewer; editing a target's projects, domains, profiles or files; running arbitrary commands; a web or phone viewer; machines of another account or a team; serving a target's dev server to the viewer outside public URLs; message history.

## Names

- **Machine:** one signed-in installation of Localify, known by its machine id (see public-urls.md, "Names"). The id is the identity. The name is a label: it can repeat over time, since a same-name sign-in replaces the previous machine with a new id.
- **Roster:** the account's active machines, from Localify Cloud: id, name, and remote key. Machine names may appear there; nothing else about a machine does.
- **Remote key:** a machine's X25519 static public key, 32 bytes, base64url without padding. Its private half never leaves the machine.
- **Fingerprint:** the first 8 bytes of SHA-256 of the remote key, as four groups of four hex digits (`1a2b 3c4d 5e6f 7a8b`).
- **Pairing:** a ceremony that makes two machines pin each other's remote key, after a person confirms the same 6-digit code on both.
- **Peer:** a machine this one has paired with, by machine id and pinned key.
- **Viewer:** the machine a person looks from. **Target:** the machine looked at. Any paired machine can be either.
- **Session:** a Noise channel between a viewer and a target, through the relay.
- **Run:** one start of a service by `localify dev`, known by its run id. **Owner:** who holds a run: the desktop app's terminal panel (`app`), a detached process (`background`), a terminal Localify does not own (`terminal`), or a process listening on the port with no run record (`unknown`).
- **Linked machine:** an active machine that has uploaded a remote key. An account has at most 10, the same limit as signed-in machines.

## Target settings

Three switches on each machine, set only on that machine, in the app's Settings → Remote access or with `localify machines settings`. All three start off. No message from another machine, and nothing from the cloud, can change them.

| Setting | Off | On |
|---|---|---|
| `share` | Paired machines see this machine's name, version and connection state only. | Paired machines see projects, environments, services, run states, public URLs and output. |
| `control` | No start, stop or restart from another machine. | Paired machines may start, stop and restart services this machine owns (see "Controls"). Needs `share`. |
| `backgroundStarts` | A start needs the desktop app running here. | When the app is closed, a start runs detached in the background. Needs `control`. |

Turning `share` off also turns the others off. Turning a switch off ends what depends on it at once: watches and follows end with `not_shared`, and controls already past their checks finish, but no new one starts.

## Trust

### Keys

On first need, which is the first relay connection, the daemon generates an X25519 key pair and writes it to `<data dir>/remote/identity.json` with `write_private_file` (0600), together with the machine id from `cloud.json`. It uploads the public key with `PUT /v1/machines/self/remote-key`. A machine's key is set once: the cloud refuses a second key for the same machine id. An identity file whose machine id is not the one in `cloud.json` belongs to an earlier sign-in and is deleted. The daemon runs outside the login session, so the Keychain is not used, for the same reason as `cloud.json` (ADR 0020).

Pins live in `<data dir>/remote/peers.json` (0600), written through a temporary file and a lock, as saved passwords are (public-urls.md, "Saved passwords"):

```json
{ "<machine id>": { "publicKey": "…", "name": "teo-mini", "pairedAt": "2026-11-08T10:12:00Z" } }
```

The target settings live in `<data dir>/remote/settings.json` (0600): `{"share": false, "control": false, "backgroundStarts": false}`. Signing out deletes the whole `remote/` directory, so a new sign-in starts with a new key, no pins and every switch off.

The roster's keys are never the root of trust. They tell a viewer which machines have a key and show a fingerprint, but a session only proceeds with a key pinned through pairing. The cloud can remove trust (by revoking a machine, which ends its token) but cannot add any.

### Pairing

Pairing runs over the relay as a Noise session with purpose `pair` (see "Sessions"), followed by a commit and reveal that makes the short code safe against a relay in the middle. Without the commitment, a relay running two handshakes could try ephemeral keys until both codes match; with it, each attempt succeeds with probability 10⁻⁶ and costs a person's click.

1. On the viewer A, the switcher lists B as "Not paired" with a `Pair` button. `localify machines pair <B>` does the same.
2. A sends a pairing request: the three Noise XX handshake messages with purpose `pair`. Both now hold the other's static key and the handshake hash `h`.
3. B's daemon checks that a person can answer: the desktop app is subscribed to its events, or `localify machines pair --accept` is waiting. Otherwise it answers `pair.declined` with `nobody_to_confirm`, and A shows: "Nobody can confirm on B. Open Localify on B, or run `localify machines pair --accept` there, then try again."
4. B picks a random 32-byte `nB` and sends `pair.commit` with `c = BLAKE2s("localify-pair-commit" ‖ h ‖ nB)`.
5. A picks a random 32-byte `nA` and sends `pair.nonce` with it.
6. B sends `pair.reveal` with `nB`. A checks it against `c`, and abandons the pairing on a mismatch.
7. Both compute the code: the first 4 bytes of `BLAKE2s("localify-pair-code" ‖ h ‖ nA ‖ nB)` as a big-endian integer, modulo 1,000,000, shown as two groups of three digits (`482 913`).
8. Both show it.
   - A: title "Pair with B", body "Check that B shows this code: **482 913**", buttons `Codes match` and `Cancel`.
   - B: title "A wants to pair with this Mac" (or "this computer" on Linux), body "Pair only if A shows the same code: **482 913**. A will then see this Mac's name. It sees projects and output, or controls services, only after you turn that on in Settings → Remote access.", buttons `Codes match, pair` and `Cancel`.
   - CLI: `A wants to pair. Code: 482 913. Type yes if A shows the same code:`.
9. Each side sends `pair.confirm` with `accept` once its person answers. A side writes its pin only when its own person accepted and the peer's `accept: true` arrived. A cancel, a mismatch, a closed session or 2 minutes without both answers pins nothing, and both show "Pairing cancelled. Nothing was saved."
10. After pairing, B shows "Paired with A" and a link to Settings → Remote access, with the three switches still off. A shows B as "Paired · not sharing" until B turns `share` on.

A target accepts one pairing at a time and at most 5 pairing attempts an hour; more answer `busy` with `retryAfter`. Pairing is symmetric: both machines pin each other, and each one's own settings decide what the other may see or do.

### Sessions

Every session, pairing included, uses `Noise_XX_25519_ChaChaPoly_BLAKE2s`, through the `snow` crate. The viewer is always the initiator.

- **Prologue:** `"localify-remote/1"`, the purpose (`pair` or `session`), the account's user id, the initiator's machine id, the responder's machine id, and the session id, each length-prefixed. A handshake replayed into another session, account, direction or purpose fails.
- **Session id:** 16 random bytes from the initiator, base64url, carried by every relay frame of the session.
- **Peer check:** once the handshake gives the peer's static key, each side compares it with the pin of the machine id the relay says the frame came from. A missing pin or a different key ends the session with `not_paired` or `key_mismatch`, and the UI says so. Pairing is the only purpose that accepts an unpinned key, and it pins nothing without the code.
- **Hello:** the first transport message each way is `hello` (see "Session messages"). Neither side sends anything else before it, and the target sends no inventory before it has checked the viewer's pin.
- **Replay:** each session uses fresh ephemeral keys, and each direction has a 64-bit nonce counter. Frames arrive in order over one WebSocket, so a frame that fails to decrypt, is repeated or is missing ends the session. Nothing from an earlier session decrypts in a later one.
- **Lifetime:** a session ends after 1 hour, after 100,000 messages, or after 120 seconds with no watch, follow or request open. The viewer opens a new one if the person is still looking.
- **Limits:** a target handles at most 4 handshakes at once and one session per viewer; a new session from the same viewer replaces the old one.

Why XX and `snow`: pairing starts without any known key, so the pattern must carry both static keys, authenticated, and XX does that with identity hiding for both. Using XX for every session keeps one code path. IK would save half a round trip once per session, under 100 ms through the relay, and its first message is replayable and sent before the responder's key is confirmed. `snow` is the most used Noise implementation in Rust, used by `rust-libp2p`, and gives the handshake hash the code needs. Its default resolver is pure Rust (RustCrypto's `chacha20poly1305`, `blake2` and `curve25519-dalek`). ChaChaPoly is constant-time in software on every CPU Localify supports.

### Unpairing, replacement, lost keys, revocation

- **Unpair:** `Unpair` in Settings → Remote access, or `localify machines unpair <machine>`, deletes the pin and sends `bye` with `unpaired` when a session is open. A peer that receives it deletes its pin too. A peer that is offline keeps its pin, but can do nothing with it: the next session fails the pin check on the other side.
- **New computer, or a machine wiped:** it signs in, gets a new machine id and key, and is paired again. The same name does not inherit trust: pins name machine ids, and the old id is revoked.
- **Lost key on a machine still signed in:** the cloud keeps the old public key for that machine id and refuses a new one, so the daemon reports "This machine's remote key is missing. Sign out and sign in again, then pair it again." A new sign-in gives a new id.
- **Removing a machine:** `localify machines remove <machine>`, or `Remove from account` in the app, calls `DELETE /v1/machines/{id}`. That signs the machine out as `logout` would (public-urls.md, "Signing out") and pushes `revoked` to the relay, which closes its socket and tells every connected peer, which deletes its pin. Its token is gone, so it can never connect again.
- **A pin whose machine left the roster** without a `revoked` push (the push failed, or the peer was offline) is shown as "Signed out" with an `Unpair` button. It cannot be used: that machine id has no token.

### The limits of end-to-end encryption

E2E keeps Localify Cloud, and anyone who controls the relay, from reading or changing inventory, output and controls, and from pairing a machine of their own. It does not protect against a compromised endpoint. A process running as the user on a paired machine can read `identity.json` and use the management socket, so it can see and do whatever that machine may. The relay still sees metadata: which machine talks to which, when, and how much. It can also refuse to route, which is a denial of service and nothing more.

## Relay

### Connecting

`GET /v1/remote/connect`, a WebSocket upgrade on `api.localify.dev`:

| Header | Value |
|---|---|
| `Authorization` | `Bearer <token>` |
| `Sec-WebSocket-Protocol` | `localify-remote.1` |
| `X-Localify-Version` | the daemon's Localify version |

The Worker:

1. Checks the token, the ban and the account's `remoteAccess` (see "Entitlement"), and that the machine has a remote key.
2. Applies `REMOTE_CONNECT_LIMITER`, keyed by machine id: 6 upgrades a minute.
3. Forwards the request to the account's `RemoteRelay`, `idFromName("user:" + userId)`, with the user id, machine id, version and the time it checked, in internal headers it sets itself after removing any the client sent.

The object accepts the socket with `ctx.acceptWebSocket(ws, [machineId])`, so it hibernates while the socket is idle. A second socket for the same machine replaces the first, which is closed with `4001`. It then sends `welcome` to the new socket and `presence` to the others.

| Answer | Meaning |
|---|---|
| `101` | connected |
| `401` | unknown or revoked token |
| `402 subscription_required` | the account has no `remoteAccess` |
| `403 banned` | the account is banned |
| `409 remote_key_missing` | upload a key first |
| `426 unsupported_protocol` | no subprotocol this Worker speaks |
| `429 rate_limited` | with `Retry-After` |

### State

The object stores nothing: no storage calls, no alarms, no timers, no history. Each socket's attachment (`serializeAttachment`, at most 2 KiB) holds the machine id, a connection id, the version and protocol range, the connection time, and `authorizedUntil`. After hibernation, the constructor rebuilds the map from machine id to socket from `ctx.getWebSockets()` and those attachments. Everything else, inventory and output included, lives on the endpoints and is asked for again.

`ctx.setWebSocketAutoResponse` answers the text frame `ping` with `pong` without waking the object. Clients send `ping` every 30 seconds and treat 75 seconds without `pong` or any frame as a lost connection.

On every frame other than `ping`, the object compares `authorizedUntil` with the clock. When it has passed, it reads the account and the machine from D1 (one query): still entitled, not banned, machine active. It then sets `authorizedUntil` 60 seconds ahead, or closes the socket with `4402`, `4403` or `4401`. That bounds unauthorized relaying to a minute without a timer.

A frame for a machine with no socket gets `error` with `peer_offline` back. The object never queues.

### Limits

| Limit | Value | Over it |
|---|---|---|
| Frame | 64 KiB | close `1009` |
| Application message, before encryption | 32 KiB | the sender splits it (pages, chunks); the receiver ends the session |
| Sockets | one per active machine of the account, so at most 10 | the older one is closed with `4001` |
| Frames from one socket | 50 a second on average over 10 seconds, bursts of 200 | `error` `rate_limited`, then close `4429` if it goes on |
| Bytes from one socket | 512 KiB a second on average over 10 seconds | same |
| Upgrades | 6 a minute per machine (`REMOTE_CONNECT_LIMITER`) | `429` with `Retry-After` |

The counters live in memory and start over after hibernation, which only makes them more permissive for a moment.

### Reconnects

The daemon keeps one connection while it is signed in, holds a key, and its account has `remoteAccess`. It has at most one connection attempt in flight.

- **Backoff:** full jitter, `sleep = random(0, min(300, 1 × 2^attempt))` seconds. A connection that stayed up 60 seconds resets the attempt count.
- **Server hints:** a `Retry-After`, or a close reason carrying `{"retryAfter": n}`, sets the least wait; jitter adds up to 20% on top.
- **Deploys:** a close with `1012` or `1001` waits `random(0, 10)` seconds first, so a deploy that drops every socket does not bring them all back in the same second.
- **Wake and network change:** one immediate attempt after a wake from sleep (the wall clock moved more than a minute past the monotonic one, as in public-urls.md, "Tunnel"), with `random(0, 2)` seconds of jitter, then the backoff.
- **Final closes:** `4401` signs the machine out locally, as a `401` does. `4403` stops and says the account is suspended. `4402` waits for `public_refresh` or a plan change, or 1 hour. `4002` waits for an update.
- **Bounded authentication:** each daemon has one connection attempt in flight; the Worker checks a token once per upgrade, at most 6 times a minute per machine; a target runs at most 4 Noise handshakes at once.
- **After a reconnect:** every session is new. The viewer asks for fresh snapshots and offsets, and never resends a control that had no answer (see "Controls").

### Cost

The relay costs Durable Object requests and duration. Duration dominates: an object is billed at 128 MB for as long as it is in memory, and a viewer following a busy service keeps it there. At the published Workers Paid prices ($12.50 per million GB-s past 400,000 included each month, and $0.15 per million requests past one million, where 20 incoming WebSocket messages count as one request), an object kept awake for an hour costs 0.125 GB × 3,600 s = 450 GB-s, about $0.0056. Someone watching output two hours a working day costs about $0.25 a month, against $7.99 for Pro. A hibernated object costs nothing, and auto-answered pings do not wake it.

What is not known is how long an object stays billable after its last event before it hibernates, and so what a chatty but slow output stream costs. PR 2 measures it on staging: two daemons, one viewer following a service that prints a line a second for an hour, then ten lines a second, read from the Durable Object analytics in Cloudflare's GraphQL API. The result goes in `apps/cloud/README.md` with an alert on the namespace's monthly duration. Machine tunnels (`m-<id>.localify.dev`) are not used: they exist only while a machine serves public URLs (ADR 0025) and each one past 1,000 costs $4.50 a month.

## Wire protocol

### Relay frames

Text frames of JSON, at most 64 KiB, plus the bare `ping` and `pong`. Field names are camelCase. The relay reads only the envelope.

From a daemon:

```ts
type Send = {
  t: "send";
  to: string;               // machine id
  sid: string;              // session id, 22 characters
  kind: "hs" | "msg" | "close";
  data?: string;            // base64url Noise message; absent for "close"
};
```

From the relay:

```ts
type Welcome = {
  t: "welcome"; v: 1; connId: string; machineId: string;
  peers: Presence[];        // the other machines connected now
};
type Presence = {
  t: "presence"; machineId: string; state: "online" | "offline";
  version: string; proto: { min: number; max: number }; since: string;  // RFC 3339
};
type Recv = { t: "recv"; from: string; sid: string; kind: "hs" | "msg" | "close"; data?: string };
type Revoked = { t: "revoked"; machineId: string };   // removed from the account: delete its pin
type RelayError = {
  t: "error"; code: "peer_offline" | "rate_limited" | "bad_frame";
  sid?: string; message: string; retryAfter?: number;
};
```

The relay sets `from` from the sender's attachment, never from the frame. `close` is unauthenticated, so it only ever ends a session early. Close codes:

| Code | Meaning | The daemon |
|---|---|---|
| `1001`, `1012` | going away, restart | reconnects after `random(0, 10)` s |
| `1009` | frame too large | reconnects with backoff, logs an error |
| `4000` | bad frame | same |
| `4001` | replaced by a newer connection of this machine | does not reconnect from this connection |
| `4002` | protocol version no longer accepted | waits for an update |
| `4401` | token revoked | signs out locally |
| `4402` | remote access lapsed | waits for a plan change, or 1 hour |
| `4403` | banned | stops |
| `4429` | rate limited | reconnects after `retryAfter` |

### Session messages

JSON objects inside Noise transport messages, at most 32 KiB before encryption. Every request carries an `id` the viewer picks (at most 32 characters, unique within the session), and every answer carries it back as `re`.

```ts
// Both ways, first.
type Hello = {
  t: "hello"; proto: { min: 1; max: 1 }; version: string;
  now: string;                         // RFC 3339, the sender's clock
  role: "viewer" | "target";
  // Target only.
  settings?: { share: boolean; control: boolean; backgroundStarts: boolean };
  app?: "running" | "absent";          // the desktop app, through its bridge
};

// Viewer → target.
type InventoryGet   = { t: "inventory.get"; id: string; have: Have | null };
type InventoryWatch = { t: "inventory.watch"; id: string; have: Have | null };
type InventoryUnwatch = { t: "inventory.unwatch"; id: string };
type Have = { epoch: string; rev: number };
type OutputRead   = { t: "output.read"; id: string; runId: string; offset: number; maxBytes: number };  // ≤ 16,384
type OutputFollow = { t: "output.follow"; id: string; runId: string; offset: number; window: number }; // ≤ 131,072
type OutputAck    = { t: "output.ack"; re: string; offset: number };
type OutputUnfollow = { t: "output.unfollow"; re: string };
type Keep = { t: "keep"; ids: string[] };        // renews watches and follows
type Control = {
  t: "control"; id: string;
  action: "start" | "stop" | "restart";
  service: { projectId: string; environment: string | null; serviceId: string };
  profile: string | null;              // start only: a profile name from localify.toml
  expect: { runId: string | null; configRev: string };
  expiresAt: string;                   // RFC 3339, in the target's clock
};

// Target → viewer.
type InventoryPage = {
  t: "inventory.page"; re: string; epoch: string; rev: number;
  page: number; pages: number;         // 1-based; pages ≤ 32
  machine: MachineSummary;             // on every page
  items: InventoryItem[];
  truncated: boolean;                  // more than 32 pages' worth: the rest is left out
};
type InventoryUnchanged = { t: "inventory.unchanged"; re: string; epoch: string; rev: number };
type OutputData = {
  t: "output.data"; re: string; runId: string;
  offset: number;                      // where these lines start
  next: number;                        // where to read on from
  lines: { text: string; cut?: true }[];  // cut: the line goes on in the next item
  rotated?: { firstOffset: number };   // the asked offset was dropped; data starts there
  ended: boolean;                      // the run ended and everything was sent
};
type ControlProgress = { t: "control.progress"; re: string; phase: "dispatched" | "accepted" | "registered" };
type ControlResult = {
  t: "control.result"; re: string;
  outcome: "done" | "refused" | "conflict" | "failed" | "unknown";
  code: string | null;                 // see "Errors" when not done
  message: string;
  runId: string | null;                // the run now in place, when known
};

// Both ways.
type SessionError = { t: "error"; re?: string; code: string; message: string; retryAfter?: number };
type Bye = { t: "bye"; reason: "done" | "unpaired" | "lifetime" | "idle" | "not_shared" | "lease_expired" };

// Pairing session only.
type PairCommit = { t: "pair.commit"; c: string };      // base64url
type PairNonce = { t: "pair.nonce"; n: string };
type PairReveal = { t: "pair.reveal"; n: string };
type PairConfirm = { t: "pair.confirm"; accept: boolean };
type PairDeclined = { t: "pair.declined"; reason: "nobody_to_confirm" | "busy" | "cancelled" };
```

`InventoryItem` and `MachineSummary` are in "Inventory". A message that does not parse, has an unknown `t`, or breaks a bound ends the session with `error` `bad_request` then `bye`.

### Errors

The `code` of a `SessionError`, or of a `ControlResult` that is not `done`:

| Code | Meaning | The viewer says |
|---|---|---|
| `not_paired`, `key_mismatch` | the peer check failed | pair again; for a mismatch, "Don't trust this machine until you pair it again." |
| `not_shared` | `share` is off on the target | "B doesn't share its projects. Turn on sharing in Localify on B." |
| `control_disabled` | `control` is off | "B doesn't allow remote controls." |
| `background_disabled` | the app is closed on the target and `backgroundStarts` is off | "Localify isn't open on B, and B doesn't allow background starts." |
| `not_owned` | the run is in a terminal Localify does not own, or an unknown process holds the port | "This runs in a terminal on B. Stop it there." |
| `owner_gone` | an app run whose app has quit is still alive: stop is allowed, restart is not | "Stop it, then start it again." |
| `refused_profile` | a dangerous, unknown or removed profile | "Start it on B to use this profile." |
| `refused_argument` | a command, path, env value or option a remote control never takes | — (the CLI and the MCP server refuse these before sending; only a modified client gets here) |
| `unknown_service` | the ids name nothing in the target's config | "This service no longer exists on B." |
| `conflict` | the run or the config changed since the viewer looked | "B changed in the meantime." with the fresh state |
| `expired` | received after `expiresAt`, or `expiresAt` more than 60 s ahead | "The request took too long to reach B. Nothing was done." |
| `busy` | the service's lock is held, or too many handshakes or pairings | "Something else is starting or stopping it on B." |
| `unauthorized` | the target's lease is missing, stale or refused | "Remote access is not available right now." |
| `output_unavailable` | no output stream for that run, with the reason | see "Output" |
| `gone` | the run id is not the service's last run | the viewer moves to the current run |
| `too_many` | over a watch, follow or session bound | — |
| `unsupported` | a message the peer's version does not know | "Update Localify on B." |
| `bad_request` | malformed, or out of bounds | — |

### Versioning

Two versions move separately. The relay protocol is the WebSocket subprotocol, `localify-remote.1`; a Worker that drops one closes its sockets with `4002`. The session protocol is `proto` in `hello` and in `presence`. Peers use the highest version both support; when their ranges do not overlap, the viewer shows "Update needed" before opening a session. A new optional field is added without a new version, and a receiver ignores fields it does not know, except in `control`, where an unknown field is refused so that no argument is ever silently dropped. A new message type, or a change in meaning, takes a new version, and a sender never sends a type the peer's version lacks.

## Inventory

A target's inventory is a replacement snapshot, built on the target and sent only while someone views. It is never a stream of events.

```ts
type MachineSummary = {
  machineId: string; name: string; version: string; os: "macos" | "linux";
  settings: { share: boolean; control: boolean; backgroundStarts: boolean };
  app: "running" | "absent";
};
type InventoryItem = {
  project: { id: string; name: string };
  environment: { slug: string | null; name: string; primary: boolean; branch: string | null };
  service: { id: string; name: string; configRev: string };
  localUrl: string;                 // resolves on the target only
  owner: "app" | "background" | "terminal" | "unknown" | null;  // null: nothing runs, port free
  run: {
    runId: string; startedAt: string; profile: string | null; profileDanger: boolean;
    exit: { at: string; code: number | null } | null;
  } | null;                         // the service's last recorded run
  process: "running" | "stopped";   // whether the run's process is alive
  port: { number: number; reachable: boolean };
  output: { available: true; runId: string }
        | { available: false; reason: "terminal" | "unknown_owner" | "no_run" | "app_output_unsupported" | "older_run_record" };
  publicUrl: { url: string; serving: boolean;
               reason: "suspended" | "taken_down" | "served_elsewhere" | "tunnel_down" | null } | null;
  controls: Record<"start" | "stop" | "restart", { allowed: true } | { allowed: false; code: string }>;
};
```

- **Sources:** the configured projects, environments and services (the config the daemon loads, polled every 2 seconds); the running-service records in `services/`; a TCP connect to each service's port on `127.0.0.1`, 200 ms timeout; the daemon's own runtime for public URLs and the tunnel (public-urls.md, "Tunnel").
- **Four states:** `process` comes from the record's PID and start time; `port.reachable` from the connect; `output.available` from the run's stream; `publicUrl.serving` from the URL's state, the machine that serves it, and this machine's tunnel being connected. They disagree often, and the UI shows each one: a process running with its port not reachable yet is "Starting"; a reachable port with no running record is `owner: "unknown"`.
- **`configRev`:** the first 16 hex digits of SHA-256 over the canonical JSON of the service's entry as the daemon loaded it: command, folder, port, domains and environment. It changes whenever the entry does.
- **`controls`:** what this viewer may do right now, with the code a refusal would carry, so the UI disables a button and says why before anyone clicks.
- **`share` off:** the target sends `machine` and no items.
- **Epoch and revision:** `epoch` is random, chosen at daemon start; `rev` starts at 1 and goes up whenever the snapshot's content changes. A request whose `have` matches gets `inventory.unchanged`.
- **Pages:** items sorted by project name, environment and service; split so each page fits 32 KiB; at most 32 pages. A viewer replaces its copy only once it holds every page of one revision; pages of an older revision are dropped.
- **When it is sent:** on `inventory.get`; while a watch is open, whenever the content changes, checked every 2 seconds for config and records and every 10 seconds for ports; right after every control's result; and on a 30-second reconcile while watched. Unchanged snapshots are never sent.
- **Watches:** at most one per session. A watch ends with `inventory.unwatch`, with the session, or 120 seconds after the last `keep`. The viewer sends `keep` every 60 seconds while the machine is on screen, and stops when the window is hidden. With no watch open, the target computes nothing and sends nothing.
- **Deltas,** if they come later, carry the epoch and the revision they apply to. A gap or another epoch makes the viewer ask for a snapshot.

## Output

### A per-run service stream

Output shown remotely is the service's own output for one run, never a terminal. Today an app-owned service tab copies every byte of its shell into the service's output file (`src-tauri/src/terminal.rs`), including the prompt and any command typed after the service exits. That file is never exported. PR 1 replaces it with a per-run stream that both owners write:

- **Run id:** `localify dev` picks 16 random bytes, base64url, when it registers a run, and records it as `runId` in the run's record.
- **Location:** `<state dir>/services/output/<runId>/`, made of 4 MiB segments named by their index (`0.log`, `1.log`, …), segment `k` holding logical bytes `[k × 4 MiB, (k + 1) × 4 MiB)`. The writer keeps the two newest, so between 4 and 8 MiB of the end of a run stay readable. Both writers derive the path from the run id; neither ever reads a path from the output.
- **Background runs:** `localify dev --detach` gives the service pipes for stdout and stderr, stdin closed, and copies them into the stream. That is what the service printed and nothing else.
- **App runs:** `localify dev`, when `LOCALIFY_SERVICE_ORIGIN=app`, writes a start marker to its terminal after it registers and an end marker before it exits: `ESC ] 7731 ; localify-run ; start ; <runId> BEL`, and the same with `end`. xterm.js ignores unknown OSC sequences, so nothing shows. The app's output thread recognizes the markers in the PTY's bytes, across read boundaries. It writes to the stream only between a start marker and the matching end, and only while the PTY's foreground process group (`tcgetpgrp` on the master) is led by the run's PID. It starts only for a run id whose record exists, belongs to this tab's service, has `origin: app` and is running. A marker printed by the service itself can therefore only end its own stream early. A run killed before its end marker loses the foreground, which closes the stream.
- **Terminal runs** have no stream: `output.reason` is `terminal`.
- **Until both sides support it,** an app run whose record has no `runId`, or whose app does not announce `runOutput` over the bridge, has no stream: `app_output_unsupported` or `older_run_record`. The old whole-shell file is never a fallback.
- **Locally,** `localify logs` and `service_logs` read the same stream, so an agent no longer reads shell prompts either, and the whole-shell copy and `LOCALIFY_SERVICE_OUTPUT` go away. A record without a run id, from an older version, keeps its `logFile` for `localify logs` only.
- **Retention:** as today, a run's stream is deleted when the next run of the same service registers, or when the service is forgotten.

What the stream contains for an app run is what the terminal displayed while the run held the foreground. Keystrokes echoed during the run are part of it, as they are in any terminal log. Input with echo turned off, such as a password prompt, is not.

### Reads

`output.read {runId, offset, maxBytes}`:

- **Bounded:** the target opens the segment holding `offset`, seeks, and reads at most `maxBytes` (16 KiB at most), never to the end of the file. It trims what it sends so the encoded message stays within 32 KiB, and `next` says where it stopped.
- **Lines:** bytes become plain lines as `plain_lines` makes them today (colors and cursor moves removed, a line redrawn with `\r` kept as its last state), and every remaining control character except tab is removed, C1 controls and the Unicode bidirectional overrides (`U+202A`–`U+202E`, `U+2066`–`U+2069`) included. Invalid UTF-8 becomes `U+FFFD`.
- **Partial lines:** a line still being written is held back, and `next` stops before it. A line longer than 8 KiB is sent in 8 KiB pieces, each but the last with `cut: true`. Once the run has ended, its last unfinished line is sent as it is, and `ended` is true.
- **Rotation:** an `offset` older than the oldest segment kept gets `rotated: {firstOffset}`, with data from there, and the viewer shows "Earlier output was dropped."
- **Another run:** a `runId` that is not the service's last run gets `gone`. The viewer reads the new run from offset 0 and shows "Restarted at 10:42".
- **Offsets** are logical bytes from the start of the run, never reused. An offset past what was written is an error, `bad_request`.

### Following

`output.follow {runId, offset, window}` sends `output.data` as lines arrive, the first one at once. The target never has more than `window` bytes sent and not acknowledged; the viewer sends `output.ack` with the `next` it has shown. The target looks for new bytes every 250 ms while a follow is open, locally. A follow ends with `output.unfollow`, at the end of the run once everything was sent, with the session, or 120 seconds after the last `ack` or `keep`. At most 2 follows per session, 4 per target. The viewer stops following when the Output panel closes or the window is hidden for 60 seconds, so an output nobody looks at costs nothing.

### Showing it

The panel is labelled "Output", never "Terminal". It shows plain lines with selectable text, a search box, and a Follow / Pause switch. Pausing stops scrolling while lines keep arriving. After 60 seconds paused, the viewer ends the follow, and follows again from where it stopped when the person resumes. It keeps at most 10,000 lines and drops the oldest. Rendering through xterm.js in read-only mode is allowed, but it gets the same plain lines, never raw bytes.

## Controls

### What a control may name

A control carries explicit ids: the target machine (the session), the project, the environment slug (`null` for the primary checkout), the service, and for a start an optional profile name. It carries nothing else. A remote control never takes a command, a folder, a path, an env file, an env value, a domain, a port, `confirm_profile`, or anything that would create a project or an environment. A message with an unknown field in `control` is refused with `refused_argument`. A profile must exist in the checkout's `localify.toml` and not be `danger = true`.

### Target checks

In order; the first that fails answers the control:

1. The session's peer is pinned, and `hello` has been exchanged.
2. `share` and `control` are on.
3. `expiresAt` has not passed in the target's clock and is at most 60 seconds ahead. The viewer learns the target's clock from its `hello`.
4. A fresh lease: the target calls `GET /v1/remote/lease` now, and the viewer's machine id is among its active machines (see "Entitlement"). A failure refuses with `unauthorized`: the target fails closed.
5. The ids name a configured service; the profile, when given, is allowed.
6. The per-service lock (below) is free within 2 seconds, or `busy`.
7. Under the lock, the state matches: for a start, no running run and a free port; for a stop or a restart, the running run is `expect.runId`. For a start or a restart, the service's `configRev` is `expect.configRev`; a stop ignores it. Otherwise `conflict`, with the fresh snapshot sent right after.
8. The owner allows it (below).

A control whose `id` was already answered in this session gets the same answer again without running anything. That is correlation, not exactly-once: what keeps a repeated restart from running twice is `expect.runId`, which no longer matches once the first restart replaced the run.

### The per-service lock

`services/<record name>.lock`, an exclusive `flock`. It serializes whoever decides to start or stop a service: the remote control executor in the daemon, the app's launch and stop actions (button, deep link, bridge), and the CLI's `dev --in-app`, `dev --detach`, `stop` and `restart` before they act. The decider holds it from its check until the new run registers, the old one stops, or 15 seconds pass. The `localify dev` process that registers a run never takes it; `register` refusing a second live run stays the final guard.

### Who acts

| Action | Owner now | App on the target | What the target does |
|---|---|---|---|
| start | none | running | bridge `launch`: the app opens the service's tab in its terminal panel and runs it there |
| start | none | absent, `backgroundStarts` on | a detached start, as `localify dev --detach` with the ids and the profile |
| start | none | absent, `backgroundStarts` off | refuses `background_disabled` |
| start | `app` or `background`, running | — | `conflict`: the viewer thought nothing ran |
| stop | `app` | running | bridge `stop`: interrupts the foreground command and keeps the shell (ADR 0012) |
| stop | `app` | absent | the process outlived its terminal: stops its process tree |
| stop | `background` | — | stops its process tree, as `localify stop` does |
| restart | `app` | running | bridge `restart`: stops it, then runs it again in the same tab on the run's profile |
| restart | `app` | absent | refuses `owner_gone` |
| restart | `background` | — | stops it, then starts it detached on the run's profile |
| restart | any | — | refuses `refused_profile` when the run's profile is now dangerous, unknown or removed |
| any | `terminal`, `unknown` | — | refuses `not_owned`; the status stays visible |

A detached start from the daemon runs `localify dev <service> --project <id> [--env <slug>] [--profile <name>] --detach` through the user's login shell (`$SHELL -l -c`), every argument quoted, so the service finds the same `PATH` and version managers it finds in a terminal. The daemon itself runs under launchd or systemd, with a minimal environment.

### The app bridge

The desktop app listens on `<data dir>/app-bridge.sock`, a Unix socket only its owner can open, while it runs. One JSON request per connection, one JSON answer, as on the management socket. A refused connection or a missing socket means the app is absent.

| Request | Answer |
|---|---|
| `{"cmd": "hello"}` | `{"ok": true, "data": {"version": "…", "bridge": 1, "runOutput": true}}` |
| `{"cmd": "launch", "requestId", "service": {projectId, environment, serviceId}, "profile", "requestedBy": {machineId, name}}` | `{"ok": true, "data": {"accepted": true, "sessionId": "terminal-7"}}` once the command is written into a PTY, within 5 s |
| `{"cmd": "stop", "requestId", "service", "runId", "requestedBy"}` | `accepted` once the interrupt is sent |
| `{"cmd": "restart", "requestId", "service", "runId", "requestedBy"}` | `accepted` once the stop is under way |

Errors are `{"ok": false, "error": {"code", "message"}}`, with the codes of "Errors". The app opens the tab without taking focus and shows "Started by A" (or stopped, restarted) for 5 seconds. `localify dev --in-app` uses the bridge too when the app answers, and keeps the deep link only to open an app that is not running.

### State machine

On the target, for one control:

```text
received ──▶ checked ──▶ locked ──▶ matched ──▶ dispatched ──▶ accepted ──▶ settling ──▶ done
               │           │          │           │                           │
               ▼           ▼          ▼           ▼                           ├──▶ failed: the run exited
            refused       busy     conflict   reconciling ──▶ done | unknown  └──▶ unknown: still starting after 90 s
           or expired                         (no ack in 5 s, bridge closed, app quit)
```

For a background start or a stop the target does itself, `dispatched` and `accepted` are one step: the process was spawned, or signalled.

- **Settling:** a start is `done` when a run with a new run id has registered and listens on its port, within 90 seconds. It is `failed` when the run exits first, with its last 40 lines in the message. After 90 seconds without either, it is `unknown` with "Still starting on B". A stop is `done` when the process tree is gone. A restart is both.
- **Reconciling:** when the bridge drops or does not acknowledge, the app may or may not have written the command. The target never falls back to a background start for a request it dispatched to the app. For 10 seconds it watches for a run of that service that registered after the dispatch, and for the port. It answers `done` if one appears and `unknown` otherwise: "The Localify app on B closed while starting web. Nothing else was started."
- **Progress:** `control.progress` is sent at `dispatched`, `accepted` and `registered`, so the viewer can say "Starting in the app on B…".

On the viewer:

```text
idle ─▶ sent ─▶ (progress) ─▶ result: done | refused | conflict | failed | unknown
          │
          └─ session lost, or no result in 120 s ─▶ unknown ─▶ fresh snapshot ─▶ idle
```

- **Lost acknowledgement:** the viewer shows "Outcome unknown. Checking B…", opens a new session when it can, and shows the snapshot the target sends. It never sends the control again by itself.
- **Queued controls:** the viewer sends a control only on an open session. A control the person asked for while the session was down is not queued; the button is disabled with the connection state.
- **One at a time:** the viewer allows one control per service at a time; the target's lock enforces it across viewers and local paths.

## Cloud

### API

Added to the API of public-urls.md, with the same authentication, scoping and error shape:

| Method and path | Body | Response |
|---|---|---|
| `GET /v1/machines` | — | `{machines: [RosterMachine]}`, the account's active machines, this one included |
| `PUT /v1/machines/self/remote-key` | `{publicKey}` | `204`, also when the machine already has this same key; `409 remote_key_set` when it has another one; `409 remote_key_taken` when another machine has this one; `422 invalid_key` |
| `DELETE /v1/machines/{id}` | — | `204`. Signs that machine out as `POST /v1/auth/logout` does for the caller, and pushes `revoked` to the relay. Works at zero entitlement. On the caller's own id, it is a sign-out. |
| `GET /v1/remote/connect` | WebSocket upgrade | see "Relay", "Connecting" |
| `GET /v1/remote/lease` | — | `{until, machines: [id]}`: `until` is 60 seconds from now, `machines` the account's active machine ids. `402 subscription_required`; `403 banned`. |

```ts
type RosterMachine = {
  id: string; name: string; createdAt: string;
  remoteKey: string | null;       // base64url, null until uploaded
  self: boolean;
};
```

`GET /v1/me` reports `plan.limits.remoteAccess` (see "Entitlement"). Whether a machine is online is never in the API: only the relay knows, through `presence`.

### Data (D1)

Migration `0005_remote_access.sql`, after PR #56's `0004_serving_state.sql`:

```sql
-- Remote machines (docs/remote-machines.md, "Trust" and "Entitlement").
-- A machine's remote key is set once and never changes: a machine that lost
-- its key signs in again and gets a new id.
ALTER TABLE machines ADD COLUMN remote_key TEXT;
ALTER TABLE machines ADD COLUMN remote_key_at INTEGER;
CREATE UNIQUE INDEX one_machine_per_remote_key ON machines (remote_key) WHERE remote_key IS NOT NULL;

-- An override replaces the plan entirely, remote access included. Every
-- override that exists was granted by hand and keeps remote access.
ALTER TABLE plan_overrides ADD COLUMN remote_access INTEGER NOT NULL DEFAULT 0
  CHECK (remote_access IN (0, 1));
UPDATE plan_overrides SET remote_access = 1;
```

No table holds sessions, presence, pairings, inventory or output. A revoked machine keeps its `remote_key`, which only the unique index still reads.

### The relay object

`wrangler.jsonc` gains a Durable Object binding `RELAY` for the class `RemoteRelay`, exported from `src/relay.ts`, with a `new_sqlite_classes` migration. It never touches its storage. Its RPC methods, called by the Worker only:

- `connect(request)`: the upgrade, after the Worker's checks;
- `revokeMachine(machineId)`: closes that machine's socket with `4401` and sends `revoked` to the others;
- `revokeAccount(code)`: closes every socket with `4402` or `4403`.

New configuration: the rate-limit binding `REMOTE_CONNECT_LIMITER` (6 per 60 seconds). Nothing else: the relay needs no secret, since it authenticates nothing itself.

## Entitlement

`remoteAccess` is its own entitlement, a boolean next to `servingMachines` and `publicUrls` in PR #56's `Entitlement`. It is never inferred from tunnel quotas: an account can have remote access and no serving slot.

- **Pro:** an entitled Pro plan, per the lifecycle table, gives `remoteAccess: true`. Add-ons change nothing. There is no free tier and no trial apart from Pro's.
- **Override:** an override replaces the plan entirely, so it carries `plan_overrides.remote_access`. `cloud:admin override` sets it to 1 unless given `--no-remote-access`, and `show` prints it.
- **Ban:** none. **No plan:** none.
- **`/v1/me`:** `plan.limits` becomes `{servingMachines, publicUrls, remoteAccess}`. `plan.entitled` keeps its meaning (serving), so the public URL paywall does not move. PR 2 updates public-urls.md's `Plan` and "Plans" with it.
- **Linked machines:** at most 10 per account, which the 10 signed-in machines already bound.

**Losing it.** When the account loses `remoteAccess`, through a lapse, a ban or an override that ends, or a machine is removed or signs out:

1. New operations are refused: the connect route answers `402` or `403`, and each lease request too.
2. The Worker pushes at once where it makes the change: a billing refresh that leaves the account without `remoteAccess` calls `revokeAccount`, which does nothing when no socket is open; sign-out, roster removal and a same-name sign-in that replaces a machine call `revokeMachine`. Open sessions and output follows end with their sockets.
3. Where the Worker does not see the change (an override expiring, the end of a canceled period or of the `past_due` grace, a `cloud:admin` command, which writes D1 only), the lease is the fallback. The relay re-reads the account on the first frame after `authorizedUntil`. Each endpoint holds a lease while it has a session open, renewed every 45 seconds with up to 5 seconds of jitter. A refused renewal, a lease that has passed, or a peer missing from the lease's `machines` ends the session with `bye` `lease_expired`. Either way, open streams stop within about a minute.
4. Before each control, the target asks for a fresh lease (see "Target checks").

Nothing local stops. Services keep running, the target's settings and pins are kept, and the app shows "Remote access needs Localify Cloud Pro" with the plan's action. ADR 0027 still applies separately to public traffic. Roster removal, sign-out, `/v1/me` and billing keep working without the entitlement.

## Daemon

The daemon is the only relay endpoint on a machine, for both roles: one key, one connection, one lease. The app, the CLI and the MCP server view other machines through it.

New management socket commands. The streaming ones keep the connection open and send one JSON line per event until the client closes it, which ends the watch or follow:

| Command | Answer |
|---|---|
| `remote_status` | `{relay: "off" \| "connecting" \| "connected" \| "reconnecting" \| "blocked", blocked: {code, message} \| null, retryAt, settings, fingerprint}` |
| `remote_machines` | the roster merged with pins and presence: `[{id, name, self, paired, state, version, fingerprint}]`, `state` as in "Desktop app" |
| `remote_settings {share?, control?, backgroundStarts?}` | the settings after the change |
| `remote_pair {machineId}` | streaming: `code`, then `paired` or `cancelled` |
| `remote_pair_answer {pairingId, accept}` | `{ok}` |
| `remote_unpair {machineId}` | `{ok}` |
| `remote_inventory {machineId}` | one snapshot, all pages |
| `remote_watch {machineId}` | streaming: snapshots, connection states |
| `remote_output_read {machineId, runId, offset, maxBytes}` | one `output.data` |
| `remote_output_follow {machineId, runId, offset}` | streaming `output.data`; the daemon acknowledges as the client reads |
| `remote_control {machineId, action, service, profile, expect}` | streaming: progress, then the result; the client waits up to 120 s, not the usual 2 |
| `remote_events` | streaming: pairing requests, presence, revocations; the app subscribes while it runs |

The daemon opens a session only when one of these asks for it, and closes it when the last one ends. `public_refresh` also re-reads `/v1/me` for `remoteAccess`, so subscribing turns remote access on without a restart.

## Desktop app

- **Machine switcher,** at the top of the sidebar: "This Mac" first, then every machine of the roster with its name and a state, using T3 Code's distinct states plus Localify's own:

  | State | Shows |
  |---|---|
  | `connecting` | "Connecting…": this machine's relay connection or the session handshake is in progress |
  | `connected` | the name, with no badge |
  | `reconnecting` | "Reconnecting… (in 8 s)" with a `Retry now` button |
  | `offline` | "Offline", and "since 10:42" when the relay saw it leave |
  | `incompatible` | "Update needed: B runs a version of Localify that can't talk to this one." with the version that needs updating |
  | `not_paired` | "Not paired" with `Pair` |
  | `key_mismatch` | "Key mismatch: don't trust this machine until you pair it again." with `Unpair` |
  | `not_sharing` | "Paired · not sharing" |
  | `signed_out` | "Signed out" with `Unpair` |
  | `unavailable` | "Remote access needs Localify Cloud Pro", with the plan card's action |

- **A remote machine's view** is the Projects screen of that machine, read-only: no editor, no domain or profile editing, no terminal. A banner says "You're viewing B. Its .localify URLs open only on B." Each service row shows its owner ("In the app", "In the background", "In a terminal", "Unknown process"), and four chips: Process, Port, Output, Public URL. The Start, Stop and Restart buttons follow `controls`; a disabled one says why in its tooltip. A control in flight shows its progress, and `unknown` shows "Outcome unknown. Checking B…".
- **URLs:** a remote `.localify` URL is shown as "web.louez.localify · on B", with a copy button and never an open button. When the service has a public URL that serves, `Open public URL` opens it.
- **Output** opens in the bottom panel as a tab "Output · web · B", with search, Follow / Pause and selectable text (see "Output").
- **Settings → Remote access,** on every machine: the three switches with one line each ("Share projects and output with paired machines", "Allow paired machines to start, stop and restart services", "Allow starts when the app is closed"), a note that output can contain secrets the services print, this machine's fingerprint, and the paired machines with `Unpair` and `Remove from account`.
- **Pairing dialogs,** with the wording of "Pairing".

All of it goes through `DesktopApi` and the management socket. The UI does no network.

## CLI

```bash
localify machines                          # the roster: name, id, state, paired, sharing, version
localify machines pair <machine>           # pairs with another machine: a code to confirm on both
localify machines pair --accept            # waits on this machine for a pairing request
localify machines unpair <machine>
localify machines remove <machine> [--yes] # signs it out of the account
localify machines settings [--share on|off] [--control on|off] [--background-starts on|off]
localify services status --machine <machine>
localify logs <service> --machine <machine> [--project <id>] [--env <slug>] [--lines <n>] [--follow] [--cursor <runId>:<offset>]
localify dev <service> --machine <machine> [--project <id>] [--env <slug>] [--profile <name>]
localify stop <service> --machine <machine> [--project <id>] [--env <slug>]
localify restart <service> --machine <machine> [--project <id>] [--env <slug>]
```

- **Resolving `--machine`:** an exact machine id; otherwise an exact name, ignoring case, among the active machines of the roster. Active names are unique, since a same-name sign-in replaces the older machine; a name that still matches two, differing only in case, is an error that lists both with their ids. A name of no active machine lists the roster. This machine's own id or name acts locally, as without `--machine`.
- **Resolving a service:** there is no folder to read on another machine, so a service name must be unique on the target, or come with `--project`, and `--env` names a non-primary checkout. An ambiguous name lists the matches.
- **What `--machine` refuses:** with `dev`, every option but `--project`, `--env` and `--profile`: no command after `--`, no `--detach`, `--in-app`, `--env-file`, `--confirm-profile` or `--domain`. The target decides where the service runs.
- **Output:** human on a terminal, JSON otherwise, as everywhere. A control prints its progress, then its result; `unknown` prints the fresh status and exits with code 3. A refusal prints the message of "Errors" and exits with code 2.
- `pair` and `remove` need a terminal: an agent cannot pair or remove machines.

## MCP

- `list_machines`: the roster with states, read-only.
- An optional `machine` argument on `list_services`, `service_status`, `service_logs`, `service_start`, `service_stop` and `service_restart`, and on no other tool. With it, a tool ignores `cwd`.
- With `machine`, `service_start` takes only `service`, `project`, `env` and `profile`. `background`, `env_files`, `confirm_profile`, `command` and `domain` make it fail with an error that says the target decides how its services start. It never creates a project.
- `service_logs` with `machine` returns a cursor `<runId>:<offset>`.
- Pairing, unpairing, settings and removal have no tool. Like signing in (ADR 0018), they stay a person's decision.
- An error from a target reaches the agent with its code and message, so `control_disabled` tells it to ask the user to allow controls on that machine, and `unknown` tells it to check `service_status` before trying again.

## Failure modes

| Situation | What happens |
|---|---|
| The relay restarts (deploy) | sockets close with `1012`; daemons come back over 10 seconds; sessions are new; a control without an answer is `unknown` on the viewer |
| Cloud unreachable | the relay connection backs off; sessions end once the lease passes; controls are refused at once (`unauthorized`); local work goes on |
| The target sleeps or loses the network | its socket stops answering; the relay closes it after the platform's timeout, the viewer after 75 s without a frame; the switcher shows "Offline" |
| Versions do not overlap | "Update needed" before any session |
| The app quits while dispatching a start | `unknown` after a 10-second reconcile; never a background start in its place |
| The app quits while its service runs | the run usually dies with the shell; if it lives, stop works and restart is refused `owner_gone` |
| The viewer drops during a control | the target finishes it; the viewer shows `unknown`, then the next snapshot |
| The same request arrives twice | the recorded answer is sent again; a restart repeated under a new id fails `expect.runId` |
| Someone starts the service locally meanwhile | `conflict`, or `busy` while the other launch holds the lock |
| The config changes during a control | `conflict` on `configRev`; the snapshot follows |
| A background start fails to find a tool | `failed` with the last lines; the person fixes the login shell |
| Output rotates while following | `rotated` with the new first offset |
| The service restarts while following | `gone`; the viewer follows the new run from 0 |
| A huge config | inventory pages, `truncated` past 32 |
| A peer floods | `rate_limited`, then `4429`; the target's own bounds on watches, follows and handshakes |
| The plan lapses | `4402`; "Remote access needs Localify Cloud Pro"; nothing local stops |
| A machine is removed from the account | `4401` on it, `revoked` to the others, which delete its pin |
| A same-name new sign-in | a new id, not paired; the old one is revoked and its pin dropped |
| The identity file is lost | "remote key is missing", until a new sign-in |
| The roster shows a key other than the pin | `key_mismatch`; no session |
| Clocks differ | controls use the target's clock from `hello`; nothing else depends on clocks |
| Every daemon reconnects at once | jitter spreads them; `REMOTE_CONNECT_LIMITER` and `Retry-After` bound the Worker's load |

## Security considerations

- **The cloud is not trusted with content.** Inventory, output and controls are encrypted between pinned keys. The cloud decides who may connect and can deny service, but cannot read, forge or replay a message, and cannot pair a machine of its own without the code.
- **A stolen token** lets its holder connect as that machine and see presence and sizes, but not open a session: it lacks the private key. Removing the machine ends the token.
- **A paired machine is trusted with what its peer shares.** Each target decides with its own switches, off by default, and checks them itself on every request. The cloud's view of permissions is never consulted for that.
- **The management socket and the app bridge** are Unix sockets only their owner can open, so a local process can use them only as the user, who could already read the key.
- **Output can hold secrets** that a service prints. Settings says so next to `share`. Output is never written to the daemon log, analytics or error reports, on either machine.
- **Untrusted text.** Output and names come from another machine. The viewer shows plain text with control characters and bidirectional overrides removed, never HTML or raw escape sequences.
- **No remote code.** A control names ids. It cannot carry a command, a path, a variable or a new project, and a dangerous profile is refused. The worst a paired machine with `control` can do is start, stop and restart configured services on permitted profiles.
- **Replay and delay.** Noise nonces stop replays within a session, fresh ephemeral keys across sessions, `expiresAt` a delayed control, and `expect.runId` a repeated one.
- **The pairing code** resists a relay in the middle through the commitment, at 10⁻⁶ per attempt, with attempts bounded by the target and by a person's click.
- **Metadata** stays visible to the relay: who talks to whom, when, and how much. The relay logs errors only, without payloads.
- **Denial of service** by a peer is bounded by the relay's limits and the target's own bounds.

## PR plan

1. **Local foundations.** Run ids in run records; the per-run output stream with segments, written by background runs through pipes and by the app between run markers; `localify logs` and `service_logs` on it; removing the whole-shell copy and `LOCALIFY_SERVICE_OUTPUT`; bounded reads by `{runId, offset}`; the inventory projection with its four states; the per-service lock in every launch path; the app bridge, used by `dev --in-app`; ownership-aware start, stop and restart as a local use case that the controls will call.
2. **Trust and relay.** Keys, pins and settings files; Noise sessions and pairing with `snow`; the `RemoteRelay` object and routes; migration 0005; `remoteAccess` in the entitlement, `/v1/me` and `cloud:admin`; leases and revocation pushes; reconnects; the management socket commands for status, roster and pairing; `localify machines`, `pair`, `unpair`, `remove` and `settings`; the cost measurement.
3. **Desktop viewing,** the first useful release: the switcher, the pairing dialogs, Settings → Remote access, the remote Projects view, Output.
4. **Remote controls:** the control executor on the target, its state machine and reconcile; the buttons and their states in the app.
5. **CLI and MCP:** `--machine` on status, logs, dev, stop and restart; `list_machines` and the `machine` argument.

### Acceptance tests

Controls do not ship (PR 4) until each of these passes in CI, against a real daemon and a relay running in `wrangler dev` or its test harness:

- **Shell input is never exported.** An app tab runs a service that prints known lines and exits; the test then types `echo secret-after-exit` into the same shell. The stream holds the service's lines and not `secret-after-exit`. Variants: the service killed with `SIGKILL` before its end marker; a service that prints a fake end marker, then a fake start marker for another run id.
- **A desktop disconnect cannot double-start.** A start is dispatched to an app that is killed before acknowledging, and once after acknowledging. In both, at most one run exists afterwards, no background run was started, and the viewer got `done` or `unknown`.
- **A reconnect cannot repeat a restart.** The viewer's socket is cut right after it sends a restart, and the test sends the same control again on a new session, with the same and with a new request id. The service restarted once, and both second attempts get `conflict`: a request id is only remembered within its session.
- **Revocation cuts an open output stream.** With a follow open: `revokeAccount`, `revokeMachine`, and a D1 change with no push (an expired override) each end it, the first two within 2 seconds, the last within 75 seconds.
- **External starts and config changes converge.** While a viewer watches, the service is started from a terminal, stopped with `localify stop`, and its config edited. Within 12 seconds, the viewer's snapshot shows each state, and a control built on the old one gets `conflict`.
- **A reconnect storm stays bounded.** 200 simulated daemons across 20 accounts are disconnected at once with `1012`. Upgrades reach the Worker spread over at least 10 seconds, no machine exceeds the rate limit, every daemon is connected again within 60 seconds, and no control is replayed.

## Open questions

- **Bans reaching open sessions.** `cloud:admin` writes D1 only, so a ban reaches open sessions through the lease, within about a minute, not at once. An authenticated admin route that calls `revokeAccount` would make it immediate, against the choice of having no admin route on the Worker.
- **Login shell flags.** `$SHELL -l -c` misses what `.zshrc` sets, such as `nvm` set up for interactive shells only, and `-i` can print or prompt. Which one background starts use is decided with measurements on common setups in PR 4.
- **The billable time after a wake,** and so the cost of a slow output stream; measured in PR 2.
- **Sharing by project** rather than by machine: v1 shares a machine's whole inventory or nothing.
- **Echoed input.** Keystrokes typed into an app tab while its service runs are in the stream. Nothing reliable tells them apart from output.
- **A browser or phone viewer** would need its own keys and pairing, which v1 does not cover.
