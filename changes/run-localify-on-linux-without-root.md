---
type: improved
---

## Run Localify on Linux with nothing running as root

On Linux with systemd, `localify install` now asks for your password once. After that
the daemon runs under your own account and still starts at boot on ports 80 and 443.
`register`, `stop`, `start` and updates need no password, and only this machine can
connect until you run `localify lan enable`. Your files move to `~/.config/localify`.
