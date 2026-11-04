---
type: fixed
---

## Localify's daemon now runs under your account, not as an administrator

Once it has opened ports 80 and 443, the daemon gives up administrator rights, so a static route can
only serve files you can already read. Other apps on your Mac can no longer use Localify to read or
change files as an administrator. Custom locations in the `[paths]` section of
`/etc/localify/config.toml` are now ignored.
