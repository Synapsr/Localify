# Launch profiles

A launch profile decides where a service's environment variables come from
when Localify starts it: local dotenv files, Infisical, Proton Pass, or a mix. The service,
its `.localify` domain and its port stay the same. Only the variables change.

```bash
# From the monorepo root: pick the app in the selector, and it starts.
localify dev --profile prod
localify dev -p prod          # same, with the short flag
```

## Profiles and project environments

These are two different things, with two different flags:

| | `--env <name>` | `--profile <name>` |
| --- | --- | --- |
| What changes | The checkout: a Git worktree or branch | The variables the service starts with |
| Domain and port | Its own (`feat-auth.my-app.localify`) | Unchanged |
| Declared in | Localify config (`localify project env ensure`) | `localify.toml` in the checkout, or implicit with Infisical |

You can combine them: `localify dev web --env feat-auth --profile staging`.

## Set up with `localify profile init`

Run it from the project folder:

```bash
localify profile init
```

It walks you through the setup:

- **Questions:** what services start with when `--profile` is omitted, then one profile at a time
  (name, sources, dangerous or not).
- **Infisical:** offered first when the project has `.infisical.yaml`. It asks for the environment
  slug and the folder of each service.
- **Proton Pass:** it lists your vaults and suggests the note title pattern your existing notes
  already follow, such as `Acme/{service_path}/.env.production`.
- **Checks:** it marks each service ✓ or ✗ depending on whether its file, note or Infisical folder
  exists. It reads note titles and folder names only, never values.
- **Writing:** it shows the file, and writes it only after you confirm. Rewriting an existing file
  drops its comments.

It needs a terminal. Without one, write the file by hand as described below.

## `localify.toml`

Profiles live in `localify.toml` at the root of the project checkout. The file
only holds addresses (file paths, vault names, note titles), never values, so
commit it. Every worktree gets the same profiles.

```toml
# localify.toml at the root of the monorepo

[profiles.default]
# Used when --profile is omitted. It can never be dangerous.
sources = [
  { file = ".env.development" },
]

[profiles.prod]
danger = true
# max_duration = "4h"          # optional: stop the service automatically
# browser_indicator = false    # optional: hide the in-page indicator
sources = [
  { proton_pass_note = { vault = "Env", title = "Acme/{service_path}/.env.production" } },
]

[profiles.staging]
sources = [
  { file = ".env.staging.refs" },   # values such as pass://Staging/acme-db/password
  { file = ".env.development" },    # fills in the keys the first file does not define
]
```

Rules:

- **Order:** sources are read in order, and the first source that defines a key wins. Inside one file,
  the last duplicate wins, as with dotenv.
- **`file`:** the path is relative to the **service folder** (`apps/web/`), not to the folder you ran
  the command from.
- **`infisical`:** reads every secret of one Infisical folder; see [Infisical](#infisical).
- **`proton_pass_note`:** reads a Proton Pass note whose body is a complete dotenv file. `vault` and
  `title` are names. `{service_path}` expands to the service folder relative to the project root, and
  `{service_name}` to its last segment (`web` for `apps/web`), so one profile covers every app of a
  monorepo. For `apps/web`, the example above reads
  `Acme/apps/web/.env.production`. For a service at the root, `{service_path}/` disappears and the
  title becomes `Acme/.env.production`.
- **`pass://` references:** any value that starts with `pass://`, in a `file` source or in
  `--env-file`, is resolved through Proton Pass at launch. Names work (`pass://Vault/Item/field`); a
  title that contains `/` must be encoded as `%2F`.
- **`danger`:** turns on the guardrails described below.
- **`max_duration`:** optional; accepts durations such as `"4h"` or `"90m"`. Without it, the service
  runs until you stop it.
- **`browser_indicator`:** shows the profile in the app's pages (see below). On by default for
  dangerous profiles, off otherwise; set it to change that.
- **Typos:** an unknown key, such as `dangr = true`, is an error, so a typo cannot silently turn a
  guardrail off.
- **No profile:** without `localify.toml`, without `.infisical.yaml` and without `--profile`,
  services launch exactly as before.

`--env-file <path>` adds a one-off dotenv file, before the profile's sources. It can be repeated,
its path is relative to the current directory, and a missing file is always an error.

## Switching off side effects: `[profiles.<name>.env]`

Running locally on production data should not send real emails, push notifications or invoices.
Most of these side effects are driven by an environment variable, so a profile can force safe values
over its sources:

```toml
[profiles.prod.env]
CRONS_ENABLED = "false"               # a switch your app reads
INFAST_DRY_RUN = true                 # a dry-run flag the integration already has
RESEND_TOKEN = "re_disabled"          # keeps the "re_" format validators expect; sending fails
SMTP_HOST = "smtp.disabled.invalid"   # .invalid never resolves
```

- **Priority:** these values win over every source and over the framework's own files. Only the
  shell and Localify's runtime variables win over them.
- **Values:** strings, booleans and numbers.
- **Neutral values:** pick values that keep the app booting (env validators often check a format) but
  make the side effect fail or no-op.
- **No switch in the app:** when a side effect has no switch, such as a cron that always runs, add one
  (`CRONS_ENABLED`) and set it here. Checking `LOCALIFY_DANGER=1` also works, but a named switch keeps
  the app independent of Localify.
- **Setup:** for a dangerous profile, `localify profile init` lists the keys that look like side
  effects (dry-run and `*_ENABLED` switches, crons, email, SMS and push providers), by name only,
  and writes the ones you pick.

## Commands that are not services: `localify exec`

A database studio, a migration or a worker with its own port is not a Localify service. Run it with
the variables of a profile from its folder:

```bash
cd packages/database && localify exec --profile prod -- drizzle-kit studio --port 3005
```

- **Which `localify.toml`:** the nearest one in the current folder or its parents, or the nearest
  `.infisical.yaml` for implicit profiles.
- **Sources:** they are read relative to the current folder, which also becomes `{service_path}`
  (`packages/database`).
- **Variables:** Localify injects no `PORT`, `HOST` or URL variables, only the profile markers
  (`LOCALIFY_LAUNCH_PROFILE`, `LOCALIFY_DANGER`). The shell still wins over the profile.
- **Same rules as `dev`:** masking, `--env-file`, `--confirm-profile` without a terminal, and for a
  dangerous profile the red banner, the terminal title and the audit log (so the daemon must run).
- **Exit code:** it is the command's own, so `localify exec` works in `package.json` scripts, for
  example `"db:studio:prod": "localify exec --profile prod -- drizzle-kit studio"`.

## Which value wins

From strongest to weakest:

1. **Localify runtime variables**, which keep routing working: `PORT`, `HOST`, `LOCALIFY_URL`, the
   URL variables, `NODE_EXTRA_CA_CERTS`, `LOCALIFY_LAUNCH_PROFILE` and `LOCALIFY_DANGER`. URL keys
   found in the profile, such as `NEXT_PUBLIC_APP_URL` or `AUTH_URL`, are rewritten to the local
   domain, so a production profile cannot send OAuth callbacks to production.
   `BETTER_AUTH_TRUSTED_ORIGINS` from the profile or the shell is kept, with your public URLs'
   origins appended ([README](README.md#apps-that-check-origin)).
2. **Variables already exported in your shell.** Localify prints the names of the profile keys your
   shell overrides.
3. **`[profiles.<name>.env]`, then `--env-file`, then the profile's sources.**
4. **Files the framework loads itself** (`.env.development`, `.env.local`…). Next.js, Vite and dotenv
   never override a variable already present in the process.

One side effect of rule 4: an injected variable also wins over `.env.local`. Prefer listing files the
framework does not load on its own.

The service sees `LOCALIFY_LAUNCH_PROFILE=<name>`, plus `LOCALIFY_DANGER=1` for a dangerous profile.
Use them to render the app's own banner or block risky actions.

## Missing sources

- **Normal profile:** the service starts, and Localify prints which file, note, reference or Infisical
  folder is missing. For Infisical, this includes an expired session or a missing CLI, so a plain
  `localify dev` keeps working.
- **Dangerous profile:** the launch fails. Starting on a mix of development and production values is
  worse than not starting.

## Dangerous profiles (`danger = true`)

- **No prompt in a terminal:** `--profile prod` is already explicit, and the indicators below follow.
  Without a terminal (agents, scripts, CI), `--confirm-profile <name>` is required and must repeat the
  name, so automation cannot start a dangerous profile by accident.
- **Terminal:** a red banner prints at startup, and the terminal title becomes `[PROD] web`.
- **Desktop app:** the service row shows a red badge with the profile name while it runs.
- **Browser:** the tab title gets a `[PROD]` prefix, and a small red pill with the profile name sits
  on top of the page. Drag it anywhere; the position is remembered per site.
- **Browser tools and API clients:** the proxy adds `X-Localify-Profile: <name>` to the service's
  responses.
- **No default:** `default` cannot be dangerous, and the desktop app never remembers a chosen profile,
  so a relaunch the user did not choose uses `default`.
- **Time limit:** with `max_duration`, Localify stops the service and its child processes when the
  time is up, and says so.
- **Daemon required:** a dangerous launch needs the Localify daemon, which records it. If the daemon
  cannot be reached, the service does not start.
- **Audit log:** every start and stop goes to the daemon log (`localify logs`), with service,
  profile, sources with key counts, timestamps and stop reason. Values are never logged.

## Infisical

If the project is linked to Infisical, you have nothing to write. Run this once at the project root:

```bash
infisical init      # writes .infisical.yaml; commit it
```

Without `localify.toml`, Localify then offers three profiles that read the Infisical environment of
the same name:

| Command | Infisical environment | Dangerous |
| --- | --- | --- |
| `localify dev` | `dev` | no |
| `localify dev --profile staging` | `staging` | no |
| `localify dev --profile prod` | `prod` | yes |

Each service reads the folder named after its own folder: `apps/web` reads `/web`, and a service at
the root reads `/`. The desktop app's launch menu shows the same three profiles.

To change any of this (another environment, another folder, a mix with local files, forced values
for `prod`), declare the profiles in `localify.toml`, which then replaces the implicit ones:

```toml
[profiles.default]
sources = [
  { infisical = {} },                 # environment dev, folder /{service_name}
  { file = ".env.development" },      # fills in the keys Infisical does not define
]

[profiles.prod]
danger = true
sources = [
  { infisical = { env = "production", path = "/{service_path}" } },
]
```

- **`env`:** the environment slug, as shown in the project settings on Infisical. Without it,
  `default` reads `dev` and other profiles read the environment of their own name.
- **`path`:** the folder, starting with `/`. Defaults to `/{service_name}`. Secrets imported into
  that folder in Infisical are included.
- **CLI:** Localify calls the official `infisical` CLI with your login, from the project root. The
  instance (cloud, EU or self-hosted) and the project come from the CLI's own configuration.
  Values stay in memory and are masked in the output, like Proton Pass values.
- **Several organizations:** when the CLI's current login profile belongs to another organization
  than the project, Localify tries your other login profiles (`infisical profile list`) and shows
  the one that worked next to the source. `infisical profile bind <name>` at the project root
  skips that retry.
- **Personal overrides:** your personal secrets override shared ones, as in `infisical run`.
- **Unreadable values:** a key whose value your account may not read is not injected, and counts as
  a missing source: a dangerous profile refuses to start.
- **Time limit:** a CLI call that takes more than 20 seconds (typically an expired session waiting
  for a browser login) is stopped, and the source counts as missing.

```bash
brew install infisical/get-cli/infisical
infisical login
```

## Proton Pass

Localify calls the official CLI, `pass-cli`, and keeps values in memory. It writes no temporary file
and keeps no cache.

```bash
brew install protonpass/tap/pass-cli
pass-cli login
```

If `pass-cli` is missing or signed out, the error gives the command to run.

**Masking:** when a value comes from Proton Pass or Infisical, Localify masks it in the service's output as `***`.

- Values shorter than 8 characters are not masked.
- `NEXT_PUBLIC_*`, `VITE_*`, `PUBLIC_*` and `EXPO_PUBLIC_*` values are not masked, since they ship to
  the browser anyway.
- Masking is best effort: a value the app encodes or splits is not recognized.
- The service's stdout and stderr go through a pipe. Localify sets `FORCE_COLOR=1` when you did not,
  and stdin stays your terminal, so dev-server shortcuts keep working.
- `--no-masking` turns masking off.

**Why not `pass-cli run`:** `pass-cli run --env-file` overwrites variables already exported in the
shell, which breaks rules 1 and 2 above. It also cannot read a note as a whole dotenv file, and it
detaches the service from the terminal.

### Migrating from a sync script

If your notes are named `<project>/<path of the file>` in one vault (for example
`Acme/apps/web/.env.production` in `Env`), point a profile at them with
`title = "Acme/{service_path}/.env.production"`. You do not need to rename anything.

## Desktop app

- **Launching:** the launch button uses `default`. When the checkout has other profiles (declared,
  or implicit for a project linked to Infisical), a small
  chevron next to it offers "Launch with <name>". Dangerous profiles are red and show their time limit.
- **Terminal:** the launch opens the integrated terminal and gives it focus.
- **Badges:** running services show a badge with their profile name. `default` gets none.
- **Broken file:** a `localify.toml` that cannot be read shows a warning instead of the chevron.

The desktop app runs the same `localify dev --profile <name>` command as the CLI and duplicates none
of its logic.

## The browser indicator

For a profile with `browser_indicator` on, the proxy adds one tag right after `<head>` in page
navigations (not in fetches, assets, iframes or HMR):

```html
<script src="/.localify/launch-profile.js" data-profile="prod" defer></script>
```

The script is served by Localify on the app's own origin. After the page's `load` event, once the
framework has hydrated, it prefixes the tab title and adds the pill. It keeps the prefix when the app
changes the title during client-side navigation, and puts the pill back if the app re-renders `<body>`.

How it stays out of the app's way:

- **CSP:** the tag is a same-origin script, allowed by `script-src 'self'`. When the page's CSP header
  uses a nonce, the tag reuses it. The pill lives in a shadow root and is styled through the CSSOM,
  which `style-src` does not restrict. Tested with `style-src 'self'` on Next.js 16.
- **Hydration:** nothing touches the DOM before `load`. React 19 also skips the extra `<script>` in
  `<head>` during hydration. Tested on Next.js 16 and 14 with client navigation.
- **Streaming:** only the start of the document is scanned; the rest of the stream passes through.
  To edit the page, the proxy asks the dev server for an uncompressed response.

Limits: a CSP that allows neither `'self'` scripts nor a header nonce blocks the script (the app keeps
working, without the indicator). Apps that hydrate the whole document with React 18 may report a
hydration warning for the extra tag. Turn it off with `browser_indicator = false`; apps can still
read `LOCALIFY_DANGER=1` and render their own banner.
