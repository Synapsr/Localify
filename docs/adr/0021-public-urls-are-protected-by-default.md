> **Amended.** [ADR 0026](0026-abuse-controls-and-shared-cookies-before-the-public-suffix-list.md) puts a warning page before the password, so a link that carries the password waits for one click.

# Public URLs Are Protected by Default

A new public URL asks for a password unless its creator opens it explicitly. This amends ADR 0004, which made protection optional. A dev server exposes unfinished code, debug pages and sometimes real data, and agents can create public URLs, so the safe state has to be the default. When no password is given, Localify generates four words joined by hyphens and shows them once; Localify Cloud stores them as a salted PBKDF2 hash, so it cannot show them again, only replace them. The machine that sets a password keeps a copy in its data directory, next to the token that could replace it anyway, so it can offer a link that carries the password after `#` (`docs/public-urls.md`, "Saved passwords"); that link signs a browser in without the password crossing any server in the URL.

The Worker checks access before anything reaches the machine. A correct password sets a signed, host-only `__Host-` cookie for seven days; changing or removing the password signs every browser out. Each label of a wildcard URL signs in separately, because the cookie belongs to one host. WebSocket handshakes need the cookie too. Every URL under `localify.fyi` is the same site for a browser, so `SameSite` cannot stop another user's page from posting to this one with the visitor's cookie: requests that can change state, and WebSocket handshakes, are refused when their `Origin` belongs to another public URL.

The path exceptions ADR 0004 planned for webhooks are not in v1. A review showed that matching an exception against a path the upstream app decodes differently can let a request past the password, and the first use case, opening a dev server from another machine, does not need them.

Opening a URL to everyone is for a person: the MCP server has no tool for it, and `public_url_create` cannot make an open URL.
