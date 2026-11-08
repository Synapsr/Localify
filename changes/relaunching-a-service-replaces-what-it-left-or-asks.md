---
type: improved
---

## Starting a service again stops what it left behind, or asks first

When a service's `localify dev` died and left processes behind, starting it again now stops
them and starts, instead of asking you to run `localify stop`. When the service really is
still running, `localify dev` asks in your terminal whether to stop it and start it again;
pass `--replace` to do that without asking. Agents and scripts still leave a running service
alone unless they ask to replace it.
