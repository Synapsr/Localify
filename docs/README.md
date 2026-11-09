# Localify user guide

Localify gives the apps you run on your machine stable `https://<name>.localify` URLs
with trusted HTTPS. It runs on macOS and Linux; the desktop app is macOS only. This guide covers installing it, running your apps through it, and what to
do when something goes wrong. Each feature with its own page is linked from here.

## Contents

- [Quick start](#quick-start)
- [Run your apps](#run-your-apps)
  - [Start a service with `localify dev`](#start-a-service-with-localify-dev)
  - [Services in the app, from the command line](#services-in-the-app-from-the-command-line)
  - [Service tabs in the desktop app](#service-tabs-in-the-desktop-app)
  - [Launch profiles](#launch-profiles)
- [Projects and worktrees](#projects-and-worktrees)
  - [Projects](#projects)
  - [Worktrees](#worktrees)
  - [Cleaning up worktrees](#cleaning-up-worktrees)
- [Domains and routes](#domains-and-routes)
- [Coding agents](#coding-agents)
- [Localify Cloud](#localify-cloud)
  - [Public URLs](#public-urls)
  - [Apps that check origin](#apps-that-check-origin)
  - [Plan and billing](#plan-and-billing)
  - [Remote machines](#remote-machines)
- [Access from other devices](#access-from-other-devices)
- [Docker](#docker)
- [Reference](#reference)
  - [Commands](#commands)
  - [Configuration](#configuration)
  - [Files and directories](#files-and-directories)
  - [The daemon](#the-daemon)
  - [Updates](#updates)
  - [Logging and verbosity](#logging-and-verbosity)
  - [Reverse proxy headers](#reverse-proxy-headers)
  - [Shell completions](#shell-completions)
- [Troubleshooting](#troubleshooting)

## Quick start

On a Mac, the simplest way in is the app: download it from
[localify.dev](https://localify.dev) or take the DMG,
<https://downloads.localify.dev/latest/localify.dmg>. On first launch it asks for your
administrator password once and sets everything up, including the `localify` command.
It updates itself.

For the command line alone, on macOS (Apple Silicon) or Linux (x86_64):

```bash
curl -fsSL https://localify.dev/install.sh | sh
```

The script downloads the latest release from GitHub, checks its SHA-256, puts
`localify` in `~/.local/bin`, then runs `localify install`, which asks for your
administrator password once. Run it again to update: the background service restarts
on the new binary. Over SSH, keep a terminal so `sudo` can ask:
`ssh -t <host> 'curl -fsSL https://localify.dev/install.sh | sh'`.
`LOCALIFY_VERSION`, `LOCALIFY_INSTALL_DIR` and `LOCALIFY_NO_SETUP=1` change what it does;
the [script](../apps/web/public/install.sh) says how. On a Mac that has the app, the
script leaves it alone. To build from source, see [CONTRIBUTING.md](../CONTRIBUTING.md).
On Linux, read [Localify on Linux](linux.md).

`localify install` creates a root certificate authority limited to the `.localify`
suffix, points the system resolver at Localify's DNS for `*.localify`, starts the
daemon under launchd or systemd, and connects the coding agents it finds (not on Linux
without systemd, where it runs as root). On macOS it
also trusts the CA in booted iOS Simulators. After that, nothing of Localify's runs as
root.

On macOS, HTTPS trust waits for a second, separate step, so that the install asks for
one password. HTTP works right away, and so does HTTPS from Node.js through
`NODE_EXTRA_CA_CERTS`. Browsers warn until you click Trust HTTPS in the app or run
`localify trust`, and macOS asks you to confirm once. That confirmation needs a screen,
so run it on the Mac itself. On Linux with systemd, the install trusts the CA in the
same `sudo`.

Now run an app:

```bash
cd ~/code/journal
localify dev
# then open https://journal.localify
```

Or point a domain at anything that already listens:

```bash
localify register myapp.localify --route "/=3000"
open https://myapp.localify        # macOS
xdg-open https://myapp.localify    # Linux
```

None of Localify's commands need `sudo`, and Localify refuses to run under it.
`install`, `uninstall` and `lan` ask for an administrator's approval when they change
the system. The exception is Linux with no systemd, for example in a container: run the
commands as root there, and start the daemon yourself with `localify start`.

## Run your apps

### Start a service with `localify dev`

`localify dev` starts a service at its `.localify` URL. Run it in the app's folder, or
name the service: `localify dev web`. From a monorepo root without a name, it asks which
app to start.

The first run in a folder that isn't a Localify project yet sets one up:

- it creates the project, named after the folder, and finds the apps in it (each
  package with a `dev` script);
- it picks a free port for each app and keeps it, instead of using framework defaults
  such as `3000` or `5173`;
- it registers domains. A single app gets `<project>.localify`; several apps get
  `web.<project>.localify`, `api.<project>.localify` and so on. In a terminal it asks
  you to confirm them, and `--domain <name>` sets the main one;
- it allows the domain in Next.js `allowedDevOrigins` or Vite `server.allowedHosts` when
  the project already has that config file. It never creates one, and never rewrites
  your `package.json` scripts. `init-project`, `project add-service` and
  `project configure-dev` do that when you pass `--write-scripts`.

Then it runs the service's `dev` script with your package manager, and sets:

- `PORT` to the service's port, and `HOST` to `127.0.0.1`;
- the service's URL in `LOCALIFY_URL` and the common URL variables (`NEXT_PUBLIC_APP_URL`,
  `AUTH_URL`, `BETTER_AUTH_URL`, `APP_URL`, `VITE_APP_URL` and a few more), plus the URL
  variables your env files already define;
- the domain in `LOCALIFY_DOMAIN`;
- `NODE_EXTRA_CA_CERTS` to Localify's CA, so Node.js trusts the `.localify` HTTPS.

To run something else for one launch, pass it after `--`: `localify dev web -- pnpm
start`. `localify get web` prints a service's URL, and `localify services list` lists
the services of every project.

**When the service already runs**, `localify dev` asks in a terminal whether to stop it
and start it again. `--replace` does that without asking. Anything without a terminal
(an agent, a script) says it already runs and starts nothing, unless it passes
`--replace`. When an earlier `localify dev` died and left processes behind, the next
launch stops them on its own.

`localify dev` exits with the service's own exit code, and with 130 after Ctrl+C, so
`localify dev web && …` stops where it should. Ctrl+C also stops a service that
ignores it, within 20 seconds, and a second Ctrl+C stops it at once.

### Services in the app, from the command line

`localify dev --in-app` starts a service in the desktop app's terminal, as its launch
button does. It hands the service to the open app, which opens it in a new tab without
taking focus, and opens the app first only when it isn't running. The command prints the
URL and returns once the service listens. Without the app, `--detach` runs the service
in the background instead.

```bash
localify dev web --in-app          # add --env feat-login for a worktree
localify logs web -n 100           # the app terminal's lines, without colors
localify logs web --follow         # until the service stops
localify services status           # running or not, PID, URL, port, since, exit code
localify restart web               # stops it and starts it again where it ran
localify stop web
```

`stop`, `restart` and `logs` work on any `localify dev`: in the app, in the background,
or in a terminal. Only the output of a service run in your own terminal stays in that
terminal. Without a service name, `stop`, `restart` and `logs` act on the daemon.

A service run in the background keeps its own output, stdout and stderr only, and the
last 4 to 8 MiB of it. Stopping it stops everything it started, even processes that
ignore Ctrl+C, and a new start waits until the previous run is fully gone.

When `logs` writes to something other than a terminal, it ends with `Next cursor: C`
(`<run id>:<offset>` for a background run). `--cursor C` then prints only the lines
that came after. Agents use this to follow a service.

The link `localify://action/launch-service?project=<id>&env=<slug>&service=<id>`, with
an optional `&profile=<name>`, starts a service in the app the same way.

### Service tabs in the desktop app

A service you start from the app runs in its own terminal tab, with your shell's
`PATH`, `nvm` and other setup. The tab is the service: when it ends, the tab shows the
exit code with `Restart` and `Open a terminal here`. To type commands, open a shell tab
with `Open a terminal here` or the terminal button. Stop ends the service and everything
it started. `localify logs` shows only what the service printed.

The terminal panel lists the projects and worktrees with something running. A red dot
warns you when a service stops in the background, and `+` starts a service or opens a
shell on any other project. If you change your shell setup, `Reload shell environment`
in the panel's menu reads it again, for service tabs and for services started in the
background for your other machines.

A service still running from an app older than 1.4 keeps running in its old tab, marked
"Started before this update".

### Launch profiles

A launch profile chooses where a service's variables come from: dotenv files, Infisical,
Proton Pass, or a mix. The service, its domain and its port stay the same. Profiles live
in `localify.toml` at the root of the checkout, and `localify profile init` writes one
interactively.

```bash
localify dev --profile staging
localify exec --profile staging -- pnpm db:studio   # a command that is not a service
```

A profile marked `danger = true`, typically production data, is flagged in the
terminal, the browser and the app while it runs. Without a terminal, it starts only
with `--confirm-profile <name>`. Responses of a service started with a profile carry
`X-Localify-Profile: <name>`.

Everything else, from `[profiles.<name>.env]` to Infisical and Proton Pass, is in
[Launch profiles](launch-profiles.md).

## Projects and worktrees

### Projects

A project groups the services of one repository and their domains. `localify dev`
creates one when it needs to. To set one up without starting anything:

```bash
localify init-project journal --yes
```

`init-project` is for the first setup. To repair or extend a project, use the targeted
commands instead of running it again:

| To | Run |
| --- | --- |
| Add an app the first setup missed | `localify project add-service --path apps/admin` |
| Change a service's domain | `localify project domain set web app.journal.localify` |
| Reapply the Next.js or Vite config | `localify project configure-dev` |
| Rename, disable or enable, or re-link a moved project | `localify project update journal --name … / --disable / --enable / --root <folder>` |
| Move a registered domain in or out of a project | `localify project domain attach <domain> --project journal` / `detach <domain>` |
| List projects | `localify list-projects` |
| Remove a project | `localify remove-project <project-id> [--delete-domains]` |

Each project folder holds `.localify/project.json`, which names its project on this
machine, so Localify finds a project again after you move its folder. It belongs to
this machine only: Localify adds `**/.localify/project.json` to the repository's
`.git/info/exclude`, shared by its worktrees, so it is never committed. A marker
committed before that stays tracked; stop tracking it with
`git rm --cached .localify/project.json`.

### Worktrees

Running `localify dev` inside a Git worktree of a project gives it its own environment:
its own domains, certificates and ports, next to the main checkout. The domain gets one
more label: `feat-login.journal.localify` for a single-app project,
`web--feat-login.journal.localify` for a monorepo.

```bash
localify project env list
localify project env discover --project journal   # worktrees Git knows about
localify dev web --env feat-login --project journal   # from anywhere
```

`.env` files are not copied into a worktree. Copy them with
`localify project env copy-env --project journal --env feat-login --all`.

`--env` changes the checkout; `--profile` changes only the variables.

### Cleaning up worktrees

When the work in a worktree is done, remove it from another folder:

```bash
localify project env remove feat-login --delete-worktree
```

This stops the environment's services, deletes the worktree with `git worktree remove`,
and unregisters its domains. Git refuses when the worktree has uncommitted changes, and
nothing is removed; `--force` discards them. The branch is kept. Without
`--delete-worktree`, the folder stays and only Localify's side goes. For a worktree
deleted another way, `localify project env prune` removes the environments whose folder
is gone.

## Domains and routes

A domain doesn't need a project. `localify register` points it at a target, and
`localify list` shows them all. A new domain works as soon as the command returns.

A route maps a path prefix to a target, written `PATH=TARGET`. The target is a port on
`127.0.0.1`, a `host:port`, or a folder:

```bash
localify register app.localify --route "/=3000"
localify register app.localify --route "/=192.168.1.50:3000"
localify register docs.localify --route "/=/var/www/html"
localify register app.localify --route "/=3000" --route "/api=3001"
```

With several routes, the longest matching prefix wins. Routes can change later:

```bash
localify route add app.localify /webhooks 9000
localify route remove app.localify /webhooks
localify route list app.localify
localify unregister app.localify
```

**Wildcards.** `--wildcard` matches the domain and any single-level subdomain, with a
wildcard certificate:

```bash
localify register myapp.localify --wildcard --route "/=3000"
```

| URL | Matches? |
| --- | --- |
| `https://myapp.localify` | yes |
| `https://blog.myapp.localify` | yes |
| `https://a.b.myapp.localify` | no, two levels |

An exact registration of the same domain wins over the wildcard, so you can register
`myapp.localify` on port 3000 and `myapp.localify --wildcard` on port 4000 for its
subdomains. `route add|remove|list` and `unregister` take `--wildcard` to act on the
wildcard registration and leave the exact one alone. Unregistering a wildcard also
removes its certificate.

**Static folders.** A folder target serves `index.html` when a directory has one, and a
file listing when it doesn't.

## Coding agents

`localify mcp` is an MCP server that gives Claude Code, Codex and Cursor every Localify
action as a tool: starting and stopping services, reading their output, projects,
domains, routes, worktree environments, public URLs, and the daemon. Each tool runs the
CLI command of the same name, so an agent gets what you get in a terminal. Because the
server is the installed binary, agents always see the actions of the installed version.

```bash
localify agent install   # connect every agent installed here
localify agent status    # see which ones are connected
```

`agent install` uses `claude mcp add` and `codex mcp add`, and edits
`~/.cursor/mcp.json` for Cursor. It also removes the Localify skill earlier versions
installed: the copies Localify made and the links to them, never a skill or link of
your own. Restart open agent sessions afterwards.

You rarely need to run it. `localify install` and the app's setup connect the agents
installed at that time, and after an update the app connects them once, with a message
saying so. Settings → AI agents has the switch to turn this off, and Connect agents for
an agent installed later.

Agents start services in the app's terminal when the app runs, so you see them, and
clean up the worktrees they're done with. Some things stay yours: signing in to Localify Cloud,
paying, opening a public URL to everyone, pairing machines, and starting a dangerous
launch profile unless you asked for it.

## Localify Cloud

Localify Cloud is the paid, hosted part of Localify: public URLs and remote machines.
You sign in with GitHub, from the app (Settings → Localify Cloud) or with
`localify login`, which shows a code to enter on github.com. `localify whoami` shows
who is signed in, the plan, and how this machine's tunnel is doing. `localify logout`
signs this machine out.

### Public URLs

A public URL opens a project domain of this machine from anywhere, such as your laptop
or phone on another network: `https://<slug>.localify.fyi` reaches
`web.journal.localify` on the machine that serves it.

```bash
localify login                                   # once per machine
localify public-url create web.journal.localify  # prints the URL, its password, and a link with it
localify public-url create journal.localify --wildcard
localify public-url list
```

- **Names follow your local ones.** `journal.localify` opens at
  `https://journal.localify.fyi`. `--slug` picks another name.
- **Protected by default.** Each URL asks for a password, generated unless you pass
  `--password`. A browser that enters it stays signed in for 7 days on that host; each
  wildcard host asks once. The link with the password,
  `https://<slug>.localify.fyi/__localify/access#<password>`, signs a browser in without
  typing it. The password sits after `#`, so it never reaches a server. The machine
  that set the password keeps it to give that link again, in the app and in
  `public-url list --json`. `protect` sets a new password and signs every browser out.
  `unprotect`, or `--public` at creation, opens the URL to everyone.
- **Wildcard.** `--wildcard` on a project's namespace, such as `journal.localify`,
  reaches every service and worktree under it: `journal--web.localify.fyi` opens
  `web.journal.localify`, and `journal--web--feature.localify.fyi` opens
  `web--feature.journal.localify`. Only domains of that project are reached. In the app,
  Public URL on any domain creates this one URL for the whole project.
- **One machine serves each URL.** `create` serves it from this machine, and `claim`
  moves it here from another of your machines. The machine needs Localify running; it
  downloads Cloudflare's `cloudflared` on first use and runs it while it serves at least
  one URL.
- **What is never public:** domains outside a project, Docker domains, and Localify's
  own `/.localify/` pages.
- `remove` deletes a URL and keeps its slug for you for 30 days. `logout` takes this
  machine's URLs offline; after you sign in again, `claim` serves them again.

The app does the same in Settings → Localify Cloud and in each domain's Public URL
section. Coding agents get the same tools over MCP, except signing in and opening a URL
to everyone: an agent always creates a protected URL and hands you its password.

How it works, for contributors: [public-urls.md](public-urls.md).

### Apps that check origin

Through a public URL, your app gets the local host in `Host` and the public host in
`X-Forwarded-Host` ([ADR 0023](adr/0023-public-traffic-reaches-upstream-with-the-local-host.md)).
Writes such as a `POST` keep the browser's public `Origin`; only Next.js's own dev files
and hot reload get the local origin. An app that checks `Origin` against its own URL
therefore refuses them, as Better Auth does with `Invalid origin`.

For Better Auth, `localify dev` does it for you. When public URLs of yours that this
machine serves reach the service, it sets `BETTER_AUTH_TRUSTED_ORIGINS` to their exact
origins: `https://journal.localify.fyi` for the URL on `journal.localify`, and
`https://journal--api.localify.fyi` for `api.journal.localify` under a wildcard URL.
Better Auth adds this variable to its `trustedOrigins` option, so
`trustedOrigins: [env.AUTH_URL]` keeps working.

- Set in your shell or a launch profile, your value is kept and the public origins are
  appended to it.
- Set only in an env file of the service or the checkout root (`.env`, `.env.local`…),
  Localify leaves it alone, since your framework decides which file wins. It prints what
  to add instead: `BETTER_AUTH_TRUSTED_ORIGINS is set in <file>: add <origins> to it so
  Better Auth accepts your public URL.`

`LOCALIFY_PUBLIC_URL` gets the first of these origins, unless you set it. Localify reads
the URLs the daemon last learned from Localify Cloud, so a launch never waits on the
network; signed out, or with no URL served here, it adds nothing. A URL you create while
the service runs takes effect at its next restart.

Other libraries need the public origins in their own allowlist. In development, trust
your own public URL and the hosts of its services and worktrees, so a new worktree needs
no change. With the project URL `https://journal.localify.fyi`:

```ts
const publicUrls = ["https://journal.localify.fyi", "https://journal--*.localify.fyi"];
allowedOrigins: [env.APP_URL, ...(isDev ? publicUrls : [])],
```

For links Better Auth builds, such as OAuth redirects, give it a base URL that follows
`X-Forwarded-Host`, from an allowlist:
`baseURL: { allowedHosts: ["journal.localify", "*.journal.localify", "journal.localify.fyi", "journal--*.localify.fyi"], fallback: "https://journal.localify" }`.

Do not trust every `*.localify.fyi` origin. Localify Cloud refuses a write whose
`Origin` belongs to another public URL ([public-urls.md](public-urls.md), "Public
traffic", step 2), but your app also answers on its local domain, where nothing checks
that: a page on someone else's public URL, open in your browser, could post to it.

### Plan and billing

Signing in is free. Serving public URLs and using remote machines takes Localify Cloud
Pro, after a 7-day free trial. Prices are on
[localify.dev/pricing](https://localify.dev/pricing).

```bash
localify subscribe                 # opens the checkout and waits for the plan
localify subscribe --yearly
localify subscribe --machines 2    # on Pro: add serving machines
localify subscribe --url-packs 1   # on Pro: add a pack of 10 public URLs
localify billing                   # card, invoices, interval, cancelling
```

The app has the same in Settings → Localify Cloud, with Add capacity for the add-ons.
When you hit a limit, Localify names the command that raises it. A URL your plan no
longer covers is suspended: it keeps its slug and password, and serves again once the
plan covers it. The full plan rules are in [public-urls.md](public-urls.md#plans).

### Remote machines

Sign in to Localify Cloud on two or more machines, and the app on each one shows you the
others: their projects and services, which ones run and who started them, whether their
ports answer, their public URLs, and a service's live output. With the other machine's
permission, you can also start, stop and restart its services. It needs Localify Cloud
Pro.

**Pair the machines.** In the app, use the machine switcher next to the Projects title,
or Settings → Remote access. In a terminal:

```bash
localify machines                  # this account's machines: online, paired, sharing
localify machines pair teo-desktop # on this machine
localify machines pair --accept    # or wait for a request on the other one
```

Both machines show the same 6-digit code, and you confirm it on each. What the machines
say to each other is end-to-end encrypted, so Localify Cloud relays it without reading
it. Pairing and removing a machine need a person at a terminal or in the app; agents
can't do it. `localify machines unpair <machine>` forgets a paired machine, and
`localify machines remove <machine>` signs it out of the account.

**Choose what others may see and do here.** Each machine decides for itself, in Settings
→ Remote access or with `localify machines settings`. Everything starts off.

| Setting | When on |
| --- | --- |
| `--share on` | Paired machines see this machine's projects, services, public URLs and output. |
| `--control on` | They may start, stop and restart services here. Needs `share`. |
| `--background-starts on` | A remote start runs in the background when the app is closed here. Needs `control`. |

Output can contain secrets your services print, and what you type into a service tab.

**Use another machine.** Pick it in the switcher. Its projects look like this Mac's, read
only. Show output opens a service's output with search and Follow / Pause. Its
`.localify` URLs only open on that machine, so the app copies them instead; a public URL
opens in one click.

Start, Stop and Restart work when that machine allows controls. A service starts in a new
tab of Localify there when the app is open, or in the background when the machine allows
background starts. You follow it from here until it answers or says why it stopped.
Nothing is queued while the connection is down, and a service running in someone's own
terminal is never taken over.

When a machine doesn't allow controls yet, its buttons stay clickable and offer
`Ask <machine>`. Localify there shows who asks, with their fingerprint, and the person
can allow it, refuse it, or stop that machine from asking again. When Localify is closed
there, the card gives the one command to run on it instead, such as
`localify machines settings --control on`.

A service started from an app older than 1.5 shows "Relaunch to share". Relaunch it, after
a confirmation, and its output and controls work from then on.

How it works, for contributors: [remote-machines.md](remote-machines.md).

## Access from other devices

By default only your machine can reach Localify: the proxy and its DNS listen on
loopback, and the DNS server answers `127.0.0.1` and `::1`.

To test from a phone or another computer on your network, run `localify lan enable`, or
turn on network access in Settings. It rewrites the launchd plist or systemd units with
one approval. Localify then listens on every interface, and its DNS answers other
devices with this machine's LAN address. `localify lan disable` takes that back, and
`localify lan status` tells you where you are. Do this on networks you trust: anyone on
the network can then reach your dev servers.

`lan enable` fails before asking for anything if another program already listens on
every interface on that port, as Tailscale can on 443.

Run as root on Linux, or with no systemd, the daemon listens on every interface, as it
does in a container. Set `lan_access = false` in the config to keep it on this machine.

To reach your app from anywhere, not just your network, use a [public URL](#public-urls).

## Docker

Localify can discover Docker Compose services and register them as `.localify` domains.
Enable it in the config, then restart the daemon:

```toml
[docker]
enabled = true
```

Starting a compose stack then registers domains such as `<service>.<project>.localify`
with HTTPS. Labels, container-to-Localify traffic and troubleshooting are in
[Docker integration](docker.md).

## Reference

### Commands

Every command takes `-c, --config <PATH>` to use another config file and `-v, --verbose`
for debug output. `localify <command> --help` lists the rest.

| Command | What it does |
| --- | --- |
| **Setup** | |
| `localify install` | Set up the CA, DNS and the daemon; connect coding agents |
| `localify uninstall [--force]` | Remove everything Localify set up |
| `localify trust` | Trust HTTPS in browsers on this Mac |
| `localify lan enable\|disable\|status` | Let other devices connect, or not |
| **Services** | |
| `localify dev [service]` | Start a service; `--in-app`, `--detach`, `--replace`, `--env`, `--profile`, `--domain` |
| `localify run [service]` | Run a project service with Localify's environment variables |
| `localify services list` / `status` | List the project's services / show what Localify started |
| `localify logs <service> [-n N] [-f] [--cursor C]` | Print a service's output |
| `localify stop <service>` / `restart <service>` | Stop a service / restart it where it ran |
| `localify get <service>` | Print a service's URL |
| `localify exec --profile <name> -- <cmd>` | Run a command that is not a service on a launch profile |
| `localify profile init` | Write `localify.toml` interactively |
| **Projects** | |
| `localify init-project [name]` | Create a project and its domains |
| `localify list-projects` | List projects and their domains |
| `localify remove-project <id>` | Remove a project |
| `localify project add-service\|configure-dev\|update` | Extend, repair, rename or move a project |
| `localify project domain set\|attach\|detach` | Change a service's domain, or move a domain in or out of a project |
| `localify project env list\|discover\|ensure\|remove\|prune\|copy-env` | Manage worktree environments |
| **Domains** | |
| `localify register <domain> --route PATH=TARGET [--wildcard]` | Register a domain |
| `localify unregister <domain> [--wildcard]` | Remove a domain |
| `localify list` | List domains |
| `localify route add\|remove\|list` | Manage a domain's routes |
| **Daemon** | |
| `localify start [--foreground]` / `stop` / `restart` | Start, stop or restart the daemon |
| `localify reload` | Reload the configuration |
| `localify status` | Show the daemon and domain status |
| `localify logs [-n N] [-f] [--clear]` | Show the daemon's log |
| **Agents** | |
| `localify agent install` / `status` | Connect coding agents to the MCP server / show which are |
| `localify mcp` | The MCP server agents start |
| **Localify Cloud** | |
| `localify login` / `logout` / `whoami` | Sign this machine in or out / show the account and plan |
| `localify subscribe [--yearly \| --machines N \| --url-packs N]` | Start Pro, or add capacity |
| `localify billing` | Manage the subscription |
| `localify public-url create\|list\|remove\|protect\|unprotect\|claim` | Manage public URLs |
| `localify machines [pair\|unpair\|remove\|settings]` | Pair machines and choose what they may do here |
| **Other** | |
| `localify completions <shell>` | Generate shell completions |

### Configuration

The configuration lives in
`~/Library/Application Support/Localify Desktop/config.toml` on macOS and in
`~/.config/localify/config.toml` on Linux (`/etc/localify/config.toml` as root).
`--config` uses another file. Most of it is written by commands; these are the parts you
may edit by hand.

```toml
[daemon]
http_port = 80
https_port = 443
dns_port = 1053
log_level = "info"
lan_access = false
```

The three ports must differ. launchd or systemd opens the HTTP and HTTPS ports for the
daemon, so changing them needs one approval: use the app's settings, or run
`localify install` after editing the file. `lan_access` is described in
[Access from other devices](#access-from-other-devices).

Each registered domain gets a section. A wildcard registration has a `*.` prefix:

```toml
[domains.myapp-localify]
domain = "myapp.localify"
https_enabled = true

[[domains.myapp-localify.routes]]
path = "/"
target = "127.0.0.1:3000"

[domains.wildcard-myapp-localify]
domain = "*.myapp.localify"
https_enabled = true
```

Domain names end with `.localify` and contain letters, numbers, hyphens, and dots for
subdomains.

`[paths]` moves Localify's data. The values below are the defaults on Linux with
systemd; you only need the section to change them.

```toml
[paths]
data_dir = "/home/me/.config/localify/data"
pid_file = "/home/me/.config/localify/localify.pid"
log_file = "/home/me/.config/localify/logs/localify.log"
certs_dir = "/home/me/.config/localify/data/certs"
```

`[docker]` is described in [Docker](#docker).

### Files and directories

On macOS, everything of yours lives in your user folder:

```text
~/Library/Application Support/Localify Desktop/
├── config.toml          # Main configuration
├── localify.pid         # PID file (when the daemon runs)
├── localify.sock        # Management socket, yours only
├── localify.stopped     # Present while you have the daemon stopped
├── data/
│   ├── ca.key           # Root CA private key, 0600
│   ├── ca.crt           # Root CA certificate
│   └── certs/
│       ├── <domain>.key # Per-domain private key, 0600
│       └── <domain>.crt # Per-domain certificate
└── logs/
    └── localify.log     # Daemon log file
```

The one file outside it is `/Library/LaunchDaemons/dev.localify.daemon.plist`, which
tells launchd to open the ports and run the daemon as you.

On Linux with systemd, the same layout lives in `~/.config/localify/`. The files outside
it are `localify.service`, `localify-http.socket` and `localify-https.socket` in
`/etc/systemd/system/`, which tell systemd to open the ports and run the daemon as you.
Run as root, or with no systemd, Localify keeps its files in `/etc/localify/`,
`/var/run/localify.pid` and `/var/log/localify/`.

The resolver file that sends `*.localify` to Localify's DNS is `/etc/resolver/localify`
on macOS and `/etc/systemd/resolved.conf.d/localify.conf` on Linux with
systemd-resolved.

### The daemon

`localify install` hands the daemon to launchd on macOS and systemd on Linux. It starts
at boot, before anyone signs in, runs as you, and restarts if it crashes. These commands
steer it without any prompt:

- `localify stop` asks it to exit and leaves a marker file. The service manager still
  holds the ports and wakes the daemon on the next connection, so while the marker
  exists the daemon closes that connection and exits again. Names stop resolving, no
  process stays behind, and it stays stopped across a reboot.
- `localify start` removes the marker and opens a connection, which wakes the daemon.
- `localify restart` asks it to exit with a code the manager restarts on. An update needs
  no command: the daemon notices its binary was replaced and does the same.

The manager holds the ports, so outside it a daemon can only use ports from 1024 up on
loopback. With no systemd, `localify start` forks to the background and writes a PID
file; as root it logs to `/var/log/localify/localify.log`. `localify start --foreground`
stays in the terminal and logs to stdout until Ctrl+C, which helps when debugging.

### Updates

From 1.7 on macOS, Localify installs new versions by itself while the app is closed,
even on a machine nobody sits at, and checks at once when a paired machine is on a newer
version. It only installs official releases signed by Localify. With the app open,
Settings and a small banner offer `Restart to update`, which says which tabs it will
close; the app never restarts by itself.

New installs start with automatic updates on. An install from before 1.7 keeps updating
by hand until you turn them on, in Settings or from a terminal, which also works over
SSH:

```bash
localify update --auto on    # or off
localify update --status     # what is installed and available, and why it can't install
localify update install      # install now
```

The previous version is kept in `~/Library/Application Support/Localify/previous/` if
you ever need to go back. On Linux, Localify tells you when an update is available, and
you run the install command again. The details are in
[Automatic updates](desktop.md#automatic-updates).

### Logging and verbosity

```bash
localify logs              # the daemon's last 50 lines
localify logs -n 100       # last 100 lines
localify logs -f           # follow (like tail -f)
localify logs --clear      # clear the log file
```

The log level comes from, highest priority first:

1. the `LOCALIFY_LOG` environment variable: `LOCALIFY_LOG=debug localify start --foreground`;
2. `--verbose`, which sets `debug`;
3. `log_level` under `[daemon]` in the config. Under launchd or systemd this is the way:
   edit it, then run `localify restart`;
4. the default, `info`.

The levels are `error`, `warn`, `info` and `debug`. The app's Logs screen shows the same
log, and the requests in it.

### Reverse proxy headers

Localify adds these headers to every proxied request:

| Header | Value |
| --- | --- |
| `X-Forwarded-Host` | The `Host` the client sent |
| `X-Forwarded-Proto` | `http` or `https` |
| `X-Forwarded-Port` | `80` or `443`, following `X-Forwarded-Proto` |
| `X-Forwarded-For` | The client IP, appended to any existing chain |

Most frameworks read them on their own: Django reads `X-Forwarded-Proto` to build
`https://` URLs, and Rails uses `X-Forwarded-Host`. Localify strips hop-by-hop headers
in both directions, as
[RFC 7230 §6.1](https://www.rfc-editor.org/rfc/rfc7230#section-6.1) asks: `Connection`,
`Keep-Alive`, `Proxy-Authenticate`, `Proxy-Authorization`, `TE`, `Trailer`,
`Transfer-Encoding`, `Upgrade`, and any header listed in `Connection`. WebSockets work.

A service started with a [launch profile](launch-profiles.md) adds
`X-Localify-Profile: <name>` to its responses. For a dangerous profile, Localify also
adds a same-origin `<script src="/.localify/launch-profile.js">` to HTML pages, which
prefixes the tab title and shows a draggable indicator.

To see the headers Localify sets on each request, run the daemon with debug logging:

```bash
LOCALIFY_LOG=debug localify start --foreground
```

```text
DEBUG Forwarding headers set x_forwarded_host=myapp.localify
  x_forwarded_port=443 x_forwarded_proto=https x_forwarded_for=127.0.0.1
DEBUG Proxying HTTP request target=127.0.0.1:3000
```

### Shell completions

`localify completions <shell>` prints completions for bash, elvish, fish, powershell or
zsh. Write them where your shell looks, then restart it:

```bash
localify completions fish > ~/.config/fish/completions/localify.fish
localify completions bash > ~/.local/share/bash-completion/completions/localify
localify completions zsh > ~/.zfunc/_localify
```

For zsh, `~/.zfunc` must be in your `fpath`. Add this to `.zshrc`:

```bash
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

## Troubleshooting

Start with `localify status`. It says whether the daemon runs and whether HTTPS trust is
still pending, with the command to run.

### The browser says "Not secure"

- On macOS, trust may still be pending: click Trust HTTPS in the app or run
  `localify trust`.
- A browser that was open during the install doesn't see the new CA yet. Quit it
  completely and open it again.
- **iOS Simulator:** `localify install` and `localify dev` add the CA to booted
  simulators. If Safari still warns, boot the simulator, use Settings → Troubleshooting →
  Trust booted simulators in the app, then quit and reopen Safari in the simulator.
- **Linux snap browsers** (Firefox, Chromium) can't read the system trust store. See
  [Snap browsers](linux.md#snap-browsers-and-certificate-trust) for the one-time
  `certutil` fix.

### "This site can't be reached"

Check that the daemon runs, and start it if it doesn't:

```bash
localify status
localify start
```

Then check DNS:

```bash
dig myapp.localify               # macOS
resolvectl query myapp.localify  # Linux
```

### A page says "Your app isn't answering"

Localify answered, but nothing listens on the target. Check the service:

```bash
localify services status
localify logs web -n 100
curl http://localhost:3000       # for a domain you registered by hand
```

### Ports already in use

If the daemon can't start because ports 80, 443 or 1053 are taken, find what holds them,
then stop it or [change Localify's ports](#configuration):

```bash
sudo lsof -i :80
sudo lsof -i :443
sudo lsof -i :1053
```

### Linux

DNS, auto-start and snap browsers on Linux have their own section in
[Localify on Linux](linux.md#troubleshooting).
