# Changelog

What changes for people using Localify, one section per version. The command line
and the desktop app share the same version.

## 1.2.0 - 2026-11-05

Your coding agents can now do everything you do in Localify, and clean up after
themselves.

### New

- **Let Claude Code, Codex, and Cursor do everything Localify does.** Connect them with
  `localify agent install`, or **Connect agents** in Settings → AI agents. They get every
  action of the app and the command line, from starting services to managing domains,
  and always the ones of the version you have installed. This replaces the Localify
  skill: `localify agent install-skill` is now `localify agent install`.
  [Coding agents](https://github.com/Synapsr/Localify/blob/main/docs/README.md#coding-agents)
- **Start a service in the app from the command line, and read its output anywhere.**
  `localify dev --in-app` starts a service in the app's terminal, as its launch button
  does, and returns once it answers. `localify logs <service>` prints the same lines the
  app shows, `localify services status` shows what runs and since when, and
  `localify stop` and `restart` act on it from any folder. Without the app, `--detach`
  runs the service in the background.
  [Services in the app](https://github.com/Synapsr/Localify/blob/main/docs/README.md#services-in-the-app-from-the-command-line)
- **Remove a worktree and its domains when the work is done.**
  `localify project env remove <env> --delete-worktree` stops the worktree's services,
  deletes its folder, and unregisters its domains, so finished branches leave nothing
  behind. If the worktree has uncommitted changes, nothing is removed.
  `localify project env prune` cleans up after worktrees you deleted another way.
  [Cleaning up worktrees](https://github.com/Synapsr/Localify/blob/main/docs/README.md#cleaning-up-worktrees)

### Improved

- **Rename, disable, or move a project from the command line.** `localify project update`
  renames a project, enables or disables it, or points it at the folder it moved to, and
  `localify project domain attach` and `detach` move a domain in or out of a project, as
  the app already could.

### Fixed

- `localify dev` no longer changes `.gitignore` in a Git worktree, which showed up as a
  change to commit there.

## 1.1.1 - 2026-11-05

### Fixed

- `localify uninstall` now also removes the app's login item. Before, only uninstalling
  from the app did.
- The desktop app sends usage analytics and error reports, as the privacy policy describes.
  Version 1.1.0 shipped without them.

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
