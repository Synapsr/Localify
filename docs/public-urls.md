# Public URLs

The contract for public URLs and the paid Localify Cloud plan: what the cloud, the daemon, the CLI, the MCP server and the desktop app each do, and the messages between them. The decisions behind it are ADRs 0019 to 0027. The measurements that chose the transport are in `spikes/public-url/README.md` on the `feat/public-urls-spike` branch.

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

Anyone with a GitHub account can sign in. Serving public URLs takes a paid plan: Localify Cloud Pro, with a 7-day trial (see "Plans"). Not covered here: path exceptions (webhooks reach a protected URL only after the password), custom domains, temporary URLs, renaming a slug, several URLs per local domain, Docker or external domains, the online state of a URL served by another machine, request inspection, the Team plan.

## Names

- **Slug:** 3 to 32 characters, `a-z`, `0-9` and `-`, starting with a letter, not ending with `-`, never containing `--`. Unique across all users, deleted ones included (see "Data"). When not chosen with `--slug`, it follows the local name, so the public URL reads like the local one. The local name is every label of the local domain but the last, in order, joined with `-`, lowercased, every run of characters outside `a-z0-9` replaced by one `-`, leading digits and `-` removed, cut to 32 characters without a trailing `-`: `louez.localify` gives `louez`, `web.louez.localify` gives `web-louez`. The Worker takes the first free of: the local name, the local name with `-` and the owner's GitHub login (`louez-ryan-ede`), then the local name cut to 27 characters with `-` and 4 random hex characters (`url` when the name is empty). A candidate that breaks the slug rules, such as one shorter than 3 characters, is skipped. A slug is free when no live URL has it and no other user's deleted URL holds it.
- **Public host:** `<slug>.localify.fyi`, or `<slug>--<label>.localify.fyi` for a wildcard URL. The first `--` separates the slug from the label, so labels may contain `--` (`web--feature`). The slug comes first so that `--` never sits at positions 3–4 of the DNS label, which IDNA reserves (ADR 0022).
- **Local host:** `<local-domain>` for `<slug>`, and `<label>.<local-domain>` for `<slug>--<label>`. A label is `a-z`, `0-9` and `-`, does not start or end with `-`, and is at most `61 − slug length` characters so the public DNS label fits in 63. Longer names cannot be served; the CLI says so instead of truncating.
- **Exposable local domain:** a domain registered by a project in this machine's config, or a project's namespace `<project-id>.localify` even when no service uses it as its own domain. The namespace is what a wildcard URL usually targets: in a monorepo, `<slug>--web--feature.localify.fyi` reaches `web--feature.<project-id>.localify`. The project of a public URL is the one that registers its local domain or owns that namespace.
- **Machine:** one signed-in installation of Localify. Its tunnel hostname is `m-<machine-id>.localify.dev`. That name and its CNAME last as long as the machine. Its tunnels come and go (see "Tunnels").

## Plans

| Plan | Price | Serving machines | Public URLs | Remote access |
|---|---|---|---|---|
| None (signed in, no plan) | — | 0 | 0 | no |
| Pro | $7.99 a month or $79 a year (€7.99 or €79), after a 7-day trial with a card | 2 | 10 | yes |
| Add-on: machine | $3 a month per unit, monthly only | +1 per unit | — | — |
| Add-on: URLs | $2 a month per unit, monthly only | — | +10 per unit | — |

Each Polar product has a USD and an EUR price. Polar picks the currency from the IP address that creates the checkout, so the Worker passes the buyer's (see "Billing").

Add-ons ship after launch. The contract counts them from the start, so that adding them changes no table or response shape. An add-on counts only while the Pro plan is entitled. Add-ons have no trial. The Team plan appears on the pricing page as "contact us" and is not part of this contract.

- **Entitlement:** what an account may use right now: `servingMachines`, `publicUrls`, and `remoteAccess`, the relay between its machines ([remote-machines.md](remote-machines.md), "Entitlement"). It comes from its override when it has one that has not expired. Otherwise it comes from its Pro plan when the lifecycle table makes that plan entitled, plus its entitled add-ons. Otherwise it is zero. A banned account has none, whatever else it holds.
- **Override:** a row in `plan_overrides` with its own limits, remote access or not, a reason and an optional expiry. It replaces the plan entirely. The team gets one each: 5 serving machines, 10 URLs, remote access, reason `team`, no expiry. The people on the old allowlist got it in a single migration, and an id in `TEAM_GITHUB_IDS` gets it when it signs in without an override (see "Device flow"). An operator sets or removes any override with `pnpm cloud:admin override` (see "Moderation"). Deleting the row ends that access, unless the id is still in `TEAM_GITHUB_IDS`: its next sign-in creates the row again.
- **Serving slot:** a machine whose tunnel state is anything but `none` (see "Tunnels"). An account uses at most `servingMachines` slots. Signing in uses none. A machine signed in with no slot can still list, create and change URLs. It just carries no traffic.
- **Signed-in machines:** at most 10 per account, plan or not. They cost nothing: they hold no tunnel.
- **Suspended:** a URL that exists but does not serve. It is kept with its slug and password. It is suspended when its owner's entitlement is zero, or when it is beyond `publicUrls` (see "Downgrades"). It is taken down when an operator removes it for abuse. A taken-down URL shows a removal page, its owner cannot bring it back, and its slug is never released.

### Lifecycle

The Worker reads the user's subscriptions from Polar (see "Billing"). The Pro plan is the user's subscription to the monthly or the yearly Pro product. When there are several, it is an entitled one, ending latest, and otherwise the most recently started. Polar's statuses map as follows:

| Polar `status` | Other fields | Entitled | `plan.status` in `/v1/me` |
|---|---|---|---|
| `trialing` | — | yes | `trialing` |
| `active` | `cancel_at_period_end` false | yes | `active` |
| `active` | `cancel_at_period_end` true | yes, until `current_period_end` | `canceling` |
| `past_due` | less than 7 days after Polar's `past_due_at` | yes | `past_due` |
| `past_due` | 7 days or more after `past_due_at` | no | `past_due` |
| `canceled` | `ends_at` in the future | yes, until `ends_at` | `canceling` |
| `canceled` | `ended_at` set, or `ends_at` past | no | `none` |
| `unpaid`, `paused`, `incomplete`, `incomplete_expired` | — | no | `none` |
| no subscription | — | no | `none` |

A refund changes nothing by itself. When Polar revokes the subscription as well, the status above follows. An account with a live override reports `plan.status: "override"`. A ban turns every row into "no".

### Downgrades

Whenever a user's entitlement changes, the Worker applies it:

- **URLs:** the live URLs that serve are the `publicUrls` oldest by `created_at`, then by `id`, among those without `suspended_at` (below). The rest are suspended. Removing a URL lets the next one serve, so the owner chooses by removing.
- **Serving state:** `public_urls.suspended_at` records what the last reconcile found: null while the URL served, the time it stopped otherwise. A reconcile works out the serving URLs from the entitlement as above. For each owner, in one batch, it first sets `rotate_requested_at` on every machine of the owner that holds a slot and has no rotation pending, when one of the URLs that served and no longer does (suspended or taken down) still has `suspended_at` null. It then sets their `suspended_at`, and clears it on each URL that serves again. Every machine of the owner rotates, not only the URL's own: the set is resolved by that statement at write time, so a claim that moved the URL after the read cannot escape it, and once `suspended_at` records the stop, a retry or a late second reconcile marks nothing. An owner holds few serving machines, and stops are rare, so a short reconnect of the others is the price. The reconcile reads the entitlements and URLs first, so a change made in between can only cost an extra rotation or a wrong clear, which the next reconcile corrects. A URL serves only when it ranks within the limit and its `suspended_at` is null, everywhere the Worker decides it: the runtime, the public path (402 while the mark is set), the sweep, the API's `state`, and the claim. Suspending is immediate: a URL ranked beyond the limit stops serving before any reconcile has marked it. Resuming waits for a reconcile, which alone clears the mark, for a URL that now ranks within the limit: right away after a removal, or within 5 minutes if that reconcile fails. A URL with `suspended_at` set has therefore not served since its mark, which was written in the same batch as its owner's rotation, so nothing needs rotating for it again. The 5-minute sweep reconciles every owner with a URL before it builds its revocation queue, so a change no request announces still closes the streams of the URLs that stop serving within one sweep. That covers an override that expires, the end of a canceled period, and the end of the `past_due` grace. Creating and removing a URL reconcile its owner right away, and so does every `cloud:admin` command (see "Moderation").
- **Serving slots:** when an account holds more slots than `servingMachines`, the Worker releases the slots provisioned most recently (`tunnel_ready_at`, then machine `id`) until it fits.
- **Kept working:** listing, removing, protection changes, sign-out, `/v1/me` and billing, at zero entitlement too. Create, claim and serving answer `402 subscription_required` at zero, and `403 quota_exceeded` when above a non-zero limit.

## Cloud (`apps/cloud`)

One Cloudflare Worker on the Workers Paid plan, deployed with wrangler, with a D1 database. It runs two scheduled jobs:
- every 5 minutes: the tunnel sweep, webhook retries and the cleanup of revoked machines;
- every day at 03:00 UTC: billing reconciliation, and purging URLs deleted more than 30 days ago.

- `api.localify.dev`: the JSON API for the CLI, the daemon and the app, and Polar's webhooks (Worker custom domain).
- `*.localify.fyi/*`: public traffic (Worker route; the zone has a proxied `*` AAAA record).

### Configuration

| Name | Kind | Value |
|---|---|---|
| `GITHUB_CLIENT_ID` | var | the OAuth App's client ID (device flow, no secret) |
| `SIGNUP` | var | `allowlist` or `open`: who may finish a new sign-in |
| `ALLOWED_GITHUB_IDS` | var | under `SIGNUP=allowlist`, comma-separated numeric GitHub user ids that may sign in; logins can be renamed and reused, ids cannot |
| `TEAM_GITHUB_IDS` | var | the team, in the same format: each id gets the `team` override when it signs in without one; it stays once `SIGNUP` opens and the allowlist goes |
| `MAX_TUNNELS` | var | `950`: the most serving slots the whole account holds; an error is logged above 800 |
| `PUBLIC_BASE_DOMAIN` | var | `localify.fyi` |
| `MACHINE_DOMAIN` | var | `localify.dev` |
| `CLOUDFLARE_ACCOUNT_ID`, `MACHINE_ZONE_ID` | var | account, and the `localify.dev` zone |
| `POLAR_API_BASE` | var | `https://api.polar.sh`, or `https://sandbox-api.polar.sh` outside production |
| `POLAR_PRODUCT_PRO_MONTHLY`, `POLAR_PRODUCT_PRO_YEARLY` | var | the two Pro product ids; both carry a USD and an EUR price and a 7-day trial in Polar |
| `POLAR_PRODUCT_ADDON_MACHINE`, `POLAR_PRODUCT_ADDON_URLS` | var | the add-on product ids, unit-priced, without trial; empty until add-ons ship |
| `CLOUDFLARE_API_TOKEN` | secret | Tunnel Edit, DNS Edit (Infisical `localify` `/cloud`) |
| `EDGE_KEY` | secret | the value the Worker sends in `X-Localify-Edge` to machine hostnames; the `localify.dev` zone refuses those hostnames without it (see "Edge rule") |
| `COOKIE_KEY` | secret | signs access and warning cookies |
| `INGRESS_KEY` | secret | derives each machine's ingress secret |
| `POLAR_ACCESS_TOKEN` | secret | an organization access token with `checkouts:write`, `customer_sessions:write`, `subscriptions:read`, `subscriptions:write` and `orders:read`; refunds are made in the dashboard, so no refund scope |
| `POLAR_WEBHOOK_SECRET` | secret | the `whsec_…` secret of the webhook endpoint, a Standard Webhooks secret; optional, since the endpoint is created after the first deploy, and webhooks answer `503` without it |

In Polar, the organization turns on "Prevent trial abuse", and once add-ons ship, "Enable subscription unit changes" in the customer portal. The webhook endpoint is `https://api.localify.dev/v1/webhooks/polar` with `api_version: "2026-10"`, and subscribes to every `subscription.*` event and to `customer.state_changed`. Every REST call sends `Polar-Version: 2026-10`, so requests and webhook payloads share one contract. Polar removes a version about nine months after it ships (2026-10 is removed in April 2027), so the Worker and the endpoint move to a newer version together, before then.

`SIGNUP` only gates the device flow. Entitlements are enforced in both modes. Going back to `allowlist` stops new sign-ins, but accounts already signed in keep their tokens, their plans and their URLs.

`TEAM_GITHUB_IDS` only grants. Removing an id leaves its override in place: `pnpm cloud:admin override --remove <github-login>` ends it (see "Moderation"), once the id is out of the list.

### Data (D1)

```sql
users       (id TEXT PRIMARY KEY, github_id INTEGER UNIQUE NOT NULL, login TEXT NOT NULL, created_at INTEGER NOT NULL,
             trial_used_at INTEGER, banned_at INTEGER, banned_reason TEXT, serving_stopped_at INTEGER)
machines    (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), name TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN ('provisioning', 'active', 'revoked')),
             tunnel_state TEXT NOT NULL DEFAULT 'none'
               CHECK (tunnel_state IN ('none', 'provisioning', 'ready', 'releasing')),
             tunnel_generation INTEGER NOT NULL DEFAULT 0, tunnel_op TEXT, tunnel_op_at INTEGER,
             tunnel_id TEXT, tunnel_ready_at INTEGER, down_seen_at INTEGER, rotate_requested_at INTEGER,
             dns_record_id TEXT, ingress_port INTEGER NOT NULL,
             created_at INTEGER NOT NULL, revoked_at INTEGER,
             reconcile INTEGER NOT NULL DEFAULT 0, cleanup_attempts INTEGER NOT NULL DEFAULT 0, next_cleanup_at INTEGER)
tokens      (hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id),
             machine_id TEXT NOT NULL REFERENCES machines(id), created_at INTEGER NOT NULL)
public_urls (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), slug TEXT UNIQUE NOT NULL,
             local_domain TEXT NOT NULL, wildcard INTEGER NOT NULL, machine_id TEXT REFERENCES machines(id),
             protected INTEGER NOT NULL, password_hash TEXT, password_salt TEXT, password_version INTEGER NOT NULL,
             created_at INTEGER NOT NULL, deleted_at INTEGER, taken_down_at INTEGER, taken_down_reason TEXT,
             suspended_at INTEGER,
             CHECK (protected = 0 OR (password_hash IS NOT NULL AND password_salt IS NOT NULL)))
plan_overrides (user_id TEXT PRIMARY KEY REFERENCES users(id), serving_machines INTEGER NOT NULL,
             public_urls INTEGER NOT NULL, reason TEXT NOT NULL, expires_at INTEGER, created_at INTEGER NOT NULL)
billing     (user_id TEXT PRIMARY KEY REFERENCES users(id), polar_customer_id TEXT,
             pro_subscription_id TEXT, pro_status TEXT, pro_interval TEXT CHECK (pro_interval IN ('month', 'year')),
             cancel_at_period_end INTEGER NOT NULL DEFAULT 0, current_period_end INTEGER, trial_end INTEGER,
             ends_at INTEGER, ended_at INTEGER, past_due_at INTEGER,
             addon_machines INTEGER NOT NULL DEFAULT 0, addon_url_packs INTEGER NOT NULL DEFAULT 0,
             revision INTEGER NOT NULL, refreshed_at INTEGER NOT NULL)
billing_revisions (user_id TEXT PRIMARY KEY REFERENCES users(id), last INTEGER NOT NULL,
             check_due_at INTEGER, serving_due_at INTEGER)
checkouts   (user_id TEXT NOT NULL REFERENCES users(id),
             kind TEXT NOT NULL CHECK (kind IN ('pro', 'addon_machine', 'addon_urls')),
             state TEXT NOT NULL CHECK (state IN ('creating', 'open')), op TEXT,
             checkout_id TEXT, url TEXT, units INTEGER NOT NULL DEFAULT 1,
             expires_at INTEGER, created_at INTEGER NOT NULL,
             PRIMARY KEY (user_id, kind))
webhook_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, type TEXT NOT NULL,
             user_id TEXT, received_at INTEGER NOT NULL,
             processed_at INTEGER, attempts INTEGER NOT NULL DEFAULT 0, next_attempt_at INTEGER)
duplicate_subscriptions (subscription_id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id),
             revoked_at INTEGER, refunded_at INTEGER, attempts INTEGER NOT NULL DEFAULT 0,
             next_attempt_at INTEGER, created_at INTEGER NOT NULL)
CREATE UNIQUE INDEX one_url_per_local_domain ON public_urls (user_id, local_domain) WHERE deleted_at IS NULL;
CREATE INDEX live_urls_by_owner ON public_urls (user_id, created_at, id) WHERE deleted_at IS NULL;
CREATE INDEX slots_by_owner ON machines (user_id, tunnel_state) WHERE tunnel_state != 'none';
CREATE INDEX pending_webhooks ON webhook_events (next_attempt_at) WHERE processed_at IS NULL;
```

- **Secrets:** tokens are stored as SHA-256 hashes. Passwords use PBKDF2-SHA-256 with a random salt and 100,000 iterations. No tunnel token is stored: the Worker asks Cloudflare for it when a daemon needs a new generation.
- **Limits:** a user has at most 10 signed-in machines and 10 + 10 × `addon_url_packs` live URLs (or their override's count). A live URL is one that is neither deleted nor taken down. Its owner can still delete a taken-down URL, but its slug stays held. The limits are checked in the same statement as the insert, so two concurrent requests cannot both pass.
- **Deleted URLs:** deleting a URL sets `deleted_at` and keeps the row for 30 days, so its slug cannot serve someone else's page behind old bookmarks. Only its owner can take that slug again during that time. The daily job removes older deleted rows, except taken-down ones, which are kept with their slug forever.
- **Billing:** `billing` is a snapshot of the user's Polar subscriptions, written only by a refresh (see "Billing").
- **Refresh revisions:** `billing_revisions.last` is a per-user counter. Each refresh takes the next value, and only the winning revision may write. `check_due_at` queues the user for the reconciliation. `serving_due_at` records that a refresh changed the snapshot and owes the user's serving reconcile: set in the batch that writes the snapshot, cleared only in the batch of a reconcile that commits.
- **Checkouts:** `checkouts` holds at most one Pro checkout per user, for both intervals, and one per add-on, so a second request reuses it instead of starting a second purchase.
- **Webhook events:** `webhook_events` records each delivery under its `webhook-id`, so a repeated delivery is recognized and a failed one is retried. `seq` orders a user's events for the refresh's watermark. Processed events are deleted after 30 days.
- **Duplicate subscriptions:** `duplicate_subscriptions` records a duplicate Pro subscription before anything is done to it, so its revocation is retried until it is settled, and an operator knows what to refund (see "Billing"). `refunded_at` is not written: refunds are made in Polar's dashboard.
- **Trials:** `trial_used_at` is set the first time a refresh finds a `trial_start` on any of the user's Pro subscriptions, past ones included. From then on, every checkout for that user is created without a trial.
- **Migration:** the migration that adds these tables, run once, gives each existing user whose GitHub id is in `ALLOWED_GITHUB_IDS` an override (5 serving machines, 10 URLs, reason `team`). It marks every active machine that already has a tunnel as `ready` at generation 1. Generation 0 only ever means "no tunnel, no token". Those older tunnels are named `localify-<machine-id>` and are released like any other.

### API

Every request except the device flow carries `Authorization: Bearer <token>`. The token gives the user and the calling machine; the API never takes a machine id from the request. Every read and change is scoped to the token's user: an id that does not exist and an id owned by someone else both answer `404 not_found`. Errors are `{"error": "<code>", "message": "<sentence for a person>"}` with a 4xx or 5xx status. Field names are camelCase.

| Method and path | Body | Response |
|---|---|---|
| `POST /v1/auth/device` | — | `{deviceCode, userCode, verificationUri, interval, expiresIn}` |
| `POST /v1/auth/device/token` | `{deviceCode, machineName, ingressPort}` | `200 {token, user, machine}`; `428 authorization_pending`; `429 slow_down` with `{interval}`, the new polling interval; `403 access_denied` (the person declined on GitHub); `403 not_allowed` (under `SIGNUP=allowlist`); `403 banned`; `403 quota_exceeded` (10 signed-in machines); `410 expired_token` |
| `GET /v1/me` | — | `{user, machine, plan: Plan}` |
| `POST /v1/billing/checkout` | `{product: "pro_monthly" \| "pro_yearly"}`; after launch also `{product: "addon_machine" \| "addon_urls", units}` | `{kind: "checkout" \| "portal", url}`, the page to open in a browser. When the Pro plan is already entitled, a Pro request gets the customer portal (`kind: "portal"`), where the interval is switched, instead of a checkout. `409 checkout_in_progress` while another request of the same user is creating one (retry after 2 seconds); `402 subscription_required` for an add-on without an entitled Pro plan; `422 invalid_product`, for an add-on too until add-ons ship; `502 billing_unavailable` when Polar fails; `503 billing_busy` under Polar's rate limit. |
| `POST /v1/billing/portal` | — | `{url}`, a Polar customer portal session. `404 no_billing` when the user has never checked out. |
| `POST /v1/webhooks/polar` | Polar's signed delivery | `202`; `403` on a bad signature; `503` while `POLAR_WEBHOOK_SECRET` is not set. Not authenticated by a Localify token. |
| `POST /v1/auth/logout` | — | `204`, see "Signing out" |
| `GET /v1/public-urls` | — | `{publicUrls: [PublicUrl]}`, the user's URLs, deleted ones excluded |
| `GET /v1/slugs?localDomain=<domain>` | — | `{slug, url}`: the slug a new URL for that domain would get, and its URL, so clients never write the public domain themselves. `422 invalid_local_domain`. |
| `GET /v1/slugs/<slug>` | — | `{slug, available}`, the slug lowercased; free for the caller as defined in "Names". `422 invalid_slug`. |
| `POST /v1/public-urls` | `{localDomain, wildcard, slug?, protection: {mode: "password", password?} \| {mode: "public"}}` | `201 {publicUrl, password?}`. The calling machine serves it. `password` is present only when the server generated it. `409 slug_taken`, `409 local_domain_taken`, `422 invalid_slug`, `422 invalid_local_domain`, `402 subscription_required`, `403 quota_exceeded`. |
| `DELETE /v1/public-urls/{id}` | — | `204` |
| `PUT /v1/public-urls/{id}/protection` | `{protection}`, the same object as in `POST` | `{publicUrl, password?}`. Changing or removing the password signs every browser out. |
| `POST /v1/public-urls/{id}/claim` | — | `{publicUrl}`, now served by the calling machine, in one update. `402 subscription_required`; `403 taken_down`; `403 quota_exceeded` when the URL does not serve: beyond the owner's `publicUrls` oldest, or marked suspended until the next reconcile, checked in the same update. |
| `GET /v2/machines/self/runtime?generation=<n>` | — | `MachineRuntime`, see "Tunnels". `n` is the tunnel generation the daemon runs, or `0` when it runs none: generations start at 1, and 0 never names a tunnel. |
| `GET /v1/machines/self/runtime` | — | For clients before this contract. Runs the same reservation and provisioning as v2. When the state is `ready`, it returns `{tunnelToken, ingressSecret, publicUrls: [PublicUrl]}` with the current generation's token every time, and only the URLs whose state is `serving`, since those clients serve every URL they get. Otherwise it returns `409 runtime_not_ready` with `{state}`. Removed once no such client remains (see "Rollout order"). |

Error codes this contract adds, everywhere they apply:

| Status and code | Meaning | What the client says |
|---|---|---|
| `402 subscription_required` | the account's entitlement is zero | the plan is needed, how to start the trial (`localify subscribe`) |
| `403 banned` | the account is banned; every route but sign-out answers it | the account was suspended for breaking the terms, and where to write |
| `403 taken_down` | this URL was removed for abuse | the URL was removed, and where to write |
| `403 quota_exceeded` | above a non-zero limit | which limit, and how to raise it |
| `503 capacity` | Localify Cloud holds `MAX_TUNNELS` slots | serving resumes by itself as slots free up |

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
  // "serving": entitled and within the limits; "suspended": beyond them or
  // without a plan (see "Downgrades"); "taken_down": removed for abuse.
  state: "serving" | "suspended" | "taken_down";
  createdAt: string;           // RFC 3339
};
type Plan = {
  status: "none" | "trialing" | "active" | "past_due" | "canceling" | "override";
  entitled: boolean;
  interval: "month" | "year" | null;
  trialEndsAt: string | null;  // RFC 3339, while trialing
  renewsAt: string | null;     // next charge, when one is due
  endsAt: string | null;       // when "canceling" stops, or an override expires
  trialAvailable: boolean;     // false once trial_used_at is set
  // remoteAccess never moves `entitled`, which is about serving.
  limits: { servingMachines: number; publicUrls: number; remoteAccess: boolean };
  usage: { servingMachines: number; publicUrls: number };
  addons: { machines: number; urlPacks: number };
};
```

**Device flow.** The Worker calls `POST https://github.com/login/device/code` with the client id and no scope, then polls `POST https://github.com/login/oauth/access_token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`, both with `Accept: application/json`, and maps GitHub's `authorization_pending`, `slow_down` (adding 5 seconds to the interval it returns), `access_denied` and `expired_token` to the responses above. With an access token, it reads `GET https://api.github.com/user` for the id and login and discards the GitHub token. It then refuses a banned user, and under `SIGNUP=allowlist` an id outside `ALLOWED_GITHUB_IDS`. When the id is in `TEAM_GITHUB_IDS`, it inserts the `team` override (5 serving machines, 10 URLs, remote access, no expiry) unless the account already has one or is banned, for a new account and an existing one alike. It never overwrites an override, so one an operator changed is kept. Both device routes are rate limited per client IP. The CLI refuses `login` while the machine is signed in.

**Signing in registers the machine.** In one D1 batch, the Worker inserts the machine as `active` with `tunnel_state` `none` and issues the token. It makes no Cloudflare call: the tunnel comes when the machine first has something to serve (see "Tunnels"). The `provisioning` machine status remains only for rows from before this contract, and the cleanup revokes such a row after 10 minutes. If the user already has an active machine with the same name, the new one replaces it: the old machine is revoked like a sign-out, its URLs move to the new one, and its tunnel is released. That covers signing in again on a machine whose credentials were lost. The machine name is the computer's name, with ` (dev)` added under the dev profile so that a dev build never replaces the installed app's machine, cut to 64 UTF-16 units. The ingress secret is `HMAC-SHA-256(INGRESS_KEY, machine id)` in hex; it is never stored.

**Signing out.** In one D1 batch, the Worker deletes the token, marks the machine `revoked` and detaches its URLs (`machine: null`). It pushes `revoked` to the account's relay, which closes the machine's socket and tells its peers (remote-machines.md, "Unpairing, replacement, lost keys, revocation"); a same-name sign-in that replaces a machine pushes the same. Then, if there is a tunnel, it gives it a new secret, removes its connections and deletes it, and it deletes the DNS record if there is one. Whatever fails stays recorded on the revoked machine, and the 5-minute cleanup retries it. The Worker's own Cloudflare token does this work, so the user's revoked token is never needed again. A ban releases the user's tunnels the same way but keeps their tokens and URLs, so the API keeps answering `403 banned` and an unban restores everything.

### Tunnels

A machine's tunnel state, in `machines.tunnel_state`:

| State | Meaning | Holds a serving slot |
|---|---|---|
| `none` | no tunnel at Cloudflare | no |
| `provisioning` | an operation is creating tunnel `localify-<machine-id>-<generation>` | yes |
| `ready` | the tunnel exists, its ingress is set, and the CNAME points at it | yes |
| `releasing` | an operation is deleting the tunnel | yes, until Cloudflare confirms |

**Wanting a tunnel.** A machine wants one when it is active, its owner is entitled and not banned, and at least one of its URLs is `serving`. A `GET /v2/machines/self/runtime` from a machine that wants one, in state `none`, reserves a slot. One statement does the reservation:

- it sets `tunnel_state = 'provisioning'`, bumps `tunnel_generation`, and writes a random `tunnel_op` and `tunnel_op_at`;
- the row must still be `none`;
- the owner must hold fewer than `servingMachines` machines in any other state;
- the whole database must hold fewer than `MAX_TUNNELS` machines in any other state.

When the owner's limit stops it, the runtime is `blocked` with `serving_slots_full`. When the account cap stops it, the runtime is `blocked` with `capacity`. When the reservation succeeds, the request answers `provisioning` and the provisioning runs in `waitUntil`. A concurrent request sees `provisioning` and answers the same.

**Provisioning**, as the holder of `tunnel_op`:

1. Creates the named tunnel with `config_src: "cloudflare"`, or finds it by name when a retry already made it.
2. Records `tunnel_id`.
3. Sets its ingress to `m-<machine-id>.localify.dev → http://127.0.0.1:<ingressPort>`, with a final `http_status:404` rule.
4. Points the CNAME at `<tunnel-id>.cfargotunnel.com`: it creates the proxied record the first time and updates its content after that, recording `dns_record_id`.
5. Sets `ready`, `tunnel_ready_at = now`, `down_seen_at = NULL`, and clears `tunnel_op`.

Each write is conditioned on the row still holding this `tunnel_op` and this `tunnel_generation`. An operation that finds the row taken over deletes the tunnel it created and stops. An operation older than 2 minutes counts as abandoned: the sweep releases what it may have created, by name, and returns the row to `none`.

**Releasing**, as the holder of a new `tunnel_op`, from `ready` or from an abandoned `provisioning`:

1. Gives the tunnel a new `tunnel_secret` (`PATCH` on the tunnel), so its token stops working and `cloudflared` cannot reconnect before the delete.
2. Drops the tunnel's connections, which closes every open stream (ADR 0027).
3. Deletes the tunnel.
4. Sets `none` and clears `tunnel_id`, conditioned on the operation.

The batch that takes the row also stamps the owner's quiet period (see "Quiet period"). A release keeps the CNAME and `dns_record_id`: the next tunnel only moves the record. A machine is released when:

- it no longer wants a tunnel: its last URL was deleted, claimed elsewhere or suspended, or a runtime fetch found nothing to serve;
- its owner lost the entitlement or was banned (ADR 0027);
- a downgrade frees slots (see "Downgrades");
- the sweep reaps it;
- it signs out, which also deletes the CNAME.

**Rotating.** One of a machine's URLs can stop serving, through a takedown or a suspension, while the machine keeps serving others. Releasing would not be enough for that URL: its open WebSockets and SSE streams ride the same tunnel as the others. Whoever removes the URL's right (a refresh, `cloud:admin`, or the sweep's reconcile, see "Downgrades") sets `rotate_requested_at` on the machine. The Worker, right away or at the next sweep, then releases the machine's current generation as above. The machine still wants a tunnel, so its next runtime fetch provisions generation `n + 1`. The daemon's next fetch comes when `cloudflared` exits twice, which deleting the tunnel causes. The URLs that keep their right to serve are offline for those seconds. ADR 0027 covers this case.

**Quiet period.** The rule: no new tunnel generation of an owner's machines is reserved until 15 seconds after that owner's latest serving stop or tunnel release, measured on D1's clock, and that time never moves backwards. The public path caches each lookup for 10 seconds (see "Public traffic"), so without the rule a request routed before a URL stopped could reach the generation that replaces the old one, provisioned for the URLs that keep serving, and a stream it opens there would outlive the rotation. `users.serving_stopped_at` holds that time. It is written by D1 itself, as `MAX(COALESCE(serving_stopped_at, 0), <D1's now in milliseconds>)`, measured from the statement that stamps it, which commits within the same batch of indexed writes and comes as late in that batch as its guard allows. An older operation cannot shorten a newer period. A stamping batch that takes longer than 2 seconds is logged as an error, by the Worker and by `cloud:admin`. It is written in the batch that records a stop (see "Downgrades"), with a ban, and in the batch that claims any tunnel release, whatever its reason: a rotation, a lost entitlement, a machine with nothing left to serve, a reap. Revoked and replaced machines need nothing: their successor has a new id and hostname. The runtime and the reservation statement compare it with D1's clock in the same SQL. During the period the runtime answers `provisioning`, with `retryAfter` set to the seconds left. A routing decision on the public path, in turn, lives at most 10 seconds from the start of its D1 read, cached or not: before forwarding, a decision older than that is read again once, bypassing the cache, and made again. When that read fails, or itself ends more than 10 seconds after it started, the request gets 503 with `Retry-After: 10` and is not forwarded. Any decision that still says a stopped URL serves was therefore read before the stop committed, and has ended before a new generation can exist. The 15 seconds are the cache lifetime plus 5 seconds, from one constant shared with the cache. The margin covers the stamping batch's duration and the skew between a Worker's clock and D1's.

**Edge rule.** A machine hostname is public, and whoever holds its tunnel token could run their own origin behind it and serve arbitrary pages on `localify.dev`. The `localify.dev` zone therefore has two rules:

- a WAF custom rule that blocks every request to a host matching `m-*.localify.dev` unless its `X-Localify-Edge` header equals `EDGE_KEY`;
- a late request-header transform rule that removes `X-Localify-Edge` before the request reaches the tunnel.

The Worker sends that header on every forwarded request (see "Public traffic"). A tenant who learns the key gains nothing, since a victim's browser never sends it, and the tunnel never sees it.

**The sweep**, every 5 minutes, within a budget of 300 Cloudflare calls a run, below the account's limit of 1,200 calls per 5 minutes, and of 900 D1 queries, below the 1,000 a Worker invocation may send. D1 counts a batch as one query. The run reads every owner's entitlement and URLs in a fixed number of queries, whatever their number, and writes the reconcile in batches of about 150 statements, each owner's in one batch. D1 gives a batch 30 seconds and rolls it back whole on a failure, so a failed batch is logged, the others still run, and the next run retries it. The batches stop while fewer than 185 queries would be left: the slot reads, 60 revocations at 2 queries each, 60 for maintenance, and the end of the run. 60 revocations take at most 240 of the 300 calls, which leaves maintenance its 40:

1. Lists the account's tunnels, page by page.
2. For each `ready` machine, reads the status of its tunnel in that listing; a tunnel missing from a complete listing reads as down:
   - healthy (or degraded): clears `down_seen_at`;
   - down or inactive, with `down_seen_at` empty: sets `down_seen_at`;
   - down or inactive again, with `down_seen_at` and `tunnel_ready_at` both more than 15 minutes old: releases it;
   - a failed read: changes nothing.
3. Releases abandoned operations, machines that no longer want a tunnel, and `ready` machines whose `rotate_requested_at` is set, clearing it with the release. A machine is found to want no tunnel from the URLs the run read at its start, so its release also requires, as it takes the row, that no live URL is on the machine: a URL created during the run keeps its new tunnel.
4. Compares each `ready` machine's CNAME content with its `tunnel_id` and corrects it, since a generation check cannot stop a DNS call already on its way.
5. Deletes `localify-*` tunnels no machine row holds, once they are more than an hour old.
6. Logs an error when the database holds more than 800 slots.

Before all of this, the run reconciles the serving state of every owner with a URL (see "Downgrades"), so the rotations it marks are revoked in the same run. It also takes over any active machine that has a `tunnel_id` but `tunnel_state` `none`. A Worker from before this contract can sign such a machine in while a deploy rolls out. It becomes `ready` at generation 1 when it has a live URL, and `releasing` otherwise, so it holds a slot and its tunnel is released like any other.

Revocations come first in every run: releases and rotations caused by `rotate_requested_at`, a ban, a takedown or a lost entitlement. Each run attempts the first 60 whatever else waits, then more while maintenance keeps its share of 40 calls and 60 queries: the revoked-machine cleanup, the reaps, the CNAME corrections and the orphan tunnels. What maintenance leaves goes back to the revocations still queued. Revocations that keep failing therefore cannot hold maintenance back, nor maintenance the first 60. They never back off longer than one run. A revocation costs at most 4 calls (a lookup by name, the new secret, the connections, the delete), so at least 60 queued revocations are done in the next run, within 5 minutes. More than that, or Cloudflare failing, delays them by one run per 60. A revocation keeps `rotate_requested_at` set until Cloudflare confirms the delete. The queue takes the revocations not yet tried first, then the others by their last attempt, so one that keeps failing goes to the back of the queue without losing its urgency, and cannot starve the others. The budget counts every request actually sent, fallbacks included. The run logs an error for every revocation older than 10 minutes. The rest of the work is taken oldest `down_seen_at` first, with what the budget leaves. What it leaves undone waits for the next run, and a machine whose release keeps failing backs off like the revoked-machine cleanup.

**Runtime.** `GET /v2/machines/self/runtime?generation=<n>` answers:

```ts
type MachineRuntime = {
  state: "idle" | "provisioning" | "ready" | "blocked";
  // Present when state is "blocked".
  blocked: { reason: "subscription_required" | "serving_slots_full" | "capacity"; message: string } | null;
  // Present when state is "ready". token only when generation differs from the
  // daemon's: one Cloudflare call per new generation, none per poll.
  tunnel: { generation: number; token: string | null } | null;
  ingressSecret: string | null;   // present when state is "ready"
  publicUrls: PublicUrl[];        // the URLs this machine serves, with their state
  retryAfter: number | null;      // seconds, when state is "provisioning" or "blocked"
};
```

- `idle`: the machine wants no tunnel. Its owner may still be entitled.
- `provisioning`: a slot is reserved and the tunnel is on its way, including while the previous generation is being released for a rotation; `retryAfter` is 3. During an owner's quiet period (see "Quiet period"), no slot is reserved yet and `retryAfter` is the seconds the period has left.
- `ready`: serve with this generation's tunnel.
- `blocked`: the machine has something to serve but may not. The reason is the plan, its owner's slots, or the account cap. `retryAfter` is 60 for `capacity` and `null` otherwise, since only a plan or a URL change unblocks it, and the CLI and the app send `public_refresh` after either.

A banned owner gets `403 banned` instead of a runtime.

**Rollout order.** Clients already installed only know `/v1`, and treat anything but a 401 as a passing failure.

1. The cloud ships `/v2`, the lifecycle, and `/v1` running the same logic. Every user who serves today has an override, so nothing they see changes.
2. The daemon compatibility release ships `/v2`, the state table in "Daemon", and the terminal stop on `blocked` and `403 banned`.
3. Only after that release are users without an override able to reach `blocked` or `402`: either `SIGNUP` opens, or an override is removed.

Until then, an old daemon that is refused keeps retrying harmlessly. The cloud has already released its tunnel, so it serves nothing (ADR 0027).

### Billing

Polar is the merchant of record (ADR 0024). The Worker calls it at `POLAR_API_BASE` with `POLAR_ACCESS_TOKEN` and `Polar-Version: 2026-10`, from the API and the scheduled jobs only, never from the public path. The Polar customer's external id is always the Localify `users.id`. The Worker sets it itself, so a client never chooses it.

**Checkout.** `POST /v1/billing/checkout` maps `product` to its configured product ids, so a client can only buy those. A Pro request from a user whose Pro plan is entitled gets the portal instead (below). Otherwise, one checkout per user and kind exists at a time, and creating it is serialized through the `checkouts` row:

1. Claims the row: inserts `creating` with a random `op`, or takes over a row that is `open` and expires less than 5 minutes from now, or `creating` for more than 30 seconds. An `open` row that expires more than 5 minutes from now is returned as is. A Pro checkout lists both intervals, so a request for the other one reuses it, and the person picks the interval on Polar's page. A row another request is `creating` answers `409 checkout_in_progress`.
2. Calls `POST /v1/checkouts/` with the fields below. When the call fails, deletes the row it claimed, so the next request tries again at once.
3. Sets `open`, `checkout_id`, `url` and `expires_at`, conditioned on `op`, and returns the `url` that update stored. When another request took the row over in the meantime, the update stores nothing: the request returns the URL the row holds, or `409 checkout_in_progress` while that one is not ready. Its own checkout's URL is never given out; a subscription made through it anyway is a duplicate (below).

| Field | Value |
|---|---|
| `products` | for Pro, both Pro ids with the requested interval first, which Polar selects by default; for an add-on, its one id |
| `external_customer_id` | the user id |
| `allow_trial` | `false` once `trial_used_at` is set, and for add-ons |
| `metadata` | `{userId}` |
| `success_url` | `https://localify.dev/cloud/subscribed?checkout_id={CHECKOUT_ID}` |
| `return_url` | `https://localify.dev/pricing` |
| `units` | for an add-on |
| `customer_ip_address` | the request's `CF-Connecting-IP`, when present: Polar picks the currency, USD or EUR, from it, and would otherwise see the Worker's Cloudflare address |

The success page only says to go back to Localify: it grants nothing. Access starts when a refresh sees the subscription.

**Portal.** `POST /v1/billing/portal` calls `POST /v1/customer-sessions/` with `external_customer_id` and `return_url: https://localify.dev/cloud`, and returns `customer_portal_url`. It answers `404 no_billing` when the user has neither a `billing` row naming a Polar customer nor a `checkouts` row, and when Polar knows no such customer (a checkout never completed). Cancelling, the card, invoices, refunds requested through support, and (after launch) add-on quantities all live there.

**Webhooks.** `POST /v1/webhooks/polar`:

1. Reads the raw body and verifies `webhook-id`, `webhook-timestamp` and `webhook-signature` with `POLAR_WEBHOOK_SECRET`, Standard Webhooks style: the key is the base64 part of `whsec_<base64>`, and the signed content is `<webhook-id>.<webhook-timestamp>.<raw body>`. A bad signature, or a timestamp more than 5 minutes off, answers 403. While `POLAR_WEBHOOK_SECRET` is not set, it answers 503 and Polar retries.
2. `INSERT OR IGNORE`s the event into `webhook_events` with its user. The user is the customer's `external_id`: `data.customer.external_id` on `subscription.*`, `data.external_id` on `customer.state_changed`. An event that names no existing user is stored as processed, so a repeat is still recognized. A pending event gets `next_attempt_at` one minute ahead: the 5-minute job retries it only if the refresh of step 4 did not settle it.
3. Answers `202` within Polar's 10-second timeout, also for event types it ignores and for customers without an external id.
4. Refreshes that user in `waitUntil`. A refresh that fails leaves the event pending. The 5-minute job retries pending events with backoff (1 minute, doubling, up to 6 hours), and logs an error after 10 attempts.

The payload is never trusted for state: it only names who to refresh.

**Refresh.** For one user:

1. Takes the next revision: `UPDATE billing_revisions SET last = last + 1 … RETURNING last`, inserting the row at 1 the first time.
2. Captures the watermark: the highest `seq` among the user's stored webhook events. It also reads the user's recorded duplicates and the subscription `billing` keeps.
3. Calls `GET /v1/subscriptions/?external_customer_id=<user id>&limit=100`, page by page, every status included.
4. Works out the state:
   - picks the Pro subscription (see "Lifecycle");
   - sums the units of add-on subscriptions that are `active` or `trialing`;
   - copies Polar's `past_due_at`;
   - sets `trial_used_at` when any Pro subscription, past ones included, has a `trial_start`.
5. Commits in one batch. First, each only when its revision is newer than `billing.revision`, the subscription it keeps is not recorded as a duplicate, and the one `billing` keeps is not among its duplicates: `serving_due_at`, when the snapshot differs from the one `billing` holds; the duplicates (below); and the upsert of `billing`. Then, each conditioned on `billing.revision` being its own: `trial_used_at`, and clearing `check_due_at` when it was set before the refresh started.
6. When `serving_due_at` is set, by this refresh or one before it: when the account has no `remoteAccess` left, `revokeAccount` on its relay with this revision, which reads the account again and closes its sockets unless access came back meanwhile (remote-machines.md, "The relay object"); then the serving reconcile of the user (see "Downgrades"), whose rotations the sweep then releases (ADR 0027), and `forgetSite` for the user's slugs. Its batch reads the entitlement first, and every write in it, the clearing of `serving_due_at` included, holds only while `billing.revision` is still this refresh's. A refresh overtaken between that read and its batch therefore changes nothing, and leaves the debt to the newer one. A batch that fails fails the refresh.
7. When steps 5 and 6 have committed, marks processed the user's events with `seq` at or below the watermark.

Losing to a newer revision is not a failure: the newer refresh read Polar later, and marks the events. When the check of step 5 fails for its duplicates instead, another refresh recorded the subscription this one keeps, or keeps one this one would record: nothing was written, and the refresh starts again from step 1, three times at most. Events stored after the watermark stay pending and cause another refresh. A refresh that fails leaves its events pending for the retry, and sends the user to the back of the reconciliation queue. Polar's rate limit (`429`) is no failure: the events, and the user's turn in the reconciliation queue, wait what `Retry-After` asks, a minute at least, without counting an attempt. More than 5 pages of subscriptions for one user fails the refresh rather than reading half of them. Step 7 also waits for `serving_due_at` to be clear, read in the statement that marks: a refresh whose reconcile was a no-op because a newer one overtook it, or whose newer sibling's reconcile failed, leaves its events pending while the debt stands. The 5-minute job runs the serving reconciles still owed, every run, guarded by the revision `billing` then holds: those owed longest first, and one that fails goes to the back, so a few that keep failing cannot hold back the others.

**Duplicate Pro subscriptions.** A single pending checkout makes a second Pro subscription rare, but a second checkout opened from an old link could still create one. When a refresh finds two entitled Pro subscriptions:

1. It keeps the one `billing` already keeps, while that one is entitled. Otherwise it keeps the one ending latest: one that renews ends after one that is canceling, then the later `current_period_end`, the later `started_at` and the id break ties. A subscription already recorded as a duplicate is never the one kept, and the one `billing` keeps is never recorded (step 5 of "Refresh"), so two refreshes that cross cannot record both.
2. It records the others in `duplicate_subscriptions`, in the same commit as the snapshot, before calling Polar, and logs an error: the alert. Once revoked, a subscription no longer looks like a duplicate, so the record has to outlive the call.
3. The 5-minute job revokes it with `DELETE /v1/subscriptions/{id}` and sets `revoked_at`; Polar's `AlreadyCanceledSubscription` counts as revoked. Revocations come first in the run, on a Polar budget of their own (10 a run), so refresh work never holds them back. Those never tried go first, then the others by their next attempt: one that fails waits 5 minutes × 2^(attempts − 1), at most a day, behind the rest, so old failures cannot hold back a new duplicate. Polar's rate limit ends the run's revocations and waits what `Retry-After` asks, without counting an attempt. Each revocation logs an error again, naming the listing below.
4. An operator refunds, in Polar's dashboard. `pnpm cloud:admin duplicates` lists every recorded duplicate with its orders, read from Polar at that moment (`GET /v1/orders/?subscription_id=`): each order's status, what was paid, what was refunded and what is still refundable, with unpaid ones too, so a payment that completes after the revocation shows up on a later run. The command reads only.

Refunds are not automated. A duplicate takes a checkout opened from an old link, past the single pending checkout and the portal that a Pro user gets, so it should all but never happen. Refunding by hand keeps the Worker away from moving money: no refund lifecycle (pending, failed, reversed), no orders whose payment completes after the revocation, and no `refunds:write` scope on the token.

**Add-ons**, contracted and deferred: they need an entitled Pro plan. When the Pro plan stops being entitled, the refresh cancels them at period end, so they stop charging for nothing.

**Reconciliation.** Once a day, the job sets `check_due_at` for every user who has a `billing` row or a `checkouts` row. It also pages through the organization's subscriptions (`GET /v1/subscriptions/?limit=100`) and does the same for each `customer.external_id` that is a user. That catches a customer whose state changed without any event reaching us. It reads at most 20 pages a day: the first, which holds the newest subscriptions, then a window over the others that moves each day, so every page is read within a few days whatever their number. A user already queued keeps their place. The 5-minute job refreshes the users with a webhook retry due, then those due for the check, oldest first, within its budget (25 refreshes, 80 D1 queries and 40 Polar calls a run), and clears `check_due_at` when a refresh commits. A `429` from Polar ends the run's refreshes. Every user is therefore checked against Polar at least once a day, so a missed webhook costs at most a day, and the 7-day `past_due` grace ends on time.

### Moderation

`pnpm cloud:admin <command>` runs the operator's actions against the production D1, through wrangler with `CLOUDFLARE_DEPLOY_TOKEN` from Infisical, or the operator's own wrangler login when that key is missing; `pnpm cloud:admin --local <command>` runs them against the local D1 of `pnpm cloud:dev`. Each command writes D1 only (`duplicates` writes nothing and reads Polar), binds every value as a quoted literal (wrangler takes no parameters), and prints what it changed. New requests feel it within the public path's 10-second cache. A command that touches an account or a URL sends its change and the reconcile of that owner (see "Downgrades") in one `wrangler d1 execute` call, which D1 runs as one batch, so a failure applies none of it. A takedown also stops its URL itself, deleted or not, with the same owner-wide rotation, guarded by the URL's `suspended_at` at write time: a takedown written without its rotation is repaired by running it again, and a complete one changes nothing. The reconcile runs even when the change was already made: retrying a command repairs a rotation an earlier attempt missed, and the sweep's own reconcile repairs it within 5 minutes anyway. The next sweep releases or rotates the affected tunnels, which closes open streams within 5 minutes while Cloudflare answers and fewer than 60 revocations are queued (see "The sweep"; ADR 0027). There is no admin route on the Worker to protect.

- `show <github-login>`: the ban, the override, the billing snapshot, the entitlement and each URL's state;
- `ban <github-login> <reason>` and `unban <github-login>`;
- `takedown <slug> <reason>` and `restore <slug>`, which undoes a takedown;
- `override <github-login> <serving-machines> <urls> <reason> [--expires <date>] [--no-remote-access]`, which gives remote access unless told otherwise, and `override --remove <github-login>`;
- `refresh <github-login>`, which sets `check_due_at` so the next 5-minute run refreshes billing;
- `duplicates`, which lists every recorded duplicate Pro subscription and its orders, live from Polar, for the operator to refund in Polar's dashboard (see "Billing"). It writes nothing, and calls Polar with the production `POLAR_ACCESS_TOKEN` from Infisical, or with `--local`, the sandbox and `POLAR_ACCESS_TOKEN` from the environment.

A login is the one GitHub gave at the last sign-in, so two accounts can share one; `id:<github-id>` names either of them.

Reports arrive at `abuse@localify.dev`, linked from every page the Worker serves.

### Public traffic

For a request to `*.localify.fyi`, the Worker:

1. Parses the host into slug and label. It loads the public URL with its machine and tunnel state, and its owner's GitHub login, ban and entitlement, including the URL's rank among the owner's live URLs. Lookups are cached in the isolate for 10 seconds, so a deleted URL, a new password, a claim, a plan change, a ban or a takedown reaches every location within that time. New requests feel it then. Streams already open close when the tunnel is released (ADR 0027). A decision held longer than 10 seconds from its read, behind the traffic limit or a slow check, is made again before forwarding, and no machine starts a new tunnel generation while a decision made before its owner's latest stop could still be in use (see "Quiet period").

   | Case | Page |
   |---|---|
   | unknown or deleted slug, a label on a non-wildcard URL, an invalid label | 404 |
   | URL taken down, or owner banned | 410 removal page |
   | URL `suspended`: no entitlement, or ranked beyond `publicUrls` | 402 "paused" page, which says its owner's plan does not cover it right now |
   | no serving machine, or its tunnel not `ready` | 503 "offline" page |
2. Applies the owner's traffic limit: `OWNER_LIMITER`, keyed by the owner's user id, 2,000 requests per 60 seconds across all their URLs. That absorbs a Vite page loading hundreds of modules at once. It is approximate load shedding per Cloudflare location, not a global allowance or a cost cap: its counts are kept per location and are eventually consistent, and a request it refuses has already run the Worker and is billed. Over it: 429 with `Retry-After: 60`, a page for a navigation (as in step 4) and a plain body otherwise. A Cloudflare rate-limiting rule on the `localify.fyi` zone also limits each client IP, before the Worker runs, so a flood from one address costs no Worker requests; `apps/cloud/README.md` gives the rule.
3. Refuses cross-origin requests that could change state. Every URL under `localify.fyi` is the same site for a browser, so `SameSite` does not stop another user's page from posting to this one. For any method other than `GET`, `HEAD` and `OPTIONS`, and for every WebSocket handshake, an `Origin` header that is present must belong to the same public URL: `<slug>.localify.fyi` or `<slug>--<label>.localify.fyi` with this URL's slug. A page whose referrer policy is `no-referrer` sends `Origin: null` even to itself; that passes only with `Sec-Fetch-Site: same-origin`, which the browser sets and a page cannot. Otherwise: 403. A request without `Origin` is not a browser's cross-origin request and goes on to the access check. The Worker's own pages use `Referrer-Policy: same-origin`, so their form sends its origin.
4. Shows the warning page (ADR 0026) to a browser opening the host for the first time, on protected and open URLs alike.
   - **Who gets it:** a navigation, of any method, whose `Sec-Fetch-Dest` is `document`, `iframe`, `frame`, `object` or `embed`, or a `GET` without `Sec-Fetch-Dest` whose `Accept` includes `text/html`, when it has no valid `__Host-localify-notice` cookie and no `Localify-Skip-Warning` header. A browser cannot add that header to a navigation, so only scripts and tools can skip the page. The fallback on `Accept` is for `GET` only, since a webhook may ask for HTML too. A form posted by a navigation before the warning gets the page; `Continue` then comes back to its path with a `GET`, and the posted body is not kept.
   - **The page:** 200, HTML. It says this is a development preview served from a personal computer through Localify, by GitHub user `<login>`. It says to continue only when the person trusts whoever sent the link, and never to enter passwords or payment details for another site there. It links to `abuse@localify.dev`. Its `Continue` button posts to `/__localify/notice` with a same-host relative `next` and a form token `notice` = `<expiry>.<hex HMAC-SHA-256(COOKIE_KEY, "notice-form"|host|expiry)>`, valid 1 day. The Worker takes it only with an `Origin` of this exact host (or `Origin: null` with `Sec-Fetch-Site: same-origin`), so a sibling label of the same URL cannot acknowledge the warning for this host. A missing or stale token, or another origin, gets the page again with a fresh token (403). A valid one sets `__Host-localify-notice` = `<expiry>.<hex HMAC-SHA-256(COOKIE_KEY, "notice"|host|url id|expiry)>`, valid 7 days, `Secure; HttpOnly; SameSite=Lax; Path=/`, and redirects with 303. `next` is resolved against the host and kept only when it stays on that origin and its resolved path does not start with `//`; the redirect carries that resolved path and query, `/` otherwise.
   - **Everything else passes:** assets, `fetch`, HMR, WebSockets and webhooks.
   - **Caching:** every app response whose `Content-Type` is `text/html`, of any status and with or without the notice cookie, gets `Cache-Control: no-store` in place of what the app sent, and loses any `Expires`. Otherwise a shared cache could serve it to a visitor without the cookie, or a browser could reuse it after the cookie expires. The app's response to any request without a valid notice cookie also gets `Vary: Sec-Fetch-Dest, Sec-Fetch-Mode`, so a page that fetched it first cannot hand it to a later navigation. Other responses keep the app's caching, and WebSocket upgrades pass unchanged.
   - **Framing:** every Worker page sends `Content-Security-Policy: frame-ancestors 'none'`, so it cannot be framed and clicked through. The one exception is the warning for a framed navigation (`iframe`, `frame`, `object`, `embed`): it has no `Continue` form, only a link that opens the same URL top level in a new tab (`target="_blank" rel="noopener"`), where the person acknowledges the warning before reloading the frame. It sends `frame-ancestors https://*.localify.fyi`, so a Localify app can embed another URL.
5. Checks access when the URL is protected. Access is the cookie `__Host-localify-access` = `<expiry>.<password version>.<hex HMAC-SHA-256(COOKIE_KEY, host|url id|version|expiry)>`, valid 7 days, `Secure; HttpOnly; SameSite=Lax; Path=/`, set for the exact host (each wildcard label signs in separately). Without it, `GET` and `HEAD` return the password page (401, HTML) and other methods a plain 401, WebSocket handshakes included. The page posts to `/__localify/access` with the password and a same-host relative `next` path; a correct password sets the cookie and redirects with 303. Attempts are rate limited per URL and client IP. A link that carries the password is `https://<host>/__localify/access#<password, percent-encoded>`: the fragment never reaches a server or the app, and link previews see only the password page. On a URL without a password, `/__localify/access` answers `303` to `/#`, whose empty fragment replaces the one an old link carries; the path never reaches the app. `GET /__localify/access` on a protected URL serves that page with `next=/` and one inline script, allowed by a per-response CSP nonce. The script reads the fragment and removes it from the address bar and history. It submits the form only when the notice cookie is valid. Without that cookie, the page shows the warning's text with a `Continue` button, and the form posts the password with the warning's form token as `notice`, setting both cookies. The token counts under the same rules as on `/__localify/notice`; otherwise only the password is checked. A link that carries the password thus waits for one click from a browser that has not seen the warning. That path never redirects, since a browser would carry the fragment to the app's page.
6. Forwards to `https://m-<machine-id>.localify.dev<path>?<query>` with the method, body and headers, after removing:
   - `Host`, `Forwarded`, `X-Real-IP`;
   - every `X-Forwarded-*`, `X-Localify-*` and `CF-*` header;
   - the `Localify-Skip-Warning` header and any incoming `X-Localify-Edge`;
   - the access and notice cookies from `Cookie`. It adds:

| Header | Value |
|---|---|
| `X-Localify-Edge` | `EDGE_KEY`, which the `localify.dev` zone checks and then removes (see "Edge rule") |
| `X-Localify-Ingress` | the machine's ingress secret |
| `X-Localify-Public-Host` | the public host |
| `X-Localify-Local-Host` | the local host |
| `X-Localify-Url-Id` | the public URL id |
| `X-Localify-Client-Ip` | `CF-Connecting-IP` |

   WebSocket upgrades pass through the same `fetch`. Cloudflare rewrites `X-Forwarded-Host` on the way to the tunnel, which is why the public host travels in a Localify header.
7. Streams the response back with `redirect: "manual"`, after removing the `Domain` attribute from every `Set-Cookie`, so an app's cookies stay on its own host instead of reaching every URL under `localify.fyi`. Script can still set a cookie for the parent domain. Isolating apps from each other completely needs `localify.fyi` on the Public Suffix List; ADR 0026 says why that risk is accepted until then.

## Daemon

### Credentials

`cloud.json` in the data directory, written with `write_private_file` (0600). The CLI, the app and the daemon all read it; the daemon runs as the user but outside the login session, so the Keychain is not an option.

```json
{ "apiBase": "https://api.localify.dev", "token": "lfy_…", "login": "teo-goulois",
  "machineId": "…", "machineName": "teo-laptop" }
```

A signed-in machine always talks to the `apiBase` it signed in with, so its token never goes to another endpoint by accident. `LOCALIFY_CLOUD_API` only chooses the API for a new sign-in, for development against `wrangler dev`; plain `http://` is accepted only for a loopback host.

Signing out calls the API, then deletes the saved passwords (below) and the file, then sends `public_refresh`. When the API is unreachable, both are deleted anyway after a warning, and the cloud's cleanup never needs them. Saved passwords that cannot be deleted add a warning and do not stop the sign-out.

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
5. Hands the request to the router with `Host` set to the local host. A `GET` or `HEAD` (WebSocket handshakes included) on a Next.js dev path, `/_next/…` or `/__nextjs…`, whose single `Origin` is the request's own public origin gets `Origin: https://<local host>`: Next.js answers 403 to its own scripts and fonts, and refuses its HMR socket, from a host name missing from `allowedDevOrigins`, which Localify fills with local domains. The origin carries no port, so it is the local one only on the default HTTPS port; Next.js compares host names only. Every other request keeps the public `Origin`, which apps compare with the public `X-Forwarded-Host`: Server Actions, or a Socket.IO server that allows only its public origin. Also a public source carrying the public host and client IP, and the runtime state it was checked against, which the router routes it with: a config reload in between cannot send it to a registration the checks never saw. The proxy then sets `X-Forwarded-Host` to the public host, `X-Forwarded-Proto: https`, `X-Forwarded-Port: 443`, `X-Forwarded-For` to the client IP and `X-Localify-Local-Domain` to the local host, for HTTP and WebSocket alike. The client address the router sees is the public client IP, never loopback.
6. Holds one of 32 permits from the moment the request is admitted until its response head arrives. Over HTTP/2 a browser sends a whole module graph at once, and the proxy opens one upstream connection per request; without the cap the dev server's accept backlog overflows. Releasing at the head keeps SSE and other long responses from starving the rest.

Errors on public traffic use generic pages: no project paths, no commands, no config details. Public responses get neither the launch profile indicator, whose script lives under `/.localify/`, nor the `X-Localify-Profile` header. Request log lines for public traffic add `source=public public_host=<host>`.

### Tunnel

When signed in, this machine serves at least one public URL, and the ingress listens on its port, the daemon runs `cloudflared --config <data dir>/cloudflared.yml tunnel --no-autoupdate --protocol http2 --metrics 127.0.0.1:<ingress port + 10> run` with the tunnel token in the `TUNNEL_TOKEN` environment variable, never on the command line. It restarts it with backoff when it exits. It stops it when the runtime is no longer `ready`, when the credentials are gone, when the API answers 401 or `403 banned`, and on daemon shutdown. Stopping also drops the ingress secret. HTTP/2 rather than QUIC: on a network that blocks UDP, `cloudflared` would try QUIC for about two minutes before falling back. The config file holds only `{}`: without one, `cloudflared` would read `~/.cloudflared/config.yml`, which belongs to the person's other tunnels. Its output goes to `cloudflared.log` next to the daemon log, truncated at each start. Without the ingress, the tunnel would hand public traffic and the ingress secret to whatever program holds the port, so the status shows the error instead.

`cloudflared` is downloaded on first use from its GitHub release, at a version pinned in the code, into `<data dir>/bin/cloudflared-<version>`, and its SHA-256 is checked against a value pinned next to the version before it is made executable. A mismatch deletes the file and reports an error. Supported: macOS arm64 and x86_64, Linux x86_64 and arm64.

The daemon fetches `GET /v2/machines/self/runtime?generation=<the generation it runs, or 0>` at these times:

- at start, every 5 minutes, and when the management socket receives `public_refresh`;
- after `retryAfter` seconds when the answer gives one, plus up to 20% jitter;
- right away when the machine wakes from sleep (the wall clock moved more than a minute past the monotonic one between two ticks);
- when `cloudflared` exits twice within 2 minutes. That is how a laptop learns its tunnel was reaped while it slept. These refreshes happen at most once every 30 seconds.

`public_refresh` answers at once and the fetch runs in the background, because the management client waits 2 seconds and a cloud call may take 20. The CLI and the app send it after every change, including a plan becoming entitled.

| Answer | The daemon |
|---|---|
| `ready`, same generation | keeps `cloudflared` running; updates the served URLs and the ingress secret |
| `ready`, new generation (with its token) | stops the old `cloudflared`, then starts one on the new token, and keeps the ingress secret throughout. A new generation exists only once the old tunnel is deleted, so the old process carries nothing, and two `cloudflared` cannot share the metrics port. |
| `provisioning` | runs no `cloudflared`, keeps the ingress secret dropped, asks again after `retryAfter` |
| `idle` | stops `cloudflared` and drops the ingress secret |
| `blocked` | stops `cloudflared` and drops the ingress secret, and shows `blocked.message`. This is final until a plan or URL change, or until `retryAfter` for `capacity`. |
| `401` | stops everything: the machine is signed out (as before) |
| `403 banned` | stops everything, stays signed in, and says the account is suspended |
| network error, `5xx`, `429` | keeps serving what it already had, and retries from 10 seconds, doubling up to 5 minutes |

The management command `public_status` describes this machine only:

```json
{ "signedIn": true, "tunnel": "stopped" | "starting" | "connected" | "error", "connections": 4,
  "runtime": "idle" | "provisioning" | "ready" | "blocked" | "banned" | null,
  "blocked": null | { "reason": "subscription_required" | "serving_slots_full" | "capacity", "message": "…" },
  "error": null, "servedUrls": ["…id"] }
```

`runtime` is `null` before the first answer. The new fields are added next to the old ones, so an older app that reads `public_status` keeps working.

`public_urls` answers `{ "urls": [PublicUrl], "machineId": "…" | null }`, the URLs of the last `ready` runtime, at once and without calling the network. They are empty unless `cloud.json` still holds the account and machine they were fetched for, so a sign-out or a sign-in as someone else hides them before the next fetch. `localify dev` reads it to trust the origins of the URLs this machine serves for the service ([README](README.md#apps-that-check-origin)); a daemon that does not know the command, or one signed out, makes it trust none.

`connections` comes from the `/ready` endpoint of the metrics server. A connected tunnel does not mean the dev server behind a URL is running.

## CLI

Human output on a terminal, JSON when stdout is not a terminal or with `--json`. Nothing prompts when stdin is not a terminal. Every failure says what to do next.

```bash
localify login                      # device flow: prints the code and URL, opens the browser on a terminal, waits
localify logout
localify whoami                     # login, machine, and the plan: status, dates, limits and usage
localify subscribe [--yearly]       # opens the Polar checkout for Pro (monthly unless --yearly), waits for the plan
localify billing                    # opens the Polar customer portal
localify public-url list
localify public-url create <local-domain> [--wildcard] [--slug <slug>] [--password <p> | --public]
localify public-url remove <url-or-slug-or-id> [--yes]
localify public-url protect <url-or-slug-or-id> [--password <p>]   # sets or rotates; generates when omitted
localify public-url unprotect <url-or-slug-or-id> [--yes]
localify public-url claim <url-or-slug-or-id>
```

`create` checks that the local domain is exposable on this machine (see "Names") before calling the cloud; without `--slug`, Localify Cloud picks the suggested name, which mirrors the local one. It then prints the URL, the password when one was generated, and the link with the password. `protect` prints the same. A generated password is 4 lowercase words from a fixed list, joined by `-`. `list` leaves the link out, as it may be on a shared screen; JSON output carries it as `passwordLink` on every URL whose password this machine set. `list` also shows each URL's `state`, and why a suspended one does not serve.

`subscribe`:

1. Needs a sign-in.
2. Calls `POST /v1/billing/checkout`.
3. Prints the URL, and opens it on a terminal.
4. Polls `GET /v1/me` every 5 seconds for up to 15 minutes, until `plan.entitled`.
5. Sends `public_refresh` and prints the plan.

When it gets the portal instead (`kind: "portal"`), it says the plan is already active and opens the portal, where the interval can be switched. Without a terminal, it prints the URL (JSON: `{kind, url}`) and returns without waiting.

`billing` calls `POST /v1/billing/portal` and opens the URL the same way. `whoami` prints the plan the way `/v1/me` gives it, for example:

- `Pro (trial), ends 14 Nov 2026 · 1 of 2 serving machines · 3 of 10 URLs`;
- `No plan: public URLs need Localify Cloud Pro. Start a 7-day trial with localify subscribe.`

Every `402 subscription_required` from `create` or `claim` prints that same line, `blocked: serving_slots_full` names the machines that hold the slots, and `403 banned` says the account is suspended and gives `abuse@localify.dev`.

## MCP

One tool per command except `login`, `logout`, `unprotect`, `subscribe` and `billing`: signing in, opening a URL to everyone and paying are for a person to decide (ADR 0018). When a tool gets `402 subscription_required`, its error tells the agent the plan's price and trial, and to ask the user to run `localify subscribe`. An agent never starts a checkout or passes on a checkout link it made. `public_url_create` has no `public` option, so an agent always creates a protected URL and receives the link with its password, and the password, to pass on. The instructions steer an agent that wants several services or worktrees of a project to one wildcard URL on the project's namespace. `public_url_remove` is destructive.

## Desktop app

- **Settings → Localify Cloud:** sign in (shows the code, opens github.com), who is signed in, sign out, and this Mac's tunnel status, with the reason when the runtime is `blocked`.
  - **Plan card**, from `/v1/me`:
    - its status in words: no plan; trial, ends `<date>`; Pro monthly or yearly, renews `<date>`; ending `<date>`; payment failed, update the card; team access;
    - serving machines and URLs used against the limits;
    - one action. Without a plan: `Start 7-day free trial`, with a monthly / yearly choice ($7.99 / $79), or `Subscribe` once the trial is used. With a plan: `Manage billing`.
  - **After a checkout or the portal opens:** both open in the browser through `DesktopApi`. The app polls `/v1/me` every 5 seconds for 15 minutes, and again whenever its window regains focus. When the plan becomes entitled, it sends `public_refresh`.
- **Domain detail → Public URL:** one URL per project, as locally. Opened on the project's namespace or on any domain under it, the dialog creates or shows the project's wildcard URL on the namespace; any other domain gets a URL of its own. The project menu opens it on the namespace, even when no service registers it, as in a monorepo.
  - **Paywall:** without an entitled plan, the dialog shows no create form. It says what a public URL does, the price, the 7-day trial and that a card is asked. It has the same trial button as the plan card. URLs already created show their state, so a suspended one says why. When this Mac has something to serve but every serving slot is taken, the dialog names the machines holding them. It offers to remove a URL there or to serve it from this Mac, and after launch to add a machine.
  - **Create:** the exact public URL of the opened domain at the top (`https://<slug>--<label>.localify.fyi` under the namespace), with the project URL below it, from `GET /v1/slugs?localDomain=`. A pencil opens the name: an input with `.localify.fyi` after it, checked as `PublicSlug` checks it and then with `GET /v1/slugs/<slug>` 300 ms after the last key, showing Available, Taken or the format rule, and a button back to the suggestion. Creating waits for a free name; a name taken in the meantime reopens the editor on it. A domain whose label does not fit beside the chosen name (61 characters minus its length) is said inline. One Password switch, on by default: the password is generated and shown once with its link; off says that anyone with the link opens the project's services. There is no custom password at creation; it can be changed after.
  - **Details:** for a domain under a project URL, its own host (`<slug>--<label>`) with copy, open and the link with the password on that host, since each host signs in separately, and the project URL below. A project URL made before this, without wildcard, says it opens only the namespace and that removing it and creating it again reaches every service and worktree. Then the password shown once after rotate with buttons to copy it and the link with it, protection on and off, remove. When this Mac serves the URL, its state comes from this Mac's tunnel; when another machine serves it, the section says "Served by <machine>" with a button to serve it from this Mac, and claims nothing about whether it is online.
  - **Badges:** a globe beside each domain a URL reaches, its own or the project's wildcard one, opening the dialog.

All of it goes through `DesktopApi`, which calls the same application use cases as the CLI. The app does no HTTP itself.
