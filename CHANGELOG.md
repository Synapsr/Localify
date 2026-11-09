# Changelog

What changes for people using Localify, one section per version. The command line
and the desktop app share the same version.

## 1.8.0 - 2026-11-10

### New

- **Check and control another machine's services from a terminal or an agent.** Add
  `--machine <name>` to `services status`, `logs`, `dev`, `stop` and `restart` to act on
  another of your machines, as the app's machine switcher does. Coding agents get the
  same through `list_machines` and a `machine` argument on the service tools.
  [Remote machines](https://github.com/Synapsr/Localify/blob/main/docs/README.md#remote-machines)
- **Pair two machines over SSH, with nobody at either screen.** From a machine that
  reaches the other over SSH, `localify machines pair <machine> --over-ssh <destination>`
  starts the other side through `ssh`, compares both codes itself, and confirms both only
  when they match. An agent you gave SSH access to both machines can now pair them. It
  needs OpenSSH 8.7 or later on the machine you run it on.

### Improved

- **A pairing cut short says what each machine saved.** Instead of claiming nothing was
  saved, it reports each side, and pairing again completes it. A machine on 1.7.0 or
  earlier can't say whether it saved, so a pairing with it reports only this machine's
  side.

## 1.7.0 - 2026-11-09

### New

- **Your Macs install new Localify versions by themselves.** While the app is closed, a
  Mac installs a new version on its own, even a runner nobody sits at, and checks at
  once when a paired machine is on a newer version. Only official releases, signed by
  Localify, are installed. With the app open, Settings and a small banner offer
  `Restart to update`, which says which tabs it will close first; the app never
  restarts by itself. New installs start with automatic updates on. An existing install
  turns them on in Settings or with `localify update --auto on`, which works over SSH.
  `localify update` shows what is installed and available, and `localify update install`
  installs now. [Updates](https://github.com/Synapsr/Localify/blob/main/docs/desktop.md#automatic-updates)

## 1.6.0 - 2026-11-09

### New

- **Start, stop and restart your other machines' services from the app.** In another
  machine's view, the Start, Stop and Restart buttons now work. A service starts in a new
  tab of Localify on that machine when it is open, or in the background when that machine
  allows it, and you follow it from here until it answers or says why it stopped. Nothing
  is ever queued while the connection is down, and a service running in someone's
  terminal is never taken over. Each machine decides in Settings → Remote access whether
  others may control it.
  [Remote machines](https://github.com/Synapsr/Localify/blob/main/docs/remote-machines.md)
- **Ask another machine for access, from the button that needs it.** When a machine
  doesn't allow remote controls yet, its buttons stay clickable and offer `Ask` with that
  machine's name. Localify there shows who asks, with their fingerprint, and lets the
  person allow it, refuse it, or stop that machine from asking again. When Localify is
  closed there, the card gives the one command to run on it instead, for you or an agent
  with access to it.
- **Relaunch a service started before 1.5 so it works from here.** A service still
  running from an older Localify shows "Relaunch to share". Relaunch it from another
  machine, after a confirmation, and its output and controls work from then on.

### Improved

- **`localify dev --in-app` hands the service straight to the open app.** It opens in a
  new tab without stealing focus, and only opens the app first when it isn't running.
- **Reload shell environment also covers background starts.** The terminal panel's
  `Reload shell environment` now also refreshes the `PATH` and setup Localify uses to
  start services in the background for your other machines.

### Fixed

- `localify dev --detach --replace` no longer stops the running service when the new
  start is refused, for example for an unknown profile or a broken `localify.toml`.
- A service tab opened while another one was closing no longer disappears from the
  terminal panel.

## 1.5.0 - 2026-11-09

### New

- **See your other machines' services, public URLs and output from the app.** Pick
  another Mac in the switcher next to Projects and see its projects as it sees them: which
  services run and who started them, whether their ports answer, and their public URLs,
  which open in one click. Open a service's output to read and search it live, as plain
  text. Pair machines from the switcher or Settings → Remote access by confirming the same
  6-digit code on both, and choose there whether this Mac shares its projects and output,
  which stays off until you turn it on. Needs Localify Cloud Pro.
  [Remote machines](https://github.com/Synapsr/Localify/blob/main/docs/remote-machines.md)
- **Pair your machines from the terminal.** `localify machines` lists the other computers
  signed in to your Localify Cloud account, with whether each one is online and paired.
  `localify machines pair <machine>` pairs this one with another: both show the same
  6-digit code, and you confirm it on each, so even Localify Cloud cannot slip in a
  machine of its own. What paired machines may see or do here stays off until you turn it
  on with `localify machines settings`.
- **Start Localify Cloud Pro from the terminal or the app.** `localify subscribe` opens
  the checkout for Localify Cloud Pro, with its 7-day free trial, and `localify billing`
  manages the subscription. `localify whoami` and Settings → Localify Cloud show your
  plan, its dates and how much of it you use, and a Mac whose public URLs cannot be served
  now says why and what to do.
  [Public URLs](https://github.com/Synapsr/Localify/blob/main/docs/README.md#public-urls)
- **Buy more machines and URLs from the app or the terminal.** On Localify Cloud Pro,
  Settings → Localify Cloud now has Add capacity: pick how many serving machines or packs
  of 10 public URLs you need, see the monthly price, and finish the checkout in your
  browser. In a terminal, `localify subscribe --machines 2` or
  `localify subscribe --url-packs 1` does the same and waits for the new limit. When you
  hit a limit, Localify names the command that raises it.

### Improved

- **See how your agents run Localify on first launch.** The welcome screens now show in a
  few lines how Localify works with Claude Code, Codex, and Cursor, with a link to the
  docs for each part. Choose whether setup connects your coding agents, or copy a prompt
  that has your agent set Localify up for your workflow.

## 1.4.0 - 2026-11-08

### Improved

- **Each service tab runs the service in its own terminal.** A service you start from
  the app now gets a terminal of its own, with your shell's `PATH`, `nvm` and other
  setup, and `localify logs` shows only what the service printed. When it ends, its tab
  shows the exit code with `Restart` and `Open a terminal here`, and Stop ends the
  service and everything it started instead of pressing Ctrl+C in the tab. To run a
  command after a service stops, open a terminal; services already running from an
  earlier version keep running and show "Started before this update".
- **Starting a service again stops what it left behind, or asks first.** When a
  service's `localify dev` died and left processes behind, starting it again now stops
  them and starts, instead of asking you to run `localify stop`. When the service really
  is still running, `localify dev` asks in your terminal whether to stop it and start it
  again; pass `--replace` to do that without asking. Agents and scripts still leave a
  running service alone unless they ask to replace it.
- **Background services stop cleanly and keep their own output.** A service started
  with `localify dev --detach` now stops with everything it started, even processes that
  ignore Ctrl+C, and a new start waits until the previous one is fully gone instead of
  fighting it for its port. `localify logs` shows only the service's own output, keeps
  its last 4 to 8 MiB, and says when output was cut off because Localify itself was
  stopped. `localify dev` now exits with the service's own exit code, and with 130 after
  Ctrl+C, so `localify dev web && …` stops where it should. Ctrl+C also stops a service
  that ignores it, within 20 seconds, and a second Ctrl+C stops it at once.
- **Better Auth trusts your public URL automatically.** An app opened through its public
  URL no longer answers `Invalid origin` from Better Auth. `localify dev` sets
  `BETTER_AUTH_TRUSTED_ORIGINS` to the exact public origins that reach the service, after
  any value from your shell or launch profile, and sets `LOCALIFY_PUBLIC_URL` unless you
  did. When an env file already sets `BETTER_AUTH_TRUSTED_ORIGINS`, Localify leaves it
  alone and prints the origins to add to it. Only your own URLs that this machine serves
  count, never `*.localify.fyi`. A public URL you create while the service runs takes
  effect at its next restart.
  [Apps that check origin](https://github.com/Synapsr/Localify/blob/main/docs/README.md#apps-that-check-origin)

### Fixed

- Next.js apps with next-intl and `localePrefix: "as-needed"` open their default-locale
  pages again, instead of failing with a 500 and `Failed to proxy ... EPROTO` because of
  a Next.js bug ([vercel/next.js#94745](https://github.com/vercel/next.js/issues/94745)).
  Until Next.js fixes it, Localify starts these apps on every network interface, so other
  devices on your network can reach their dev port.

## 1.3.4 - 2026-11-07

### Improved

- **Switch between your running projects in the terminal.** A new sidebar lists only the
  projects and worktrees with something running, in the same order as the projects
  screen, and the tabs show the processes of the one you select. A red dot warns you when
  a service stops in the background, and `+` starts a service or opens a shell on any
  other project. Background processes also keep their real terminal width, so their
  output no longer wraps after a few columns.

## 1.3.3 - 2026-11-07

### New

- **Install the command line in one line.** `curl -fsSL https://localify.dev/install.sh | sh`
  installs Localify on macOS (Apple Silicon) or Linux (x86_64), checks the download, and
  sets it up. Run it again to update. Coding agents find it, with the first steps,
  agents and public URLs, at <https://localify.dev/llms.txt>.
  [Quick Start](https://github.com/Synapsr/Localify/blob/main/docs/README.md#quick-start)

### Improved

- **A project whose folder moved says how to find it again.** `localify dev` used to
  answer "has no detected command". It now says the folder no longer exists and gives
  the command that points the project at its new one. When a folder already belongs to
  another project, the message names the file that says so and how to remove it.
- **Worktrees no longer change `next.config` or `vite.config`.** Localify used to add
  each worktree's host to Next.js `allowedDevOrigins` or Vite `server.allowedHosts`, and
  missed subdomains such as `teo.feature.louez.localify`. It now adds the site of the
  domain once: `louez.localify` and `**.louez.localify` for Next.js, `.louez.localify`
  for Vite, which cover every worktree and subdomain under `louez.localify`. The
  entries added before stay; you can remove them.
- **Localify no longer edits your project's `.gitignore`.** The file that links a folder
  to its project on your machine stays out of Git through the repository's own exclude
  file, which covers every worktree.

## 1.3.2 - 2026-11-06

### Fixed

- **Next.js dev servers work through a public URL.** Hot reload, fonts and some scripts
  failed with a 403, because the browser sent the public origin, which Next.js does not
  list in `allowedDevOrigins`. Localify now passes the local origin to Next.js's own dev
  files and hot reload connection, so no project needs to list `localify.fyi`. Your
  app's own requests keep the public origin.

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
