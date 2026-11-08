---
type: improved
---

## Background services stop cleanly and keep their own output

A service started with `localify dev --detach` now stops with everything it started,
even processes that ignore Ctrl+C, and a new start waits until the previous one is
fully gone instead of fighting it for its port. `localify logs` shows only the
service's own output, keeps its last 4 to 8 MiB, and says when output was cut off
because Localify itself was stopped. `localify dev` now exits with the service's own
exit code, and with 130 after Ctrl+C, so `localify dev web && …` stops where it
should. Ctrl+C also stops a service that ignores it, within 20 seconds, and a second
Ctrl+C stops it at once.
