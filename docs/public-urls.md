# Public URLs

The v1 contract for public URLs: what the cloud, the daemon, the CLI, the MCP server and the desktop app each do, and the messages between them. The decisions behind it are ADRs 0019 to 0023. The measurements that chose the transport are in `spikes/public-url/README.md` on the `feat/public-urls-spike` branch.

## What it does

A signed-in user exposes a local domain of one of their projects at `https://<slug>.localify.fyi`. With `--wildcard`, every environment of the project is reachable too, at `https://<slug>--<label>.localify.fyi`. That is the main use case: open from a laptop or a phone the dev servers an agent runs on another machine. Every URL asks for a password by default.

```text
browser ──https──▶ Cloudflare edge ──▶ Localify Cloud Worker (password, routing)
                                          │ fetch https://m-<machine>.localify.dev
                                          ▼
                              named Cloudflare Tunnel of the serving machine
                                          │ cloudflared on that machine
                                          ▼
                     daemon public ingress (127.0.0.1) ──▶ Localify router ──▶ dev server
```

V1 is invite-only: GitHub accounts on an allowlist. Out of v1: path exceptions (webhooks reach a protected URL only after the password), custom domains, temporary URLs, renaming a slug, several URLs per local domain, Docker or external domains, the online state of a URL served by another machine, request inspection, billing.

## Names

- **Slug:** 3 to 32 characters, `a-z`, `0-9` and `-`, starting with a letter, not ending with `-`, never containing `--`. Unique across all users, deleted ones included (see "Data"). When not chosen with `--slug`, it follows the local name, so the public URL reads like the local one. The local name is every label of the local domain but the last, in order, joined with `-`, lowercased, every run of characters outside `a-z0-9` replaced by one `-`, leading digits and `-` removed, cut to 32 characters without a trailing `-`: `louez.localify` gives `louez`, `web.louez.localify` gives `web-louez`. The Worker takes the first free of: the local name, the local name with `-` and the owner's GitHub login (`louez-ryan-ede`), then the local name cut to 27 characters with `-` and 4 random hex characters (`url` when the name is empty). A candidate that breaks the slug rules, such as one shorter than 3 characters, is skipped. A slug is free when no live URL has it and no other user's deleted URL holds it.
- **Public host:** `<slug>.localify.fyi`, or `<slug>--<label>.localify.fyi` for a wildcard URL. The first `--` separates the slug from the label, so labels may contain `--` (`web--feature`). The slug comes first so that `--` never sits at positions 3–4 of the DNS label, which IDNA reserves (ADR 0022).
- **Local host:** `<local-domain>` for `<slug>`, and `<label>.<local-domain>` for `<slug>--<label>`. A label is `a-z`, `0-9` and `-`, does not start or end with `-`, and is at most `61 − slug length` characters so the public DNS label fits in 63. Longer names cannot be served; the CLI says so instead of truncating.
- **Exposable local domain:** a domain registered by a project in this machine's config, or a project's namespace `<project-id>.localify` even when no service uses it as its own domain. The namespace is what a wildcard URL usually targets: in a monorepo, `<slug>--web--feature.localify.fyi` reaches `web--feature.<project-id>.localify`. The project of a public URL is the one that registers its local domain or owns that namespace.
- **Machine:** one signed-in installation of Localify. Its tunnel hostname is `m-<machine-id>.localify.dev`.

## Cloud (`apps/cloud`)

One Cloudflare Worker, deployed with wrangler, with a D1 database and an hourly scheduled cleanup.

- `api.localify.dev`: the JSON API for the CLI, the daemon and the app (Worker custom domain).
- `*.localify.fyi/*`: public traffic (Worker route; the zone has a proxied `*` AAAA record).

### Configuration

| Name | Kind | Value |
|---|---|---|
| `GITHUB_CLIENT_ID` | var | the OAuth App's client ID (device flow, no secret) |
| `ALLOWED_GITHUB_IDS` | var | comma-separated numeric GitHub user ids; logins can be renamed and reused, ids cannot |
| `PUBLIC_BASE_DOMAIN` | var | `localify.fyi` |
| `MACHINE_DOMAIN` | var | `localify.dev` |
| `CLOUDFLARE_ACCOUNT_ID`, `MACHINE_ZONE_ID` | var | account, and the `localify.dev` zone |
| `CLOUDFLARE_API_TOKEN` | secret | Tunnel Edit, DNS Edit (Infisical `localify` `/cloud`) |
| `COOKIE_KEY` | secret | signs access cookies |
| `INGRESS_KEY` | secret | derives each machine's ingress secret |

The allowlist is checked at sign-in, on every authenticated API request, and on every public request through the URL owner. Removing an id therefore revokes that person's tokens in effect and takes their URLs offline at once, without touching the database.

### Data (D1)

```sql
users       (id TEXT PRIMARY KEY, github_id INTEGER UNIQUE NOT NULL, login TEXT NOT NULL, created_at INTEGER NOT NULL)
machines    (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), name TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN ('provisioning', 'active', 'revoked')),
             tunnel_id TEXT, dns_record_id TEXT, ingress_port INTEGER NOT NULL,
             created_at INTEGER NOT NULL, revoked_at INTEGER)
tokens      (hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id),
             machine_id TEXT NOT NULL REFERENCES machines(id), created_at INTEGER NOT NULL)
public_urls (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), slug TEXT UNIQUE NOT NULL,
             local_domain TEXT NOT NULL, wildcard INTEGER NOT NULL, machine_id TEXT REFERENCES machines(id),
             protected INTEGER NOT NULL, password_hash TEXT, password_salt TEXT, password_version INTEGER NOT NULL,
             created_at INTEGER NOT NULL, deleted_at INTEGER,
             CHECK (protected = 0 OR (password_hash IS NOT NULL AND password_salt IS NOT NULL)))
CREATE UNIQUE INDEX one_url_per_local_domain ON public_urls (user_id, local_domain) WHERE deleted_at IS NULL;
```

Tokens are stored as SHA-256 hashes. Passwords use PBKDF2-SHA-256 with a random salt and 100,000 iterations. A user has at most 10 public URLs and 5 active machines; both limits are checked in the same statement as the insert, so two concurrent requests cannot both pass. Deleting a URL sets `deleted_at` and keeps the row for 30 days, so its slug cannot serve someone else's page behind old bookmarks; only its owner can take that slug again during that time. The hourly cleanup removes older deleted rows.

### API

Every request except the device flow carries `Authorization: Bearer <token>`. The token gives the user and the calling machine; the API never takes a machine id from the request. Every read and change is scoped to the token's user: an id that does not exist and an id owned by someone else both answer `404 not_found`. Errors are `{"error": "<code>", "message": "<sentence for a person>"}` with a 4xx or 5xx status. Field names are camelCase.

| Method and path | Body | Response |
|---|---|---|
| `POST /v1/auth/device` | — | `{deviceCode, userCode, verificationUri, interval, expiresIn}` |
| `POST /v1/auth/device/token` | `{deviceCode, machineName, ingressPort}` | `200 {token, user, machine}`; `428 authorization_pending`; `429 slow_down` with `{interval}`, the new polling interval; `403 access_denied` (the person declined on GitHub); `403 not_allowed`; `410 expired_token` |
| `GET /v1/me` | — | `{user, machine}` |
| `POST /v1/auth/logout` | — | `204`, see "Signing out" |
| `GET /v1/public-urls` | — | `{publicUrls: [PublicUrl]}`, the user's URLs, deleted ones excluded |
| `GET /v1/slugs?localDomain=<domain>` | — | `{slug, url}`: the slug a new URL for that domain would get, and its URL, so clients never write the public domain themselves. `422 invalid_local_domain`. |
| `GET /v1/slugs/<slug>` | — | `{slug, available}`, the slug lowercased; free for the caller as defined in "Names". `422 invalid_slug`. |
| `POST /v1/public-urls` | `{localDomain, wildcard, slug?, protection: {mode: "password", password?} \| {mode: "public"}}` | `201 {publicUrl, password?}`. The calling machine serves it. `password` is present only when the server generated it. `409 slug_taken`, `409 local_domain_taken`, `422 invalid_slug`, `422 invalid_local_domain`, `403 quota_exceeded`. |
| `DELETE /v1/public-urls/{id}` | — | `204` |
| `PUT /v1/public-urls/{id}/protection` | `{protection}`, the same object as in `POST` | `{publicUrl, password?}`. Changing or removing the password signs every browser out. |
| `POST /v1/public-urls/{id}/claim` | — | `{publicUrl}`, now served by the calling machine, in one update |
| `GET /v1/machines/self/runtime` | — | `{tunnelToken, ingressSecret, publicUrls: [PublicUrl]}`, the URLs this machine serves |

```ts
type User = { login: string };
type Machine = { id: string; name: string };
type PublicUrl = {
  id: string;
  slug: string;
  url: string;                 // https://<slug>.localify.fyi
  localDomain: string;
  wildcard: boolean;
  machine: Machine | null;     // null when no machine serves it
  // version: the D1 password_version, bumped by every protection change,
  // so a client knows whether a password it saved is still the current one.
  // After a create or a protection change, it is the version of that very
  // change, even if another one landed since.
  protection: { mode: "password" | "public"; version: number };
  createdAt: string;           // RFC 3339
};
```

**Device flow.** The Worker calls `POST https://github.com/login/device/code` with the client id and no scope, then polls `POST https://github.com/login/oauth/access_token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`, both with `Accept: application/json`, and maps GitHub's `authorization_pending`, `slow_down` (adding 5 seconds to the interval it returns), `access_denied` and `expired_token` to the responses above. With an access token, it reads `GET https://api.github.com/user` for the id and login, checks the id against the allowlist, and discards the GitHub token. Both device routes are rate limited per client IP. The CLI refuses `login` while the machine is signed in.

**Signing in registers the machine.** The Worker first inserts the machine as `provisioning`, then creates a named tunnel `localify-<machine-id>` with `config_src: "cloudflare"`, sets its ingress to `m-<machine-id>.localify.dev → http://127.0.0.1:<ingressPort>` with a final `http_status:404` rule, creates the proxied CNAME `m-<machine-id>.localify.dev → <tunnel-id>.cfargotunnel.com`, records both ids, and only then marks the machine `active` and issues the token. If a step fails, it deletes what it created and answers `502 provisioning_failed`; the hourly cleanup removes any machine left in `provisioning` for more than 10 minutes. If the user already has an active machine with the same name, the new one replaces it: the old machine is revoked like a sign-out and its URLs move to the new one. That covers signing in again on a machine whose credentials were lost. The machine name is the computer's name, with ` (dev)` added under the dev profile so that a dev build never replaces the installed app's machine, cut to 64 UTF-16 units. The ingress secret is `HMAC-SHA-256(INGRESS_KEY, machine id)` in hex; it is never stored.

**Signing out.** In one D1 batch, the Worker deletes the token, marks the machine `revoked` and detaches its URLs (`machine: null`). Then it removes the tunnel's connections, deletes the tunnel and deletes the DNS record. Whatever fails stays recorded on the revoked machine and the hourly cleanup retries it. The Worker's own Cloudflare token does this work, so the user's revoked token is never needed again.

### Public traffic

For a request to `*.localify.fyi`, the Worker:

1. Parses the host into slug and label, and loads the public URL with its machine and its owner's GitHub id. Unknown or deleted slug, a label on a non-wildcard URL, an invalid label, or an owner no longer on the allowlist: a 404 page. No serving machine: a 503 "offline" page. Lookups are cached in the isolate for 10 seconds, so a deleted URL, a new password or a claim reaches every location within that time. These changes apply to new requests: a WebSocket or an SSE stream already open stays open until it closes.
2. Refuses cross-origin requests that could change state. Every URL under `localify.fyi` is the same site for a browser, so `SameSite` does not stop another user's page from posting to this one. For any method other than `GET`, `HEAD` and `OPTIONS`, and for every WebSocket handshake, an `Origin` header that is present must belong to the same public URL: `<slug>.localify.fyi` or `<slug>--<label>.localify.fyi` with this URL's slug. A page whose referrer policy is `no-referrer` sends `Origin: null` even to itself; that passes only with `Sec-Fetch-Site: same-origin`, which the browser sets and a page cannot. Otherwise: 403. A request without `Origin` is not a browser's cross-origin request and goes on to the access check. The Worker's own pages use `Referrer-Policy: same-origin`, so their form sends its origin.
3. Checks access when the URL is protected. Access is the cookie `__Host-localify-access` = `<expiry>.<password version>.<hex HMAC-SHA-256(COOKIE_KEY, host|url id|version|expiry)>`, valid 7 days, `Secure; HttpOnly; SameSite=Lax; Path=/`, set for the exact host (each wildcard label signs in separately). Without it, `GET` and `HEAD` return the password page (401, HTML) and other methods a plain 401, WebSocket handshakes included. The page posts to `/__localify/access` with the password and a same-host relative `next` path; a correct password sets the cookie and redirects with 303. Attempts are rate limited per URL and client IP. A link that carries the password is `https://<host>/__localify/access#<password, percent-encoded>`: the fragment never reaches a server or the app, and link previews see only the password page. On a URL without a password, `/__localify/access` answers `303` to `/#`, whose empty fragment replaces the one an old link carries; the path never reaches the app. `GET /__localify/access` on a protected URL serves that page with `next=/` and one inline script, allowed by a per-response CSP nonce, which reads the fragment, removes it from the address bar and history, and submits the form. That path never redirects, since a browser would carry the fragment to the app's page.
4. Forwards to `https://m-<machine-id>.localify.dev<path>?<query>` with the method, body and headers, after removing `Host`, `Forwarded`, `X-Real-IP`, every `X-Forwarded-*`, `X-Localify-*` and `CF-*` header, and the access cookie from `Cookie`. It adds:

| Header | Value |
|---|---|
| `X-Localify-Ingress` | the machine's ingress secret |
| `X-Localify-Public-Host` | the public host |
| `X-Localify-Local-Host` | the local host |
| `X-Localify-Url-Id` | the public URL id |
| `X-Localify-Client-Ip` | `CF-Connecting-IP` |

   WebSocket upgrades pass through the same `fetch`. Cloudflare rewrites `X-Forwarded-Host` on the way to the tunnel, which is why the public host travels in a Localify header.
5. Streams the response back with `redirect: "manual"`, after removing the `Domain` attribute from every `Set-Cookie`, so an app's cookies stay on its own host instead of reaching every URL under `localify.fyi`. Script can still set a cookie for the parent domain; isolating apps from each other completely needs `localify.fyi` on the Public Suffix List (ADR 0022), which is acceptable while every user is invited.

## Daemon

### Credentials

`cloud.json` in the data directory, written with `write_private_file` (0600). The CLI, the app and the daemon all read it; the daemon runs as the user but outside the login session, so the Keychain is not an option.

```json
{ "apiBase": "https://api.localify.dev", "token": "lfy_…", "login": "teo-goulois",
  "machineId": "…", "machineName": "teo-laptop" }
```

A signed-in machine always talks to the `apiBase` it signed in with, so its token never goes to another endpoint by accident. `LOCALIFY_CLOUD_API` only chooses the API for a new sign-in, for development against `wrangler dev`; plain `http://` is accepted only for a loopback host.

Signing out calls the API, then deletes the saved passwords (below) and the file, then sends `public_refresh`. When the API is unreachable, both are deleted anyway after a warning, and the cloud's hourly cleanup never needs them. Saved passwords that cannot be deleted add a warning and do not stop the sign-out.

### Saved passwords

When the CLI or the app sets a password, generated or chosen, it keeps it with the URL's `protection.version` in `public-url-passwords.json` next to `cloud.json`, written with `write_private_file` (0600) to a temporary file renamed over it, so a reader never sees half a file:

```json
{ "<url id>": { "version": 3, "password": "…" } }
```

Localify Cloud keeps only a hash, so this copy is what lets the CLI, the MCP and the app give the link with the password (see "Public traffic") after the moment it was set. Whoever can read the file can read `cloud.json` too, whose token can change the password anyway. A saved password counts only while its version is the URL's current one, so a password changed or removed from another machine gives no link. Every change of the file reads, changes and writes it while holding a lock (`flock`) on the data directory, so the app, the CLI and the MCP server never undo each other's change. An answer that arrives after a newer change never replaces or forgets the newer password, and then gives no link. Turning protection off or removing the URL forgets its password. Listing forgets a password only when its URL's version is newer, since the list may be older than a change being saved at that moment; the password of a URL removed from another machine stays until sign-out. Signing out deletes the file, and the temporary copy an interrupted write may have left. A file that cannot be read or written costs the links, with a warning, never the change itself, nor a sign-out. A file that is not valid JSON starts over empty.

### Public ingress

A listener on `127.0.0.1:<ingress port>`. The port is fixed per brand: 7381 for Localify, 7382 for Localify (Dev); `LOCALIFY_PUBLIC_INGRESS_PORT` moves it, so a test can run a daemon next to an installed one. When the port is taken, the daemon runs without public traffic and `public_status` says why. The ingress secret is a bearer secret for this machine: whoever holds it can send any public host and URL id to this listener, so it travels only between the Worker and the tunnel, never in logs, and the daemon trusts nothing it carries beyond the checks below. For each request it:

1. Compares `X-Localify-Ingress` with the ingress secret in constant time. A mismatch, or no secret yet because the machine is signed out or the runtime has not loaded: 403 with a generic body.
2. Reads and removes every `X-Localify-*` header.
3. Refuses paths under `/.localify/` with 404, so public traffic never reaches the open-in-editor or run-dev-server actions.
4. Checks the request against the runtime inventory, with a generic 404 for any failure: the URL id is one this machine serves; the public host is that URL's own host or, for a wildcard URL, one of its labels; the local host is exactly what that public host maps to; the registration the router selects for the local host belongs to the project of the URL (see "Exposable local domain"). Docker and external registrations are never served. Without the last check, a wildcard URL on `app.localify` would reach `private.app.localify` of another project through the more specific registration.
5. Hands the request to the router with `Host` set to the local host, and, for a `GET` or `HEAD` (WebSocket handshakes included) whose `Origin` is the request's own public origin, `Origin` set to `https://<local host>`: dev servers such as Next.js answer 403 to their own scripts, fonts and HMR from an origin missing from their allowlist, which Localify fills with local domains. Other methods keep the public `Origin`, which Next.js Server Actions compare with the public `X-Forwarded-Host`. Also a public source carrying the public host and client IP, and the runtime state it was checked against, which the router routes it with: a config reload in between cannot send it to a registration the checks never saw. The proxy then sets `X-Forwarded-Host` to the public host, `X-Forwarded-Proto: https`, `X-Forwarded-Port: 443`, `X-Forwarded-For` to the client IP and `X-Localify-Local-Domain` to the local host, for HTTP and WebSocket alike. The client address the router sees is the public client IP, never loopback.
6. Holds one of 32 permits from the moment the request is admitted until its response head arrives. Over HTTP/2 a browser sends a whole module graph at once, and the proxy opens one upstream connection per request; without the cap the dev server's accept backlog overflows. Releasing at the head keeps SSE and other long responses from starving the rest.

Errors on public traffic use generic pages: no project paths, no commands, no config details. Public responses get neither the launch profile indicator, whose script lives under `/.localify/`, nor the `X-Localify-Profile` header. Request log lines for public traffic add `source=public public_host=<host>`.

### Tunnel

When signed in, this machine serves at least one public URL, and the ingress listens on its port, the daemon runs `cloudflared --config <data dir>/cloudflared.yml tunnel --no-autoupdate --protocol http2 --metrics 127.0.0.1:<ingress port + 10> run` with the tunnel token in the `TUNNEL_TOKEN` environment variable, never on the command line. It restarts it with backoff when it exits, and stops it when the last URL leaves this machine, when the credentials are gone or the API answers 401, and on daemon shutdown; stopping also drops the ingress secret. HTTP/2 rather than QUIC: on a network that blocks UDP, `cloudflared` would try QUIC for about two minutes before falling back. The config file holds only `{}`: without one, `cloudflared` would read `~/.cloudflared/config.yml`, which belongs to the person's other tunnels. Its output goes to `cloudflared.log` next to the daemon log, truncated at each start. Without the ingress, the tunnel would hand public traffic and the ingress secret to whatever program holds the port, so the status shows the error instead.

`cloudflared` is downloaded on first use from its GitHub release, at a version pinned in the code, into `<data dir>/bin/cloudflared-<version>`, and its SHA-256 is checked against a value pinned next to the version before it is made executable. A mismatch deletes the file and reports an error. Supported: macOS arm64 and x86_64, Linux x86_64 and arm64.

The daemon fetches `GET /v1/machines/self/runtime` at start, every 5 minutes, and when the management socket receives `public_refresh`; after a failed fetch it retries sooner, from 10 seconds doubling up to 5 minutes, and keeps serving the URLs it already had. `public_refresh` answers at once and the fetch runs in the background, because the management client waits 2 seconds and a cloud call may take 20. The CLI and the app send it after every change. The management command `public_status` describes this machine only:

```json
{ "signedIn": true, "tunnel": "stopped" | "starting" | "connected" | "error", "connections": 4,
  "error": null, "servedUrls": ["…id"] }
```

`connections` comes from the `/ready` endpoint of the metrics server. A connected tunnel does not mean the dev server behind a URL is running.

## CLI

Human output on a terminal, JSON when stdout is not a terminal or with `--json`. Nothing prompts when stdin is not a terminal. Every failure says what to do next.

```bash
localify login                      # device flow: prints the code and URL, opens the browser on a terminal, waits
localify logout
localify whoami
localify public-url list
localify public-url create <local-domain> [--wildcard] [--slug <slug>] [--password <p> | --public]
localify public-url remove <url-or-slug-or-id> [--yes]
localify public-url protect <url-or-slug-or-id> [--password <p>]   # sets or rotates; generates when omitted
localify public-url unprotect <url-or-slug-or-id> [--yes]
localify public-url claim <url-or-slug-or-id>
```

`create` checks that the local domain is exposable on this machine (see "Names") before calling the cloud; without `--slug`, Localify Cloud picks the suggested name, which mirrors the local one. It then prints the URL, the password when one was generated, and the link with the password. `protect` prints the same. A generated password is 4 lowercase words from a fixed list, joined by `-`. `list` leaves the link out, as it may be on a shared screen; JSON output carries it as `passwordLink` on every URL whose password this machine set.

## MCP

One tool per command except `login`, `logout` and `unprotect`: signing in and opening a URL to everyone are for a person to decide (ADR 0018). `public_url_create` has no `public` option, so an agent always creates a protected URL and receives the link with its password, and the password, to pass on. The instructions steer an agent that wants several services or worktrees of a project to one wildcard URL on the project's namespace. `public_url_remove` is destructive.

## Desktop app

- **Settings → Localify Cloud:** sign in (shows the code, opens github.com), who is signed in, sign out, and this Mac's tunnel status.
- **Domain detail → Public URL:** one URL per project, as locally. Opened on the project's namespace or on any domain under it, the dialog creates or shows the project's wildcard URL on the namespace; any other domain gets a URL of its own. The project menu opens it on the namespace, even when no service registers it, as in a monorepo.
  - **Create:** the exact public URL of the opened domain at the top (`https://<slug>--<label>.localify.fyi` under the namespace), with the project URL below it, from `GET /v1/slugs?localDomain=`. A pencil opens the name: an input with `.localify.fyi` after it, checked as `PublicSlug` checks it and then with `GET /v1/slugs/<slug>` 300 ms after the last key, showing Available, Taken or the format rule, and a button back to the suggestion. Creating waits for a free name; a name taken in the meantime reopens the editor on it. A domain whose label does not fit beside the chosen name (61 characters minus its length) is said inline. One Password switch, on by default: the password is generated and shown once with its link; off says that anyone with the link opens the project's services. There is no custom password at creation; it can be changed after.
  - **Details:** for a domain under a project URL, its own host (`<slug>--<label>`) with copy, open and the link with the password on that host, since each host signs in separately, and the project URL below. A project URL made before this, without wildcard, says it opens only the namespace and that removing it and creating it again reaches every service and worktree. Then the password shown once after rotate with buttons to copy it and the link with it, protection on and off, remove. When this Mac serves the URL, its state comes from this Mac's tunnel; when another machine serves it, the section says "Served by <machine>" with a button to serve it from this Mac, and claims nothing about whether it is online.
  - **Badges:** a globe beside each domain a URL reaches, its own or the project's wildcard one, opening the dialog.

All of it goes through `DesktopApi`, which calls the same application use cases as the CLI. The app does no HTTP itself.
