---
type: fixed
---

## Next.js apps with next-intl open their default-locale pages

With next-intl and `localePrefix: "as-needed"`, opening `/` failed with a 500 and
`Failed to proxy ... EPROTO` in the dev server's output, because of a Next.js bug
([vercel/next.js#94745](https://github.com/vercel/next.js/issues/94745)). Until Next.js
fixes it, Localify starts these apps on every network interface instead of this
machine only, so other devices on your network can reach their dev port.
