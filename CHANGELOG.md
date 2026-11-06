# Changelog

What changes for people using Localify, one section per version. The command line
and the desktop app share the same version.

## 1.3.2 - 2026-11-06

### Fixed

- **Next.js dev servers work through a public URL.** Hot reload, fonts and some scripts
  failed with a 403, because the browser sent the public origin, which Next.js does not
  list in `allowedDevOrigins`. Localify now passes the local origin for the page's own
  reads and hot reload connection, so no project needs to list `localify.fyi`.

## 1.3.1 - 2026-11-06

### New

- **One public URL per project, named like your local domains.** In the app, Public URL
  on any domain of a project creates the project's URL: `louez.localify` opens at
  `https://louez.localify.fyi`, and each service and worktree under it, such as
  `web.louez.localify`, at `https://louez--web.localify.fyi`. The window shows the exact
  URL before you create it, lets you change its name with a live check that it is free,
  and generates the password. `localify public-url create` also names a new URL after
  its local domain instead of adding random characters.
  [Public URLs](https://github.com/Synapsr/Localify/blob/main/docs/README.md#public-urls)
- **Share a link that opens a public URL without typing the password.** Copy the link
  with the password from the Public URL window in the app, from `localify public-url
  create` and `protect`, or ask your coding agent. The password sits after `#` in the
  link, so it never reaches a server, and chat apps that preview links show only the
  password page. The link stays available on the Mac that set the password, until the
  password changes.
  [Public URLs](https://github.com/Synapsr/Localify/blob/main/docs/README.md#public-urls)

### Fixed

- **The password page lets you in.** In Chrome, a correct password could answer
  "Requests to this public URL that can change something must come from its own
  pages." Apps that send no referrer can also post to their own public URL now.
- **This Mac's tunnel starts in seconds on networks that block UDP,** instead of about
  two minutes.
- **Localify's tunnel no longer reads your own `~/.cloudflared/config.yml`.**

## 1.3.0 - 2026-11-06

Open your dev servers from another network, behind a password.

### New

- **Open your dev servers from anywhere, behind a password.** Sign in with GitHub,
  with `localify login` or in Settings → Localify Cloud, then give a project domain a
  public URL such as `https://journal-a8f3.localify.fyi` to open it from your laptop or
  phone on another network, with hot reload working. Each URL asks for a password
  unless you open it to everyone, and a wildcard URL reaches every service and branch
  of a project. Use `localify public-url`, the Public URL item of each domain in the
  app, or your coding agent. Accounts are invite-only for now.
  [Public URLs](https://github.com/Synapsr/Localify/blob/main/docs/README.md#public-urls)

## 1.2.1 - 2026-11-05

### Improved

- **Your coding agents connect to Localify by themselves.** After the update, Localify
  connects Claude Code, Codex, and Cursor to its MCP server once, tells you which ones,
  and removes the old Localify skill it had installed for them. `localify install`
  connects them too. Turn this off in Settings → AI agents.
  [Coding agents](https://github.com/Synapsr/Localify/blob/main/docs/README.md#coding-agents)

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
