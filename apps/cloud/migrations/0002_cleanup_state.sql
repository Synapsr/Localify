-- Cleanup bookkeeping for revoked machines.
-- `reconcile`: a sign-in that failed or got stuck may have created a tunnel or
-- a DNS record without recording its id; the cleanup looks them up by name.
-- `cleanup_attempts` and `next_cleanup_at`: a machine whose cleanup keeps
-- failing waits longer each time, so it cannot hold back the others.
ALTER TABLE machines ADD COLUMN reconcile INTEGER NOT NULL DEFAULT 0;
ALTER TABLE machines ADD COLUMN cleanup_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE machines ADD COLUMN next_cleanup_at INTEGER;
