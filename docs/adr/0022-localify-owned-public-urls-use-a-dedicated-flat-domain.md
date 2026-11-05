# Localify-Owned Public URLs Use a Dedicated Flat Domain

Public URLs live under `localify.fyi`, not `localify.dev`, which serves the website, the API and the downloads. If they shared the product domain, one phishing report or Safe Browsing flag against a user's URL could block the site, and any user app could set cookies the website would receive. Until `localify.fyi` is on the Public Suffix List, sibling URLs still share a registrable domain, so Localify only sets host-only `__Host-` cookies there and the domain serves nothing but public URLs.

Every URL is one DNS label under the domain, so Cloudflare's free Universal SSL certificate covers them all and environment names stay out of Certificate Transparency logs: `<slug>.localify.fyi` for a URL and `<slug>--<label>.localify.fyi` for a label under a wildcard URL. The slug comes first and never contains `--`, so the first `--` is the separator, labels may contain `--` like multi-service environment names do, and `--` never lands at positions 3 and 4 of the label, which IDNA reserves. The spike used `<label>--<slug>`, which turned a two-letter label into a reserved form.

The domain is configuration in the Worker, not a literal in the clients, because it may change once public URLs have real users, and a move has to serve both domains for a while.
