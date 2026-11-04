# Public URLs

> **Design note for a feature that is not built.** Nothing described here ships in Localify today.

Planning reference for native Localify public URLs. This document summarizes the v1 product and architecture decisions captured in `CONTEXT.md` and `docs/adr/`.

## Intent

Public URLs let a Localify user expose a project-owned local domain to the internet without changing the local routing model. The public feature should feel as simple as copying a stable URL, while the implementation keeps ownership, DNS, access protection, and connector state explicit.

LocalCan is a useful UX reference for persistent URLs, pause/resume, custom domains, access protection, and traffic visibility. Localify should not integrate LocalCan or copy its daemon/project model; public URLs are native Localify Cloud functionality.

## Language

Use the terms from `CONTEXT.md` consistently:

- `Local Domain`: the configured `.localify` domain attached to a project.
- `Public Endpoint`: the cloud-owned exposure of one local domain.
- `Public URL`: an internet-reachable address attached to a public endpoint.
- `Persistent Public URL`: a stable Localify-owned public URL.
- `Custom Public URL`: a user-owned hostname attached to a public endpoint.
- `Access Protection`: one shared password for one public URL.
- `Access Exception`: a path prefix that bypasses access protection.

Avoid using `tunnel` as the product term. It can describe internal transport, but the user-facing model is public endpoints and public URLs.

## V1 Scope

V1 supports:

- Project-attached configured local domains only.
- Exact local domains.
- Wildcard local domains using existing Localify semantics: root plus one subdomain label.
- Localify-owned public URLs with a generated, editable slug.
- Custom public URLs with DNS verification and managed TLS.
- Optional access protection per public URL.
- Path-prefix access exceptions per protected public URL.
- Native Localify connector integrated with the daemon/runtime.
- Cloudflare-hosted Localify Cloud control plane and edge entry.
- CLI parity for all desktop UI actions.
- Enriched request logs for public traffic.

V1 does not support:

- Temporary/random public URLs.
- External or Docker-discovered domains.
- Standalone/unassigned local domains.
- Multi-level wildcard forwarding.
- Custom header rewrite rules.
- Full traffic inspection with bodies, replay, or complete header capture.
- Import/export of public endpoints through local settings transfer.
- Multiple invited users or multiple passwords per public URL.
- Method, header, token, or IP based access rules.

## Core Model

One configured local domain may have zero or one active public endpoint.

One public endpoint may have one or more public URLs. A public URL may be Localify-owned or custom.

Example:

```text
Project: Journal
Local Domain: journal.localify
Public Endpoint: pe_123
Public URLs:
  - https://journal-a8f3.localify.dev
  - https://preview.journal.com
```

The public endpoint targets the local domain, not the route target. Public traffic flows through Localify's existing local routing:

```text
Public URL -> Localify Cloud -> Localify connector -> Local Domain -> Route Target
```

This preserves project enablement, route matching, path routing, local diagnostics, and logs.

## Wildcards

Wildcard public endpoints mirror Localify wildcard semantics.

Supported:

```text
*.journal.localify
*.preview.journal.com

preview.journal.com          -> journal.localify
client-a.preview.journal.com -> client-a.journal.localify
client-b.preview.journal.com -> client-b.journal.localify
```

Not supported in v1:

```text
a.b.preview.journal.com -> a.b.journal.localify
```

Wildcard custom public URLs require wildcard DNS records. If the root hostname should work, it needs its own DNS record too:

```text
preview.journal.com   CNAME <endpoint>.localify...
*.preview.journal.com CNAME <endpoint>.localify...
```

Access protection on a wildcard public URL applies to the whole wildcard. If one subdomain later needs a different policy, model it as a more specific exact public URL.

## Localify-Owned Public URLs

Localify-owned public URLs use a human-readable slug plus a short uniqueness suffix:

```text
journal-a8f3.localify.dev
```

Rules:

- Generated at creation time.
- Editable with availability validation.
- Changing it is explicit because the old URL may break.
- Deleted Localify-owned public URLs stay reserved for the same cloud identity for 30 days.
- A wildcard public URL counts as one public URL reservation.

## Custom Public URLs

Custom public URLs require DNS verification before becoming active.

Flow:

1. User adds a hostname.
2. Localify Cloud returns the required DNS record.
3. User creates the DNS record.
4. Localify verifies DNS.
5. Localify provisions/validates TLS.
6. URL becomes active.

Custom URLs detach immediately when removed and require DNS verification to attach elsewhere.

## Access Protection

Access protection belongs to each public URL, not the public endpoint.

Example:

```text
https://journal-a8f3.localify.dev -> password protected
https://preview.journal.com       -> public
```

V1 behavior:

- One shared password per protected public URL.
- Passwords are never stored in plaintext.
- Protection is enforced at Localify Cloud before traffic reaches the connector.
- Browser users see a Localify password page.
- Successful auth sets a browser cookie scoped to that public URL.
- HTTP Basic Auth is not the v1 UX.
- Path-prefix exceptions can bypass protection.

Access exceptions use Localify path-prefix semantics:

```text
/api/webhooks/stripe
/auth/callback
/healthz
/.well-known
```

`/api` matches `/api` and `/api/users`, but not `/apiv2`.

## Request Routing And Headers

Localify Cloud resolves the public URL to a public endpoint. The connector routes by local domain.

The upstream app should see the public host by default:

```text
Host: preview.journal.com
X-Forwarded-Host: preview.journal.com
X-Forwarded-Proto: https
X-Forwarded-For: <client-ip>
X-Localify-Local-Domain: journal.localify
```

V1 does not expose user-configurable header rewrite rules.

## Runtime States

Keep endpoint runtime state separate from public URL provisioning state.

Endpoint runtime examples:

- `online`: connector is serving the endpoint.
- `offline`: no active connector.
- `disabled`: endpoint reservation exists but is intentionally not served.
- `targetUnavailable`: connector is online but the local target is down.
- `offlineByProject`: project is disabled, so public serving is stopped.
- `orphanedLocalDomain`: cloud endpoint exists but the matching local domain is missing locally.

Public URL provisioning examples:

- `active`: URL can receive traffic.
- `pendingDns`: custom DNS is not verified.
- `pendingCertificate`: DNS is verified, TLS is not ready.
- `error`: provisioning failed with a specific message.

Endpoint creation is allowed when the local target is not reachable. The URL can be provisioned while Localify reports the target as unavailable until the dev server starts.

## Project And Domain Rules

Public endpoints v1 attach only to configured local domains assigned to projects.

Current compatibility paths for standalone/unassigned domains may still exist in code, but new public endpoint flows should not use them.

Rules:

- Project disabled: public endpoints are not served, but reservations remain.
- Endpoint disabled: reservation remains, connector does not serve it.
- Deleting a public local domain: detach endpoint by default; deleting the cloud endpoint is an explicit destructive option.
- Renaming or deleting a local domain that affects a public endpoint requires Localify Cloud to be reachable.
- Cloud-aware mutations must write through Cloud first, then sync locally.
- If a domain is removed outside the cloud-aware flow, the next cloud sync can surface the endpoint as orphaned.

## Cloud Identity

Cloud identity will be defined when Localify Cloud is built. It must cover endpoint ownership, contact email, quotas, and the machines that may serve an endpoint.

Localify Cloud checks the identity server-side before cloud mutations. The desktop app must not rely on local cached state alone for URL reservation or endpoint mutations.

## Machine Ownership

Endpoints are owned by the cloud identity, but served by one selected local machine at a time.

Rules:

- A connector authenticates with a short-lived token scoped to endpoint, machine, and cloud identity.
- A long-lived credential is not sent on every connector session.
- Automatic daemon startup may reconnect endpoints selected for that machine.
- It must not silently take over endpoints currently served by another machine.
- Switching machines is an explicit claim.
- A claim replaces the previous connector session.

## Localify Cloud

V1 runs on Cloudflare.

Responsibilities:

- Ownership by cloud identity.
- Endpoint and URL reservation.
- DNS verification.
- TLS readiness.
- Access protection.
- Public traffic entry.
- Connector token issuance.
- Runtime coordination.
- Retention of deleted Localify-owned URLs.

Recommended shape:

- Worker API for CRUD, DNS verification, auth, and CLI/DesktopApi calls.
- One Durable Object per public endpoint for connector session and traffic dispatch.
- Storage for endpoint metadata, public URLs, quotas, retention, and audit fields.

The local connector remains native Localify Rust code. Cloudflare is the hosted control plane and public edge runtime, not an external tunnel binary dependency.

## Local Cache

The desktop may keep a public endpoint cache outside `config.toml`, for example:

```text
~/Library/Application Support/Localify/public_endpoints_cache.json
```

Rules:

- Cache is resynchronizable from Localify Cloud.
- Cache is excluded from settings export/import.
- Cache must not store shared passwords or public access secrets.
- Cache may store ids, URLs, statuses, access mode, exception paths, timestamps, and summary counts.

## Desktop API

Use dedicated public endpoint commands instead of embedding cloud mutations in local domain commands.

Minimum v1 commands:

- `list_public_endpoints`
- `create_public_endpoint`
- `update_public_endpoint_enabled`
- `delete_public_endpoint`
- `create_public_url`
- `update_public_url`
- `delete_public_url`
- `check_public_url_dns`
- `update_public_url_access_protection`
- `rotate_public_url_password`
- `select_public_endpoint_machine` or `claim_public_endpoint`

Domain list DTOs may include only lightweight public endpoint summaries for project-list display. Detailed public endpoint state comes from dedicated commands.

## CLI

Every desktop action must be available through the CLI so agents can perform the same work.

User-facing namespace:

```bash
localify public-url
```

Expected v1 commands:

```bash
localify public-url list
localify public-url create <local-domain>
localify public-url add-custom <local-domain> <hostname>
localify public-url remove <url-or-id>
localify public-url enable <endpoint-or-domain>
localify public-url disable <endpoint-or-domain>
localify public-url protect <url-or-id>
localify public-url unprotect <url-or-id>
localify public-url password rotate <url-or-id>
localify public-url exceptions add <url-or-id> <path-prefix>
localify public-url exceptions remove <url-or-id> <path-prefix>
localify public-url dns check <url-or-id>
localify public-url claim <endpoint-or-domain>
```

CLI behavior:

- Human-readable output for interactive terminals.
- JSON automatically when stdout is not a TTY.
- `--json` forces JSON.
- Interactive prompts are allowed in TTY sessions.
- Every prompt must have a flag-driven non-interactive path.
- Non-TTY execution must never block waiting for input.
- Destructive flows need explicit confirmation or flags such as `--yes`.

## Logs

V1 enriches existing request logs rather than building a full traffic inspector.

Capture enough dimensions for debugging:

- source: local or public
- public URL used
- local domain routed
- method, path, status, duration
- client IP and derived location if available
- access protection outcome
- access exception matched, if any
- endpoint id and public URL id where useful

Logs remain scoped by local domain. Public URLs are aliases/sources for that local entrypoint.

## Quotas

Track public endpoint and public URL quotas separately from local project/domain limits.

Rules:

- One public endpoint exposes one local domain.
- One public URL is one address attached to an endpoint.
- Wildcard public endpoint counts as one endpoint.
- Wildcard public URL counts as one URL.

Exact quota values are not fixed in this document.

## Open Implementation Decisions

The following still need design during implementation:

- Exact connector transport protocol between daemon and Durable Object.
- Cloudflare storage choice for metadata beyond Durable Object state.
- DNS/TLS provisioning details for custom domains.
- Exact DTO fields and status enum names.
- Exact CLI prompts and flag names.
- Exact UI layout and polish.
