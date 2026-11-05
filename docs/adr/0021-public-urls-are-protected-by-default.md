# Public URLs Are Protected by Default

A new public URL asks for a password unless its creator opens it explicitly. This amends ADR 0004, which made protection optional. A dev server exposes unfinished code, debug pages and sometimes real data, and agents can create public URLs, so the safe state has to be the default. When no password is given, Localify generates four words joined by hyphens and shows them once; they are stored as a salted PBKDF2 hash and cannot be shown again, only replaced.

The Worker checks access before anything reaches the machine. A correct password sets a signed, host-only `__Host-` cookie for seven days; changing or removing the password signs every browser out. Each label of a wildcard URL signs in separately, because the cookie belongs to one host. WebSocket handshakes need the cookie too, and a same-host `Origin`. Path exceptions keep webhooks and OAuth callbacks reachable; the application behind them still has to check those requests itself.

Opening a URL to everyone is for a person: the MCP server has no tool for it, and `public_url_create` cannot make an open URL.
