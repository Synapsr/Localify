> **Amended.** [ADR 0019](0019-public-urls-run-on-named-cloudflare-tunnels.md) replaces the Durable Object data plane with named Cloudflare Tunnels. Localify Cloud still runs on Cloudflare: a Worker and a D1 database.

# Cloudflare for Localify Cloud V1

Localify Cloud v1 runs on Cloudflare so public endpoints can use edge-hosted HTTP handling, DNS/TLS integration, and stateful connector coordination without Localify operating its own global proxy fleet. The local connector remains native to Localify; Cloudflare is the hosted control plane and edge runtime for reservation, verification, access protection, and public traffic entry. Each public endpoint is coordinated by its own Durable Object so one endpoint owns one active connector session, runtime state, and traffic dispatch boundary.
