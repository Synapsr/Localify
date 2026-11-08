-- When a public URL last stopped serving (docs/public-urls.md, "Downgrades").
-- NULL means it served at the last reconcile. A URL that stops serving gets
-- the time, and its machine a rotation, so its open streams close (ADR
-- 0027); one that serves again gets NULL back. Existing rows start NULL: the
-- first sweep rotates whatever the current entitlements already suspend.

ALTER TABLE public_urls ADD COLUMN suspended_at INTEGER;
