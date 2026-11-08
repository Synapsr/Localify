# Localify Cloud

The Cloudflare Worker behind public URLs and the paid Localify Cloud plan. The contract it implements is [docs/public-urls.md](../../docs/public-urls.md), sections "Plans" and "Cloud", and the decisions behind it are ADRs 0019 to 0027. It runs on the Workers Paid plan: the tunnel sweep needs more than the 50 subrequests the free plan allows.

One Worker, routed by host:

- **`api.localify.dev`**: the JSON API the CLI, the daemon and the desktop app call, and Polar's webhooks.
  - Sign-in runs GitHub's device flow with the OAuth App's client ID alone, refuses banned accounts (and, under `SIGNUP=allowlist`, ids outside the allowlist), and registers the machine without any Cloudflare call.
  - A machine gets a named Cloudflare Tunnel `localify-<machine-id>-<generation>` only when it has a public URL to serve and its owner is entitled. Its ingress goes to the daemon on `127.0.0.1:<ingressPort>`, behind the proxied CNAME `m-<machine-id>.localify.dev`, which outlives the tunnels.
  - The API manages the user's public URLs, their password, which machine serves them, and the plan: a Polar checkout for Pro or an add-on, the customer portal, and a snapshot of the subscriptions that webhooks refresh.
  - `GET /v1/pricing`, public, gives the website the live prices of Pro and its add-ons, read from Polar and cached an hour.
  - Remote machines ([docs/remote-machines.md](../../docs/remote-machines.md), ADRs 0028 and 0032): the roster, each machine's remote key, removing a machine, authorization leases, and `/v1/remote/connect`, which hands each daemon's WebSocket to its account's `RemoteRelay` Durable Object. The relay routes end-to-end encrypted frames between the account's machines, stores nothing, and hibernates while idle, so an account nobody watches costs nothing.
- **`*.localify.fyi`**: public traffic. The Worker:
  - finds the URL by slug, and checks its owner's ban and entitlement;
  - applies the owner's traffic limit;
  - refuses state-changing requests whose `Origin` is another public URL;
  - shows the warning page on a browser's first visit;
  - asks for the password (a signed, host-only `__Host-localify-access` cookie for 7 days);
  - forwards to `https://m-<machine-id>.localify.dev` with the `X-Localify-*` headers the daemon checks.

  It removes `Domain` from the app's `Set-Cookie` headers on the way back.
- **Every 5 minutes**: rotates the machines of URLs that stopped serving since the last run (a plan change, an expiry, a ban or a takedown), reaps tunnels that have been down for 15 minutes, and releases machines with nothing left to serve. It corrects CNAMEs, retries failed webhook refreshes, cancels the add-ons of lapsed plans at period end, and removes what a sign-out could not remove.
- **Every day**: reconciles every plan with Polar, and purges URLs deleted more than 30 days ago.

Data lives in D1 (`localify-cloud`, schema in [`migrations/`](migrations/)). Tokens are stored as SHA-256 hashes, passwords as PBKDF2-SHA-256 with 100,000 iterations.

Lookups on the public path are cached in each isolate for 10 seconds. A deleted URL, a new password, a claim, a plan change, a ban or a takedown reaches every location within that time, for new requests. Streams already open close when the affected tunnel is released, within the next sweep at most.

## Code

| File | What it does |
|---|---|
| `src/index.ts` | Routes by host, and runs the 5-minute and daily jobs |
| `src/api.ts` | The API routes |
| `src/machines.ts` | Sign-in, sign-out, replacing a machine, the cleanup of revoked machines, and the purge of deleted URLs |
| `src/tunnels.ts` | Serving slots, provisioning and releasing tunnels, the machine runtime, the serving reconcile, and the 5-minute sweep |
| `src/queries.ts` | The D1 query budget of a scheduled run |
| `src/billing.ts` | The Pro and add-on checkouts, the portal, Polar's webhooks, the refresh of the `billing` snapshot, duplicate subscriptions, the add-ons of lapsed plans, and the daily reconciliation |
| `src/pricing.ts` | `GET /v1/pricing`: live prices from Polar, the currency, the Cache API, CORS |
| `src/upgrade.ts` | The `upgrade` hint of `402 subscription_required`, `403 quota_exceeded` and a blocked runtime |
| `src/entitlement.ts` | What an account may use: its ban, its override, or its Pro plan from the `billing` snapshot, remote access included; and which URLs lose their right after a change |
| `src/relay.ts` | The `RemoteRelay` Durable Object, one per account, and the Worker's revocation pushes to it |
| `src/public.ts`, `src/sites.ts` | The public traffic path |
| `src/github.ts`, `src/cloudflare.ts`, `src/polar.ts` | The GitHub, Cloudflare and Polar API calls |
| `src/names.ts` | Slugs, labels, public hosts, and the origin rule |
| `src/cookie.ts`, `src/headers.ts`, `src/crypto.ts` | Access and notice cookies, forwarded headers, hashing |
| `src/pages.ts`, `src/passwords.ts` | The HTML pages (warning, password, errors) and generated passwords |
| `scripts/admin.ts` | `pnpm cloud:admin`: bans, takedowns, overrides, billing refreshes, and the duplicate subscriptions to refund |

## Run locally

```bash
cp apps/cloud/.dev.vars.example apps/cloud/.dev.vars   # once
pnpm cloud:dev
```

`pnpm cloud:dev` applies the migrations to a local D1 in `apps/cloud/.wrangler/` and starts `wrangler dev` on `http://127.0.0.1:8787`. Point the CLI at it for a new sign-in:

```bash
LOCALIFY_CLOUD_API=http://127.0.0.1:8787 localify login
```

Sign-in reaches the real GitHub and makes no Cloudflare call. Serving takes an entitlement, so give your account one: `pnpm cloud:admin --local override <github-login> 5 10 dev`. Once the machine has a URL to serve, its next runtime fetch creates a real tunnel and DNS record when `.dev.vars` has a `CLOUDFLARE_API_TOKEN`; without one, the runtime stays `provisioning` and the Worker logs that the token is not set. `localify logout` removes what the machine created.

`.dev.vars.example` points billing at Polar's sandbox, with add-ons on sale. With the sandbox `POLAR_ACCESS_TOKEN` from Infisical (`dev`), `localify subscribe` opens a sandbox checkout (test card `4242 4242 4242 4242`). Without a sandbox webhook reaching your Worker, run `pnpm cloud:admin --local refresh <github-login>` and trigger the 5-minute job to see the plan.

To try the public path, send the public host in the `Host` header. The `dev` script passes `--infer-origin-from-routes=false` so the Worker sees that host instead of its first route's.

```bash
curl -H 'Host: journal-a8f3.localify.fyi' http://127.0.0.1:8787/
```

Over plain HTTP, the Worker answers with the 404 or offline page, or redirects to HTTPS: nothing else, the password least of all, travels over HTTP. To go further, serve HTTPS locally and use `curl -k`:

```bash
cd apps/cloud && pnpm exec wrangler dev --infer-origin-from-routes=false --local-protocol https
curl -k -H 'Host: journal-a8f3.localify.fyi' https://127.0.0.1:8787/
```

Trigger the 5-minute sweep with `curl 'http://127.0.0.1:8787/cdn-cgi/local/scheduled?cron=*/5+*+*+*+*'`, and the daily job with `cron=0+3+*+*+*`.

## Checks

```bash
pnpm cloud:typecheck   # wrangler types, then tsc
pnpm cloud:test        # vitest
```

The route tests run the Worker in Node against a D1 stand-in on `node:sqlite` with the real migrations, and a fake `fetch` for GitHub, Cloudflare and Polar. The relay runs over fake hibernatable WebSockets the tests hold, with `cloudflare:workers` aliased to a stub of its base class ([`test/cloudflare-workers.ts`](test/cloudflare-workers.ts)); a test drops the object's instance to simulate hibernation. `@cloudflare/vitest-pool-workers` pins its own wrangler and an alpha miniflare, so it is not used.

## Setup, once

Steps 1 to 4 were done for `localify.fyi` on 2026-11-05. Step 5 was done on 2026-11-08 but for the webhook endpoints, which come after the deploy that serves them. This is how to do them again.

1. **GitHub OAuth App** (in the Synapsr organization: Settings → Developer settings → OAuth Apps): any homepage and callback URL, **Enable Device Flow** checked. Its client ID is `GITHUB_CLIENT_ID` in [`wrangler.jsonc`](wrangler.jsonc). It needs no client secret.
2. **Infisical**, project `localify`, folder `/cloud`, same keys in `dev` and `prod`, with values that differ between them:
   - `CLOUDFLARE_API_TOKEN`: a token with Cloudflare Tunnel Edit and DNS Edit on `localify.dev` and `localify.fyi`, used by the Worker to manage tunnels; it cannot deploy.
   - `COOKIE_KEY`, `INGRESS_KEY` and `EDGE_KEY`: `openssl rand -hex 32` each.
   - `POLAR_ACCESS_TOKEN`: an organization access token; `dev` holds the sandbox one, which the sandbox Worker also uses.
   - `POLAR_WEBHOOK_SECRET`: the `whsec_…` secret Polar shows when the webhook endpoint is created. The deploy runs without it, and the Worker then answers webhooks with 503.
   - `CLOUDFLARE_DEPLOY_TOKEN`, `prod` only: wrangler's identity for `pnpm cloud:deploy`, `pnpm cloud:migrate` and `pnpm cloud:admin`, so nobody's `wrangler login` is needed. Account: Workers Scripts Edit, D1 Edit, Account Settings Read; zones `localify.dev` and `localify.fyi`: Workers Routes Edit, Zone Read; User Details Read.
   - `CLOUDFLARE_OPS_TOKEN`, `prod` only: an operator's token for the zone rules below (Zone WAF Edit, Transform Rules Edit, Zone Read on both zones). Neither script reads it, and it never reaches the Worker.
   - `CLOUDFLARE_ACCOUNT_ID`: read by the scripts with `CLOUDFLARE_DEPLOY_TOKEN`. For reference only: `GITHUB_CLIENT_ID` and `D1_DATABASE_ID`.

   Changing `COOKIE_KEY` signs every browser out and shows every visitor the warning page again. Changing `INGRESS_KEY` makes every daemon refuse public traffic until its next runtime fetch, within 5 minutes. Changing `EDGE_KEY` means updating the zone's WAF rule in the same step, or public traffic stops.
3. **D1 database**: `pnpm exec wrangler d1 create localify-cloud --location weur`, then put the printed id in `database_id` in `wrangler.jsonc`.
4. **Migrations and deploy**: `pnpm cloud:migrate`, then `pnpm cloud:deploy`. Both run wrangler as `CLOUDFLARE_DEPLOY_TOKEN` from Infisical, or with your own `wrangler login` when that key is missing ([`scripts/infisical.mjs`](scripts/infisical.mjs)). The deploy script ([`scripts/deploy.mjs`](scripts/deploy.mjs)) passes the Worker's secrets to `wrangler deploy --secrets-file`, keeping the Tunnel and DNS token out of wrangler's own environment.

5. **Polar**, once in the sandbox (`sandbox.polar.sh`) and once in production:
   - two Pro products, one a month and one a year (Polar takes one interval per product), each with a USD and an EUR price ($7.99 and €7.99, $79 and €79) and a 7-day trial; their ids go in `POLAR_PRODUCT_PRO_MONTHLY` and `POLAR_PRODUCT_PRO_YEARLY`, the production ones in `wrangler.jsonc` and the sandbox ones in `.dev.vars.example`. Polar picks the currency from the IP address that creates the checkout, so the Worker passes the buyer's `CF-Connecting-IP` as `customer_ip_address`;
   - "Prevent trial abuse" turned on (Settings → Subscriptions);
   - "Enable subscription plan changes" turned on in the customer portal settings, so a subscriber can switch between monthly and yearly there;
   - a webhook endpoint at `https://api.localify.dev/v1/webhooks/polar` (the sandbox points at a dev deployment), with `api_version` `2026-10`, subscribed to every `subscription.*` event and `customer.state_changed`; its `whsec_…` secret goes in Infisical as `POLAR_WEBHOOK_SECRET`, then `pnpm cloud:deploy` again;
   - two add-on products, monthly, without trial, each with a unit-based USD and EUR price, volume tiering with a single tier: one serving machine at $3 / €3 a unit, and ten public URLs at $2 / €2 a unit. Their ids go in `POLAR_PRODUCT_ADDON_MACHINE` and `POLAR_PRODUCT_ADDON_URLS`, production's at the top of `wrangler.jsonc` and the sandbox's in `env.sandbox` and `.dev.vars.example`;
   - "Enable subscription unit changes" turned on in the customer portal settings, so a subscriber changes an add-on's quantity there;
   - "Allow multiple subscriptions" turned on for the organization before `ADDONS` is set to `on`: an add-on is a second subscription next to Pro, and Polar refuses it otherwise. Production keeps `ADDONS` at `off` until then;
   - an organization access token with `checkouts:write`, `customer_sessions:write`, `subscriptions:read`, `subscriptions:write`, `orders:read` and `products:read`, the last one for `/v1/pricing`. Refunds are made in the dashboard, so it needs no refund scope.

   The Worker sends `Polar-Version: 2026-10` on every call. Polar removes that version in April 2027: move the Worker and the webhook endpoint to the next version together before then.

The `localify.fyi` zone needs a proxied `*` AAAA record (`100::`) so every label reaches the Worker route, and a rate-limiting rule per client IP (docs/public-urls.md, "Public traffic"). The rule runs before the Worker, so a flood from one address costs no Worker requests.

The zone is on the Free plan, which allows one rate-limiting rule, matching on the URI path and verified bots only, counting by IP, over 10 seconds, with a 10-second block and Cloudflare's default response (a custom response needs Pro). The rule fits those limits. Create it under Security rules → Create rule → Rate limiting rules:

- **Name:** `Per-IP flood limit`
- **If incoming requests match:** `(http.request.uri.path wildcard "/*")`, every request
- **With the same characteristics:** IP
- **When rate exceeds:** 5,000 requests per 10 seconds
- **Then take action:** Block, with the default response (429)
- **For duration:** 10 seconds

The rule targets floods, not normal use, so its threshold sits far above any real visitor. A cold load of a Vite app in dev can request a thousand modules or more within a few seconds. Many people can also share one address: an office, a school, or a phone carrier's NAT (CGNAT), where everyone behind it counts as one IP. A block there would cut all of them off for 10 seconds at a time. 5,000 requests in 10 seconds leaves room for several such cold loads at once, and still blocks one address sending more than 500 requests a second. Through the API (`CLOUDFLARE_OPS_TOKEN`), it is one rule in the zone's `http_ratelimit` phase entrypoint:

```json
{
  "description": "Per-IP flood limit",
  "expression": "(http.request.uri.path wildcard \"/*\")",
  "action": "block",
  "ratelimit": {
    "characteristics": ["ip.src", "cf.colo.id"],
    "period": 10,
    "requests_per_period": 5000,
    "mitigation_timeout": 10
  }
}
```

The Worker's own `OWNER_LIMITER` (2,000 requests a minute per owner) is approximate load shedding, not a global allowance or a cost cap. Workers rate limiting counts per Cloudflare location and is eventually consistent, so an owner whose visitors reach several locations gets up to that much at each one, and a burst can pass a little over it. A request it refuses has already run the Worker and is billed. It keeps one owner's traffic from crowding the others' at a location; the zone rule is what stops a flood before it costs anything.

The `localify.dev` zone needs two rules, so that nobody holding a tunnel token can serve pages on a machine hostname directly (docs/public-urls.md, "Edge rule"):

- **WAF custom rule, action Block:**
  `starts_with(http.host, "m-") and ends_with(http.host, ".localify.dev") and not any(http.request.headers["x-localify-edge"][*] eq "<EDGE_KEY>")`
- **Request Header Transform Rule, operation Remove, header `X-Localify-Edge`:**
  `starts_with(http.host, "m-") and ends_with(http.host, ".localify.dev")`

  Request header rules run in the `http_request_late_transform` phase, after the WAF custom rules, so the header never reaches the tunnel. The Worker sends `X-Localify-Edge` on every forwarded request and never calls the rules API: an operator sets both rules in the dashboard.

### Configuration

| Name | Kind | Value |
|---|---|---|
| `SIGNUP` | var | `allowlist` while invite-only, `open` once anyone may sign in |
| `ALLOWED_GITHUB_IDS` | var | under `SIGNUP=allowlist`, who may sign in |
| `TEAM_GITHUB_IDS` | var | who gets the `team` override at sign-in; stays once sign-up opens |
| `MAX_TUNNELS` | var | `950`; the sweep logs an error above 800. `0`, in the sandbox only, turns serving off |
| `POLAR_API_BASE` | var | `https://api.polar.sh`, or `https://sandbox-api.polar.sh` in `.dev.vars` |
| `POLAR_PRODUCT_PRO_MONTHLY`, `POLAR_PRODUCT_PRO_YEARLY` | var | Pro product ids, each with a USD and an EUR price |
| `POLAR_PRODUCT_ADDON_MACHINE`, `POLAR_PRODUCT_ADDON_URLS` | var | add-on product ids, each with a unit-based USD and EUR price |
| `ADDONS` | var | `off` until the organization allows multiple subscriptions, then `on`. Off refuses add-on checkouts (`409 addons_unavailable`); add-on subscriptions that exist still count |
| `POLAR_ACCESS_TOKEN` | secret | from Infisical; `dev` holds the sandbox one |
| `POLAR_WEBHOOK_SECRET` | secret | from Infisical, once the webhook endpoint exists; webhooks answer 503 without it |
| `EDGE_KEY` | secret | from Infisical; must match the `localify.dev` WAF rule |

The full table, with the Cloudflare and GitHub settings, is in [docs/public-urls.md](../../docs/public-urls.md), "Configuration".

## Sandbox

`env.sandbox` in `wrangler.jsonc` is a second Worker, `localify-cloud-sandbox`, for end-to-end billing tests against Polar's sandbox. It runs on `workers.dev` only, with its own D1 database (`localify-cloud-sandbox`), the sandbox products, `ADDONS=on`, and `SIGNUP=allowlist` with the founder's GitHub id alone. `TEAM_GITHUB_IDS` is empty there, so the account's plan is the one Polar's sandbox gives it.

It never serves. It shares the Cloudflare account with production, so a sweep that listed tunnels would find production's and delete them as orphans. Two things keep it away from them:

- `MAX_TUNNELS` is `0`: no slot is ever reserved (a machine with a URL gets `blocked` with `capacity`), and the sweep stops after the serving reconcile, before any Cloudflare call;
- it has no `CLOUDFLARE_API_TOKEN`: `pnpm cloud:deploy --env sandbox` never passes one, so any Cloudflare call would fail before it left the Worker.

Its public and machine domains are `localify-sandbox.invalid`, a name that never resolves, and `MACHINE_ZONE_ID` and `CLOUDFLARE_ACCOUNT_ID` are empty. Public URLs can be created and listed there, but not visited.

Both scripts take `--env sandbox`. The deploy passes the Worker secrets of Infisical's `dev` (`COOKIE_KEY`, `INGRESS_KEY`, `EDGE_KEY`, the sandbox `POLAR_ACCESS_TOKEN`, and `POLAR_WEBHOOK_SECRET` once it exists); both run wrangler as `prod`'s `CLOUDFLARE_DEPLOY_TOKEN`, since the sandbox lives in the same account.

```bash
# Once: create its D1, then put the printed id in env.sandbox.d1_databases[0].database_id.
cd apps/cloud && node scripts/infisical.mjs d1 create localify-cloud-sandbox --location weur

pnpm cloud:migrate --env sandbox
pnpm cloud:deploy --env sandbox     # prints https://localify-cloud-sandbox.<subdomain>.workers.dev
```

Then, in Polar's sandbox, create a webhook endpoint at `https://localify-cloud-sandbox.<subdomain>.workers.dev/v1/webhooks/polar` with `api_version` `2026-10`, subscribed to every `subscription.*` event and `customer.state_changed`. Put its `whsec_…` secret in Infisical `dev` as `POLAR_WEBHOOK_SECRET` (local development shares it, so a local Worker then needs that endpoint's deliveries, or `pnpm cloud:admin --local refresh`), and deploy again. Sign in with `LOCALIFY_CLOUD_API=https://localify-cloud-sandbox.<subdomain>.workers.dev localify login`.

## Sign-up, overrides and moderation

`SIGNUP` decides who may finish a new sign-in. Under `allowlist`, `ALLOWED_GITHUB_IDS` in [`wrangler.jsonc`](wrangler.jsonc) holds numeric GitHub user ids, comma-separated, because a login can be renamed and then taken by someone else. To find someone's id:

```bash
curl -s https://api.github.com/users/<login> | jq .id
```

The allowlist only gates sign-in. Serving takes an entitlement in both modes: a plan, or an override. Removing an id from the allowlist does not stop an account that is already signed in.

`TEAM_GITHUB_IDS`, in the same format, lists the team. When one of those ids signs in without an override and is not banned, the Worker gives it the `team` override: 5 serving machines, 10 URLs, no expiry. It never overwrites an override, so one you changed with `pnpm cloud:admin override` stays. The list outlives the allowlist, which goes when `SIGNUP` opens. It only grants: removing an id leaves its override, so remove the id first, then run `pnpm cloud:admin override --remove <github-login>`.

Overrides, bans and takedowns go through `pnpm cloud:admin` (docs/public-urls.md, "Moderation"), and `pnpm cloud:admin --help` lists them. Each one writes the production D1 as `CLOUDFLARE_DEPLOY_TOKEN` from Infisical, or with your own `wrangler login` when that key is missing, and prints what it changed. It reaches the public path within 10 seconds and closes the affected streams at the next sweep. `--local` runs it on the D1 of `pnpm cloud:dev`.

```bash
pnpm cloud:admin show teo-goulois
pnpm cloud:admin takedown journal-a8f3 "phishing page"
pnpm cloud:admin override teo-goulois 5 10 team --expires 2027-06-30
```

A duplicate Pro subscription is revoked automatically and logged as an error, but never refunded by the Worker. `pnpm cloud:admin duplicates` lists each one with its orders, read live from Polar: refund the orders it marks in Polar's dashboard (Sales → Orders, reason Duplicate), and run it again later for payments that were still pending. It writes nothing.
