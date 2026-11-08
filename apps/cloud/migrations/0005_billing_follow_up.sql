-- What a billing refresh leaves to do (docs/public-urls.md, "Billing").
-- Additive only. Times are Unix milliseconds.

-- When a refresh changed the user's snapshot, so the serving reconcile of
-- the user is owed. Set in the batch that writes the snapshot, cleared only
-- by a reconcile that commits; the 5-minute job retries it. NULL: nothing
-- owed.
ALTER TABLE billing_revisions ADD COLUMN serving_due_at INTEGER;
