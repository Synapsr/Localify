# Changelog

What changes for people using Localify, one section per version. The command line
and the desktop app share the same version.

## 1.1.0 - 2026-11-05

The first version with open source code, and the first with nothing running as root.

### Improved

- **Create as many projects as you want, for free.** Everything Localify does on your
  machine is now free and has no limits, and you no longer need a license key. Existing
  projects stay as they are. The source code is now open under the AGPL.
- **Run Localify on your Mac with nothing running as root.** Localify asks for an
  administrator's password once, at install. After that it runs under your own account
  and still starts at boot on ports 80 and 443. Updates no longer ask for your password.
  When you want HTTPS in browsers, click Trust HTTPS in the app or run `localify trust`,
  and macOS asks you to confirm once.
- **Run Localify on Linux with nothing running as root.** On Linux with systemd,
  `localify install` asks for your password once. After that the daemon runs under your
  own account and still starts at boot on ports 80 and 443. `register`, `stop`, `start`
  and updates need no password. Your files move to `~/.config/localify`.
- **Choose whether other devices can reach your projects.** Only your own machine can
  reach Localify from now on. To test from a phone or another computer, turn on network
  access in Settings, or run `localify lan enable`.
- **Use a domain as soon as you register it.** A new domain or route now works the moment
  `localify register`, `unregister` or `route` finishes. Before, requests could get a 404
  for up to two seconds, which tripped up scripts and agents that called the domain
  straight away.
- **Send error reports without your project names and domains.** The desktop app now
  removes `.localify` domains and quoted names from error reports before sending them,
  along with URLs and file paths. Onboarding also describes the folder picker and
  `localify init-project` as they really work.

### Fixed

- Commands like `localify status | head -1` now stop quietly when the output is closed.
  Before, they crashed and left a crash report behind.
