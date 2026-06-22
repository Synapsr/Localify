# Localify

Localify gives local development projects stable local domains and can expose selected local entrypoints for outside access.

## Language

**Project**:
A local development workspace that groups one or more local domains.
_Avoid_: App, repository

**Agent Skill**:
A reusable instruction package that teaches coding agents how to use Localify consistently across projects.
_Avoid_: Prompt, plugin, script

**Agent-Initialized Project**:
A project created or bootstrapped by a coding agent with Localify local domains and framework settings configured from the start.
_Avoid_: Generated app, AI app

**Local Domain**:
A stable `.localify` hostname for one local entrypoint in a project.
_Avoid_: Public URL, tunnel

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
