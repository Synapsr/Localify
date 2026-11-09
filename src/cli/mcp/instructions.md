Localify gives local apps stable `https://<name>.localify` URLs with trusted HTTPS, through a local DNS server and proxy. Each tool runs one `localify` CLI command and returns its output.

Working on an app:
- Start it with `service_start` (pass `cwd`, the app or repo folder). It runs in the Localify app's terminal, where the user sees it, and returns the URL once the service listens; use that URL, never `localhost:<port>`. If it already runs, it says so and starts nothing; pass `replace: true` to stop the running instance first and start it again. On first use in a folder it creates the Localify project, picks free ports, registers domains and patches Next.js/Vite allowlists. Without the app, pass `background: true`.
- Get a URL with `get_service_url`, and service names with `list_services`.
- Read its output with `service_logs`: the same lines as the app's terminal. Each result ends with a cursor; pass it back to read only the new lines. `service_status` shows what runs, since when, and the exit code of what stopped. `service_restart` restarts where it ran; `service_stop` stops it. Stop what you started before you finish, unless the user wants it kept running.
- Do not pick ports like 3000 yourself, and pass `target` only when the user asked for a fixed one.
- A monorepo app the first setup missed: `add_service`. Framework config drifted: `configure_dev`. Another domain: `set_service_domain`. Do not rerun `init_project` to repair.
- Use the `.localify` URL in OAuth callbacks, webhooks, CORS and cookie origins when that is the route under test.

Worktrees:
- In a Git worktree of a project, `service_start` with `cwd` set to the worktree gives it its own environment: its own domains and ports, next to the primary checkout.
- `.env` files are not copied into a worktree. Copy them with `copy_environment_files` when the app needs them.
- When the work in a worktree is done (merged, abandoned, or handed back), clean up so the project is not polluted: call `remove_environment` with `delete_worktree: true` and `cwd` outside that worktree. It stops the worktree's services, unregisters its domains and certificates, and deletes the folder. Git refuses when there are uncommitted changes: commit them, or pass `force` only if the user agreed to lose them. The branch is kept.
- If a worktree was deleted another way (`git worktree remove`, your own tooling), call `prune_environments`.

Launch profiles (`localify.toml`, committed at the checkout root) change only the variables a service starts with: `profile` on `service_start` and `run_command`. `env` changes the checkout, `profile` the variables. To create or edit profiles, write `localify.toml` yourself, for example:

```toml
[profiles.default]
sources = [{ file = ".env.development" }]

[profiles.prod]
danger = true
sources = [{ infisical = { env = "prod" } }]
```

Never pass `confirm_profile` unless the user asked for that dangerous profile in this task, then say the service runs on its data. Never print, copy or write profile values into files. If `infisical` or `pass-cli` is signed out, ask the user to sign in.

Public URLs (Localify Cloud), only when the user wants to reach an app from another device or share it:
- `public_url_create` gives a project domain a `https://<slug>.localify.fyi` URL served from this machine. To reach several services or worktrees of a project, prefer one URL on the project's namespace `<project-id>.localify` with `wildcard`: each domain under it opens at `https://<slug>--<label>.localify.fyi`, and the name mirrors the local one (`louez.localify` gives `https://louez.localify.fyi`). It is always protected by a password: give the user `publicUrl.passwordLink`, the link that signs a browser in by itself, or the URL and the password together. `public_url_protect` replaces the password and signs every browser out. `public_url_list` carries `passwordLink` for the URLs whose password was set on this machine.
- Opening a URL to everyone is the user's call: they run `localify public-url unprotect` themselves. So is signing in: when a tool says the machine is not signed in, ask them to run `localify login`.
- Serving public URLs takes Localify Cloud Pro: $7.99 a month or $79 a year, after a 7-day free trial that asks for a card. Paying is the user's call too: when a tool answers that a plan is needed, tell them the price and the trial and ask them to run `localify subscribe`. When a tool answers that a limit is reached and names a command that raises it, suggest the user run that command. Never start a checkout or pass on a checkout link. `whoami` shows the plan (`plan.entitled`, dates, limits and usage) and why this machine's tunnel is blocked, if it is.
- `public_url_list` shows which machine serves each URL and its `state`: `suspended` when the plan does not cover it, `taken_down` when it was removed for abuse. `public_url_claim` moves one to this machine; `public_url_remove` takes it offline. When every serving slot is taken, removing the URLs another machine serves, or claiming them here, frees its slot.

When something does not work:
- Call `status` first. A domain that does not resolve: the daemon may be stopped (`daemon` start) or Localify not installed (`install_localify`). A page saying the target is unavailable: the app is not listening; read its logs.
- Do not edit /etc/hosts, run another proxy, bypass certificate errors, edit Localify's config file, or run Localify with sudo.
- `trust_https` and `lan_access` enable open macOS approval windows or expose the user's servers: only when the user asks.
- Read the daemon's own errors with `read_daemon_logs`.
