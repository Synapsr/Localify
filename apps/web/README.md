# Localify website

The Next.js site behind [localify.dev](https://localify.dev): a landing page, a pricing page, terms, a privacy policy, and the two Localify Cloud pages Polar's checkout and customer portal return to, in nine languages. It sends people to the download and the source. There's no checkout and no accounts: the Localify Cloud trial starts with `localify login` and `localify subscribe`. Two API routes tell the team about plan interest on Discord (see "Contact and clicks").

## Prices

The landing and pricing pages show the same plan cards, [`app/_components/pricing-plans.tsx`](app/_components/pricing-plans.tsx), with Localify Cloud prices from [`lib/pricing.ts`](lib/pricing.ts). On each request it reads the visitor's country from Cloudflare's `CF-IPCountry` header (euros in the EU and EEA, US dollars elsewhere) and fetches `GET https://api.localify.dev/v1/pricing?currency=…`, kept an hour per Worker isolate. When the endpoint fails, it uses the built-in prices, so the site never breaks. The endpoint names currencies in upper case (`EUR`), and [`lib/pricing-response.ts`](lib/pricing-response.ts) accepts either case. Without the header, as in `pnpm web:dev`, prices show in US dollars. Team isn't sold through Polar, so its price and seats (`TEAM_PLAN`) are the site's own, in the visitor's currency.

## Contact and clicks

Two routes forward to the team's Discord, through the webhook in the `DISCORD_CONTACT_WEBHOOK_URL` secret:

- `POST /api/track` takes `{event: "plan_clicked", plan: "pro" | "team", locale, page}`, sent with `navigator.sendBeacon` ([`lib/track.ts`](lib/track.ts)) when someone clicks the Pro trial or Team button, and posts one line such as `👀 Team button clicked · fr · /pricing`. It carries no identifier.
- `POST /api/contact` takes the Team contact form that the Team button opens ([`app/_components/team-contact.tsx`](app/_components/team-contact.tsx)): `name` and `email` (required), `company`, `size`, `message`, `plan: "team"`, `locale`, `page`, and the `website` honeypot. It posts an embed with the fields. A filled honeypot gets `204` and nothing is sent.

Both check every field and cap the body (1 KB and 8 KB), refuse cross-site browser requests, and count requests per IP address with the `TRACK_LIMITER` (20 a minute) and `CONTACT_LIMITER` (5 a minute) rate-limit bindings in [`wrangler.jsonc`](wrangler.jsonc). They answer with a status and no body: `204` once Discord has the message, `400`, `403`, `429`, `502` when Discord fails, and `503` when the secret is missing. The form then offers the same message by email to support@localify.dev. Mentions are off in every post, and the webhook URL never reaches the browser or a log. [`lib/discord.ts`](lib/discord.ts) has what both share, and types the secret and the bindings on OpenNext's `CloudflareEnv`.

## Run

```bash
pnpm web:dev
```

## Build

```bash
pnpm web:typecheck
pnpm web:test
pnpm web:build
```

`pnpm web:build:cloudflare` builds the Cloudflare Worker with OpenNext and needs no secrets. `pnpm web:deploy` and `pnpm web:upload` publish to Cloudflare and need its credentials. They keep the Worker's secrets as they are.

## Environment

Nothing is required to run or build the site. It reads these public values at build time, listed in the root [`.env.example`](../../.env.example). Maintainers keep them in Infisical, project `localify`, folder `/web`, and deploy with `infisical run --env prod --path /web -- pnpm web:deploy`:

- `NEXT_PUBLIC_POSTHOG_TOKEN` turns on PostHog and its cookie banner in production. PostHog stores and sends nothing until the visitor accepts.
- `NEXT_PUBLIC_POSTHOG_HOST` defaults to `https://us.i.posthog.com`.
- `NEXT_PUBLIC_POSTHOG_DISABLED=true` turns PostHog and the banner off.
- `NEXT_PUBLIC_POSTHOG_DEBUG=true` turns PostHog and the banner on in `pnpm web:dev`.
- `NEXT_PUBLIC_LOCALIFY_DOWNLOADS_BASE_URL` overrides the download host, `https://downloads.localify.dev`.

The Umami script in [`app/layout.tsx`](app/layout.tsx) loads in production builds only.

The Worker has one secret, `DISCORD_CONTACT_WEBHOOK_URL`, the Discord webhook of "Contact and clicks". Maintainers keep it in Infisical, project `localify`, folder `/web`, environment `prod`, and set it once on the Worker:

```bash
infisical secrets get DISCORD_CONTACT_WEBHOOK_URL --env prod --path /web --plain \
  | pnpm --filter @localify/web exec wrangler secret put DISCORD_CONTACT_WEBHOOK_URL
```

Locally, `pnpm web:dev` reads it from `apps/web/.dev.vars` (see [`.dev.vars.example`](.dev.vars.example)). Point it at a local test endpoint, never at the real webhook. Without it, both routes answer `503`.

## Copy

[LANDING.md](LANDING.md) has the positioning and the rules for writing copy. [TECHNICAL-REFERENCE.md](TECHNICAL-REFERENCE.md) has what the product does. Check a claim against both, and against the code, before it goes on the page.

Messages live in [`messages/`](messages/), one file per locale: `de en es fr ja ko pt-BR ru zh-CN`. Every file must have the same keys. English is the runtime fallback, so a missing key shows English instead of failing the build.

## Privacy policy

The `Privacy` messages describe what the code does, so they change with it. Update all nine locales when you change any of these:

- [`lib/analytics.ts`](lib/analytics.ts), [`instrumentation-client.ts`](instrumentation-client.ts), and [`app/_components/cookie-banner.tsx`](app/_components/cookie-banner.tsx): website PostHog, its consent, and the download attribution token.
- [`app/layout.tsx`](app/layout.tsx): Umami.
- [`lib/pricing.ts`](lib/pricing.ts): the visitor's country, read to pick the currency.
- [`lib/discord.ts`](lib/discord.ts), [`app/api/`](app/api/), [`lib/track.ts`](lib/track.ts), and [`app/_components/team-contact.tsx`](app/_components/team-contact.tsx): what the contact form and the plan-button clicks send to Discord.
- [`desktop-ui/src/lib/analytics.ts`](../../desktop-ui/src/lib/analytics.ts): desktop analytics, error reports, and session replay.

## Licensing

The site's code is under the repository's AGPL-3.0-only license. The media in [`public/`](public/) and the Localify name and logo aren't. See [NOTICE](../../NOTICE).
