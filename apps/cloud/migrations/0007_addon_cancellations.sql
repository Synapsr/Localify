-- Add-on grants and the cancellation of the add-ons of a lapsed Pro plan
-- (docs/public-urls.md, "Billing"). Additive only. Times are Unix
-- milliseconds.

-- Each add-on subscription the refresh found, with its own lifecycle, as a
-- JSON array of {id, kind, units, status, cancel_at_period_end,
-- current_period_end, trial_end, ends_at, ended_at, past_due_at}, kind
-- `machine` or `urls`. The entitlement reads each grant through the
-- lifecycle table at read time, so one that ends stops counting on time,
-- with no refresh. `addon_machines` and `addon_url_packs` are superseded by
-- it: no longer read or written.
ALTER TABLE billing ADD COLUMN addon_grants TEXT NOT NULL DEFAULT '[]';

-- An add-on subscription to cancel at period end because its Pro plan
-- lapsed. Recorded by the refresh that finds it, in the commit of its
-- snapshot, at that snapshot's `revision`; the 5-minute job cancels it only
-- while `billing` still holds that revision and the plan is still lapsed.
-- `state`: `pending`, to cancel; `canceled`, done; `restoring`, canceled
-- while the plan came back, so the job sets it to renew again. Rows
-- canceled more than 30 days ago are purged by the daily job.
CREATE TABLE addon_cancellations (
  subscription_id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id),
  revision INTEGER NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'canceled', 'restoring')),
  canceled_at INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE INDEX pending_addon_cancellations ON addon_cancellations (next_attempt_at)
  WHERE state != 'canceled';
