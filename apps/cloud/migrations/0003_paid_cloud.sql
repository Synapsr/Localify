-- The paid Localify Cloud (docs/public-urls.md, sections "Data" and
-- "Tunnels"). Times are Unix milliseconds.

ALTER TABLE users ADD COLUMN trial_used_at INTEGER;
ALTER TABLE users ADD COLUMN banned_at INTEGER;
ALTER TABLE users ADD COLUMN banned_reason TEXT;

-- A machine holds a tunnel only while it serves. `tunnel_state` is anything
-- but `none` while a tunnel may exist at Cloudflare: that is a serving slot.
-- Each provisioning bumps `tunnel_generation` and names its tunnel
-- `localify-<machine-id>-<generation>`; 0 means "no tunnel, no token".
-- `tunnel_op` is the operation that holds the row: every write of a
-- provisioning or a release is conditioned on it. `dns_record_id` outlives
-- the tunnels: a new tunnel only moves the CNAME.
ALTER TABLE machines ADD COLUMN tunnel_state TEXT NOT NULL DEFAULT 'none'
  CHECK (tunnel_state IN ('none', 'provisioning', 'ready', 'releasing'));
ALTER TABLE machines ADD COLUMN tunnel_generation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE machines ADD COLUMN tunnel_op TEXT;
ALTER TABLE machines ADD COLUMN tunnel_op_at INTEGER;
ALTER TABLE machines ADD COLUMN tunnel_ready_at INTEGER;
ALTER TABLE machines ADD COLUMN down_seen_at INTEGER;
ALTER TABLE machines ADD COLUMN rotate_requested_at INTEGER;

ALTER TABLE public_urls ADD COLUMN taken_down_at INTEGER;
ALTER TABLE public_urls ADD COLUMN taken_down_reason TEXT;

CREATE TABLE plan_overrides (
  user_id TEXT PRIMARY KEY REFERENCES users (id),
  serving_machines INTEGER NOT NULL,
  public_urls INTEGER NOT NULL,
  reason TEXT NOT NULL,
  expires_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE TABLE billing (
  user_id TEXT PRIMARY KEY REFERENCES users (id),
  polar_customer_id TEXT,
  pro_subscription_id TEXT,
  pro_status TEXT,
  pro_interval TEXT CHECK (pro_interval IN ('month', 'year')),
  cancel_at_period_end INTEGER NOT NULL DEFAULT 0,
  current_period_end INTEGER,
  trial_end INTEGER,
  ends_at INTEGER,
  ended_at INTEGER,
  past_due_at INTEGER,
  addon_machines INTEGER NOT NULL DEFAULT 0,
  addon_url_packs INTEGER NOT NULL DEFAULT 0,
  revision INTEGER NOT NULL,
  refreshed_at INTEGER NOT NULL
);

CREATE TABLE billing_revisions (
  user_id TEXT PRIMARY KEY REFERENCES users (id),
  last INTEGER NOT NULL,
  check_due_at INTEGER
);

CREATE TABLE checkouts (
  user_id TEXT NOT NULL REFERENCES users (id),
  kind TEXT NOT NULL CHECK (kind IN ('pro', 'addon_machine', 'addon_urls')),
  state TEXT NOT NULL CHECK (state IN ('creating', 'open')),
  op TEXT,
  checkout_id TEXT,
  url TEXT,
  units INTEGER NOT NULL DEFAULT 1,
  expires_at INTEGER,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, kind)
);

CREATE TABLE webhook_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT UNIQUE NOT NULL,
  type TEXT NOT NULL,
  user_id TEXT,
  received_at INTEGER NOT NULL,
  processed_at INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER
);

CREATE TABLE duplicate_subscriptions (
  subscription_id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users (id),
  revoked_at INTEGER,
  refunded_at INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE INDEX live_urls_by_owner ON public_urls (user_id, created_at, id) WHERE deleted_at IS NULL;
CREATE INDEX slots_by_owner ON machines (user_id, tunnel_state) WHERE tunnel_state != 'none';
CREATE INDEX pending_webhooks ON webhook_events (next_attempt_at) WHERE processed_at IS NULL;

-- Tunnels made at sign-in before this migration become generation 1. They
-- keep their old name, `localify-<machine-id>`, and are released by id.
UPDATE machines SET tunnel_state = 'ready', tunnel_generation = 1, tunnel_ready_at = created_at
WHERE status = 'active' AND tunnel_id IS NOT NULL;

-- Everyone on the allowlist (ALLOWED_GITHUB_IDS in wrangler.jsonc) keeps
-- serving through an override. Nothing re-creates it: deleting the row ends
-- that access.
INSERT INTO plan_overrides (user_id, serving_machines, public_urls, reason, expires_at, created_at)
SELECT id, 5, 10, 'team', NULL, CAST(strftime('%s', 'now') AS INTEGER) * 1000
FROM users WHERE github_id IN (77238381, 61839831, 23056644, 88707023, 261605370);
