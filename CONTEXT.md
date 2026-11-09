# Localify

Localify gives local development projects stable local domains and starts their services. Through Localify Cloud, it exposes chosen domains on public URLs and lets a person's machines see and control each other's services.

## Language

**Project**:
A local development workspace that groups one or more local domains.
_Avoid_: App, repository

**Project Namespace**:
The local domain label used to generate default service domains for a project. A project namespace is not itself a service or local domain unless the user assigns that hostname to a service.
_Avoid_: Project domain, root domain, canonical domain

**MCP Server**:
`localify mcp`, which gives coding agents every Localify action as a tool, each one a CLI command, along with the rules for using them.
_Avoid_: Skill, plugin, integration

**Agent-Initialized Project**:
A project created or bootstrapped by a coding agent with Localify local domains and framework settings configured from the start.
_Avoid_: Generated app, AI app

**Service**:
A runnable local entrypoint inside a project, such as a web app, API, docs app, or background HTTP service. A service has one primary local domain as its canonical URL and may have additional local domains.
_Avoid_: App, package, script, route

**Unavailable Service**:
A project service that cannot be run in a specific project environment because its local checkout does not contain the service path or required launch metadata. It is a configuration or diagnostic concept, not something every environment view must show by default.
_Avoid_: Broken service, deleted service

**Project Environment**:
A local checkout variant of a project, usually tied to a git worktree or branch, that can run the same services with distinct local domains and ports.
_Avoid_: Project, branch project, clone

**Launch Profile**:
A named choice of where a service's variables come from when Localify starts it, declared in the checkout's `localify.toml`, or implicit for a project linked to Infisical. It never changes the service, its local domains or its port.
_Avoid_: Environment, env, stage, mode

**Dangerous Launch Profile**:
A launch profile marked `danger = true`, typically running on production data. It is flagged in the terminal, the browser and the desktop app while it runs, and every launch is recorded in the daemon log.
_Avoid_: Production environment, prod env

**Git Worktree**:
A repository checkout that Localify may treat as a project environment for the same project.
_Avoid_: Project, clone

**Environment File**:
A local environment configuration file attached to a project environment, optionally scoped to one service inside that environment.
_Avoid_: Env, settings file, config file

**Primary Environment**:
The canonical project environment that owns the project's existing local domains unless another project environment is selected explicitly.
_Avoid_: Main branch, default branch

**Terminal Session**:
An interactive shell opened from Localify for a project environment or one service inside a project environment. In the app, a shell tab.
_Avoid_: Terminal tab, dev server, launcher

**Run**:
One execution of a service by `localify dev`, known by its run id, with its own output. The desktop app (a service tab), the background (`--detach`) or a terminal Localify does not own holds it. A run started before run ids existed is a legacy run.
_Avoid_: Process, instance, session

**Service Tab**:
The desktop app's tab for one run, in a pseudo-terminal the app owns. It is the run, not a shell: when the run ends, it shows the exit code.
_Avoid_: Terminal session, shell tab

**Local Domain**:
A stable `.localify` hostname for one local entrypoint in a project.
_Avoid_: Public URL, tunnel

**Route**:
A path mapping on a local domain that sends matching requests to a local target. A route may point at a service's local target, but it is not itself a service.
_Avoid_: Service, public URL

**Public URL**:
An internet-reachable address, `<slug>.localify.fyi`, that exposes one local domain of a project through the machine that serves it. It is protected by a password unless its creator opened it.
_Avoid_: Tunnel, public endpoint, share link

**Wildcard Public URL**:
A public URL that also exposes every one-label subdomain of its local domain, such as the project's environments, at `<slug>--<label>.localify.fyi`.
_Avoid_: Multi-level wildcard

**Slug**:
The unique name of a public URL under `localify.fyi`. It never contains `--`.
_Avoid_: Subdomain, public domain

**Access Protection**:
The shared password a public URL asks for before any traffic reaches the machine.
_Avoid_: User login, account access

**Localify Cloud**:
The paid hosted service behind public URLs and remote machines: accounts, plans, URL reservations, access protection, the public entry point and the relay between machines.
_Avoid_: Backend, server, relay

**Cloud Account**:
A person signed in to Localify Cloud with GitHub. Each machine they sign in from serves the public URLs it created or claimed, as far as their entitlement allows.
_Avoid_: License, user profile

**Plan**:
What a cloud account pays for on Polar: Localify Cloud Pro, monthly or yearly, after a 7-day trial, plus optional add-ons. Without a plan, an account can sign in but serves no public URL.
_Avoid_: Tier, license, subscription (Polar's object behind a plan)

**Entitlement**:
How many serving machines and public URLs a cloud account may use right now, computed from its plan's state and add-ons, or from an override. A ban makes it zero.
_Avoid_: Quota, permissions

**Override**:
A recorded grant of limits for one cloud account, with a reason and an optional expiry, that replaces what the plan would give, such as the team's access.
_Avoid_: Allowlist, comp, free plan

**Serving Machine**:
The one signed-in machine whose tunnel carries a public URL's traffic. Another machine takes over with an explicit claim.
_Avoid_: Host, connector, agent

**Serving Slot**:
A machine of a cloud account that holds a tunnel, being created, ready or being deleted. An entitlement allows a number of them; signing in uses none.
_Avoid_: Seat, machine limit, connection

**Suspended**:
A public URL that exists, keeps its slug and password, but does not serve, because its account's entitlement does not cover it. Paying again or removing other URLs brings it back. Not the same as taken down, which is a removal for abuse.
_Avoid_: Disabled, offline, deleted

**Machine**:
One signed-in installation of Localify, known by its machine id. Its name is only a label.
_Avoid_: Device, host

**Pairing**:
Two machines of one cloud account pinning each other's key, after a person confirms the same 6-digit code on both. Only paired machines see each other's projects.
_Avoid_: Linking, trusting

**Viewer** and **Target**:
The machine a person looks from, and the machine looked at. Any paired machine can be either. A target decides on its own what viewers may see and do there: share, control, background starts.
_Avoid_: Client, server
