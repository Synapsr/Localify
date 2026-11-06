-- Localify Cloud v1 (docs/public-urls.md, section "Data"). Times are Unix
-- milliseconds.

CREATE TABLE users (
  id TEXT PRIMARY KEY,
  github_id INTEGER UNIQUE NOT NULL,
  login TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

-- A machine is `provisioning` while sign-in creates its tunnel, `active` once
-- it has a token, and `revoked` after sign-out, replacement or a failed
-- sign-in. A revoked machine keeps `tunnel_id` and `dns_record_id` until
-- Cloudflare confirms they are gone; the hourly cleanup retries them.
CREATE TABLE machines (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users (id),
  name TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('provisioning', 'active', 'revoked')),
  tunnel_id TEXT,
  dns_record_id TEXT,
  ingress_port INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  revoked_at INTEGER
);

-- `hash` is the hex SHA-256 of the token.
CREATE TABLE tokens (
  hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users (id),
  machine_id TEXT NOT NULL REFERENCES machines (id),
  created_at INTEGER NOT NULL
);

-- A deleted URL keeps its row, and so its slug, for 30 days.
CREATE TABLE public_urls (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users (id),
  slug TEXT UNIQUE NOT NULL,
  local_domain TEXT NOT NULL,
  wildcard INTEGER NOT NULL,
  machine_id TEXT REFERENCES machines (id),
  protected INTEGER NOT NULL,
  password_hash TEXT,
  password_salt TEXT,
  password_version INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  deleted_at INTEGER,
  CHECK (protected = 0 OR (password_hash IS NOT NULL AND password_salt IS NOT NULL))
);

CREATE UNIQUE INDEX one_url_per_local_domain ON public_urls (user_id, local_domain) WHERE deleted_at IS NULL;
