-- Add-on subscriptions to cancel at the end of their period because their
-- Pro plan lapsed (docs/public-urls.md, "Billing"). Additive only. Times
-- are Unix milliseconds.

-- Recorded by the refresh that finds one, in the commit of its snapshot,
-- before any call to Polar; the 5-minute job cancels it and sets
-- `canceled_at`, retrying a failure after `next_attempt_at`. Rows canceled
-- more than 30 days ago are purged by the daily job.
CREATE TABLE addon_cancellations (
  subscription_id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id),
  canceled_at INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE INDEX pending_addon_cancellations ON addon_cancellations (next_attempt_at)
  WHERE canceled_at IS NULL;
