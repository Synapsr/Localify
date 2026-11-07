# Localify Cloud

The Cloudflare Worker behind public URLs and the paid Localify Cloud plan. The contract it implements is [docs/public-urls.md](../../docs/public-urls.md), sections "Plans" and "Cloud", and the decisions behind it are ADRs 0019 to 0027. It runs on the Workers Paid plan: the tunnel sweep needs more than the 50 subrequests the free plan allows.

One Worker, routed by host:

- **`api.localify.dev`**: the JSON API the CLI, the daemon and the desktop app call, and Polar's webhooks.
  - Sign-in runs GitHub's device flow with the OAuth App's client ID alone, refuses banned accounts (and, under `SIGNUP=allowlist`, ids outside the allowlist), and registers the machine without any Cloudflare call.
  - A machine gets a named Cloudflare Tunnel `localify-<machine-id>-<generation>` only when it has a public URL to serve and its owner is entitled. Its ingress goes to the daemon on `127.0.0.1:<ingressPort>`, behind the proxied CNAME `m-<machine-id>.localify.dev`, which outlives the tunnels.
  - The API manages the user's public URLs, their password, which machine serves them, and the plan: a Polar checkout, the customer portal, and a snapshot of the subscriptions that webhooks refresh.
- **`*.localify.fyi`**: public traffic. The Worker:
  - finds the URL by slug, and checks its owner's ban and entitlement;
  - applies the owner's traffic limit;
  - refuses state-changing requests whose `Origin` is another public URL;
  - shows the warning page on a browser's first visit;
  - asks for the password (a signed, host-only `__Host-localify-access` cookie for 7 days);
  - forwards to `https://m-<machine-id>.localify.dev` with the `X-Localify-*` headers the daemon checks.

  It removes `Domain` from the app's `Set-Cookie` headers on the way back.
- **Every 5 minutes**: reaps tunnels that have been down for 15 minutes, and releases machines with nothing left to serve. It corrects CNAMEs, retries failed webhook refreshes, and removes what a sign-out could not remove.
- **Every day**: reconciles every plan with Polar, and purges URLs deleted more than 30 days ago.

Data lives in D1 (`localify-cloud`, schema in [`migrations/`](migrations/)). Tokens are stored as SHA-256 hashes, passwords as PBKDF2-SHA-256 with 100,000 iterations.

Lookups on the public path are cached in each isolate for 10 seconds. A deleted URL, a new password, a claim, a plan change, a ban or a takedown reaches every location within that time, for new requests. Streams already open close when the affected tunnel is released, within the next sweep at most.

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

Steps 1 to 4 were done for `localify.fyi` on 2026-11-05; step 5 comes with the paid plan. This is how to do them again.

1. **GitHub OAuth App** (in the Synapsr organization: Settings → Developer settings → OAuth Apps): any homepage and callback URL, **Enable Device Flow** checked. Its client ID is `GITHUB_CLIENT_ID` in [`wrangler.jsonc`](wrangler.jsonc). It needs no client secret.
2. **Infisical**, project `localify`, folder `/cloud`, same keys in `dev` and `prod`, with values that differ between them:
   - `CLOUDFLARE_API_TOKEN`: a token with Cloudflare Tunnel Edit and DNS Edit on `localify.dev` and `localify.fyi`, used by the Worker to manage tunnels; it cannot deploy.
   - `COOKIE_KEY`, `INGRESS_KEY` and `EDGE_KEY`: `openssl rand -hex 32` each.
   - `POLAR_ACCESS_TOKEN`: an organization access token; `dev` holds the sandbox one.
   - `POLAR_WEBHOOK_SECRET`.
   - For reference only: `CLOUDFLARE_ACCOUNT_ID`, `GITHUB_CLIENT_ID` and `D1_DATABASE_ID`.

   Changing `COOKIE_KEY` signs every browser out and shows every visitor the warning page again. Changing `INGRESS_KEY` makes every daemon refuse public traffic until its next runtime fetch, within 5 minutes. Changing `EDGE_KEY` means updating the zone's WAF rule in the same step, or public traffic stops.
3. **D1 database**: `pnpm exec wrangler d1 create localify-cloud --location weur`, then put the printed id in `database_id` in `wrangler.jsonc`.
4. **Migrations and deploy**, with your own `wrangler login`: `pnpm cloud:migrate`, then `pnpm cloud:deploy`. The deploy script ([`scripts/deploy.mjs`](scripts/deploy.mjs)) reads the secrets from Infisical and passes them to `wrangler deploy --secrets-file`, keeping the Tunnel and DNS token out of wrangler's own environment.

5. **Polar**, once in the sandbox (`sandbox.polar.sh`) and once in production:
   - two Pro products, $7.99 a month and $79 a year, each with a 7-day trial; their ids go in `POLAR_PRODUCT_PRO_MONTHLY` and `POLAR_PRODUCT_PRO_YEARLY`;
   - "Prevent trial abuse" turned on (Settings → Subscriptions);
   - "Enable subscription plan changes" turned on in the customer portal settings, so a subscriber can switch between monthly and yearly there;
   - a webhook endpoint at `https://api.localify.dev/v1/webhooks/polar` (the sandbox points at a dev deployment), with `api_version` `2026-10`, subscribed to `subscription.*` and `customer.state_changed`;
   - an organization access token with `checkouts:write`, `customer_sessions:write`, `subscriptions:read`, `subscriptions:write`, `orders:read` and `refunds:write`.

   The Worker sends `Polar-Version: 2026-10` on every call. Polar removes that version in April 2027: move the Worker and the webhook endpoint to the next version together before then.

   The add-on products and "Enable subscription unit changes" come when add-ons ship.

The `localify.fyi` zone needs a proxied `*` AAAA record (`100::`) so every label reaches the Worker route, and a rate-limiting rule per client IP (docs/public-urls.md, "Public traffic").

The `localify.dev` zone needs two rules, so that nobody holding a tunnel token can serve pages on a machine hostname directly (docs/public-urls.md, "Edge rule"):

- **WAF custom rule, action Block:**
  `starts_with(http.host, "m-") and ends_with(http.host, ".localify.dev") and not any(http.request.headers["x-localify-edge"][*] eq "<EDGE_KEY>")`
- **Request header transform rule:** remove `X-Localify-Edge` on the same hosts. Transform rules run after the WAF, so the header never reaches the tunnel.

### Configuration

| Name | Kind | Value |
|---|---|---|
| `SIGNUP` | var | `allowlist` while invite-only, `open` once anyone may sign in |
| `ALLOWED_GITHUB_IDS` | var | under `SIGNUP=allowlist`, who may sign in |
| `MAX_TUNNELS` | var | `950`; the sweep logs an error above 800 |
| `POLAR_API_BASE` | var | `https://api.polar.sh`, or `https://sandbox-api.polar.sh` |
| `POLAR_PRODUCT_PRO_MONTHLY`, `POLAR_PRODUCT_PRO_YEARLY` | var | Pro product ids |
| `POLAR_PRODUCT_ADDON_MACHINE`, `POLAR_PRODUCT_ADDON_URLS` | var | add-on product ids, empty until add-ons ship |
| `POLAR_ACCESS_TOKEN`, `POLAR_WEBHOOK_SECRET` | secret | from Infisical |
| `EDGE_KEY` | secret | from Infisical; must match the `localify.dev` WAF rule |

The full table, with the Cloudflare and GitHub settings, is in [docs/public-urls.md](../../docs/public-urls.md), "Configuration".

## Sign-up, overrides and moderation

`SIGNUP` decides who may finish a new sign-in. Under `allowlist`, `ALLOWED_GITHUB_IDS` in [`wrangler.jsonc`](wrangler.jsonc) holds numeric GitHub user ids, comma-separated, because a login can be renamed and then taken by someone else. To find someone's id:

```bash
curl -s https://api.github.com/users/<login> | jq .id
```

The allowlist only gates sign-in. Serving takes an entitlement in both modes: a plan, or an override. Removing an id from the allowlist does not stop an account that is already signed in.

Overrides, bans and takedowns go through `pnpm cloud:admin` (docs/public-urls.md, "Moderation"). Each one writes D1 with your own wrangler login, reaches the public path within 10 seconds, and closes the affected streams at the next sweep.
