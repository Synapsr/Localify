---
type: new
link: docs/README.md#services-in-the-app-from-the-command-line
---

## Start a service in the app from the command line, and read its output anywhere

`localify dev --in-app` starts a service in the app's terminal, as its launch button does,
and returns once it answers. `localify logs <service>` prints the same lines the app
shows, `localify services status` shows what runs and since when, and `localify stop` and
`restart` act on it from any folder. Your agents can do the same through Localify's MCP
server. Without the app, `--detach` runs the service in the background.
