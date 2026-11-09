# Remote machines

The contract for remote machines: what the cloud, the relay, the daemon, the desktop app, the CLI and the MCP server each do, and the messages between them. The decisions behind it are ADRs 0028 to 0033 and 0035; ADRs 0030 and 0031 amend ADR 0012. It builds on [public-urls.md](public-urls.md) for accounts, machines, plans and the Worker, and on the entitlement model of PR #56.

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

Not covered here: typing into a service from another machine, attaching to a terminal, resizing one; opening a target's `.localify` URL from the viewer; editing a target's projects, domains, profiles or files; running arbitrary commands; a web or phone viewer; machines of another account or a team; serving a target's dev server to the viewer outside public URLs; message history.

## Names

- **Machine:** one signed-in installation of Localify, known by its machine id (see public-urls.md, "Names"). The id is the identity. The name is a label: it can repeat over time, since a same-name sign-in replaces the previous machine with a new id.
- **Roster:** the account's active machines, from Localify Cloud: id, name, and remote key. Machine names may appear there; nothing else about a machine does.
- **Remote key:** a machine's X25519 static public key, 32 bytes, base64url without padding. Its private half never leaves the machine.
- **Fingerprint:** the first 8 bytes of SHA-256 of the remote key, as four groups of four hex digits (`1a2b 3c4d 5e6f 7a8b`).
- **Pairing:** a ceremony that makes two machines pin each other's remote key, after a person confirms the same 6-digit code on both.
- **Peer:** a machine this one has paired with, by machine id and pinned key.
- **Viewer:** the machine a person looks from. **Target:** the machine looked at. Any paired machine can be either.
- **Session:** a Noise channel between a viewer and a target, through the relay.
- **Run:** one execution of a service by `localify dev`, known by its run id. The `localify dev` process that executes it is the run's **executor**, and the leader of the run's session and process group. A run ends when no process of that session is left and its stream is finalized, complete or interrupted, not when the executor exits.
- **Legacy run:** a service started from a shell tab by an app from before PR 1, whose record has no run id. It keeps running as it is.
- **Launch:** a request to start a run, from a **dispatcher** (the app, the daemon, `dev --detach`). The dispatcher picks a launch id, which becomes the run id, and an expiry after which the launch can no longer start. What happened to it is decided once, in its launch record.
- **Run PTY:** the pseudo-terminal the desktop app creates for one run, with `localify dev` as its only child. A service tab shows a run PTY; a shell tab shows an ordinary shell.
- **Owner:** who holds a run: a run PTY of the desktop app (`app`), a detached executor (`background`), a terminal Localify does not own (`terminal`), or a process listening on the port with no run record (`unknown`).
- **Linked machine:** an active machine that has uploaded a remote key. An account has at most 10, whatever its serving capacity: machine add-ons raise the signed-in machines (docs/public-urls.md, "Plans"), never this.

## Target settings

Three switches on each machine, set only on that machine, in the app's Settings → Remote access or with `localify machines settings`. All three start off. No message from another machine, and nothing from the cloud, can change them.

| Setting | Off | On |
|---|---|---|
| `share` | Paired machines see this machine's name, version and connection state only. | Paired machines see projects, environments, services, run states, public URLs and output. |
| `control` | No start, stop or restart from another machine. | Paired machines may start, stop and restart services this machine owns (see "Controls"). Needs `share`. |
| `backgroundStarts` | A remote start needs the desktop app running here, and a background run cannot be restarted remotely. | A remote start runs detached in the background when the app is closed, and a background run can be restarted in the background. Needs `control`. |

Turning `share` off also turns the others off. Turning a switch off ends what depends on it at once: watches and follows end with `not_shared`, and controls already dispatched finish, but no new one starts. A control or a `control.status` still going is answered `error` `not_shared` at once, and whatever it comes to is never sent, a result that completes before the daemon acted on the change included; while `share` is off, `control.status` answers `not_shared` without reading anything. The daemon confirms the change only once it has done so, with every waiting answer dropped, and once its writer is past every frame queued before: a viewer's session with messages still in the send queue has them discarded and ends with a relay `close`, since the viewer could not read anything sealed after them. Nothing a service prints afterwards reaches a paired machine.

### Asking for access

A viewer on session protocol 2 or later can ask a target to turn on `control`, or `control` and `backgroundStarts`, with `access.request`. The request changes nothing by itself: a person on the target decides, in the app (ADR 0033).

1. **Requires `share`.** An `access.request` to a target whose `share` is off is answered `not_shared`. Access never turns `share` on.
2. **Limits, in this order:** `granted` at once when the switches asked for are already on; `too_many` with `retryAfter` when a request from this pinned peer showed a dialog less than 10 minutes ago (a request answered at once, `nobody` included, does not count, so the peer can ask again as soon as someone opens the app), or less than an hour ago and was refused, or when the target has shown 5 dialogs in the last hour, counted on the daemon's monotonic clock across sessions while it runs; `busy` when another request is waiting on this target. A person who ticks "Don't ask again from A" in the dialog suppresses A's requests until they turn it back on in Settings → Remote access; those get `declined` at once.
3. **The daemon owns the request.** It records `{requestId, peer, wants, expiresAt, generation}`, where `expiresAt` is 2 minutes away and `generation` is the settings' generation, a counter every write of `remote/settings.json` raises. It sends `access_request` on `remote_events` to the app. When no app is subscribed, it answers `nobody` at once.
4. **The dialog.** The app shows it with a notification, without taking focus. It names the asker from the local pin, with its fingerprint's first 8 characters, never from a name the peer sent: "teo-laptop (3F2A 91C0) asks for remote controls." The question states the real scope: "Allow every paired machine to start, stop and restart services on this Mac, until you turn it off in Settings?" For `backgroundStarts`, it adds "…also when Localify is closed here, in the background." The buttons are `Allow` and `Don't allow`, with "Don't ask again from teo-laptop".
5. **The answer** goes back with `remote_access_answer {requestId, allow, suppress}`. The daemon applies an `Allow` atomically, under the settings lock: only if the request is still waiting, is not past `expiresAt`, and the settings' generation is still the recorded one. Any other write in between (a switch turned off, a pairing removed) cancels it. It then turns on exactly the switches asked for, raises the generation, and answers `granted`. `Don't allow` answers `declined`. An answer to a request that is no longer waiting changes nothing.
6. **Ending.** The request ends `expired` after 2 minutes, `cancelled` when the viewer sends `access.cancel` or its session ends, and `cancelled` when a settings write changes the generation. Each end sends `access_closed {requestId}` to the app, which closes the dialog, and an `access.result` with that outcome when the session is still open. The viewer that lost its session stops waiting and says "The connection to B ended before an answer."

Granting raises the inventory's revision, so the viewer's next snapshot carries the controls it now allows. `localify machines settings` and Settings → Remote access are unchanged. A person who lets an agent run commands on the target, for example over SSH, lets it run `localify machines settings --control on` too.

## Trust

### Keys

On first need, which is the first relay connection, the daemon generates an X25519 key pair and writes it to `<data dir>/remote/identity.json` with `write_private_file` (0600), together with the machine id from `cloud.json`. It uploads the public key with `PUT /v1/machines/self/remote-key`. A machine's key is set once: the cloud refuses a second key for the same machine id. An identity file whose machine id is not the one in `cloud.json` belongs to an earlier sign-in and is deleted. The daemon runs outside the login session, so the Keychain is not used, for the same reason as `cloud.json` (ADR 0020).

Pins live in `<data dir>/remote/peers.json` (0600), written through a temporary file and a lock, as saved passwords are (public-urls.md, "Saved passwords"):

```json
{ "<machine id>": { "publicKey": "…", "name": "teo-mini", "pairedAt": "2026-11-08T10:12:00Z" } }
```

The target settings live in `<data dir>/remote/settings.json` (0600): `{"share": false, "control": false, "backgroundStarts": false, "generation": 0}`, where `generation` rises with every write (see "Asking for access"). The peers whose access requests are suppressed live in `<data dir>/remote/access.json` (0600): `{"<machine id>": {"name": "teo-laptop", "since": "…"}}`. Signing out deletes the whole `remote/` directory, so a new sign-in starts with a new key, no pins and every switch off.

The roster's keys are never the root of trust. They tell a viewer which machines have a key and show a fingerprint, but a session only proceeds with a key pinned through pairing. The cloud can remove trust (by revoking a machine, which ends its token) but cannot add any.

### Pairing

Pairing runs over the relay as a Noise session with purpose `pair` (see "Sessions"), followed by a commit and reveal that makes the short code safe against a relay in the middle. Without the commitment, a relay running two handshakes could try ephemeral keys until both codes match. With it, the responder is bound to its nonce before it sees the initiator's, both nonces and the final handshake hash go into the code, and each attempt succeeds with probability 10⁻⁶ and counts against the target's limit of 5 attempts an hour.

1. On the viewer A, the switcher lists B as "Not paired" with a `Pair` button. `localify machines pair <B>` does the same.
2. A sends a pairing request: the three Noise XX handshake messages, the first one's envelope carrying purpose `pair`. Both now hold the other's static key and the final handshake hash `h`.
3. B's daemon checks that someone can answer: the desktop app is subscribed to its events, or `localify machines pair --accept` is waiting (with or without `--for-ssh`). Otherwise it answers `pair.declined` with `nobody_to_confirm`, and A shows: "Nobody can confirm on B. Open Localify on B, or run `localify machines pair --accept` there, then try again."
4. B picks a random 32-byte `nB` and sends `pair.commit` with `c = BLAKE2s("localify-pair-commit" ‖ h ‖ nB)`.
5. A picks a random 32-byte `nA` and sends `pair.nonce` with it.
6. B sends `pair.reveal` with `nB`. A checks it against `c`, and abandons the pairing on a mismatch.
7. Both compute the code: the first 4 bytes of `BLAKE2s("localify-pair-code" ‖ h ‖ nA ‖ nB)` as a big-endian integer, modulo 1,000,000, shown as two groups of three digits (`482 913`).
8. Both show it.
   - A: title "Pair with B", body "Check that B shows this code: **482 913**", buttons `Codes match` and `Cancel`.
   - B: title "A wants to pair with this Mac" (or "this computer" on Linux), body "Pair only if A shows the same code: **482 913**. A will then see this Mac's name. It sees projects and output, or controls services, only after you turn that on in Settings → Remote access.", buttons `Codes match, pair` and `Cancel`.
   - CLI: `A wants to pair. Code: 482 913. Type yes if A shows the same code:`.
   - Over SSH (ADR 0035), see "Pairing over SSH" below.
9. Each side sends `pair.confirm` with `accept` once its person answers. A side writes its pin only when its own person accepted and the peer's `accept: true` arrived. Before either side has sent `pair.confirm`, a cancel, a mismatch, a message out of order or repeated, a closed session or 2 minutes without both answers pins nothing, and both show "Pairing cancelled. Nothing was saved." After that, see "Partial results" below.
10. After pairing, B shows "Paired with A" and a link to Settings → Remote access, with the three switches still off. A shows B as "Paired · not sharing" until B turns `share` on.

**Partial results.** Before either side sends `pair.confirm`, nothing writes a new pin. After the first `pair.confirm`, a lost session can leave one side pinned and not the other. Each side then reports its own state as it verified it on disk, `pinned` or `not_pinned`, and the peer's as `pinned` (its `pair.saved` arrived), `not_pinned` (it cancelled before confirming), or `unknown`. In a session of protocol 3 or later, a side sends `pair.saved` only once its pin is durably written, and then waits up to 30 seconds for the peer's before it ends with `bye` `done`; nothing else, `bye` `done` included, says the peer saved. A peer of protocol 2 (1.7.0 and earlier) has no `pair.saved`, and ends with `bye` `done` whether it saved, failed to save, or timed out, so a side paired with it ends as soon as it wrote its own pin and reports the peer `unknown`: the CLI exits 4 and says to check on that machine or update it, since pairing again would end the same way. Nothing was written when this side wrote no pin, and either never sent `accept: true` or got the peer's `accept: false`: the peer writes its pin only once this side's `accept: true` arrived, and only if it accepted too. The CLI exits 4 unless both sides are known to be pinned (0) or nothing was written (2). Pairing again with the same machine ids and keys completes it. A different key for an existing pin never replaces it silently: that is a key mismatch, and the pairing stops right after the handshake, before any code, with `key_mismatch`. The pin write checks again under the pins' lock, so when another pairing pinned another key for the same machine in the meantime, this one writes nothing and ends `partial` with `key_mismatch`.

### Pairing over SSH

`localify machines pair <B> --over-ssh <destination> [--remote-localify <path>]` lets someone who can run commands on both machines, such as an agent a person gave SSH access, pair them without a person at either screen (ADR 0035). Either machine can be the initiator, macOS or Linux, with or without the desktop app. Pairing changes no setting on either side.

- **SSH:** A runs `ssh` directly, with no local shell, as `ssh -T -o BatchMode=yes -o StrictHostKeyChecking=yes -o StdinNull=no -o ForkAfterAuthentication=no -o ForwardAgent=no -o ForwardX11=no -o ClearAllForwardings=yes -o ControlMaster=no -o ControlPath=none -o ConnectTimeout=15 -- <destination> <command>`. The host key must already be trusted (`known_hosts` or a host CA); an unknown or changed key fails before anything else. `<destination>` must be one operand that does not start with `-`. The remote command is the executable path and fixed arguments, each quoted for a POSIX shell. `--remote-localify` is one absolute path, refused if it holds a newline or NUL. `StdinNull` and `ForkAfterAuthentication` need OpenSSH 8.7 or later, so A runs `ssh -V` first and, for any other ssh or an older version, refuses with exit 2 and a message that names the version needed, before `ssh` connects. Without `--remote-localify`, the command is `localify` as the remote non-interactive environment finds it, and a missing one fails with a message that names `--remote-localify`. Nothing is installed or updated on B, and nothing switches users or uses `sudo`.
- **The accepting side:** `localify machines pair --accept --for-ssh --from <A's machine id> --account <user id>` is a machine-readable interface for an authorized local caller. Refusing a terminal on stdin or stdout only prevents accidental interactive use. It is not authentication and it proves nothing about SSH. The comparison is A's, and processes of the daemon's own user are inside the trust boundary, as everywhere in this contract. It opens no listener. It talks to its own daemon over the management socket as the user SSH logged in as; when that user cannot reach the daemon, or the daemon's account or machine id differ from what A expects, it fails before pairing.
- **Protocol:** stdout carries JSON lines only, each under 4 KiB, with `"v": 1`. stderr is diagnostics, read and logged by A but never parsed. Both are drained at once. The lines, in order:
  1. `{"v": 1, "event": "ready", "machineId", "account", "attempt"}`. It is sent only once B's daemon holds a one-shot acceptor for that account and that initiating machine id; a pairing request from another machine id cannot consume it. `attempt` is a fresh id B picks, which A passes to its daemon (`remote_pair {machineId, attempt}`) and which tags the `code` and `result` lines, so A never mixes two attempts. It is a correlation id, not a check: a relay that answers in A's name is caught by the code comparison, which is what protects the pairing.
  2. `{"v": 1, "event": "code", "attempt", "code"}`.
  3. `{"v": 1, "event": "result", "attempt", "outcome": "paired" | "cancelled" | "partial", "local", "peer", "detail"}`, with the states of "Partial results".

  A cancel or an expiry before any code ends with `ready` then `result`. A failure before `ready` (no daemon, a daemon that does not answer within 15 s, a stdin that closes or a line that comes first, wrong account, a pairing already active, over the limit) writes nothing to stdout, explains on stderr and exits non-zero; B watches its stdin from the start.

  A starts its pairing request only after a `ready` whose `machineId` is the B it resolved and whose `account` is its own. A malformed, repeated, out-of-order or foreign line ends the attempt, even after the confirms: from then on A reads no line of B's, not even an earlier `result`, never reports B `pinned`, and exits 2 or 4 with that line as the reason. After `code`, A compares it with its own code. On a match, it writes `{"v": 1, "confirm": "<code>"}` to B's stdin and confirms its own side, and keeps stdin open until B's `result`. On a mismatch, it writes `{"v": 1, "cancel": true}` and cancels its side. A closed stdin before a confirm cancels B. B confirms only when the line's code equals its own.
- **Deadlines:** 15 s for SSH to connect and 15 s more for `ready`, on A. B starts the pairing's 2 minutes on its monotonic clock when it registers the acceptor, just before it writes `ready`; A starts its own 2 minutes when it receives a valid `ready`. Neither is extended by progress, and B's expiry decides. On any failure, A cancels its side and closes B's stdin. A reaps the `ssh` child, and B's acceptor expires on its own deadline if cleanup can't reach it.
- **Limits:** the same as any pairing. One active pairing per target, and 5 attempts an hour per target, counted by B's daemon on `ready`, mismatches and cancels included; a new helper or a reconnect doesn't reset the count. There is no automatic retry after a mismatch.
- **Exit codes:** 0 paired on both sides; 2 refused or cancelled with nothing saved; 4 partial, with both sides' states.

The pairing messages come in exactly that order, each once: commit, nonce, reveal, then the two confirms, then, in a session of protocol 3 or later, each side's `pair.saved` (see "Partial results"). Anything else abandons the pairing. A target accepts one pairing at a time and at most 5 pairing attempts an hour; more answer `busy` with `retryAfter`. Pairing is symmetric: both machines pin each other, and each one's own settings decide what the other may see or do.

### Sessions

Every session, pairing included, uses `Noise_XX_25519_ChaChaPoly_BLAKE2s`, through the `snow` crate. The viewer is always the initiator.

- **Opening:** the first relay frame of a session is a `send` with `kind: "hs"`, a new session id, and `purpose`, `pair` or `session`, in its envelope. The responder uses the purpose to choose the prologue and nothing else. A relay that changes it only makes the handshake fail, since the purpose is in the prologue too. A frame for an unknown session id without a purpose is dropped.
- **Prologue,** byte for byte:

  ```text
  "localify-remote/1"                 17 bytes, ASCII
  purpose                              1 byte: 0x01 session, 0x02 pair
  len ‖ account user id                1-byte length, then UTF-8
  len ‖ initiator machine id           same
  len ‖ responder machine id           same
  session id                          16 bytes, raw
  ```

  A handshake replayed into another session, account, direction or purpose fails.
- **Session id:** 16 random bytes from the initiator, base64url (22 characters) in the envelope.
- **Handshake expiry:** each side drops a handshake that is not complete 30 seconds after its first message, measured on its monotonic clock.
- **Peer check:** once the handshake gives the peer's static key, each side compares it with the pin of the machine id the relay says the frame came from. A missing pin or a different key ends the session with `not_paired` or `key_mismatch`, and the UI says so. Pairing is the only purpose that accepts an unpinned key, and it pins nothing without the code.
- **Hello:** the first transport message each way is `hello` (see "Session messages"). Neither side sends anything else before it, and the target sends no inventory before it has checked the viewer's pin.
- **Replay:** each session uses fresh ephemeral keys, and each direction has a 64-bit nonce counter. Frames arrive in order over one WebSocket, so a frame that fails to decrypt, is repeated or is missing ends the session. Nothing from an earlier session decrypts in a later one.
- **Heartbeat:** while a session is open, the viewer sends `keep` every 30 seconds and the target answers `kept` at once. Both are encrypted, so they prove the peer itself is there, not only the relay. On its monotonic clock, the viewer marks the target `not_responding` when no authenticated message arrives within 15 seconds of a `keep`, and ends the session after 60 seconds without one. The target ends a session after 90 seconds without a `keep`.
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
| `X-Localify-Proto` | optional: the session protocol versions it speaks, `<min>-<max>`; `1-1` when absent |

The Worker, cheapest checks first:

1. Before any D1 work: the token has the shape Localify issues (`lfy_` and 64 lowercase hex digits), the subprotocol is one it speaks, and two rate limiters let the request through: `REMOTE_IP_LIMITER`, keyed by client IP, and `REMOTE_CONNECT_LIMITER`, keyed by the token's SHA-256. Both are Cloudflare rate-limiting bindings: approximate counts kept per location, so they shed load from one place and do not cap how often a token is checked worldwide.
2. Looks the token up, then checks the ban, the account's `remoteAccess` (see "Entitlement"), and that the machine has a remote key.
3. Forwards the request to the account's `RemoteRelay`, `idFromName("user:" + userId)`, with the user id, machine id, version, protocol range and the time it checked, in internal headers it sets itself after removing any the client sent.

A refused upgrade carries the API's error shape. A `429` asks for a `Retry-After` of 30 to 59 seconds, chosen at random, so a storm comes back spread out. The `101` names `localify-remote.1` in its `Sec-WebSocket-Protocol`.

The object accepts the socket with `ctx.acceptWebSocket(ws, [machineId])`, so it can hibernate while the socket is idle. A second socket for the same machine replaces the first, which is closed with `4001`. It then sends `welcome` to the new socket and `presence` to the others.

| Answer | Meaning |
|---|---|
| `101` | connected |
| `401` | malformed, unknown or revoked token |
| `402 subscription_required` | the account has no `remoteAccess` |
| `403 banned` | the account is banned |
| `409 remote_key_missing` | upload a key first |
| `426 unsupported_protocol` | no subprotocol this Worker speaks |
| `429 rate_limited` | with `Retry-After` |

### State

The object stores nothing: no storage calls, no alarms, no timers, no history. Each socket's attachment (`serializeAttachment`, at most 2 KiB) holds the machine id, a connection id, the version and protocol range, the connection time, and `authorizedUntil`. After hibernation, the constructor rebuilds the map from machine id to socket from `ctx.getWebSockets()` and those attachments. Everything else, inventory and output included, lives on the endpoints and is asked for again.

`ctx.setWebSocketAutoResponse` answers the text frame `ping` with `pong` without waking the object. Clients send `ping` every 30 seconds and treat 75 seconds without `pong` or any frame as a lost relay connection.

Authorization is checked lazily, on both ends of every frame. Before it does anything with a frame other than `ping`, answering it with an error included, the object looks at the `authorizedUntil` of the sender's socket and of the recipient's. For each one that has passed, it reads the account and the machine from D1, in one query for both: still entitled, not banned, machine active. A socket that passes gets `authorizedUntil` 60 seconds ahead. One that fails is closed with `4402`, `4403` or `4401`, and the frame is not forwarded. A sender due for a check is checked even for a frame it addressed to itself or got wrong, and hears nothing before its check passes; the recipient is checked with it when the frame names one that is connected. When D1 fails, the frame is dropped and the sender gets `error` `unavailable` with `retryAfter: 5`: the relay fails closed. A dropped frame ends its Noise session (see "Relay frames"). This bounds unauthorized relaying to a minute without a timer. It does not promise to close idle sockets: an idle socket of an account that lost access stays open until its next frame or a push (see "Entitlement"), and relays nothing in between.

A frame for a machine with no socket gets `error` with `peer_offline` back, and so does a frame whose recipient the check just closed. The object never queues.

### Limits

| Limit | Value | Over it |
|---|---|---|
| Frame | 64 KiB | close `1009` |
| Application message, before encryption | 32 KiB | the sender splits it (pages, chunks); the receiver ends the session |
| Sockets | one per active machine of the account, so at most 10 | the older one is closed with `4001` |
| Frames from one socket | 50 a second on average over 10 seconds, bursts of 200 | `error` `rate_limited` with `retryAfter`, then close `4429` with `{"retryAfter": 10}` at the 50th frame refused in a row |
| Bytes from one socket | 512 KiB a second on average over 10 seconds | same |
| Upgrades | `REMOTE_IP_LIMITER` 30 a minute per IP, `REMOTE_CONNECT_LIMITER` 6 a minute per token, both approximate and per location | `429` with `Retry-After` |

The two budgets are token buckets: one of 200 frames refilled at 50 a second, one of 5 MiB refilled at 512 KiB a second. A refused frame takes nothing from them. The size cap, the frame's shape and both budgets are checked as a frame arrives, before it waits for anything. A socket's frames are handled in order, and at most 32 frames or 512 KiB wait behind the one being handled, as when its check waits for D1; a frame past that is refused like one past the budget. While a socket is due for a check, a refusal gets no `rate_limited` answer, and the 50th in a row still closes it with `4429`. The counters live in memory and start over after hibernation, which only makes them more permissive for a moment.

**Send pacing.** Each daemon sends everything through one queue per relay socket, paced below the relay's budget: at most 40 frames and 384 KiB a second averaged over 10 seconds, for all its sessions together, whatever role it plays in each. Handshakes, heartbeats, pairing and control messages go first, then inventory pages, then output, taken round-robin between follows. Concurrent snapshots and follows therefore slow down instead of tripping the relay's limit, and a follow's window stops it from queuing more than it may.

### Reconnects

The daemon keeps one connection while it is signed in, holds a key, and its account has `remoteAccess`.

- **One attempt at a time:** a daemon never has two connection attempts in flight.
- **Backoff:** full jitter, `sleep = random(0, min(300, 1 × 2^attempt))` seconds. A connection that stayed up 60 seconds resets the attempt count.
- **Server hints:** a `Retry-After`, or a close reason carrying `{"retryAfter": n}`, sets the least wait; jitter adds up to 20% on top.
- **Deploys:** a close with `1012` or `1001` waits `random(0, 10)` seconds first, so a deploy that drops every socket does not bring them all back in the same second.
- **Wake and network change:** one immediate attempt after a wake from sleep (the wall clock moved more than a minute past the monotonic one, as in public-urls.md, "Tunnel"), with `random(0, 2)` seconds of jitter, then the backoff.
- **Final closes:** `4401` signs the machine out locally, as a `401` does. `4403` stops and says the account is suspended. `4402` asks for a lease after `random(30, 60)` seconds and reconnects once one is granted; while leases answer `402`, it asks again every 15 minutes, and at once on `public_refresh` or a plan change. A `4402` can be a moment stale, so the daemon never waits on it alone. `4002` waits for an update.
- **After a reconnect:** every session is new. The viewer asks for fresh snapshots and offsets, and never resends a control that had no answer (see "Controls").

### Cost

The relay costs Durable Object requests and duration. An object is billed duration at 128 MB while it is active: running an event handler, or kept from hibernating. An idle object whose sockets are all hibernatable and which has no event in progress is eligible for hibernation and incurs no duration charge, whether or not it has been evicted yet. Auto-answered pings do not wake it. So an account nobody watches costs nothing, and a watched one costs about the time spent handling its frames.

At the published Workers Paid prices ($12.50 per million GB-s past 400,000 included each month, and $0.15 per million requests past one million, where 20 incoming WebSocket messages count as one request), the worst case is a viewer following output that never pauses, which could keep the object active all the time. An hour of that is 0.125 GB × 3,600 s = 450 GB-s, about $0.0056. Two hours of it each working day cost about $0.25 a month, against $7.99 for Pro.

What has to be measured is how much active time a frame costs, and so what a typical session costs. PR 2 measures it on staging: two daemons, one viewer following a service that prints a line a second for an hour, then ten lines a second, read from the Durable Object analytics in Cloudflare's GraphQL API. The result goes in `apps/cloud/README.md` with an alert on the namespace's monthly duration. Machine tunnels (`m-<id>.localify.dev`) are not used: they exist only while a machine serves public URLs (ADR 0025) and each one past 1,000 costs $4.50 a month.

## Wire protocol

### Relay frames

Text frames of JSON, at most 64 KiB of UTF-8, plus the bare `ping` and `pong`. Field names are camelCase. The relay reads only the envelope.

What the relay refuses:

- A frame that is not a JSON `send` object (not JSON, a binary frame, another `t`) closes the socket with `4000`.
- A `send` frame with a malformed field is dropped with `error` `bad_frame`, which carries `sid` when the frame's own is valid. So is a frame to its own sender. `to` is 1 to 64 base64url characters, `sid` exactly 22, and `data` non-empty base64url, required for `hs` and `msg`.
- Fields it does not know are ignored and never forwarded, and `data` on a `close` is dropped.

From a daemon:

```ts
type Send = {
  t: "send";
  to: string;               // machine id
  sid: string;              // session id, 22 characters
  kind: "hs" | "msg" | "close";
  purpose?: "pair" | "session";  // on the first "hs" frame of a session only
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
type Recv = {
  t: "recv"; from: string; sid: string; kind: "hs" | "msg" | "close";
  purpose?: "pair" | "session"; data?: string;
};
type Revoked = { t: "revoked"; machineId: string };   // removed from the account: delete its pin
type RelayError = {
  t: "error"; code: "peer_offline" | "rate_limited" | "bad_frame" | "unavailable";
  sid?: string; message: string; retryAfter?: number;
};
```

The relay sets `from` from the sender's attachment, never from the frame. `close` is unauthenticated, so it only ever ends a session early.

An `error` that names a session means the relay dropped one of its frames (`peer_offline`, `rate_limited`, `unavailable`). A Noise session cannot skip a frame, since the next one's nonce would not match, so the sender ends that session at once with a `close`. It waits `retryAfter`, or the backoff, then opens a fresh session. It never resends what was dropped: it asks for fresh snapshots and offsets, and reconciles each unanswered control with `control.status`.

Close codes:

| Code | Meaning | The daemon |
|---|---|---|
| `1001`, `1012` | going away, restart | reconnects after `random(0, 10)` s |
| `1009` | frame too large | reconnects with backoff, logs an error |
| `4000` | bad frame | same |
| `4001` | replaced by a newer connection of this machine | does not reconnect from this connection |
| `4002` | protocol version no longer accepted | waits for an update |
| `4401` | token revoked | signs out locally |
| `4402` | remote access lapsed | asks for a lease after `random(30, 60)` s, reconnects once one is granted, and otherwise asks every 15 minutes |
| `4403` | banned | stops |
| `4429` | rate limited | reconnects after `retryAfter` |

### Session messages

JSON objects inside Noise transport messages, at most 32 KiB before encryption. Every request carries an `id` the viewer picks (at most 32 characters, unique within the session), and every answer carries it back as `re`.

```ts
// Both ways, first.
type Hello = {
  t: "hello"; proto: { min: 1; max: 3 }; version: string;
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
type OutputFollow = { t: "output.follow"; id: string; runId: string; offset: number; window: number;  // ≤ 131,072
  tail?: number;                       // optional: start this many bytes before the end, ignoring offset
};
type OutputAck    = { t: "output.ack"; re: string; offset: number };
type OutputUnfollow = { t: "output.unfollow"; re: string };
type Keep = { t: "keep"; ids: string[] };   // heartbeat; also renews these watches and follows
type ControlStatus = { t: "control.status"; id: string; requestId: string };  // answered with a control.result
// Session protocol 2.
type AccessRequest = { t: "access.request"; id: string; wants: "control" | "backgroundStarts" };  // backgroundStarts asks for control too
type AccessCancel  = { t: "access.cancel"; re: string };
type Control = {
  t: "control"; id: string;
  action: "start" | "stop" | "restart";
  service: { projectId: string; environment: string | null; serviceId: string };
  profile: string | null;              // start only: a name LaunchProfiles::load resolves; null is the effective default
  requestId: string;                   // 16 random bytes, base64url: names this control across sessions
  expect: { runId: string | null; configRev: string;
    legacyId?: string;                 // protocol 2, restart only: the legacy run to relaunch (see "Relaunching a legacy run")
  };
  expiresAt: string;                   // RFC 3339, in the target's clock
};

// Target → viewer.
type Kept = { t: "kept" };             // answers every keep, at once
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
  ended: boolean;                      // the run ended and everything kept was sent
  interrupted?: true;                  // with ended: capture stopped early (see "The end of a run")
};
type ControlProgress = { t: "control.progress"; re: string; phase: "dispatched" | "started" };
// Session protocol 2. Ends an access.request; busy and too_many come as an error with re.
type AccessResult = { t: "access.result"; re: string;
  outcome: "granted" | "declined" | "nobody" | "expired" | "cancelled" };
type ControlResult = {
  t: "control.result"; re: string;
  // starting: admitted and running, not listening yet. unknown: transient,
  // while nothing is decided yet: the launch has no record and its expiry has
  // not passed, or the target still works on the request. The viewer asks again.
  outcome: "done" | "starting" | "refused" | "conflict" | "failed" | "unknown";
  code: string | null;                 // see "Errors" when not done
  message: string;
  runId: string | null;                // the run now in place, when known
};

// Both ways.
type SessionError = { t: "error"; re?: string; code: string; message: string; retryAfter?: number };
type Bye = { t: "bye"; reason: "done" | "unpaired" | "lifetime" | "idle" | "not_shared" | "lease_expired" };

// Pairing session only, in this order.
type PairCommit = { t: "pair.commit"; c: string };      // base64url
type PairNonce = { t: "pair.nonce"; n: string };
type PairReveal = { t: "pair.reveal"; n: string };
type PairConfirm = { t: "pair.confirm"; accept: boolean };
type PairSaved = { t: "pair.saved" };                   // protocol 3: after both accepted, once this side's pin is written
type PairDeclined = { t: "pair.declined"; reason: "nobody_to_confirm" | "busy" | "cancelled" };
```

`InventoryItem` and `MachineSummary` are in "Inventory". A message that does not parse, has an unknown `t`, or breaks a bound ends the session with `error` `bad_request` then `bye`. An `error` with `re` answers that one request, which is over, and the session goes on; an `error` without `re` ends the session. A request whose `id` still names one in progress in the session (waiting, watched, followed, or with answers still to send), whatever its kind, is answered `bad_request`, and nothing is done for it.

### Errors

The `code` of a `SessionError`, or of a `ControlResult` that is not `done`:

| Code | Meaning | The viewer says |
|---|---|---|
| `not_paired`, `key_mismatch` | the peer check failed | pair again; for a mismatch, "Don't trust this machine until you pair it again." |
| `not_shared` | `share` is off on the target | "B doesn't share its projects. Turn on sharing in Localify on B." |
| `control_disabled` | `control` is off | "B doesn't allow remote controls." |
| `background_disabled` | the start or the restart would run in the background, and `backgroundStarts` is off | "Localify isn't open on B, and B doesn't allow background starts." |
| `not_owned` | the run is in a terminal Localify does not own, or an unknown process holds the port | "This runs in a terminal on B. Stop it there." |
| `legacy_run` | the service runs from a shell tab of an app from before run PTYs, and its PID is not proven, or the control is not a relaunch | "Relaunch it on that machine to enable remote control." |
| `owner_gone` | an app run outlived its app: stop is allowed, restart is not | "Stop it, then start it again." |
| `refused_profile` | a dangerous, unknown or removed profile | "Start it on B to use this profile." |
| `refused_argument` | a command, path, env value or option a remote control never takes | — (the CLI and the MCP server refuse these before sending; only a modified client gets here) |
| `unknown_service` | the ids name nothing in the target's config | "This service no longer exists on B." |
| `conflict` | the run, the config or the profile changed since the viewer looked, another launch won, or a previous run still has live processes | "B changed in the meantime." with the fresh state |
| `expired` | the control arrived after `expiresAt` or with `expiresAt` more than 60 s ahead, or the launch reached `localify dev` after its own expiry | "The request took too long to reach B. Nothing was started." |
| `busy` | another control of this service is in flight on the target, too many controls and `control.status`es of this viewer or of all viewers already going on B (with `retryAfter`), too many handshakes or pairings, or another access request is waiting on B | "Something else is starting or stopping it on B."; for access: "B is answering another request." |
| `unauthorized` | the target could not get a fresh lease, or the viewer is not in it | "Remote access is not available right now." |
| `output_unavailable` | no output stream for that run, with the reason | see "Output" |
| `gone` | the run id is not the service's last run | the viewer moves to the current run |
| `request_mismatch` | a `requestId` the target already has, sent with another action, service or expected revision | "This request doesn't match the one B recorded." |
| `unknown_request` | `control.status` names a request id the target does not have, or no longer has after 24 hours | "B has no record of that request." |
| `too_many` | over a watch, follow or session bound, an access request too soon (with `retryAfter`), or more new controls from this viewer than request entries allow (see "Request entries") | for access: "You asked B recently. Try again in 9 min." |
| `unsupported` | a message the peer's version does not know | "Update Localify on B." |
| `bad_request` | malformed, or out of bounds | — |

### Versioning

Two versions move separately. The relay protocol is the WebSocket subprotocol, `localify-remote.1`; a Worker that drops one closes its sockets with `4002`. The session protocol is `proto` in `hello` and in `presence`. The session's version is the highest one in both `hello` ranges, and each side computes it from the two authenticated `hello`s, never from `presence`, which the relay could alter: `presence` and `X-Localify-Proto` only let the viewer say "Update needed" before it opens a session. The prologue's label, `localify-remote/1`, names the handshake's format and does not change with the session protocol. When the `hello` ranges do not overlap, the session ends with `unsupported`. Version 2 adds `access.request`, `access.cancel`, `access.result` and `expect.legacyId`. Version 3 adds `pair.saved`. A sender never sends them in a session of an earlier version, and a receiver treats them there as an unknown `t`. So a pairing with a 1.7.0 peer, which speaks 1 to 2, agrees on 2 and never carries `pair.saved`. A new optional field is added without a new version, and a receiver ignores fields it does not know, except in `control`, where an unknown field is refused so that no argument is ever silently dropped. A new message type, or a change in meaning, takes a new version, and a sender never sends a type the peer's version lacks.

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
    runId: string | null;           // null for a legacy run
    legacy: boolean; startedAt: string; profile: string | null; profileDanger: boolean;
    legacyId?: string;              // a legacy run whose PID is proven: what a relaunch names
    exit: { at: string; code: number | null } | null;
  } | null;                         // the service's last recorded run
  process: "running" | "stopped";   // whether any process of the run's session is alive
  port: { number: number; reachable: boolean | null };  // null: not probed before the deadline
  output: { available: true; runId: string }
        | { available: false; reason: "terminal" | "unknown_owner" | "no_run" | "legacy_run" };
  publicUrl: { url: string; serving: boolean;
               reason: "suspended" | "taken_down" | "served_elsewhere" | "tunnel_down" | null } | null;
  controls: Record<"start" | "stop" | "restart", { allowed: true } | { allowed: false; code: string }>;
};
```

- **Sources:** the configured projects, environments and services (the config the daemon loads, polled every 2 seconds); the checkouts' `localify.toml`; the running-service records in `services/`; a TCP connect to each service's port on `127.0.0.1`, 200 ms timeout, from at most 16 threads and within 1 second for all of them, a port not probed by then being `reachable: null`; the daemon's own runtime for public URLs and the tunnel (public-urls.md, "Tunnel").
- **Four states:** `process` comes from the run's session, running while any process proven to belong to the run is alive (see "The end of a run"); `port.reachable` from the connect; `output.available` from the recorded run's stream, whoever holds the port now (`unknown_owner` only for a listener with no stream to attribute); `publicUrl.serving` from the URL's state, the machine that serves it, and this machine's tunnel being connected. They disagree often, and the UI shows each one: a process running with its port not reachable yet is "Starting"; a reachable port with no running record is `owner: "unknown"`.
- **`configRev`:** the first 16 hex digits of HMAC-SHA-256, under a per-machine secret, over the canonical JSON of what a launch of this service would use: the service's entry as the daemon loaded it (command, folder, port, domains, environment; the primary checkout's entry as the config gives it, whether `--env` names it or not, every domain kept and no folder inferred); the effective launch plan `localify dev` resolves from it, through the one resolver both share (the argv after package-manager and script detection, the working folder, the framework arguments Localify adds) and the profile it would choose; the `package.json` of the service folder and of the checkout root that plan was read from; and the full effective profile definitions of its checkout as `LaunchProfiles::load` gives them (each profile's name, sources, `danger`, `max_duration`, `browser_indicator`, and its inline `[profiles.<name>.env]` values). It changes whenever any of them does. The HMAC keeps a low-entropy inline value from being guessed from a revision. The secret is 32 random bytes in `<data dir>/launch-rev.key` (0600), made on first need: written to a temporary file, then linked to its name, so two first uses at once agree on one key and a key is never replaced. The executor computes the revision from the definition it loaded and runs from that same definition (see "Controls"): in PR 4, a remote launch executes the plan it checked against `--expect-rev`, and never resolves it again.
- **Legacy, in the inventory:** a record with no run id, and an app record with a run id but no stream (a shell tab of an app from before run PTYs), show `legacy: true`, `runId: null` and `output.reason: "legacy_run"`. While such a run is alive or unproven, every control is refused with `legacy_run`; once it is over, a start is allowed. A legacy run whose PID is proven, recorded by the app or by `dev --detach`, carries `legacyId`: `<boot id>.<pid>.<native start>`, which names that one process across PID reuse. Its `restart` then follows the checks of any restart (`control`, the owner, `backgroundStarts`) instead of `legacy_run`, since a restart of it is a relaunch (see "Relaunching a legacy run"). The snapshot is the same for every viewer: a 1.5 viewer sends no controls, and a control without `legacyId` on a legacy run is still refused `legacy_run`.
- **`controls`:** what a paired viewer may do right now, with the code a refusal would carry, so the UI says why, and offers a fix, before anyone clicks.
- **`share` off:** the target sends `machine` and no items.
- **Epoch and revision:** `epoch` is random, chosen at daemon start; `rev` starts at 1 and goes up whenever the content of what a viewer is sent changes: the items, and the `machine` summary with them, so the app opening or quitting or a switch turned moves it too. A request whose `have` matches gets `inventory.unchanged`.
- **Pages:** items sorted by project name, environment and service; split so each page fits 32 KiB; at most 32 pages. A viewer replaces its copy only once it holds every page of one revision; pages of an older revision are dropped.
- **When it is sent:** on `inventory.get`; while a watch is open, whenever the content changes, checked every 2 seconds (in PR 3, each check builds the whole snapshot, ports probed included: the inventory has no cheaper partial build, and the probe is bounded to 1 second); right after every control's result; and on a 30-second reconcile while watched. Unchanged snapshots are never sent.
- **Watches:** at most one per session. A watch ends with `inventory.unwatch`, with the session, or 120 seconds after the last `keep` that names it. The viewer names it in its `keep` while the machine is on screen, and stops when the window is hidden. With no watch open, the target computes nothing and sends nothing.
- **Pending answers:** a target admits at most 4 `inventory.get` and 8 `output.read` in flight per session (16 and 32 in all), from their arrival until their last answer is in the send queue, and keeps at most 1 MiB and 64 KiB of answers waiting per session, 4 MiB in all, a whole snapshot fitting in one session's. Past a count it answers `too_many`, past the bytes `busy` with `retryAfter`, without reading or paging anything. A viewer whose refusals no longer fit loses its session.
- **Deltas,** if they come later, carry the epoch and the revision they apply to. A gap or another epoch makes the viewer ask for a snapshot.

## Output

### A run PTY, and a stream per run

Output shown remotely is everything one run's own channel carried, and nothing else. Today an app-owned service tab is an interactive shell: the app types the service's command into it, copies every byte of the shell into the service's output file, and stops the service by writing Ctrl-C into the tab. The file then holds the prompt and any command typed after the service exits, and a Ctrl-C can land in an editor or a REPL that took the foreground. Telling service bytes from shell bytes after the fact does not work either: a service suspended, a command run, and the service resumed, all before the reader catches up, interleave in one byte stream. So the channel itself carries the provenance. That old file is never exported. PR 1 replaces the model:

- **A dedicated PTY per run.** To launch a service, the app spawns the pinned Localify executable directly as the only child of a new PTY: `localify --config <path> dev <service> --project <id> [--env <slug>] [--profile <name>] --launch-id <launchId>`, in the service's folder. It gets the shell environment below, plus `LOCALIFY_SERVICE_ORIGIN=app` and the variables that select the app's own installation (the installed app or a dev build), so a run always uses the app's config and state. There is no interactive shell in it, and nothing is ever typed into an existing shell. `localify dev` is the PTY's session leader, so the run has its own session and process group. Every launch, a restart included, creates a fresh run PTY. Ctrl-Z is disabled in it (`VSUSP` is cleared in the PTY's termios): with no shell, nothing could resume a suspended service.
- **The shell environment.** Without a shell, a service would lose what the person's shell init sets up: `PATH` entries, `nvm`, `asdf`, profile CLIs. So the app resolves the user's interactive login-shell environment outside any captured PTY, as editors do, through the hidden command `"<pinned localify>" shell-env <nonce>`, quoted in the syntax of the shell that runs it. The invocation depends on `$SHELL`: `zsh -ilc`, `bash -ilc` and `fish -ilc` with that command; `nu -l -i -c` with `^"<path>" shell-env <nonce>`, Nushell's form for an external command; and `/bin/sh -lc` for any other shell, which reads only the POSIX profile files. The nonce is 128 random bits, new each time. `shell-env` prints the nonce, then each variable as `KEY=VALUE`, all separated by NUL bytes, then the nonce again. The app accepts the result only when both nonces frame it, nothing but valid `KEY=VALUE` entries lie between them, and the capture stays under 1 MiB; whatever the shell init prints outside the frame is ignored. Anything malformed, interleaved or oversized counts as a failure. Stdin is closed and stderr discarded. One 10-second deadline covers the spawn, the reading and the cleanup; when it expires, the app kills the resolver's process group. Closed stdin does not stop a prompt that opens `/dev/tty`, so the deadline is what guarantees the app never waits on one. The result is cached for the app session; `Reload shell environment`, in the terminal panel's menu, resolves it again. On failure, runs start with the app's own environment, and the panel shows "Couldn't read your shell environment. Services start without what your shell config sets up." with the reason and `Reload shell environment`. Background runs use the same resolver (see "Detached starts").
- **The service tab is the run.** It shows the run PTY, interactive and resizable, so the service sees a real terminal: colors, prompts, keys and window size behave as they do today. When the run has ended (see "The end of a run"), the tab shows "Exited with code 1 at 10:42" (or "Stopped" for a signal), stops taking input, and offers `Restart` and `Open a terminal here`. The latter opens a separate, ordinary shell tab in the service's folder.
- **Stop and restart** signal the run's own session and process group, each process checked against its start time (TERM, then KILL after 10 seconds, with the process tree as `localify stop` handles it today), and never write a Ctrl-C into a tab. A Ctrl-C the person types into a running service tab still reaches the service, as in any terminal.
- **The end of a run.** Profiled or not, every service runs under the same supervisor in `localify dev`. When the executor exits, or dies, the run is not over: descendants may ignore signals or still be starting. A finisher ends it, as "The end of a run" below describes, and only then does the tab show "Exited", the launch record get its exit, and the service become free.
- **The stream:** the app reads the run PTY's master and appends every byte, and only those bytes, to the run's stream. It is the only writer. Shell tabs are never written to a stream and never exported.
- **Typed input is part of the output.** Keystrokes a person types into a running service tab are echoed by the terminal into the stream, so `localify logs` and paired machines see them. That is accepted. The tab says it in its footer while `share` is on: "What you type here appears in this service's output." Input with echo turned off, such as a password prompt, is not echoed.
- **Run id:** the launch id the dispatcher chose: the app for a run PTY, `dev --detach` for a background run, the daemon for a remote launch. A person's own `localify dev` picks one itself. It is 16 random bytes, base64url, recorded as `runId` in the run's record.
- **Location:** `<state dir>/services/output/<runId>/`, made of 4 MiB segments named by their index (`0.log`, `1.log`, …), segment `k` holding logical bytes `[k × 4 MiB, (k + 1) × 4 MiB)`. The writer keeps the two newest, so between 4 and 8 MiB of the end of a run stay readable. Writers derive the path from the run id they chose; nothing ever reads a path from the output.
- **Background runs:** the detached executor gives the service pipes for stdout and stderr, stdin closed, and copies them into the stream itself. The executor's own messages (a refusal, the launch summary) are not the service's output: `dev --detach` sends them to `services/launches/<launchId>.log`, which it shows when the launch fails, and which goes with the run's stream; so does the daemon for the executors it spawns (see "Detached starts").
- **Terminal runs** have no stream: `output.reason` is `terminal`.
- **Records** say where their output is: `output: "pty"` for a run PTY, `"pipe"` for a background run, `null` otherwise.
- **Legacy runs.** A service already running from a shell tab when the app updates (origin `app`, no run id) is left as it is. It is shown with a legacy identity, its PID and start time. Its PID is trusted, and signalled, only when the process's native start reads as the start time `ps` recorded, to the second and unambiguously, and this boot began before the record was written. Otherwise the run is `unknown`: never signalled, and to be stopped by hand; its output is unavailable remotely (`legacy_run`); and its remote controls are disabled with "Relaunch it on that machine to enable remote control." The old whole-shell file is never a fallback.
- **Locally,** `localify logs` and `service_logs` read the same stream, so an agent no longer reads shell prompts either. The whole-shell copy and `LOCALIFY_SERVICE_OUTPUT` go away. A record without a run id keeps its `logFile` for `localify logs` only.
- **Retention:** as today, a run's stream is deleted when the next run of the same service registers, or when the service is forgotten.

### The end of a run

A run ends when no process of its session is left and its stream is finalized. A **finisher** ends it: the app for a run PTY, and once the app is gone the run PTY's executor, or the daemon's pass when the executor died too; for a background run, the daemon's inventory pass, or the next launcher of that service (a dispatcher, or a person's own `localify dev`). An executor that reads its run's output itself (a background run), or whose run has no stream (a terminal run), is also the finisher of its own run when its service exits: it is alive, so its session needs no proof, and it ends what the service left there, then finalizes `complete`, or `interrupted` when a pipe was still held open after the session ended. An executor that cannot empty its session, a member surviving SIGKILL, publishes no `end.json`: the run stays open, and the next launch is refused as for a live run. Output that reached its end says nothing about the run's processes. A Ctrl+C in `localify dev` starts a bounded stop of the service and every descendant it is tracking, which goes on after the service itself exits. It keeps those descendants by identity, looking them up by scanning every process each 2 seconds while the run is healthy, and each 200 ms during a stop or a session cleanup; a descendant that forks and leaves the session between two looks of a healthy run is not known to the stop, a known limit that keeps an idle run cheap. The stop itself: SIGINT is forwarded by topology, never by sender (on macOS a terminal's Ctrl+C names a sender, as a `kill` does). `localify dev` asks its controlling terminal (`/dev/tty`, opened with `O_NOCTTY`) for its foreground group, which the terminal delivered the Ctrl+C to, and forwards SIGINT only to tracked processes outside that group, whoever leads the session and wherever a shell moved the group. Without a controlling terminal (a detached run), every tracked process gets it. A `kill -INT` of `localify dev` in a terminal therefore reaches the foreground group only through the SIGTERM and SIGKILL that follow; `localify stop` uses SIGTERM, which is forwarded and starts the same stop, SIGKILL coming after 10 seconds. A SIGHUP sent to `localify dev` is only forwarded: a service may trap it to reload. In a run PTY it means the app closed the terminal, as does a write that fails on it: `localify dev` then starts the same stop as for a SIGTERM, and every message it prints is written so that a closed terminal cannot end it before that stop does. It watches these signals from before its profile resolves: a stop while profile sources resolve ends the profile CLIs it started, known by identity from their spawn, SIGTERM then SIGKILL after the grace period, and exits before any service starts. The app cancels a launch that has no record yet through `localify dev`'s identity, read at spawn: it signals it, the members of its session and its descendants, and the cancel succeeds only once all of them are gone. `localify dev` watches these signals before it starts the service, and one that arrives before it knows the service's identity waits and is forwarded as soon as it does. SIGTERM follows after 10 seconds and SIGKILL after 20, and a second Ctrl+C kills at once; the exit code is 130.

**Identity.** A process is known by its PID and its start time, in one boot-scoped monotonic time domain that a change of the wall clock cannot move. On macOS, `proc_listpids(PROC_ALL_PIDS)` enumerates processes, `proc_pid_rusage` gives each one's start as `ri_proc_start_abstime`, in the `mach_absolute_time` timebase, `proc_pidinfo(PROC_PIDTBSDINFO)` its process group (`pbi_pgid`), and `getsid(pid)` its session. On Linux, `/proc/<pid>/stat` gives the process group (field 5), the session (field 6) and the start time in clock ticks since boot (field 22). Wall-clock times are kept for display only. PR 1 replaces the `ps -o lstart` call behind `process_started_at()`, which has a resolution of one second and spawns a process, with these native calls.

**The run record** stores the run's session id and process group id, both equal to the executor's PID, the executor's start time, and the boot id: `kern.bootsessionuuid` on macOS, `/proc/sys/kernel/random/boot_id` on Linux. The executor refreshes `lastAliveAt` in the record every 5 seconds, in the same time domain as start times: `mach_absolute_time` on macOS, `CLOCK_BOOTTIME` converted to clock ticks on Linux. A record from another boot signals nothing: a reboot ended every process of that run, so the finisher only finalizes.

The session and process group are recorded only when the executor leads its session, as in a run PTY or a detached start. In a person's own shell, the executor is a job of the shell's session, and leading a session of its own would detach it from the terminal, so its record has neither: no finisher ever signals by session there, and `localify stop` follows the process tree while the executor lives.

**Which processes belong to the run.** A session id is never adopted because its number matches. POSIX does not reuse a PID while any process still carries it as its process group or session id, so as long as one member of the run's session is alive, that session cannot have been replaced. A finisher scans for every process whose session is the run's session id, or whose process group is the run's process group id:

1. If the first scan finds at least one member whose start time is at or before `lastAliveAt`, both in that boot's monotonic time, that member lived inside the session while the executor did. Enumerating processes is not atomic, so that witness is read again after the scan: if it is gone or changed, another candidate is read; when every one is, the scan is dropped and the proof starts over, and after a few such scans nothing is decided: the run stays open, launches are refused, and the finisher says to try again. While members are being ended, each known member read alive again moves a proof horizon to that moment, and a later scan adopts the members that started by then, so a child a member started before it was killed is ended too, while a replacement session, all of whose members started later, never is. The session has therefore existed without a break since the run began, and every current member belongs to the run. The finisher signals them all (TERM, then KILL after 10 seconds), checking each one's start time before each signal, and scans again until none is left.
2. If the first scan finds no such member, the session ended at some point, and whatever carries that id now is unrelated. The finisher signals nothing and finalizes.

This leaves alone the processes that all started after the last heartbeat, in a session whose older members all died: nothing proves they belong to the run. A process still listening on the port then shows as `unknown`. A descendant that started its own session left the run; `localify stop` still follows the process tree while the executor lives.

A finisher never calls a session empty while something still carries it. When every process proven to be the run's is gone, but a process that started after the last proof still carries the run's session or process group, as a worker a proven member forked just before it died does, the finisher does not signal it and does not finalize. It saves the process to `finalize-state.json`, and reports it: the next launch is refused with "process <pid> in this run's session could not be confirmed as part of it; stop it yourself, then try again.", and `localify stop` says the same. Once it is gone, the next finisher finalizes as usual.

**Interrupted capture.** The reader of a run's output is the executor for a background run (its pipes) and the app for a run PTY (the master). When the executor of a run PTY dies, the app still reads: after the cleanup, the PTY reaches EOF and the stream is finalized `complete`, or `interrupted` with the reason when no EOF came within 3 seconds or writing failed. When the app dies, `localify dev` stops the run and finalizes what was written `interrupted` itself; when both died, the daemon's pass finds the run PTY's `reader.lock` free and does it. When the reader itself dies, the bytes still buffered in the pipe or the PTY die with it. The finisher then never waits for output nobody can reach. It finalizes the prefix already written by publishing `services/output/<runId>/end.json`, `{"reason": "complete" | "interrupted", "offset", "at"}` (`at` in Unix seconds, for display, and an `error` when writing the stream failed, such as a full disk: the executor then reads the pipes to their end and drops the rest, so the service never blocks), with a temporary file and `link` like a launch record, and goes on with the process cleanup. A viewer reading an interrupted stream gets `interrupted: true` with `ended`, and shows "Output was cut off when Localify stopped on B."

**One finalization at a time.** Each run has a lock file, `services/output/<runId>/finalize.lock`, created once and never renamed or deleted while the run's directory exists. The app, the daemon and every launcher finalize only while holding an exclusive `flock` on it, for the whole finalization. When a finisher crashes, the kernel releases its lock, so the next one takes over without checking anyone's identity. Every step is idempotent: a holder that finds `end.json` already published stops there. The app, which alone knows a run PTY's exit code once the executor died, holds the lock from publishing `end.json` until it has recorded that exit, and its `reader.lock` as long: a finisher that finds the run finalized then finds the exit with its code, and `localify stop`, which waits for the app to finalize, waits until the app lets go of `reader.lock`. Before it signals anything, a finisher writes what it proved to `services/output/<runId>/finalize-state.json` (a temporary file renamed over it): the time until which the session is proven unbroken, and the processes it found to be the run's, each marked a member of the session or a descendant that left it. The next holder, and the locked end of a stop whose executor died, start from there, once the boot id matches: a record from another boot drops it and signals nothing. A run already finalized has only those exact saved processes ended, nothing found by session. No launch of that service is admitted until its previous run is finalized: the next launcher finishes it first, as `stop` and the daemon do. When the previous executor is gone, the launcher ends what is proven to be left of its run (SIGTERM, then SIGKILL after the grace period), finalizes the run, and starts. It holds the service's lock meanwhile, so it waits at most 30 s for another finisher of that run (twice the grace period plus the SIGKILL wait, the longest a finisher signals under the lock), then refuses: another cleanup of the service is still running, try again. `stop` and the daemon may wait longer, and never hold the service's lock while they wait. A finisher counts its SIGKILL wait from the first SIGKILL it actually sends, so one suspended past the grace period, or right before that SIGKILL, still sends it and waits for it. A run folder a finisher cannot inspect (a permission or an I/O error) is an error that admits nothing; only a folder confirmed missing is a run that is gone. The launcher never signals anything it could not prove, and a proof it cannot make refuses the launch with an error that says to try again.

### Reads

`output.read {runId, offset, maxBytes}`:

- **Bounded:** the target opens the segment holding `offset`, seeks, and reads at most `maxBytes` (16 KiB at most), never to the end of the file. It trims what it sends so the encoded message stays within 32 KiB, and `next` says where it stopped.
- **Lines:** bytes become plain lines as `plain_lines` makes them today (colors and cursor moves removed, a line redrawn with `\r` kept as its last state), and every remaining control character except tab is removed, C1 controls and the Unicode bidirectional overrides (`U+202A`–`U+202E`, `U+2066`–`U+2069`) included. Invalid UTF-8 becomes `U+FFFD`.
- **Partial lines:** a line still being written is held back, and `next` stops before it. A line longer than 8 KiB is sent in 8 KiB pieces, each but the last with `cut: true`. Once the run has ended, its last unfinished line is sent as it is, and `ended` is true, with `interrupted` when the stream was finalized as interrupted.
- **Rotation:** an `offset` older than the oldest segment kept gets `rotated: {firstOffset}`, with data from there, and the viewer shows "Earlier output was dropped."
- **Another run:** a `runId` that is not the service's last run gets `gone`. The viewer reads the new run from offset 0 and shows "Restarted at 10:42".
- **Offsets** are logical bytes from the start of the run, never reused. An offset past what was written is an error, `bad_request`.

### Following

`output.follow {runId, offset, window}` sends `output.data` as lines arrive, the first one at once. The target never has more than `window` bytes sent and not acknowledged; the viewer sends `output.ack` with the `next` it has shown. The target looks for new bytes every 250 ms while a follow is open, locally. A follow ends with `output.unfollow`, at the end of the run once everything was sent, with the session, or 120 seconds after the last `ack` or `keep` that names it. At most 2 follows per session, 4 per target. With `tail`, an optional field, the follow starts that many bytes before the end, and at least at the oldest byte kept, so a viewer opening a long run reads its last lines rather than megabytes; that start is usually inside a line, and the viewer drops the piece before the first line break. A target that predates `tail` ignores it and starts at `offset`. The app's viewer gives each follow a 64 KiB window and acknowledges every 16 KiB it has shown. The viewer stops following when the Output panel closes or the window is hidden for 60 seconds, so an output nobody looks at costs nothing.

### Showing it

The panel is labelled "Output", never "Terminal". It shows plain lines with selectable text, a search box, and a Follow / Pause switch. Pausing stops scrolling while lines keep arriving. After 60 seconds paused, the viewer ends the follow, and follows again from where it stopped when the person resumes. It keeps at most 10,000 lines and drops the oldest. Rendering through xterm.js in read-only mode is allowed, but it gets the same plain lines, never raw bytes.

## Controls

### What a control may name

A control carries explicit ids: the target machine (the session), the project, the environment slug (`null` for the primary checkout), the service, and for a start an optional profile name; plus its `requestId`, what it expects and its expiry. It carries nothing else. A remote control never takes a command, a folder, a path, an env file, an env value, a domain, a port, `confirm_profile`, or anything that would create a project or an environment. A message with an unknown field in `control` is refused with `refused_argument`. Profiles follow the effective rules of `LaunchProfiles::load`: the ones `localify.toml` declares, or without it the implicit Infisical ones. A profile is allowed when it resolves and is not dangerous: the implicit `default` and `staging` are allowed, and a dangerous profile, the implicit `prod` included, is refused. `profile: null` means the effective default, validated the same way.

### The run lock and launch fencing

The executor, the `localify dev` process that runs the service (in a person's terminal, in a run PTY, or detached), is the only owner of the service's lock. Dispatchers never hold it.

1. Before it runs anything (profile sources, the service), the executor takes an exclusive, non-blocking `flock` on `services/<record name>.lock`, and holds it until it exits. The OS releases it even on a crash. If another executor holds it, the service is already running. A launch from a terminal (stdin and stderr both terminals) asks "<service> is already running (since <time>, pid <pid>). Stop it and start again? [Y/n]": Enter or `y` stops it as `localify stop` does, waits for its finalization, and starts; anything else leaves it and exits with an error. `--replace` does the same as a yes, and `--replace=<runId>` only while that run is the one running: `dev --detach` asks in the dispatcher, which has the terminal; for a launch it names, it stops that run itself, as `localify stop` does, before it dispatches, and passes the run it stopped as `--replace=<runId>`. Anything else (an agent, a script, CI, `dev --detach` without a terminal, the MCP server without `replace`, a `--remote-launch` even in a run PTY) never stops it, and says it is already running. Only the run chosen is replaced, the one a person agreed to stop or else the one seen first. Once the lock is free, the launcher checks, under it, that the record still names that run, even one that already exited; when it names another, someone else restarted the service, and the launcher refuses ("restarted by someone else") and starts nothing. Two replacing launches never both start, and only the one that takes the service over records, under the lock and after its own decision, that it replaced that run (`replacedBy`, below). A run of an older version, with no run id, is never stopped by a launch. If the previous run is not finalized, the launcher finishes it first (see "The end of a run"). Before it stops anything, the launcher makes every check that can refuse it but the port, none of which changes anything: a decision already published for its launch id, its arguments, the service and its environment, its plan and target, the profile it selects, which must exist and, when it is dangerous, be confirmed (`--confirm-profile` without a terminal), and the checks of step 2. The profile's sources run only under the lock. `dev --detach` makes the same checks for a launch it names before it asks about or stops the running service, as its executor, which has no terminal, would. A launch that will be refused stops nothing; under the lock the checks run again, and those decide. Two executors of one launch id never both reach a replacement: once its checks passed, an executor with an id takes that launch's own lock, `services/launches/<launchId>.lock`, held until its decision is published, then checks again that the launch is undecided; and right before it stops a run, it checks that once more, so a launch never stops the run its own id started from another executor. The lock's holder removes the file once the launch is decided. The daemon sweeps one an hour old that nobody holds, which a crash left, and decides that launch `expired` first when it is still undecided. A lock file is only ever removed once its launch is decided, so an executor that opened the old file, or creates a new one, finds the decision under its lock and stops.
2. Under the lock, it reads the config once, which gives the service, its environment, the daemon's ports and the service's URL, loads the launch profiles once, and checks a launch with an id against that loaded definition: `--launch-expires` has not passed; with `--expect-rev`, the `configRev` of that definition is the expected one; with `--remote-launch`, the profile is allowed (see "What a control may name"); the profile selected exists and, when dangerous, is confirmed. These checks run even inside a PTY, where a dangerous profile could otherwise be confirmed by hand. It then runs from that same definition, never from a second read.
3. It publishes its decision in the launch record (below): `admitted`, or `refused` with its code. When it finds the record already decided as `expired`, it does not start.
4. Still under the lock, it writes the run record atomically (a temporary file renamed over it). Every read-modify-write of a record (registration, heartbeats, exits) holds a short exclusive `flock` on `services/<record name>@record.lock`, and a change for one run is dropped once the record names another, so a late finisher never overwrites the next run. A failure to write is fatal: nothing starts, and the launch record gets an exit that says so.

Dispatchers pass `--launch-id <id> --launch-expires <unix ms>` (as `--launch-id=<id>`: a base64url id may start with `-`), and the daemon or the bridge, for a remote control, `--remote-launch <id>` in place of `--launch-id`, with `--expect-rev <configRev>` and `--request <machineId>:<requestId>`. A launch expires 15 seconds after it is dispatched. For a local launch, the app's restart and `dev --detach --replace`, the stop phase of a replacement runs before that budget starts: the dispatcher stops the old run and waits for its end, then sets the expiry, so a service slow to stop never makes its own restart expire. A remote launch's expiry is fixed by its dispatcher and never extended: a remote restart stops the old run before it dispatches (see "Who acts"). `--expect-rev` is compared with the revision of the effective default profile, as the inventory computes `configRev`; the profile the launch names is validated on its own, and `--expect-rev` takes no command after `--`. Whatever stops a launch with an id before its admission is published as its refusal: a service or environment no longer configured, a checkout that is gone, a config or `localify.toml` that does not load, a profile that does not exist or a dangerous one not confirmed, a target off loopback, a taken port all refuse it `conflict`, with a message that says which. `--remote-launch` only adds restrictions, so it needs no authentication. A person's own `localify dev` takes the lock the same way, so a foreground run is fenced too.

### Launch records

Each launch id has one durable record, `services/launches/<launchId>.json`, decided once, by whichever side decides first, and never seen half written.

- **Publishing:** the decider writes the complete JSON to a temporary file of its own in the same directory, named with 128 random bits and created only if no file has that name, `fsync`s it, then `link(2)`s it to the record's name. `link` fails with `EEXIST` when a decision already exists, so a winner is never replaced. The temporary file is then unlinked; a writer only ever removes a temporary file it created. The decision counts once the directory is `fsync`ed: the decider returns only after that, and a side that finds a decision syncs the directory before it obeys it. Before that, the records' folder and the two holding it, `services/` and the state folder, are synced in their parents, by every writer and on every publication, whoever created them: a folder another process just made may not be synced yet. A detached executor's log, `<launchId>.log`, is created through the same step. Readers ignore temporary files, and the daemon sweeps those older than an hour, which a crash between writing and linking can leave, here and in each run's `services/output/<runId>/` folder.
- **The executor,** after its checks and before running anything, publishes `{"outcome": "admitted", "runId", "service": {"projectId", "environment", "serviceId"}, "executor": {"pid", "start", "sid", "bootId"}, "requestedBy", "at"}`, or `{"outcome": "refused", "code": "conflict" | "expired" | "refused_profile", "message", "at"}`. `service` is the service it runs, by which a reconciler finds its run record; `environment` is its environment's slug, the primary checkout's included. `start` is in the boot's monotonic time, and an executor from another boot is dead (see "The end of a run"); `sid` is `null` when the executor did not lead its session. `at`, in every record and exit, is in Unix milliseconds.
- **The reconciler** (the daemon, or the app for its own launches), once the expiry has passed with no record, publishes `{"outcome": "expired", "at"}`.
- **The side that loses** finds the record and obeys it. An executor that finds `expired` exits without starting, so a launch delayed between its expiry check and its admission cannot start after its dispatcher gave up.
- **An admitted launch** keeps its record after the service's current record moves on to another run. When the run ends, its finisher adds `"exit": {"code", "signal", "reason", "at", "replacedBy"}`: under a lock of the records' directory (`exits.lock`), it writes the whole record to a temporary file, `fsync`s it, renames it over the record, which changes nothing but the exit, and `fsync`s the directory. The first exit stands; the executor, which knows the most, records its own before its run record's. `requestedBy` is `{machineId, requestId}` for a remote control, from `--request`, and `null` otherwise.
- **The exit:** `code` is the service's exit code and `signal` the signal that ended it, each `null` when it does not apply or is unknown. `reason` is `null` when they say how it ended, and otherwise one of: `interrupted_before_registration` (the executor died between its admission and its run record; nothing started); `registration_failed` (its run record could not be written; nothing started); `not_started` (something failed after the admission and before the service started, such as a profile source); `stopped` (the executor stopped the service: Ctrl+C, `localify stop`, a restart); `max_duration` (the launch profile's `max_duration` elapsed); `executor_lost` (the executor died before it recorded how its run ended, so another finisher recorded the exit, with no code). `replacedBy` is the launch that took the service over from this run in a restart, recorded by that launch under the run lock once it has published its own decision, and `null` otherwise.
- **Admitted, never registered:** an executor can die between its admission and its run record. Before it starts its first profile helper, it creates its run's folder, `services/output/<runId>/`, and right after starting each helper it writes there, in `executor-alive`, when it was last alive, in the boot's monotonic time: that proves the helper, and what it starts, members of the launch's session (see "The end of a run"). A reconciler that finds `admitted` and no run record with that run id checks the executor's identity. Alive, the launch is `starting`. Dead, it runs the session cleanup of "The end of a run" on the recorded session id, with the identity-checked finisher in that folder, removes the folder, then adds the exit with reason `interrupted_before_registration`. Without the folder, nothing ran there. Only the witness proves a process of that session the launch's: when the first scan finds processes in it of which none started by the last witness (a helper the executor died before witnessing, or what a helper started after the last witness, once that helper died), the reconciler signals nothing, keeps the folder, records no exit, and reports them as for an unconfirmed run ("process <pid> in this run's session could not be confirmed as part of it; stop it yourself, then try again."). The launch gets no exit while they live, and the next reconcile tries again.
- **Registered, then lost:** a run its service's record names with no exit, whose executor is dead, is completed by the reconciler or the daemon's pass: the identity-checked finisher ends what is proven left of it and finalizes it unless it already was, and the exit is recorded with `executor_lost`. A run never reads `running` once its executor is gone.
- **Retention:** records not changed for 24 hours are removed beyond the newest 100, by later launches and by the daemon, never one whose run its service's record still names, nor an admission still without its exit: the daemon's pass reconciles each such admission whose executor died, every 10 seconds, and a suspended executor's stays. Each removal is decided again under `exits.lock`, from the record as it is then.

From a record, a reconciler reports exactly one of: admitted and starting; admitted and running; admitted and exited, with its code or reason; admitted and replaced; refused, with its reason; expired, not started. "Unknown" exists only while no record exists and the expiry has not passed.

### Request entries

Every control from a peer leaves a request entry, `services/requests/<peer machine id>/<requestId>.json`, published like a launch record before the target takes any step that changes something: before a dispatch, a plain stop, or the stop phase of a restart. It holds an immutable fingerprint of the control (its action, project, environment, service, profile and expected run and revision), and then links every outcome as it happens: the launch it dispatched (admitted, refused, expired, exited), and `stopped: {runId, at}` for a stop. Updates go through a temporary file renamed over the entry and never change the fingerprint. Entries are kept for 24 hours, so a stop's correlation survives a daemon restart as a start's does. A peer creates at most 20 entries in 10 minutes: past that, a control with a new `requestId` is refused `too_many`, with how long to wait, before the target asks for its lease, and leaves no entry. A publication looks at its own name only; entries past their 24 hours are swept by a pass at most every 10 minutes, which reads 512 directory entries at most, each peer's folder counting as one, in the order the file system lists them, and goes on where the last one stopped; nothing is listed whole or sorted. Before it links a launch, the entry of a control the target still works on (a stop, or a restart's stop) reports `unknown`, and the viewer asks again. One a daemon restart left without an outcome and without a launch reports `failed`, and the viewer looks at the fresh snapshot: a restart whose stop was recorded says what it stopped, and only a stop is `done` once stopped.

### Target checks

In order; the first that fails answers the control.

Current authorization comes first:

1. The session's peer is pinned, and `hello` has been exchanged.
2. `share` and `control` are on.
3. A fresh lease: the target calls `GET /v1/remote/lease` now, and the viewer's machine id is among its active machines (see "Entitlement"). A failure refuses with `unauthorized`: the target fails closed.

Before it asks for that lease or reads anything, the target admits the control: a viewer has at most 4 controls and `control.status`es going on it, and all viewers together 16, past which it answers `busy` with `retryAfter` at once. They count until their work is over, the lease and the execution included, whatever becomes of the session: a viewer that reconnects or opens another session finds them still counted. Unpairing the viewer here, or pairing it again with another key, cancels what it asked that is not dispatched yet, a lease still to come included; right before it dispatches, the target checks again that the viewer is pinned with the key its session authenticated, and its switches. Only a control already dispatched finishes after an unpairing.

Then an existing request:

4. When an entry exists for this peer and `requestId`, the control is not run again. With the same fingerprint, it gets that entry's current report; with a different one, `request_mismatch`.

Then the checks of a fresh execution:

5. `expiresAt` has not passed in the target's clock and is at most 60 seconds ahead. The viewer learns the target's clock from its `hello`.
6. The ids name a configured service; the profile, when given, is allowed.
7. No other control of this service is in flight on this target, or `busy`. The daemon runs one at a time per service.
8. The state matches: for a start, no running run and a free port; for a stop or a restart, the running run is `expect.runId`. For a start or a restart, the service's `configRev` is `expect.configRev`; a stop ignores it. Otherwise `conflict`, with the fresh snapshot sent right after.
9. The owner allows it (below). For a restart, the replacement start must be allowed too: the owner is still there, the profile is still permitted, and a replacement that would run in the background has `backgroundStarts`. All of it is decided before anything stops.

The executor checks the expiry, the revision and the profile again when the launch reaches it, so a change between these checks and execution refuses the launch. When step 4 finds no entry, the target publishes one before step 5, so every outcome from there on, a refusal included, is linked to it, and nothing changes before it exists. A viewer that tries again after a refusal sends a new `requestId`. `control.status` asks for an entry's report without sending a control. That is correlation, not exactly-once: what keeps a restart repeated under a new `requestId` from running twice is `expect.runId`, which no longer matches once the first restart replaced the run.

### Who acts

| Action | Owner now | App on the target | What the target does |
|---|---|---|---|
| start | none | running | bridge `launch`: the app opens a new service tab on a fresh run PTY |
| start | none | absent, `backgroundStarts` on | a detached start (below) |
| start | none | absent, `backgroundStarts` off | refuses `background_disabled` |
| start | `app` or `background`, running | — | `conflict`: the viewer thought nothing ran |
| stop | `app` or `background` | — | finishes the run: signals its session and process group, then "The end of a run" |
| restart | `app` | running | stops it as above, then bridge `launch` with `replaces`, so the new run PTY takes the same tab |
| restart | `app` | absent | refuses `owner_gone` |
| restart | `background` | — | refuses `background_disabled` unless `backgroundStarts` is on; otherwise stops it, then starts it detached |
| restart | any | — | refuses `refused_profile` when the run's profile is now dangerous, unknown or removed |
| restart, with `expect.legacyId` | a legacy run from the app or `dev --detach`, its PID proven | — | relaunches it, as the row of its owner above says (below) |
| any other | a legacy run | — | refuses `legacy_run`; it keeps running |
| any | `terminal`, `unknown` | — | refuses `not_owned`; the status stays visible |

A restart keeps the run's profile. Its stop waits until the old run has ended, every process gone and its lock released, before the new launch is dispatched.

**Relaunching a legacy run.** A restart whose `expect.runId` is `null` and whose `expect.legacyId` is the one the inventory showed names a legacy run. It follows the restart rows of its owner exactly: a legacy run of the app needs the app running, which opens a new service tab (there is no run PTY to take over), or the target refuses `owner_gone`; a background one needs `backgroundStarts`. A legacy run in a terminal or of an unknown owner has no `legacyId` and is never relaunched. The target refuses `conflict` when the service's record no longer names that `legacyId`, and `legacy_run` when its PID is no longer proven. Every check, the replacement's included, passes before anything stops. The profile is the one the record names, and it must still be allowed. The target checks the identity again right before it signals, stops the run as `localify stop` stops a legacy run, and waits until the process is gone. It then dispatches the start with `--replace-legacy <legacyId>`, through the bridge's `replacesLegacy` or the detached command line. Under the lock, the executor refuses `conflict` unless the record still names that legacy run, so a local launch that replaced it in between wins. The command is the service's configured one. A command typed after `--` when it was first started is not kept, and the viewer's confirmation says so. `legacyId` is part of the request's fingerprint, and the entry records `stopped: {legacyId, at}`.

**Detached starts.** The daemon resolves its own executable once at start, canonicalized, and never looks `localify` up on `PATH`. It resolves the shell environment with the app's resolver (see "Output"), cached until the daemon restarts or the app's `Reload shell environment` asks it to resolve again; on failure it uses its own environment and says so in `remote_status`. It then spawns the executor itself, `<that path> --config <path> dev <service> --project <id> [--env <slug>] [--profile <name>] --remote-launch <id> --launch-expires <t> --expect-rev <rev> --request <machineId>:<requestId> [--replace=<runId> | --replace-legacy=<legacyId>]`, directly with that environment and `LOCALIFY_SERVICE_ORIGIN=background`, so no argument passes through a shell. There is no `dev --detach` in between: the process the daemon spawns is the one that takes the service's lock, and the daemon knows its PID and session. Stdin is `/dev/null`, stdout and stderr go to the launch's log, `<launchId>.log`, the service's output to the run's stream, and the process gets its own session. Startup is bounded: once the launch has expired, the daemon creates the `expired` record if none exists and, unless the launch was admitted, kills every process of that session and reaps the executor, so nothing of it, its lock included, is left. A run that started then has 90 seconds to listen.

### The app bridge

The desktop app listens on `<data dir>/app-bridge/app-bridge.sock`, a Unix socket only its owner can open, while it runs. The app makes the `app-bridge` folder `0700` and its own, binds the socket `0600` in it, and removes the socket when it exits. It never changes the data dir's permissions, since a layout can share the data dir: on Linux without systemd it is `/etc/localify`, owned by root. Where the app cannot make the folder, or finds one that is a link or another user's, the bridge is unavailable on that layout. The app logs it once at start and runs without it, the app counts as absent for remote starts, and `localify dev --in-app` falls back to the `launch-service` deep link. The app holds a lock on `app-bridge/app-bridge.lock` for as long as it runs. A second app finds it held, knows it does not own the bridge, and leaves it alone. Under that lock, a socket left by a crash is removed only when connecting to it is refused (`ECONNREFUSED`), and then bound again. A socket another app still answers on, a connect that fails any other way, or anything that is not a socket is left alone, and the bridge stays off. One JSON request per connection, one JSON answer, as on the management socket. Each answer line is 16 KiB at most. A refused connection or a missing socket means the app is absent, and so does a `hello` not answered within half a second. That half second covers the connect, the request and the answer together, and so do the 5 seconds of a `launch`. The daemon never asks on a session's behalf: a task of its own probes the bridge every 2 seconds, and `machine.app` and `app` in a target's `hello` give what it last saw. The bridge does not authenticate its clients: any process of the same user can hand the app a launch, just as it could run `localify dev` itself. The Unix user is the trust boundary (see "Security considerations").

| Request | Answer |
|---|---|
| `{"cmd": "hello"}` | `{"ok": true, "data": {"version": "…", "bridge": 1}}` |
| `{"cmd": "launch", "launchId", "expiresAt", "service": {projectId, environment, serviceId}, "profile", "expectRev", "replaces", "replacesLegacy", "requestedBy": {machineId, name, requestId}}` | `{"ok": true, "data": {"accepted": true}}` once the run PTY is spawned, within 5 s |


`replaces` is the run id whose tab the new run takes, or `null` for a new tab; its dispatcher stopped that run first, and the app passes it to the executor as `--replace=<runId>`, so the new launch records that it replaced it (`replacedBy`) and starts nothing when someone else restarted the service in between. `replacesLegacy`, optional, is the `legacyId` of a relaunch, never sent with `replaces`: the app passes it to the executor as `--replace-legacy` and opens a new tab. `requestedBy` and `expectRev` are both set for a remote launch and both `null` for one of this machine's own, such as `localify dev --in-app`, which the app runs with `--launch-id`. The app refuses before it spawns anything, and only then: `bad_request` for a malformed request, an unknown field included, or an expiry more than 60 seconds ahead; `expired` once `expiresAt` has passed; `unknown_service` when the ids name nothing here. Whatever stops the spawn itself is `conflict`, with its message. A dispatcher whose launch got no answer within 5 seconds, or lost the connection first, never sends it again and starts nothing in its place: it waits until the launch has expired, creates the `expired` record if none exists, and reports from the record (see "Reconciling"). Access requests do not use the bridge: they reach the app on `remote_events` (see "Asking for access"). The app spawns `localify dev` with `--remote-launch`, `--launch-expires`, `--expect-rev` and `--request` from the request, with the resolved shell environment, so the executor fences it and records who asked. Errors are `{"ok": false, "error": {"code", "message"}}`, with the codes of "Errors". The app opens the tab without taking focus and shows "Started by A" for 5 seconds. Stops need no bridge: the target signals the run itself, and the tab shows the exit. The app's own launch button and `dev --detach` pass launch ids the same way, and so does the app's restart, with `--replace=<runId>` for the run it stopped. `localify dev --in-app` uses the bridge when the app answers, keeping the deep link only to open an app that is not running (`localify://open/projects`), after which it waits up to 30 seconds for the bridge to answer and launches through it. Where the bridge is unavailable, it sends `localify://action/launch-service` as before the bridge, and waits for the run the app records.

### State machine

On the target, for one control:

```text
received ──▶ checked ──▶ matched ──▶ dispatched ──▶ started ──▶ settling ──▶ done
                │           │            │                         │
                ▼           ▼            ▼                         ├──▶ failed: the run exited
             refused     conflict   reconciling                    └──▶ starting: not listening after 90 s
          expired, busy             (no ack, bridge closed, app quit)
                                         │
                                         ▼  once the launch has expired
                              admitted | refused | expired, not started
```

- **Dispatched:** for a start or the second half of a restart, the launch is handed to the bridge or spawned detached, with its id and expiry. For a stop, the signal is sent; a stop is `done` once the run has ended (see "The end of a run").
- **Started:** the launch record says `admitted`. `refused` there ends the control with its code (`conflict` for a revision that changed, a run still alive, or anything else that stopped the launch before its admission, with its message; `expired`, `refused_profile`).
- **Settling:** the run is `done` when it listens on its port within 90 seconds, counted on the target's monotonic clock from the launch's admission. That timer is separate from the cleanup after an executor dies. The launch's record comes first: once it says the launch ended, another process listening on the port does not make it `done`. It is `failed` when it exits first, with its last 40 lines in the message, as many of the newest as fit in 16 KiB once JSON-escaped, and "[earlier output cut]" when some did not, so the result always fits in a session message. While `share` is off, the result carries no line. After 90 seconds without either, it is `starting` with "Started on B, not listening yet". The target answers `unknown` only while nothing is decided: a launch has no record and its expiry has not passed, or it still works on the request. A daemon restart in the middle of a request leaves `failed`, never `unknown` (see "Request entries").
- **Reconciling:** when the bridge drops, does not acknowledge within 5 seconds, or the app quits, the target does not know whether the app spawned the run. It never falls back to a background start for a launch it handed to the app. It waits until the launch's expiry has passed, creates the `expired` record if none exists, and answers from the record, after which no run with that id can start: admitted and starting or running (go on settling), admitted and exited (`failed`, with its code or reason), admitted and replaced (`conflict`: another launch replaced it), refused (`refused`, with its code), or expired (`failed`: "The Localify app on B closed before starting web. Nothing was started.").
- **Progress:** `control.progress` is sent at `dispatched` and `started`, so the viewer can say "Starting in the app on B…".

On the viewer:

```text
idle ─▶ sent ─▶ (progress) ─▶ result: done | starting | refused | conflict | failed
          │
          └─ session lost, or no result in 120 s ─▶ unknown ─▶ control.status, every 2 s ─▶ result | lost ─▶ snapshot ─▶ idle
```

- **Lost acknowledgement:** the viewer shows "Outcome unknown. Checking B…", opens a new session when it can, and asks `control.status` with the control's `requestId`, which reports exactly what happened to it. While B cannot be reached or answers `unknown`, it asks again every 2 seconds, for 2 minutes at most; `unknown` is never where it stops: past 2 minutes it is `lost`. Then it fetches the fresh snapshot. It never sends the control again by itself.
- **Queued controls:** the viewer sends a control only on a session already open, whose `hello` came, and sets its `expiresAt` as it sends it. A target clock so far ahead that the expiry cannot be written (past the year 9999) refuses the control `bad_request`, and nothing is sent. A control the person asked for while the session was down is not queued: the daemon refuses it at once with the connection state, and the button is disabled with it. `control.status` may open a session.
- **One at a time:** the viewer allows one control per service at a time; the target's per-service queue and the run lock enforce it across viewers and local launches.

## Cloud

### API

Added to the API of public-urls.md, with the same authentication, scoping and error shape:

| Method and path | Body | Response |
|---|---|---|
| `GET /v1/machines` | — | `{machines: [RosterMachine]}`, the account's active machines, this one included |
| `PUT /v1/machines/self/remote-key` | `{publicKey}` | `204`, also when the machine already has this same key; `409 remote_key_set` when it has another one; `409 remote_key_taken` when another machine has this one, a revoked one included; `403 quota_exceeded` past 10 linked machines; `422 invalid_key` |
| `DELETE /v1/machines/{id}` | — | `204`. Signs that machine out as `POST /v1/auth/logout` does for the caller, and pushes `revoked` to the relay. Works at zero entitlement. On the caller's own id, it is a sign-out. |
| `GET /v1/remote/connect` | WebSocket upgrade | see "Relay", "Connecting" |
| `GET /v1/remote/lease` | — | `{ttl: 60, machines: [id]}`: a lifetime in seconds and the account's active machine ids. `402 subscription_required`; `403 banned`. |

```ts
type RosterMachine = {
  id: string; name: string; createdAt: string;
  remoteKey: string | null;       // base64url, null until uploaded
  self: boolean;
};
```

A lease carries no absolute time. The client counts `ttl` from when it sent the request, on its monotonic clock, so a skewed local clock can neither extend nor shorten it (see "Entitlement"). `GET /v1/me` reports `plan.limits.remoteAccess`. Whether a machine is online is never in the API: only the relay knows, through `presence`.

### Data (D1)

Migration `0006_remote_access.sql`, after `0005_billing_follow_up.sql`:

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

`wrangler.jsonc` gains a Durable Object binding `RELAY` for the class `RemoteRelay`, exported from `src/relay.ts`, with a `new_sqlite_classes` migration. It never touches its storage. What it offers, to the Worker only:

- `fetch(request)`: the upgrade, after the Worker's checks. It is `fetch` rather than an RPC method, since a stub's `connect` is its TCP socket method;
- `revokeMachine(machineId)`: closes that machine's socket with `4401` and sends `revoked` to the others;
- `revokeAccount(revision)`: a billing refresh at that revision found the account without remote access. The push may arrive after a newer refresh gave access back, so the object reads the account and its billing revision again first. It does nothing when the account has remote access now, or when a newer revision decided. Otherwise it closes with `4402`, or `4403` for a banned account, the sockets last authorized before that read: a socket authorized since was checked after it. Removal and sign-out stay unconditional, through `revokeMachine`.

New configuration: two rate-limit bindings, `REMOTE_IP_LIMITER` (30 per 60 seconds, keyed by client IP) and `REMOTE_CONNECT_LIMITER` (6 per 60 seconds, keyed by the token's SHA-256), both load shedding as described in "Connecting". Nothing else: the relay needs no secret, since it authenticates nothing itself.

## Entitlement

`remoteAccess` is its own entitlement, a boolean next to `servingMachines` and `publicUrls` in PR #56's `Entitlement`. It is never inferred from tunnel quotas: an account can have remote access and no serving slot.

- **Pro:** an entitled Pro plan, per the lifecycle table, gives `remoteAccess: true`. Add-ons change nothing. There is no free tier and no trial apart from Pro's.
- **Override:** an override replaces the plan entirely, so it carries `plan_overrides.remote_access`. `cloud:admin override` sets it to 1 unless given `--no-remote-access`, and `show` prints it.
- **Ban:** none. **No plan:** none.
- **`/v1/me`:** `plan.limits` becomes `{servingMachines, publicUrls, remoteAccess}`. `plan.entitled` keeps its meaning (serving), so the public URL paywall does not move. PR 2 updates public-urls.md's `Plan` and "Plans" with it.
- **Linked machines:** at most 10 per account. An account with machine add-ons may sign in more than 10 machines, but no more than 10 of them link: serving capacity never raises the remote cap.

**Leases.** An endpoint holds a lease while it has a session open, as viewer or target, and renews it every 45 seconds with up to 5 seconds of jitter. A lease lasts `ttl` seconds from the moment its request was sent, measured on the monotonic clock; the endpoint never compares a time from the cloud with its wall clock. The monotonic clock may not count sleep (`Instant` on macOS does not), so a detected wake (the wall clock moved more than a minute past the monotonic one) expires the lease at once. Sessions then end until a renewal succeeds. A refused renewal, a lease that has run out, or a peer missing from the lease's `machines` ends the session with `bye` `lease_expired`.

**Losing it.** When the account loses `remoteAccess`, through a lapse, a ban or an override that ends, or a machine is removed or signs out:

1. New operations are refused: the connect route answers `402` or `403`, and each lease request too.
2. The Worker pushes at once where it makes the change: a billing refresh that leaves the account without `remoteAccess` calls `revokeAccount` with its revision, which reads the account again and does nothing when no socket is open or access came back; sign-out, roster removal and a same-name sign-in that replaces a machine call `revokeMachine`. Open sessions and output follows end with their sockets.
3. Where the Worker does not see the change (an override expiring, the end of a canceled period or of the `past_due` grace, a `cloud:admin` command, which writes D1 only), two lazy checks take over. The relay checks the sender and the recipient of each frame once their `authorizedUntil` has passed, and fails closed when D1 does not answer. Each endpoint's lease ends its sessions when renewal fails. An open output stream keeps sending frames, so it stops within about a minute. An idle socket may stay open longer, relaying nothing.
4. Before each control, the target asks for a fresh lease (see "Target checks").

Nothing local stops. Services keep running, the target's settings and pins are kept, and the app shows "Remote access needs Localify Cloud Pro" with the plan's action. ADR 0027 still applies separately to public traffic. Roster removal, sign-out, `/v1/me` and billing keep working without the entitlement.

## Daemon

The daemon is the only relay endpoint on a machine, for both roles: one key, one connection, one send queue, one lease. The app, the CLI and the MCP server view other machines through it.

New management socket commands. The streaming ones keep the connection open and send one JSON line per event until the client closes it, which ends the watch or follow:

| Command | Answer |
|---|---|
| `remote_status` | `{relay: "off" \| "connecting" \| "connected" \| "reconnecting" \| "blocked", blocked: {code, message} \| null, retryAt, settings, fingerprint, shellEnvironmentError, machineId, account}`; `shellEnvironmentError` says why background starts run with the daemon's own environment, `null` otherwise; `machineId` and `account`, the account's user id, while connected, `null` otherwise |
| `remote_reload_shell_environment` | `{shellEnvironmentError}`, once the daemon read the user's shell environment again for background starts; the app sends it on `Reload shell environment` |
| `remote_machines` | the roster merged with pins, presence and sessions: `[{id, name, self, paired, state, version, fingerprint, proto}]`, `state` as in "Desktop app"; `proto` is the session version agreed from the authenticated `hello`s of the open session, `null` with no session |
| `remote_settings {share?, control?, backgroundStarts?}` | the settings after the change |
| `remote_pair {machineId, attempt?}` | streaming: `code`, then `paired`, `cancelled`, or `partial` with `{local: "pinned" \| "not_pinned", peer: "pinned" \| "not_pinned" \| "unknown"}` (see "Partial results"); `attempt` only correlates the SSH helper's lines, and comes back on every event |
| `remote_pair_accept {machineId, account}` | streaming, for `--accept --for-ssh`: `{event: "ready", machineId, account, attempt}` once a one-shot acceptor for that account and that initiating machine id is registered (see "Pairing over SSH"), then `code` with its `pairingId`, answered with `remote_pair_answer`, then `paired`, `cancelled` or `partial`. A wrong account, this machine's own id, a pairing already active or the hour's limit is an error instead of `ready`. Closing the stream cancels the acceptor, or the pairing it started |
| `remote_pair_answer {pairingId, accept}` | `{ok}` |
| `remote_unpair {machineId}` | `{ok}` |
| `remote_inventory {machineId}` | one snapshot, all pages |
| `remote_watch {machineId}` | streaming: `{event: "connecting"}`, `{event: "snapshot", machine, epoch, rev, truncated, items}` with every page of a revision, and `{event: "lost", code, message, retryIn}`, after which the daemon asks again by itself |
| `remote_output_read {machineId, runId, offset, maxBytes}` | one `output.data` |
| `remote_output_follow {machineId, runId, offset?, tail?}` | streaming `{event: "data", …output.data}`; the daemon acknowledges as the client reads. Without `offset`, from the last `tail` bytes (256 KiB by default). `{event: "ended", code, message}` when the follow stops before its run did: `gone` for a restart, a refusal, or a lost session |
| `remote_control {machineId, action, service, profile, expect}` | streaming: `{event: "sent", requestId}` once the send queue took it, `{event: "progress", phase}`, then `{event: "result", outcome, code, message, runId}`; the client waits up to 120 s, not the usual 2. Without a session to that machine already open, it is refused at once, `{event: "result", outcome: "refused", code}` with the connection state (`not_connected`, `connecting`, `not_responding`, `peer_offline`, `not_paired`…) and no `sent`. A session lost first, or no result in 120 s, gives `{event: "unknown", code, message}`, then the result `control.status` gives on a new session, asked every 2 s while it is `unknown` or cannot be had, for 2 minutes in all (lease, query and answer included), or `{event: "lost", code, message}` past that, or when the target stopped sharing (`not_shared`); closing the stream stops the asking; the control is never sent again. A control the session cannot send (too large) is refused `bad_request`, with no `sent` |
| `remote_access {machineId, wants}` | viewer side, streaming: `{event: "waiting"}`, then `{event: "result", outcome}` with the outcomes of `access.result`, `{event: "error", code, message, retryAfter?}` for `not_shared`, `busy`, `too_many` or `unsupported`, or `{event: "lost"}` when the session ended first; closing the stream sends `access.cancel` |
| `remote_access_suppressed` | target side: `[{machineId, name, since}]`, the peers whose requests are suppressed |
| `remote_access_restore {machineId}` | target side: lets that peer ask again; `{ok}` |
| `remote_access_answer {requestId, allow, suppress}` | target side, from the app's dialog: `{outcome}`, `granted` only when the request was still waiting and the settings unchanged (see "Asking for access") |
| `remote_events {access?}` | streaming: pairing requests and how they ended (`paired`, `pair_cancelled`, `pair_partial {local, peer, reason}`, `reason` `unconfirmed` when the peer runs a version without `pair.saved`), access requests (`access_request {requestId, machineId, name, fingerprint, wants, expiresAt}`) and their end (`access_closed {requestId}`), presence, revocations; the app subscribes while it runs. Access events go only to a client subscribed with `access: true`, one that shows the access dialog, and "no app is subscribed" in "Asking for access" means none of those. The app subscribes with it |

One local command serves this machine's own inventory: `inventory {have?}` answers the snapshot the relay will page (`schema`, `epoch`, `rev`, `pages`, `truncated`, `items`), or `{unchanged: true, epoch, rev}` when `have` names its revision. One snapshot is built at a time; a request that arrives during a build shares its result. Its `controls` follow the switches and the app as the relay pages do: `machine.app`, like `app` in a target's `hello`, says whether the app's bridge answered the daemon's last `hello`, asked every 2 seconds. The local `inventory` command's `rev` counts its items only; the relay pages carry the revision of "Epoch and revision", which the summary moves too. The hidden `localify inventory [--have <epoch>:<rev>]` prints it, for tests.

The daemon opens a session only when one of these asks for it, carries every request to one target over its one session, and closes it when the last one ends. `remote_machines` without a probe reads the roster from Localify Cloud at most every 30 seconds, so the app can poll it; a probe, a pairing, presence of a machine it does not know and a revocation read it at once. `public_refresh` also re-reads `/v1/me` for `remoteAccess`, so subscribing turns remote access on without a restart.

## Desktop app

- **Service tabs,** on every machine, change with PR 1, whether or not remote access is used. A service tab is the run, not a shell: it opens when the service starts, shows the service's terminal interactively, and when the run exits shows its exit code with `Restart` and `Open a terminal here`. Typing a command after the service stops now happens in a shell tab, which `Open a terminal here` or the terminal button opens. Stop signals the service instead of pressing Ctrl-C in the tab. While `share` is on, the footer of a service tab says "What you type here appears in this service's output." Ctrl-Z does nothing in a service tab. Services start with the shell environment the app resolved (see "Output"): `Reload shell environment` is in the terminal panel's menu, and a warning there says when it could not be read. A service still running from a shell tab of the previous version keeps running there, marked "Started before this update".
- **Machine switcher,** next to the Projects screen's title (the app has no sidebar), shown only while the account has another machine. Its header shows this machine's relay connection, which says whether anything about the others is known: "Remote access: connected", "Connecting…", "Reconnecting… (in 8 s)" with `Retry now`, or "Off" with the reason. While it is not connected, the targets below show no state of their own. Then "This Mac" first, and every machine of the roster with its name and a state, using T3 Code's distinct states plus Localify's own:

  | State | Shows |
  |---|---|
  | `offline` | "Offline", and "since 10:42" when the relay saw it leave |
  | `connecting` | "Connecting…": the session handshake is in progress |
  | `connected` | the name, with no badge |
  | `not_responding` | "Not responding": on the relay, but its answers stopped (see "Sessions", heartbeat) |
  | `incompatible` | "Update needed: B runs a version of Localify that can't talk to this one." with the version that needs updating |
  | `not_paired` | "Not paired" with `Pair` |
  | `key_mismatch` | "Key mismatch: don't trust this machine until you pair it again." with `Unpair` |
  | `not_sharing` | "Paired · not sharing" |
  | `signed_out` | "Signed out" with `Unpair` |
  | `unavailable` | "Remote access needs Localify Cloud Pro", with the plan card's action |

- **A remote machine's view** is the Projects screen of that machine, drawn like this Mac's and read-only: no editor, no domain or profile editing, no terminal. A banner says "You're viewing B. Its .localify URLs open only on B." Each project is a card like this Mac's: the primary checkout's services as domain chips, and each worktree folded under one line with its count of services that answer. A chip holds a dot (green once the service answers, amber while it starts, red after a failed exit, grey when stopped), the domain, the port, the public URL's globe, the launch profile, Start or Stop, and a menu with `Show output`, `Open public URL`, `Copy URL` and `Restart`. The dot's tooltip gives the owner ("In the app", "In the background", "In a terminal", "Unknown process") and the four states: Process, Port, Output, Public URL. The Start, Stop and Restart buttons follow `controls`. A control in flight shows its progress, and `unknown` shows "Outcome unknown. Checking B…".
- **Quick actions:** a control that is not allowed stays clickable and opens a small card that says why and what fixes it, instead of a disabled button:
  - `control_disabled` or `background_disabled`, on a target whose session agreed on protocol 2 or later (`proto` in `remote_machines`): `Ask B`, which sends `access.request` and shows "Waiting for an answer on B…" with `Cancel`, then "B allowed remote controls.", "B didn't allow it.", "Nobody can answer on B: Localify isn't open there.", or the end's reason. `background_disabled` asks for `backgroundStarts`. So does `control_disabled` when the control would run in the background on B: a start while B's app is absent (`machine.app` is `absent`), or a restart of a background run. `backgroundStarts` turns `control` on too, so one request is enough; asking for `control` first would get `background_disabled` next, and a second request within 10 minutes gets `too_many`. Otherwise `control_disabled` asks for `control`. Below it, always, "Or run this on B:" with the command for the same switches, `localify machines settings --control on`, or `localify machines settings --control on --background-starts on`, and a copy button. On a target of protocol 1, only the command shows, under "Update Localify on B to ask from here."
  - `legacy_run` on a run with `legacyId`, on protocol 2 or later, Output's "Relaunch to share" for the same run, and `Restart` of a run with `legacyId` even when `controls.restart` allows it: `Relaunch on B`, always after a confirmation: "web started before this version of Localify on B. Relaunch it so its output and controls work here? It restarts with its configured command." When controls are off, the card asks for access first, and shows "Or run this on B:" with the command, as `Ask B` does; once granted, it relaunches only if the snapshot still shows the same `legacyId` and `configRev` the person confirmed, and asks again otherwise. A request that ends without access (`nobody`, `declined`, an error) leaves its reason on screen with the same command, until the person closes it.
  - `legacy_run` with no `legacyId`, or on protocol 1: "Relaunch it on B." (or "Update Localify on B."), with no action. So are `not_owned`, `owner_gone`, `refused_profile` and the others: the message, with no action.
- **URLs:** a remote `.localify` URL is copied, never opened. Clicking the domain opens the output when it is shared, and copies the URL otherwise. When the service has a public URL that serves, its globe and `Open public URL` open it.
- **Output** opens in a panel docked at the bottom, sized, coloured and resized like the terminal panel, titled with the project and service, "on B", with search, Follow / Pause and selectable text (see "Output"). The terminal panel holds this Mac's PTYs only, so remote output stays out of it; opening one closes the other.
- **Access dialog,** on a target whose app runs: the dialog of "Asking for access". Settings → Remote access lists the machines whose requests are suppressed, with `Allow requests again`.
- **Settings → Remote access,** on every machine: the three switches with one line each ("Share projects and output with paired machines", "Allow paired machines to start, stop and restart services", "Allow background starts when the app is closed"), a note that output can contain secrets the services print and what is typed into service tabs, this machine's fingerprint, and the paired machines with `Unpair` and `Remove from account`.
- **Pairing dialogs,** with the wording of "Pairing".

All of it goes through `DesktopApi` and the management socket. The UI does no network.

## CLI

```bash
localify machines                          # the roster: name, id, state, paired, sharing, version
localify machines pair <machine>           # pairs with another machine: a code to confirm on both
localify machines pair <machine> --over-ssh <destination> [--remote-localify <path>]  # pairs over SSH, comparing both codes itself
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
- **Launch flags:** `--launch-id`, `--launch-expires`, `--remote-launch`, `--expect-rev`, `--request` and `--replace-legacy` are hidden flags for dispatchers, documented here and not in `--help`.
- **Output:** human on a terminal, JSON otherwise, as everywhere. A control prints its progress, then its result; an outcome it could not learn (`lost`) prints the fresh status and exits with code 3. A refusal prints the message of "Errors" and exits with code 2.
- `pair` needs a terminal, or `--over-ssh` on the initiator and `--accept --for-ssh` on the side it starts (see "Pairing over SSH" and ADR 0035), which lets an agent with SSH access to both machines pair them. `remove` needs a terminal: an agent cannot remove a machine.

## MCP

- `list_machines`: the roster with states, read-only.
- An optional `machine` argument on `list_services`, `service_status`, `service_logs`, `service_start`, `service_stop` and `service_restart`, and on no other tool. With it, a tool ignores `cwd`.
- With `machine`, `service_start` takes only `service`, `project`, `env` and `profile`. `background`, `env_files`, `confirm_profile`, `command` and `domain` make it fail with an error that says the target decides how its services start. It never creates a project.
- `service_logs` with `machine` returns a cursor `<runId>:<offset>`.
- Pairing, unpairing, settings and removal have no tool. Like signing in (ADR 0018), they stay a person's decision, except that an agent a person gave SSH access to both machines may pair them with `localify machines pair --over-ssh` (ADR 0035).
- An error from a target reaches the agent with its code and message, so `control_disabled` tells it to ask the user to allow controls on that machine, and an outcome it could not learn (`lost`) tells it to check `service_status` before trying again.

## Failure modes

| Situation | What happens |
|---|---|
| The relay restarts (deploy) | sockets close with `1012`; daemons come back over 10 seconds; sessions are new; a control without an answer is `unknown` on the viewer |
| Cloud unreachable | the relay connection backs off; sessions end once the lease runs out; controls are refused at once (`unauthorized`); local work goes on |
| D1 fails during the relay's check | the frame is dropped with `unavailable`; the sender ends that session, opens a new one after 5 s, and reconciles unanswered controls with `control.status` |
| This machine wakes from sleep | its leases expire at once; sessions end until a renewal succeeds; the relay connection is retried right away |
| The target sleeps or loses the network | the viewer shows `not_responding` within 15 s of a `keep`, then `offline` once the relay drops its socket |
| The target's daemon hangs while its socket stays up | `not_responding`; the session ends after 60 s |
| Versions do not overlap | "Update needed" before any session |
| A handshake never completes | both sides drop it after 30 s |
| The app quits while dispatching a start | the target waits for the launch's expiry, then answers from the launch record; never a background start in its place |
| The executor dies, its descendants live on | the finisher signals the session's members once one is proven to belong to the run, finalizes the stream, then shows "Exited"; no launch is admitted until then |
| The app or a background executor dies mid-run | the stream is finalized `interrupted` at what was written; the process cleanup goes on |
| An executor dies after admission, before registering | `starting` is never reported for a dead executor; a finisher cleans up its session and records `interrupted_before_registration` |
| A finisher dies while finalizing | the kernel releases its `finalize.lock`; the next holder repeats the idempotent steps |
| A run's session id is reused by an unrelated process, even after the wall clock moved backwards | no member started before `lastAliveAt` in the boot's monotonic time, so nothing is signalled |
| A run record from before a reboot | nothing is signalled; the run is finalized |
| A crash leaves a temporary launch or `end.json` file | readers ignore it, and the daemon sweeps it after an hour |
| A `requestId` comes back with another action | `request_mismatch`; nothing runs |
| A launch pauses between its expiry check and its admission | the reconciler's `expired` record wins, and the executor does not start |
| Ctrl-Z in a service tab | nothing: suspending is disabled, since no shell could resume it |
| The shell init prompts, hangs, fails, or prints malformed or oversized output | the resolver gives up (10 s deadline, 1 MiB cap, strict framing) and kills its process group; runs start with the app's environment and a warning |
| A service runs from a shell tab of the previous version | a legacy run: kept, no remote output, remote controls disabled |
| A launch reaches `localify dev` after its expiry | it refuses and writes a launch record with `expired` |
| The profile or the config changes between the check and the launch | the executor refuses with `conflict`; the snapshot follows |
| Two launches of one service race | the run lock lets one executor run; the other refuses |
| The app quits while its service runs | closing the run PTY hangs up the run, which usually ends it; if it lives on, stop works and restart is refused `owner_gone` |
| The viewer drops during a control | the target finishes it; the viewer shows `unknown`, then reconciles with `control.status` and the next snapshot |
| The same request arrives twice | the same `requestId` gets the current report of its launch, without running again; a restart repeated under a new `requestId` fails `expect.runId` |
| A restart of a background run without `backgroundStarts` | refused before anything stops |
| A background start fails to find a tool | `failed` with the last lines; the person fixes the shell config and reloads the shell environment |
| Output rotates while following | `rotated` with the new first offset |
| The service restarts while following | `gone`; the viewer follows the new run from 0 |
| A huge config | inventory pages, `truncated` past 32 |
| Snapshots and follows at once | the send queue paces them under the relay's budget |
| A peer floods | `rate_limited`, then `4429`; the target's own bounds on watches, follows and handshakes |
| The plan lapses | `4402`; "Remote access needs Localify Cloud Pro"; nothing local stops |
| A machine is removed from the account | `4401` on it, `revoked` to the others, which delete its pin |
| A same-name new sign-in | a new id, not paired; the old one is revoked and its pin dropped |
| The identity file is lost | "remote key is missing", until a new sign-in |
| The roster shows a key other than the pin | `key_mismatch`; no session |
| Clocks differ | controls use the target's clock from `hello`; leases use monotonic lifetimes; nothing else depends on clocks |
| Every daemon reconnects at once | jitter spreads them; the pre-auth guard and the limiters shed what still piles up, approximately and per location |

## Security considerations

- **The cloud is not trusted with content.** Inventory, output and controls are encrypted between pinned keys. The cloud decides who may connect and can deny service, but cannot read, forge or replay a message, and cannot pair a machine of its own without the code.
- **A stolen token** lets its holder connect as that machine and see presence and sizes, but not open a session: it lacks the private key. Removing the machine ends the token.
- **A paired machine is trusted with what its peer shares.** Each target decides with its own switches, off by default, and checks them itself on every request. The cloud's view of permissions is never consulted for that.
- **The management socket and the app bridge** are Unix sockets only their owner can open, so a local process can use them only as the user, who could already read the key. Neither authenticates a client beyond that: a process of the same user is trusted with them.
- **Output provenance** comes from the channel: a run PTY holds only `localify dev` and its service, so no shell's bytes can reach a stream, and shell tabs are never exported. What a person types into a running service tab is echoed into its output; the tab says so.
- **Output can hold secrets** that a service prints. Settings says so next to `share`. Output is never written to the daemon log, analytics or error reports, on either machine.
- **Untrusted text.** Output and names come from another machine. The viewer shows plain text with control characters and bidirectional overrides removed, never HTML or raw escape sequences.
- **No remote code.** A control names ids. It cannot carry a command, a path, a variable or a new project, and a dangerous profile is refused, by the target and again by the executor under `--remote-launch`. The worst a paired machine with `control` can do is start, stop and restart configured services on permitted profiles.
- **Revisions** are HMACs under a local secret, so a revision a viewer sees reveals nothing about inline profile values.
- **Replay and delay.** Noise nonces stop replays within a session, fresh ephemeral keys across sessions, `expiresAt` a delayed control, the launch expiry a late launch, and `expect.runId` a repeated one.
- **Key material in memory.** The daemon wipes its copies of the private key when it drops them: the decoded bytes, the encoded string read from `identity.json`, and the buffers it is decoded through (`zeroize`). `snow` 0.10, the latest release, keeps the static and ephemeral private keys of a handshake and the transport keys in plain arrays that it does not wipe, and has no option to. Those copies live only while a handshake or session does, in the daemon's own memory, which a process of another user cannot read. Reading it takes the access that reads `identity.json` already.
- **The pairing code** resists a relay in the middle through the commitment, at 10⁻⁶ per attempt, with attempts bounded by the target and by a person's click.
- **Metadata** stays visible to the relay: who talks to whom, when, and how much. The relay logs errors only, without payloads.
- **Denial of service** by a peer is bounded by the relay's limits and the target's own bounds.

## PR plan

Each PR ships only once its gate passes in CI, on top of its own unit tests.

1. **Local foundations.** Run identity (run ids, `--launch-id`); the run PTY in the app, with the shell-environment resolver and `Reload shell environment`, the exit view with `Restart` and `Open a terminal here`, Ctrl-Z disabled, and stop and restart by session and process group; one supervisor for every launch; the run lock in the executor and fatal atomic registration; native process identity, `lastAliveAt` and the end of a run (proven membership, interrupted capture, finalization locks); legacy runs; the per-run stream from run PTYs and background pipes, with bounded reads by `{runId, offset}`; `localify logs` and `service_logs` on it; removing the whole-shell copy and `LOCALIFY_SERVICE_OUTPUT`; the inventory projection with its four states and HMAC revisions. Gate: capture isolation, shell environment, concurrent launches, end of a run, migration.
2. **Trust and relay.** Keys, pins and settings files; Noise sessions, heartbeats and pairing with `snow`; the `RemoteRelay` object and routes, with the pre-auth guard and lazy checks; send pacing; migration 0006; `remoteAccess` in the entitlement, `/v1/me` and `cloud:admin`; leases and revocation pushes; reconnects; the management socket commands for status, roster and pairing; `localify machines`, `pair`, `unpair`, `remove` and `settings`; the cost measurement. Gate: pairing, replay, identity, leases, hibernation, reconnect storm.
3. **Desktop viewing,** the first useful release: the switcher, the pairing dialogs, Settings → Remote access, the remote Projects view, Output. Gate: revocation end to end.
4. **Remote controls,** for 1.6, in four PRs:
   - **4a, launch fencing:** `--launch-expires`, `--expect-rev`, `--remote-launch` and `--request` in the executor; launch records published with `link`, the reconciler, retention and sweeps. Gate: the fencing and crash items below.
   - **4b, the app bridge:** `app-bridge/app-bridge.sock` with `hello` and `launch`; `dev --in-app` on it; the app's own launches with launch ids. Gate: the desktop disconnect item.
   - **4c, the control executor:** session protocol 2 and its negotiation; request entries and `control.status`; the target checks, ownership-aware start, stop and restart, the legacy relaunch with `--replace-legacy`, detached starts, the state machine and reconcile; access requests in the daemon (`remote_access`, `remote_access_answer`, the events); the viewer's commands in `DesktopApi`. Gate: the reconnect, dropped frame, request entry, restart, convergence, version, access and relaunch items.
   - **4d, the app:** the buttons and their states, the quick actions and the access dialog. Access stays off in the app until 4d ships the dialog. Gate: the access dialog end to end, on two machines.
5. **CLI and MCP:** `--machine` on status, logs, dev, stop and restart; `list_machines` and the `machine` argument.

### Gates

Against a real daemon, a real app process where it says so, and the relay in `wrangler dev` or its test harness.

**PR 1, capture isolation, shell environment, concurrent launches, end of a run, migration:**

- A service in a run PTY prints known lines and exits; the test opens `Open a terminal here` and types `echo secret-after-exit` there. The run's stream holds the service's lines only, and no shell tab's bytes are in any stream or in `localify logs`.
- A suspended and resumed service with a delayed reader: the app's reader is held back by a test hook while the service receives `SIGTSTP`, a shell tab runs commands, and the service receives `SIGCONT` and prints more. Once the reader catches up, the stream holds exactly the run PTY's bytes and none of the shell's.
- Concurrent launches: the app's button pressed twice and a CLI `dev --detach` of the same service, at once, leave one run; the others refuse. A run record that cannot be written starts nothing.
- Keystrokes typed into a running service tab appear in its stream, as documented.
- Stop on a service that ignores `SIGINT` and has a child in its own process group: the whole tree ends, and no byte is written into the PTY.
- A read of a 1 GiB stream, and of a 1 GiB line, never holds more than `maxBytes` plus a constant.
- Shell environment: the app is started with a minimal environment (`env -i HOME=… PATH=/usr/bin:/bin`) and launches a service whose tool exists only through the test user's shell init, once each with zsh, bash, fish and Nushell. It runs. A shell init that hangs, one that opens `/dev/tty` and waits, one that prints more than 1 MiB, and one that prints a forged nonce or text inside the frame each end in the warning within 10 s, with the resolver's process group gone, the service on the app's environment, and no byte of the shell init in any stream.
- End of a run: a service whose descendants ignore `SIGINT` and `SIGTERM` and have not opened their port yet is ended with Ctrl-C typed into its tab, and with `SIGKILL` of the executor in a run PTY. Each time, every process of the session is gone, the stream is finalized `complete` after EOF, "Exited" shows only after that, a launch attempted in between starts only once the run is finalized.
- Interrupted capture: the same service, killed with `SIGKILL` of its executor in a background run, and with `SIGKILL` of the app during a run PTY. Each time, the descendants are gone, the stream is finalized `interrupted` at the prefix written before the kill, nothing waits for the lost bytes, and the next launch is admitted only after finalization.
- Session reuse: after a run ended, an unrelated process carrying the run's session id, started after `lastAliveAt` (through a test enumerator), is never signalled, also with the wall clock moved backwards in between. A run record with another boot id signals nothing.
- Finalization: two concurrent finishers never finalize at once; when one is killed mid-finalization, the other proceeds and completes it.
- Start times and heartbeats come from the native calls, in the boot's monotonic time, and no `ps` process is spawned.
- Ctrl-Z typed into a running service tab suspends nothing.
- Migration: a service started from a shell tab by the previous app version keeps running across the update, shows as a legacy run, exports no output (not even its old whole-shell file), and has its remote controls disabled with "Relaunch it on that machine to enable remote control." until a 1.6 viewer relaunches it (ADR 0033)
- Revisions: editing an inline `[profiles.<name>.env]` value changes the service's `configRev`.

**PR 2, trust and relay:**

- Pairing: a nonce sent before the commit, a duplicated commit, a reveal that does not match the commit, a handshake hash altered by a test relay that substitutes a key, a cancel on either side, and a 2-minute timeout each pin nothing when they happen before either `pair.confirm`; a session cut after one is reported `partial` with each side's verified state. The peer counts as pinned only once its `pair.saved` arrives: a 1.7.0 peer's `bye` `done`, after its timeout or a failed pin write, leaves it `unknown`. A pin of another key written during a pairing is never replaced. A test relay running two handshakes cannot make both codes match except by chance.
- Replay: a recorded session's frames replayed into a new session fail; a duplicated or reordered transport message ends the session; a forced nonce reuse is rejected.
- Identity substitution: a relay that rewrites `from`, a roster key that differs from the pin, a purpose rewritten in the envelope, and a same-name sign-in all fail to open a session.
- Handshakes: an incomplete one is dropped after 30 s; more than 4 at once get `busy`.
- Leases: a lease not renewed ends its sessions; a refused renewal ends them at once; after a simulated wake, sessions end until renewal succeeds; a wall clock moved by hours changes nothing else.
- Heartbeat: a target that stops answering while its socket stays up is `not_responding` within 15 s of a `keep`.
- Hibernation: with the object hibernated between frames, it routes correctly, re-checks authorization on both ends, fails closed on a simulated D1 error, and uses no storage or alarm.
- Dropped frames: a frame the relay drops ends its session on both sides; the sender opens a fresh one after the retry delay and resends nothing.
- Reconnect storm: 200 simulated daemons across 20 accounts are disconnected at once with `1012`. Upgrades reach the Worker spread over at least 10 seconds, malformed tokens never reach D1, every daemon is connected again within 60 seconds, and no control is replayed.

**PR 3, revocation end to end:** with the desktop app following a remote service's output, `revokeAccount` and `revokeMachine` end the stream within 2 seconds, and a D1 change with no push (an expired override) within 75 seconds, on screen.

**PR 4, control races and launch outcomes:**

- A desktop disconnect cannot double-start: a start is dispatched to an app killed before it spawns the run PTY, and once after. In both, at most one run exists afterwards, no background run was started, and the answer comes after the launch's expiry.
- A reconnect cannot repeat a restart: the viewer's socket is cut right after it sends a restart, and the test sends the same control again on a new session, with the same and with a new `requestId`. The service restarted once; the same `requestId` gets the report of the first restart, and the new one gets `conflict`.
- A dropped frame during a control: the relay drops the frame carrying a control's result; the viewer reconciles with `control.status` on a new session, gets the exact outcome, and never sends the control again.
- A late launch is fenced: a launch delivered to `localify dev` after its expiry refuses and starts nothing. With a test hook pausing the executor between its expiry check and its admission until the expiry passes, the reconciler's `expired` record wins and nothing starts.
- Crashes: a temporary launch file left by a `SIGKILL` before its `link` is ignored by readers and swept; an executor stopped with `SIGSTOP` between its admission and its run record is reported `starting`; one killed with `SIGKILL` there has its session cleaned up and its record completed with `interrupted_before_registration`.
- Request entries: a stop whose acknowledgement was lost is still correlated by `control.status` after a daemon restart; the same `requestId` sent with another action or service gets `request_mismatch` and runs nothing.
- A lost acknowledgement is reported exactly: launch A starts and exits, a local launch B then replaces the service's record, and the bridge's acknowledgement of A was lost. The target still reports A as admitted and exited, with its code.
- A profile changed between the target's check and execution, to dangerous or to other sources, is refused by the executor.
- A restart of a background run with `backgroundStarts` off is refused, and the run keeps running.
- Versions: two 1.6 peers agree on 2; a 1.5 peer and a 1.6 peer agree on 1 and no protocol 2 message is sent; a relay that alters `presence` to claim 1 for both changes nothing, since the `hello`s decide.
- Access requests: with the app running on the target, `Allow` turns `control` on and the viewer's next snapshot allows controls; `Don't allow` and 2 minutes without an answer change nothing. With the app absent, the answer is `nobody`. A request within 10 minutes of the last one, a sixth dialog in an hour, and a suppressed peer change nothing and show nothing; a second viewer's while one waits gets `busy`. An `Allow` that arrives after the request was cancelled, expired, or after `control` was turned off in the meantime changes nothing. A session that ends closes the dialog.
- A legacy relaunch: a proven legacy run restarted with `expect.legacyId` ends and is replaced by a run with a run id and a stream. A process that reused the PID, an unproven run, and a record that names another run are never signalled. With the app absent for an app run, or with `backgroundStarts` off for a background run, it is refused before anything stops. A local launch that replaces the record between the stop and the dispatch wins, and the relaunch gets `conflict`.
- External starts and config changes converge: while a viewer watches, the service is started from a terminal, stopped with `localify stop`, and its config edited. Within 12 seconds, the viewer's snapshot shows each state, and a control built on the old one gets `conflict`.

## Open questions

- **Bans reaching open sessions.** `cloud:admin` writes D1 only, so a ban reaches open sessions through the lazy checks and the lease, within about a minute, not at once. An authenticated admin route that calls `revokeAccount` would make it immediate, against the choice of having no admin route on the Worker.
- **The active time a frame costs,** and so the cost of a typical session; measured in PR 2.
- **Sharing by project** rather than by machine: v1 shares a machine's whole inventory or nothing.
- **A browser or phone viewer** would need its own keys and pairing, which v1 does not cover.
