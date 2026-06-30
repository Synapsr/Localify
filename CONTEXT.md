# Localify

Localify gives local development projects stable local domains and can expose selected local entrypoints for outside access.

## Language

**Project**:
A local development workspace that groups one or more local domains.
_Avoid_: App, repository

**Project Namespace**:
The local domain label used to generate default service domains for a project. A project namespace is not itself a service or local domain unless the user assigns that hostname to a service.
_Avoid_: Project domain, root domain, canonical domain

**Agent Skill**:
A reusable instruction package that teaches coding agents how to use Localify consistently across projects.
_Avoid_: Prompt, plugin, script

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
An internet-reachable address attached to a public endpoint. A public URL owns its own access protection settings.
_Avoid_: Domain, project URL, public endpoint

**Access Protection**:
A public URL rule that requires one shared password before traffic can reach the public endpoint.
_Avoid_: User login, account access

**Access Exception**:
A path prefix on a protected public URL that remains reachable without the shared password.
_Avoid_: Route, public endpoint exception

**Public Endpoint**:
The public exposure of one local domain outside the developer's machine. A public endpoint has one or more public URLs and can be enabled or disabled without losing its reservation.
_Avoid_: Public URL, tunnel

**Wildcard Public Endpoint**:
A public endpoint for a wildcard local domain that preserves one subdomain label from the public URL to the local domain.
_Avoid_: Multi-level wildcard

**Persistent Public URL**:
A stable public URL reserved for a local domain and meant to survive local app or machine restarts.
_Avoid_: Temporary URL, random URL

**Custom Public URL**:
A public URL on a domain name owned by the user.
_Avoid_: Custom domain, local domain

**License Identity**:
The cloud identity derived from an active Localify license.
_Avoid_: User account, login
