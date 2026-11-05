Localify gives local apps stable `https://<name>.localify` URLs with trusted HTTPS, through a local DNS server and proxy. Each tool runs one `localify` CLI command and returns its output.

Working on an app:
- Start it with `start_service` (pass `cwd`, the app or repo folder). On first use in a folder it creates the Localify project, picks free ports, registers domains and patches Next.js/Vite allowlists. It returns the URL; use that URL, never `localhost:<port>`.
- Get a URL with `get_service_url`, and service names with `list_services`.
- Read a service's output with `read_service_logs`; stop it with `stop_service`. Stop what you started before you finish.
- Do not pick ports like 3000 yourself, and pass `target` only when the user asked for a fixed one.
- A monorepo app the first setup missed: `add_service`. Framework config drifted: `configure_dev`. Another domain: `set_service_domain`. Do not rerun `init_project` to repair.
- Use the `.localify` URL in OAuth callbacks, webhooks, CORS and cookie origins when that is the route under test.

Worktrees:
- In a Git worktree of a project, `start_service` with `cwd` set to the worktree gives it its own environment: its own domains and ports, next to the primary checkout.
- `.env` files are not copied into a worktree. Copy them with `copy_environment_files` when the app needs them.
- When the work in a worktree is done (merged, abandoned, or handed back), clean up so the project is not polluted: call `remove_environment` with `delete_worktree: true` and `cwd` outside that worktree. It stops the worktree's services, unregisters its domains and certificates, and deletes the folder. Git refuses when there are uncommitted changes: commit them, or pass `force` only if the user agreed to lose them. The branch is kept.
- If a worktree was deleted another way (`git worktree remove`, your own tooling), call `prune_environments`.

Launch profiles (`localify.toml`, committed at the checkout root) change only the variables a service starts with: `profile` on `start_service` and `run_command`. `env` changes the checkout, `profile` the variables. To create or edit profiles, write `localify.toml` yourself, for example:

```toml
[profiles.default]
sources = [{ file = ".env.development" }]

[profiles.prod]
danger = true
sources = [{ infisical = { env = "prod" } }]
```

Never pass `confirm_profile` unless the user asked for that dangerous profile in this task, then say the service runs on its data. Never print, copy or write profile values into files. If `infisical` or `pass-cli` is signed out, ask the user to sign in.

When something does not work:
- Call `status` first. A domain that does not resolve: the daemon may be stopped (`daemon` start) or Localify not installed (`install_localify`). A page saying the target is unavailable: the app is not listening; read its logs.
- Do not edit /etc/hosts, run another proxy, bypass certificate errors, edit Localify's config file, or run Localify with sudo.
- `trust_https` and `lan_access` enable open macOS approval windows or expose the user's servers: only when the user asks.
- Read the daemon's own errors with `read_daemon_logs`.
