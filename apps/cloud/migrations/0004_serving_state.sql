-- The serving state of public URLs (docs/public-urls.md, "Downgrades").

-- When a public URL last stopped serving. NULL means it served at the last
-- reconcile. A URL that stops serving gets the time, and its owner's
-- machines a rotation, so its open streams close (ADR 0027); one that
-- serves again gets NULL back. Existing rows start NULL: the first sweep
-- rotates whatever the current entitlements already suspend.
ALTER TABLE public_urls ADD COLUMN suspended_at INTEGER;

-- When one of the user's URLs last stopped serving, or the user was banned.
-- No new tunnel generation of theirs is provisioned for a quiet period
-- after it, longer than the public path's cache, so no request that cached
-- the URL as serving can reach a replacement tunnel.
ALTER TABLE users ADD COLUMN serving_stopped_at INTEGER;
