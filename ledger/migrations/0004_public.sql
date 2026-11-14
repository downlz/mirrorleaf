-- M13: the ledger on the public internet (E10) and the beta's daily totals (E12).

-- Accounts that never come back after being created are deleted after 7 days
-- (owner, 12 Nov 2026). Accounts from before this migration count as used.
ALTER TABLE accounts ADD COLUMN used BOOLEAN NOT NULL DEFAULT TRUE;

-- Since when a device has been behind another of its account, to count how
-- long edits take to reach every device (E12).
ALTER TABLE devices ADD COLUMN behind_since BIGINT;

-- The app version each device last reported, for the dashboard's count of
-- devices by version (E12).
ALTER TABLE devices ADD COLUMN app_version TEXT;

-- Short passes to the relay for devices joining an account, before they are
-- registered (M13).
CREATE TABLE relay_passes (
    device_id  BYTEA PRIMARY KEY,
    expires_at BIGINT NOT NULL
);

-- Accounts the owner blocked: they can't register again or use the relay.
CREATE TABLE blocked (
    account_id BYTEA PRIMARY KEY,
    blocked_at BIGINT NOT NULL
);

-- Daily totals for the dashboard (E12): counts only, never per device or account.
CREATE TABLE daily_counts (
    day   INTEGER NOT NULL,
    name  TEXT NOT NULL,
    value BIGINT NOT NULL,
    PRIMARY KEY (day, name)
);
