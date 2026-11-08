---
type: improved
---

## Each service tab runs the service in its own terminal

A service you start from the app now gets a terminal of its own, with your shell's
`PATH`, `nvm` and other setup, and `localify logs` shows only what the service printed.
When it ends, its tab shows the exit code with `Restart` and `Open a terminal here`, and
Stop ends the service and everything it started instead of pressing Ctrl+C in the tab.
To run a command after a service stops, open a terminal; services already running from
an earlier version keep running and show "Started before this update".
