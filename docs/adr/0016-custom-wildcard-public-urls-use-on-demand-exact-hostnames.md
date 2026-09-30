# Custom Wildcard Public URLs Use On-Demand Exact Hostnames

Custom wildcard public URLs such as `*.preview.journal.com` are in v1 because exposing every project environment under one address is a core use case. Cloudflare for SaaS only issues wildcard custom hostnames on the Enterprise plan, a negotiated annual contract that third-party sources put at $3,000 to $5,000 per month to start. Localify Cloud instead creates exact custom hostnames on demand. The user adds one wildcard CNAME to the Localify edge hostname, plus a record for the root if the root should resolve. The connector reports which labels it can serve under the wildcard local domain; Localify Cloud creates an exact custom hostname for the root and each reported label, and deletes it when the label disappears. The wildcard CNAME already points at the SaaS zone, so Cloudflare validates each certificate over HTTP without user action.

## Consequences

- A new environment reaches the custom domain only after its certificate is issued. Its Localify-owned URL works immediately because `*.localify.fyi` already covers it.
- Each exact hostname gets its own certificate, so environment and branch names under a custom domain show up in Certificate Transparency logs.
- A label the connector did not report has no certificate and fails at the TLS handshake. It does not fall back to the root the way an unknown label does on a Localify-owned wildcard URL.
- Cloudflare for SaaS includes 100 custom hostnames for the whole account and then charges $0.10 per hostname per month, so custom hostnames need a per-license quota.
- At the reported Enterprise entry price, Enterprise wildcard hostnames only become cheaper at around 50,000 active custom hostnames.
