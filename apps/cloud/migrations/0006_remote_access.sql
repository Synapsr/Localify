-- Remote machines (docs/remote-machines.md, "Trust" and "Entitlement").
-- Additive only. Times are Unix milliseconds.

-- A machine's remote key is set once and never changes: a machine that lost
-- its key signs in again and gets a new id. A revoked machine keeps its key,
-- which only the unique index still reads.
ALTER TABLE machines ADD COLUMN remote_key TEXT;
ALTER TABLE machines ADD COLUMN remote_key_at INTEGER;
CREATE UNIQUE INDEX one_machine_per_remote_key ON machines (remote_key) WHERE remote_key IS NOT NULL;

-- An override replaces the plan entirely, remote access included. Every
-- override that exists was granted by hand and keeps remote access.
ALTER TABLE plan_overrides ADD COLUMN remote_access INTEGER NOT NULL DEFAULT 0
  CHECK (remote_access IN (0, 1));
UPDATE plan_overrides SET remote_access = 1;
