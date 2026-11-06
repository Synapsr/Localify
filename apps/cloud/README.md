# Localify Cloud

The Cloudflare Worker behind public URLs. The contract it implements is [docs/public-urls.md](../../docs/public-urls.md), section "Cloud", and the decisions behind it are ADRs 0019 to 0023.

One Worker, routed by host:

- **`api.localify.dev`**: the JSON API the CLI, the daemon and the desktop app call. Sign-in runs GitHub's device flow with the OAuth App's client ID alone, checks the GitHub id against the allowlist, and registers the machine: a named Cloudflare Tunnel `localify-<machine-id>` configured by Cloudflare, its ingress to the daemon on `127.0.0.1:<ingressPort>`, and the proxied CNAME `m-<machine-id>.localify.dev`. The API then manages the user's public URLs, their password, which machine serves them, and gives each machine its tunnel token and ingress secret.
- **`*.localify.fyi`**: public traffic. The Worker finds the URL by slug, refuses state-changing requests whose `Origin` is another public URL, asks for the password (a signed, host-only `__Host-localify-access` cookie for 7 days), and forwards to `https://m-<machine-id>.localify.dev` with the `X-Localify-*` headers the daemon checks. It removes `Domain` from the app's `Set-Cookie` headers on the way back.
- **Every hour**: removes the tunnels and DNS records that a sign-out could not remove, revokes sign-ins stuck for 10 minutes, and purges URLs deleted more than 30 days ago.

Data lives in D1 (`localify-cloud`, schema in [`migrations/`](migrations/)). Tokens are stored as SHA-256 hashes, passwords as PBKDF2-SHA-256 with 100,000 iterations.

Lookups on the public path are cached in each isolate for 10 seconds. A deleted URL, a new password, a claim or a removal from the allowlist reaches every location within that time, for new requests: a WebSocket or an SSE stream already open stays open until it closes.

## Code

| File | What it does |
|---|---|
| `src/index.ts` | Routes by host, and runs the hourly cleanup |
| `src/api.ts` | The API routes |
| `src/machines.ts` | Sign-in, sign-out, replacing a machine, and the hourly cleanup |
| `src/public.ts` | The public traffic path |
| `src/github.ts`, `src/cloudflare.ts` | The GitHub and Cloudflare API calls |
| `src/names.ts` | Slugs, labels, public hosts, and the origin rule |
| `src/cookie.ts`, `src/headers.ts`, `src/crypto.ts` | Access cookie, forwarded headers, hashing |
| `src/pages.ts`, `src/passwords.ts` | The HTML pages and generated passwords |

## Run locally

```bash
cp apps/cloud/.dev.vars.example apps/cloud/.dev.vars   # once
pnpm cloud:dev
```

`pnpm cloud:dev` applies the migrations to a local D1 in `apps/cloud/.wrangler/` and starts `wrangler dev` on `http://127.0.0.1:8787`. Point the CLI at it for a new sign-in:

```bash
LOCALIFY_CLOUD_API=http://127.0.0.1:8787 localify login
```

Sign-in reaches the real GitHub, and creates a real tunnel and DNS record when `.dev.vars` has a `CLOUDFLARE_API_TOKEN`; without one, it answers `502 provisioning_failed`. `localify logout` removes what sign-in created.

To try the public path, send the public host in the `Host` header. The `dev` script passes `--infer-origin-from-routes=false` so the Worker sees that host instead of its first route's.

```bash
curl -H 'Host: journal-a8f3.localify.fyi' http://127.0.0.1:8787/
```

Over plain HTTP, the Worker answers with the 404 or offline page, or redirects to HTTPS: nothing else, the password least of all, travels over HTTP. To go further, serve HTTPS locally and use `curl -k`:

```bash
cd apps/cloud && pnpm exec wrangler dev --infer-origin-from-routes=false --local-protocol https
curl -k -H 'Host: journal-a8f3.localify.fyi' https://127.0.0.1:8787/
```

Trigger the hourly cleanup with `curl http://127.0.0.1:8787/cdn-cgi/local/scheduled`.

## Checks

```bash
pnpm cloud:typecheck   # wrangler types, then tsc
pnpm cloud:test        # vitest
```

The route tests run the Worker in Node against a D1 stand-in on `node:sqlite` with the real migrations, and a fake `fetch` for GitHub and Cloudflare. `@cloudflare/vitest-pool-workers` pins its own wrangler and an alpha miniflare, so it is not used.

## Setup, once

Already done for `localify.fyi` on 2026-11-05; this is how to do it again.

1. **GitHub OAuth App** (in the Synapsr organization: Settings → Developer settings → OAuth Apps): any homepage and callback URL, **Enable Device Flow** checked. Its client ID is `GITHUB_CLIENT_ID` in [`wrangler.jsonc`](wrangler.jsonc). It needs no client secret.
2. **Infisical**, project `localify`, folder `/cloud`, same keys in `dev` and `prod`: `CLOUDFLARE_API_TOKEN` (a token with Cloudflare Tunnel Edit and DNS Edit on `localify.dev` and `localify.fyi`, used by the Worker to manage tunnels; it cannot deploy), `COOKIE_KEY` and `INGRESS_KEY` (`openssl rand -hex 32` each), plus `CLOUDFLARE_ACCOUNT_ID`, `GITHUB_CLIENT_ID` and `D1_DATABASE_ID` for reference. Changing `COOKIE_KEY` signs every browser out; changing `INGRESS_KEY` makes every daemon refuse public traffic until its next runtime fetch, within 5 minutes.
3. **D1 database**: `pnpm exec wrangler d1 create localify-cloud --location weur`, then put the printed id in `database_id` in `wrangler.jsonc`.
4. **Migrations and deploy**, with your own `wrangler login`: `pnpm cloud:migrate`, then `pnpm cloud:deploy`. The deploy script ([`scripts/deploy.mjs`](scripts/deploy.mjs)) reads the three secrets from Infisical and passes them to `wrangler deploy --secrets-file`, keeping the Tunnel and DNS token out of wrangler's own environment.

The `localify.fyi` zone needs a proxied `*` AAAA record (`100::`) so every label reaches the Worker route.

## Allowlist

Sign-in is invite-only. `ALLOWED_GITHUB_IDS` in [`wrangler.jsonc`](wrangler.jsonc) holds numeric GitHub user ids, comma-separated, because a login can be renamed and then taken by someone else. To find someone's id:

```bash
curl -s https://api.github.com/users/<login> | jq .id
```

Add it to the list and run `pnpm cloud:deploy`. Removing an id takes effect at once: that person's tokens stop working and their URLs answer 404, without touching the database.
