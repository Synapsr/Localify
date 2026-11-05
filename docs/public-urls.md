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

V1 is invite-only: GitHub accounts on an allowlist. Out of v1: custom domains, temporary URLs, renaming a slug, several URLs per local domain, Docker or external domains, request inspection, billing.

## Names

- **Slug:** 3 to 32 characters, `a-z`, `0-9` and `-`, starting with a letter, not ending with `-`, never containing `--`. Generated as the first label of the local domain plus 4 hex characters (`journal-a8f3`), or chosen with `--slug`. Unique across all users.
- **Public host:** `<slug>.localify.fyi`, or `<slug>--<label>.localify.fyi` for a wildcard URL. The first `--` separates the slug from the label, so labels may contain `--` (`web--feature`). The slug comes first so that `--` never sits at positions 3–4 of the DNS label, which IDNA reserves (ADR 0022).
- **Local host:** `<local-domain>` for `<slug>`, and `<label>.<local-domain>` for `<slug>--<label>`. A label is `a-z`, `0-9` and `-`, does not start or end with `-`, and the whole public DNS label stays within 63 characters. Longer names cannot be served; the CLI says so instead of truncating.
- **Machine:** one signed-in installation of Localify. Its tunnel hostname is `m-<machine-id>.localify.dev`.

## Cloud (`apps/cloud`)

One Cloudflare Worker, deployed with wrangler, with a D1 database.

- `api.localify.dev`: the JSON API for the CLI, the daemon and the app (Worker custom domain).
- `*.localify.fyi/*`: public traffic (Worker route; the zone has a proxied `*` AAAA record).

### Configuration

| Name | Kind | Value |
|---|---|---|
| `GITHUB_CLIENT_ID` | var | the OAuth App's client ID (device flow, no secret) |
| `ALLOWED_GITHUB_LOGINS` | var | comma-separated, case-insensitive |
| `PUBLIC_BASE_DOMAIN` | var | `localify.fyi` |
| `MACHINE_DOMAIN` | var | `localify.dev` |
| `CLOUDFLARE_ACCOUNT_ID`, `MACHINE_ZONE_ID` | var | account, and the `localify.dev` zone |
| `CLOUDFLARE_API_TOKEN` | secret | Tunnel Edit, DNS Edit (Infisical `localify` `/cloud`) |
| `COOKIE_KEY` | secret | signs access cookies |
| `INGRESS_KEY` | secret | derives each machine's ingress secret |

### Data (D1)

```sql
users        (id TEXT PRIMARY KEY, github_id INTEGER UNIQUE NOT NULL, login TEXT NOT NULL, created_at INTEGER NOT NULL)
machines     (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, tunnel_id TEXT NOT NULL,
              dns_record_id TEXT NOT NULL, ingress_port INTEGER NOT NULL, created_at INTEGER NOT NULL)
tokens       (hash TEXT PRIMARY KEY, user_id TEXT NOT NULL, machine_id TEXT NOT NULL, created_at INTEGER NOT NULL)
public_urls  (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, slug TEXT UNIQUE NOT NULL, local_domain TEXT NOT NULL,
              wildcard INTEGER NOT NULL, machine_id TEXT, protected INTEGER NOT NULL,
              password_hash TEXT, password_salt TEXT, password_version INTEGER NOT NULL, created_at INTEGER NOT NULL)
url_exceptions (url_id TEXT NOT NULL, path_prefix TEXT NOT NULL, PRIMARY KEY (url_id, path_prefix))
```

Tokens are stored as SHA-256 hashes. Passwords use PBKDF2-SHA-256 with a random salt and 100,000 iterations. A user has at most 10 public URLs.

### API

Every request except the device flow carries `Authorization: Bearer <token>`. Errors are `{"error": "<code>", "message": "<sentence for a person>"}` with a 4xx or 5xx status. Field names are camelCase.

| Method and path | Body | Response |
|---|---|---|
| `POST /v1/auth/device` | — | `{deviceCode, userCode, verificationUri, interval, expiresIn}` (GitHub's device flow) |
| `POST /v1/auth/device/token` | `{deviceCode, machineName, ingressPort}` | `200 {token, user, machine}`; `428 authorization_pending`; `429 slow_down`; `403 not_allowed`; `410 expired_token` |
| `GET /v1/me` | — | `{user, machine}` |
| `POST /v1/auth/logout` | — | `204`. Revokes the token, deletes the machine's tunnel and DNS record, and detaches its URLs (`machine: null`). |
| `GET /v1/public-urls` | — | `{publicUrls: [PublicUrl]}`, the user's URLs |
| `POST /v1/public-urls` | `{localDomain, wildcard, slug?, protection: {mode: "password", password?} \| {mode: "public"}}` | `201 {publicUrl, password?}`. The calling machine serves it. `password` is present only when the server generated it. `409 slug_taken`, `422 invalid_slug`, `403 quota_exceeded`. |
| `DELETE /v1/public-urls/{id}` | — | `204` |
| `PUT /v1/public-urls/{id}/protection` | same `protection` object | `{publicUrl, password?}`. Changing or removing the password signs every browser out. |
| `POST /v1/public-urls/{id}/exceptions` | `{pathPrefix}` | `{publicUrl}` |
| `DELETE /v1/public-urls/{id}/exceptions` | `{pathPrefix}` | `{publicUrl}` |
| `POST /v1/public-urls/{id}/claim` | — | `{publicUrl}`, now served by the calling machine |
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
  protection: { mode: "password" | "public"; exceptions: string[] };
  createdAt: string;           // RFC 3339
};
```

Signing in registers the machine: the Worker creates a named tunnel `localify-<machine-id>` with `config_src: "cloudflare"`, sets its ingress to `m-<machine-id>.localify.dev → http://127.0.0.1:<ingressPort>` with a final `http_status:404` rule, and creates the proxied CNAME `m-<machine-id>.localify.dev → <tunnel-id>.cfargotunnel.com`. The ingress secret is `HMAC-SHA-256(INGRESS_KEY, machine id)` in hex; it is never stored.

### Public traffic

For a request to `*.localify.fyi`, the Worker:

1. Parses the host into slug and label, and loads the public URL with its machine and exceptions. Unknown slug, a label on a non-wildcard URL, or an invalid label: a 404 page. No serving machine: a 503 "offline" page. Lookups are cached in the isolate for 10 seconds, so a deleted URL or a new password reaches every location within that time.
2. Checks access when the URL is protected and the path matches no exception. Access is the cookie `__Host-localify-access` = `<expiry>.<password version>.<hex HMAC-SHA-256(COOKIE_KEY, host|url id|version|expiry)>`, valid 7 days, `Secure; HttpOnly; SameSite=Lax; Path=/`, set for the exact host (each wildcard label signs in separately). Without it, `GET` and `HEAD` return the password page (401, HTML) and other methods a plain 401. The page posts to `/__localify/access` with the password and a same-host `next` path; a correct password sets the cookie and redirects with 303. Attempts are rate limited per URL and client IP. A WebSocket handshake also needs the cookie, and its `Origin`, when present, must be the same host.
3. Path exceptions match the request path as the URL parser normalizes it: equal to the prefix, or starting with the prefix followed by `/`. A prefix starts with `/`, and may not contain `..`, `%2f`, `%2F`, `\`, `?` or `#`.
4. Forwards to `https://m-<machine-id>.localify.dev<path>?<query>` with the method, body and headers, after removing `Host`, `Forwarded`, `X-Real-IP`, every `X-Forwarded-*`, `X-Localify-*` and `CF-*` header, and the access cookie from `Cookie`. It adds:

| Header | Value |
|---|---|
| `X-Localify-Ingress` | the machine's ingress secret |
| `X-Localify-Public-Host` | the public host |
| `X-Localify-Local-Host` | the local host |
| `X-Localify-Url-Id` | the public URL id |
| `X-Localify-Client-Ip` | `CF-Connecting-IP` |

   Responses stream back unchanged, with `redirect: "manual"`. WebSocket upgrades pass through the same `fetch`. Cloudflare rewrites `X-Forwarded-Host` on the way to the tunnel, which is why the public host travels in a Localify header.

## Daemon

### Credentials

`cloud.json` in the data directory, written with `write_private_file` (0600). The CLI, the app and the daemon all read it; the daemon runs as the user but outside the login session, so the Keychain is not an option.

```json
{ "apiBase": "https://api.localify.dev", "token": "lfy_…", "login": "teo-goulois",
  "machineId": "…", "machineName": "teo-laptop" }
```

`LOCALIFY_CLOUD_API` overrides `apiBase` for development. Signing out deletes the file after the API call, or when the API is unreachable after a warning.

### Public ingress

A listener on `127.0.0.1:<ingress port>`, only when the machine is signed in. The port is fixed per brand: 7381 for Localify, 7382 for Localify (Dev). For each request it:

1. Compares `X-Localify-Ingress` with the ingress secret in constant time. A mismatch, or no secret yet: 403 with a generic body.
2. Reads and removes every `X-Localify-*` header.
3. Refuses paths under `/.localify/` with 404, so public traffic never reaches the open-in-editor or run-dev-server actions.
4. Checks that the local host resolves to a domain of a project in the config. Docker and external registrations are not served. Otherwise: a generic 404.
5. Hands the request to the router with `Host` set to the local host and a public source carrying the public host and client IP. The proxy then sets `X-Forwarded-Host` to the public host, `X-Forwarded-Proto: https`, `X-Forwarded-Port: 443`, `X-Forwarded-For` to the client IP and `X-Localify-Local-Domain` to the local host, for HTTP and WebSocket alike. The client address the router sees is the public client IP, never loopback.
6. Holds one of 32 permits from the moment the request is admitted until its response head arrives. Over HTTP/2 a browser sends a whole module graph at once, and the proxy opens one upstream connection per request; without the cap the dev server's accept backlog overflows. Releasing at the head keeps SSE and other long responses from starving the rest.

Errors on public traffic use generic pages: no project paths, no commands, no config details. Request log lines for public traffic add `source=public public_host=<host>`.

### Tunnel

When signed in and this machine serves at least one public URL, the daemon runs `cloudflared tunnel --no-autoupdate --metrics 127.0.0.1:<ingress port + 10> run` with the tunnel token in the `TUNNEL_TOKEN` environment variable, never on the command line. It restarts it with backoff when it exits, and stops it when the last URL leaves this machine, on sign-out and on daemon shutdown. Its output goes to `cloudflared.log` next to the daemon log, truncated at each start.

`cloudflared` is downloaded on first use from its GitHub release, at a version pinned in the code, into `<data dir>/bin/cloudflared-<version>`, and its SHA-256 is checked against a value pinned next to the version before it is made executable. A mismatch deletes the file and reports an error. Supported: macOS arm64 and x86_64, Linux x86_64 and arm64.

The daemon fetches `GET /v1/machines/self/runtime` at start, every 5 minutes, and when the management socket receives `public_refresh`. The CLI and the app send it after every change. The management command `public_status` returns:

```json
{ "signedIn": true, "tunnel": "stopped" | "starting" | "connected" | "error", "connections": 4,
  "error": null, "servedUrls": ["…id"] }
```

`connections` comes from the `/ready` endpoint of the metrics server.

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
localify public-url exception add <url-or-slug-or-id> <path-prefix>
localify public-url exception remove <url-or-slug-or-id> <path-prefix>
localify public-url claim <url-or-slug-or-id>
```

`create` checks that the local domain belongs to a project on this machine before calling the cloud, prints the URL and, when one was generated, the password with a note that it is shown once. A generated password is 4 lowercase words from a fixed list, joined by `-`.

## MCP

One tool per command except `login`, `logout` and `unprotect`: signing in and opening a URL to everyone are for a person to decide (ADR 0018). `public_url_create` has no `public` option, so an agent always creates a protected URL and receives its password to pass on. `public_url_remove` is destructive.

## Desktop app

- **Settings → Localify Cloud:** sign in (shows the code, opens github.com), who is signed in, sign out, and the tunnel status.
- **Domain detail → Public URL:** create (wildcard switch for a project's namespace domain), the URL with copy and open, the password shown once after create or rotate with a copy button, protection on and off, path exceptions, claim when another machine serves it, remove. Online or offline from `public_status`.

All of it goes through `DesktopApi`, which calls the same application use cases as the CLI. The app does no HTTP itself.
