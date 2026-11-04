---
type: improved
---

## Use a domain as soon as you register it

A new domain or route now works the moment `localify register`, `unregister` or `route`
finishes. Before, requests could get a 404 for up to two seconds, which tripped up
scripts and agents that called the domain straight away.
