---
type: improved
link: docs/README.md#apps-that-check-origin
---

## Better Auth trusts your public URL automatically

An app opened through its public URL no longer answers `Invalid origin` from Better Auth.
`localify dev` sets `BETTER_AUTH_TRUSTED_ORIGINS` to the exact public origins that reach
the service, after any value from your shell or launch profile, and sets
`LOCALIFY_PUBLIC_URL` unless you did. When an env file already sets
`BETTER_AUTH_TRUSTED_ORIGINS`, Localify leaves it alone and prints the origins to add to
it. Only your own URLs that this machine serves count, never `*.localify.fyi`. A public
URL you create while the service runs takes effect at its next restart.
