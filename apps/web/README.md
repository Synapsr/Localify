# Localify website

The Next.js site behind [localify.dev](https://localify.dev): a landing page, terms, and a privacy policy in nine languages. It sends people to the download and the source. There's no checkout, no accounts, and no API routes.

## Run

```bash
pnpm web:dev
```

## Build

```bash
pnpm web:typecheck
pnpm web:build
```

`pnpm web:build:cloudflare` builds the Cloudflare Worker with OpenNext and needs no secrets. `pnpm web:deploy` and `pnpm web:upload` publish to Cloudflare and need its credentials.

## Environment

Nothing is required to run or build the site. It reads these public values at build time, listed in the root [`.env.example`](../../.env.example). Maintainers keep them in Infisical, project `localify`, folder `/web`, and deploy with `infisical run --env prod --path /web -- pnpm web:deploy`:

- `NEXT_PUBLIC_POSTHOG_TOKEN` turns on PostHog and its cookie banner in production. PostHog stores and sends nothing until the visitor accepts.
- `NEXT_PUBLIC_POSTHOG_HOST` defaults to `https://us.i.posthog.com`.
- `NEXT_PUBLIC_POSTHOG_DISABLED=true` turns PostHog and the banner off.
- `NEXT_PUBLIC_POSTHOG_DEBUG=true` turns PostHog and the banner on in `pnpm web:dev`.
- `NEXT_PUBLIC_LOCALIFY_DOWNLOADS_BASE_URL` overrides the download host, `https://downloads.localify.dev`.

The Umami script in [`app/layout.tsx`](app/layout.tsx) loads in production builds only.

## Copy

[LANDING.md](LANDING.md) has the positioning and the rules for writing copy. [TECHNICAL-REFERENCE.md](TECHNICAL-REFERENCE.md) has what the product does. Check a claim against both, and against the code, before it goes on the page.

Messages live in [`messages/`](messages/), one file per locale: `de en es fr ja ko pt-BR ru zh-CN`. Every file must have the same keys. English is the runtime fallback, so a missing key shows English instead of failing the build.

## Privacy policy

The `Privacy` messages describe what the code does, so they change with it. Update all nine locales when you change any of these:

- [`lib/analytics.ts`](lib/analytics.ts), [`instrumentation-client.ts`](instrumentation-client.ts), and [`app/_components/cookie-banner.tsx`](app/_components/cookie-banner.tsx): website PostHog, its consent, and the download attribution token.
- [`app/layout.tsx`](app/layout.tsx): Umami.
- [`desktop-ui/src/lib/analytics.ts`](../../desktop-ui/src/lib/analytics.ts): desktop analytics, error reports, and session replay.

## Licensing

The site's code is under the repository's AGPL-3.0-only license. The media in [`public/`](public/) and the Localify name and logo aren't. See [NOTICE](../../NOTICE).
