---
type: improved
---

## Run Localify with nothing running as root

Localify on your Mac now asks for an administrator's password once, at install, and
macOS asks you to confirm the certificate for your account. After that it runs under
your own account and still starts at boot on ports 80 and 443. Updates no longer ask
for your password. A machine with no screen can finish everything except trusting the
certificate, and gets the exact command to run later.
