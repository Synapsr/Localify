---
type: improved
link: docs/README.md#apps-that-check-origin
---

## Better Auth trusts your public URL automatically

An app opened through its public URL no longer answers `Invalid origin` from Better Auth.
`localify dev` adds the exact public origins that reach the service to
`BETTER_AUTH_TRUSTED_ORIGINS`, after any value you set, and sets `LOCALIFY_PUBLIC_URL`
unless you did. Only your own URLs served by this machine count, never `*.localify.fyi`. A
public URL you create while the service runs takes effect at its next restart.
