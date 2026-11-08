-- What a billing refresh leaves to do (docs/public-urls.md, "Billing").
-- Additive only. Times are Unix milliseconds.

-- When a refresh changed the user's snapshot, so the serving reconcile of
-- the user is owed. Set in the batch that writes the snapshot, cleared only
-- by a reconcile that commits; the 5-minute job retries it. NULL: nothing
-- owed.
ALTER TABLE billing_revisions ADD COLUMN serving_due_at INTEGER;

-- One refund per order of a duplicate Pro subscription. `creating` is
-- written before the call to Polar, so a crash after it is found by looking
-- the order's refunds up, never by refunding again. The obligation stays
-- open until Polar reports `succeeded`; `failed` and `canceled` are
-- retried with backoff. `error` is the last error of a call on this order.
CREATE TABLE duplicate_refunds (
  order_id TEXT PRIMARY KEY,
  subscription_id TEXT NOT NULL REFERENCES duplicate_subscriptions (subscription_id),
  refund_id TEXT,
  amount INTEGER NOT NULL,
  status TEXT NOT NULL
    CHECK (status IN ('creating', 'pending', 'succeeded', 'failed', 'canceled')),
  error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX refunds_by_subscription ON duplicate_refunds (subscription_id);
