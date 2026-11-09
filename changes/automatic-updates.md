---
type: new
link: docs/desktop.md
---

## Your Macs install new Localify versions by themselves

While the app is closed, a Mac installs a new version on its own, even a
runner nobody sits at, and checks at once when a paired machine is on a newer
version. New installs start with it on; an existing install turns it on with
`localify update --auto on`, which works over SSH. `localify update` shows
what is installed and available, and `localify update install` installs now.
