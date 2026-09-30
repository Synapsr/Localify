# Public URLs

Planning reference for native Localify public URLs. This document summarizes the v1 product and architecture decisions captured in `CONTEXT.md` and `docs/adr/`.

## Intent

Public URLs let a licensed Localify user expose a project-owned local domain to the internet without changing the local routing model. The public feature should feel as simple as copying a stable URL, while the implementation keeps ownership, DNS, access protection, and connector state explicit.

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
- `License Identity`: the cloud identity derived from the active Localify license.

Avoid using `tunnel` as the product term. It can describe internal transport, but the user-facing model is public endpoints and public URLs.

## V1 Scope

V1 supports:

- Project-attached configured local domains only.
- Exact local domains.
- Wildcard local domains using existing Localify semantics: root plus one subdomain label.
- Localify-owned public URLs on `localify.fyi` with a generated, editable slug.
- Custom public URLs with DNS verification and managed TLS, including wildcard custom public URLs through on-demand exact hostnames.
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
  - https://journal-a8f3.localify.fyi
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

journal-a8f3.localify.fyi           -> journal.localify
client-a--journal-a8f3.localify.fyi -> client-a.journal.localify
```

Not supported in v1:

```text
a.b.preview.journal.com -> a.b.journal.localify
```

### Project environments

A wildcard public endpoint on a project's primary local domain exposes every project environment of that project. Environment local domains are exact registrations one label under the project namespace (ADR 0009), and the local router prefers them over the wildcard, so each public label reaches its own environment:

```text
*.app.localify on the remote machine

app-a8f3.localify.fyi                     -> app.localify                (primary environment)
feature-x--app-a8f3.localify.fyi          -> feature-x.app.localify      (single-service project)
api--feature-x--app-a8f3.localify.fyi     -> api--feature-x.app.localify (multi-service project)
```

A new worktree attached as a project environment is reachable at once on the Localify-owned URL, with no public URL to create. An unknown label falls back to the primary environment, as it does locally.

### Custom wildcard DNS

Wildcard custom public URLs need one wildcard DNS record, plus a record for the root hostname if the root should resolve:

```text
preview.journal.com   CNAME <edge>.localify.fyi
*.preview.journal.com CNAME <edge>.localify.fyi
```

Cloudflare for SaaS only issues wildcard custom hostnames on the Enterprise plan. Localify Cloud instead creates one exact custom hostname for the root and for each label the connector reports under the wildcard local domain, and deletes it when the label disappears (ADR 0016). Consequences:

- A new environment reaches the custom domain only after its certificate is issued. Its Localify-owned URL works immediately.
- Environment names under a custom domain appear in Certificate Transparency logs.
- An unreported label fails at the TLS handshake instead of falling back to the root.
- Custom hostnames count against a per-license quota, because Cloudflare charges per hostname beyond the 100 included for the whole account.

Access protection on a wildcard public URL applies to the whole wildcard. If one subdomain later needs a different policy, model it as a more specific exact public URL. On a Localify-owned wildcard URL the access cookie is host-only, so each label asks for the password once.

## Localify-Owned Public URLs

Localify-owned public URLs live on `localify.fyi`, a domain kept separate from `localify.dev` (website, checkout, downloads). They use a human-readable slug plus a short uniqueness suffix, one DNS label deep so the `*.localify.fyi` certificate covers every URL (ADR 0015):

```text
journal-a8f3.localify.fyi
client-a--journal-a8f3.localify.fyi
```

Rules:

- The last `--` separates a wildcard label from the slug, so slugs cannot contain `--`.
- The public base domain is configuration, not code. Changing it breaks existing URLs unless both domains are served during a transition.
- Generated at creation time.
- Editable with availability validation.
- Changing it is explicit because the old URL may break.
- Deleted Localify-owned public URLs stay reserved for the same license identity for 30 days.
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
https://journal-a8f3.localify.fyi -> password protected
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

The upstream app sees the local domain in `Host` and the public host in `X-Forwarded-Host` (ADR 0017). Localify adds the `.localify` hostname to dev server allowlists such as Vite's `server.allowedHosts`, so a public `Host` would be rejected:

```text
Host: journal.localify
X-Forwarded-Host: preview.journal.com
X-Forwarded-Proto: https
X-Forwarded-For: <client-ip>
X-Localify-Local-Domain: journal.localify
```

Apps that build absolute URLs, such as OAuth redirect URIs, must read `X-Forwarded-Host`.

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

## License Identity

V1 does not add a separate user account or login flow.

The active Localify license is the cloud identity:

- Ownership is keyed by the validated license.
- Email may come from Keygen metadata or owner data.
- Quotas come from the license plan.
- Machines are associated with the license.
- Localify Cloud revalidates the license server-side before cloud mutations.

The desktop app must not rely on local license cache alone for URL reservation or endpoint mutations.

## Machine Ownership

Endpoints are owned by the license, but served by one selected local machine at a time.

Rules:

- A connector authenticates with a short-lived token scoped to endpoint, machine, and license identity.
- The license key is not sent on every long-lived connector session.
- Automatic daemon startup may reconnect endpoints selected for that machine.
- It must not silently take over endpoints currently served by another machine.
- Switching machines is an explicit claim.
- A claim replaces the previous connector session.

## Localify Cloud

V1 runs on Cloudflare.

Responsibilities:

- License-backed ownership.
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
- `localify.fyi` as a Cloudflare zone with a wildcard DNS record and the Universal SSL certificate for `*.localify.fyi`, routed to the edge Worker.
- Cloudflare for SaaS on the same zone for custom public URLs, with exact custom hostnames created on demand for wildcard custom URLs.

The local connector remains native Localify Rust code. Cloudflare is the hosted control plane and public edge runtime, not an external tunnel binary dependency. Orchestrating `cloudflared` tunnels per machine was reconsidered and rejected (ADR 0003).

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

Exact quota values are plan policy, not fixed in this document.

## Open Implementation Decisions

The following still need design during implementation:

- Exact connector transport protocol between daemon and Durable Object: per-stream framing, flow control, WebSocket passthrough for HMR, cancellation, and reconnection. traforo (https://github.com/remorses/traforo) is a reference for the Durable Object side. The local spike in `spikes/public-url/` validates streaming, SSE, and HMR passthrough; flow control and real network latency remain open.
- Whether the connector dispatches public requests straight into the daemon's router instead of looping back through the local HTTP or HTTPS port.
- Cloudflare storage choice for metadata beyond Durable Object state.
- DNS/TLS provisioning details for custom domains.
- How the connector reports the labels it can serve under a wildcard local domain, and how fast exact custom hostnames follow.
- What happens when `<label>--<slug>` exceeds the 63-character DNS label limit.
- Whether one password entry can unlock every label of a Localify-owned wildcard URL, for example through a redirect-based sign-in on a shared auth hostname.
- Exact DTO fields and status enum names.
- Exact CLI prompts and flag names.
- Exact UI layout and polish.
