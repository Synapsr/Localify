# Localify

Localify gives local development projects stable local domains, and exposes chosen ones on public URLs through Localify Cloud.

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
An interactive shell opened from Localify for a project environment or one service inside a project environment.
_Avoid_: Terminal tab, dev server, launcher

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

**Access Exception**:
A path prefix on a protected public URL that stays reachable without the password, for webhooks and OAuth callbacks.
_Avoid_: Route, bypass

**Localify Cloud**:
The hosted service behind public URLs: accounts, URL reservations, access protection and the public entry point.
_Avoid_: Backend, server, relay

**Cloud Account**:
A person signed in to Localify Cloud with GitHub. Each machine they sign in from serves the public URLs it created or claimed.
_Avoid_: License, user profile

**Serving Machine**:
The one signed-in machine whose tunnel carries a public URL's traffic. Another machine takes over with an explicit claim.
_Avoid_: Host, connector, agent
