-- Ledger schema version 1 (implementation plan §5.8, assessment §7). One row
-- per device, overwritten in place: sync status only, never note content.
-- Times are Unix seconds.

-- The account ID is the account's Ed25519 public key, as in the app (ADR-004).
CREATE TABLE accounts (
    account_id BYTEA PRIMARY KEY,
    created_at BIGINT NOT NULL
);

CREATE TABLE devices (
    -- The device's Ed25519 public key, which is also its Iroh key.
    device_id     BYTEA PRIMARY KEY,
    account_id    BYTEA NOT NULL REFERENCES accounts ON DELETE CASCADE,
    -- Readable only by the account's devices. Empty in Phase 0 (ADR-004).
    enc_name      BYTEA,
    platform      TEXT NOT NULL,
    push_token    TEXT,
    notifications BOOLEAN NOT NULL DEFAULT FALSE,
    own_counter   BIGINT NOT NULL DEFAULT 0,
    -- Hex device ID -> highest change held from it, this device's own included.
    seen_counters JSONB NOT NULL DEFAULT '{}',
    -- To the minute.
    last_seen     BIGINT NOT NULL,
    online_until  BIGINT NOT NULL DEFAULT 0,
    last_push_at  BIGINT NOT NULL DEFAULT 0,
    -- A high-priority wake-up waiting for an answer (plan §5.9): the device that
    -- asked for it, and when.
    wake_for      BYTEA,
    wake_at       BIGINT
);

CREATE INDEX devices_by_account ON devices (account_id);
CREATE INDEX devices_by_last_seen ON devices (last_seen);
CREATE INDEX devices_with_wakes ON devices (wake_at) WHERE wake_at IS NOT NULL;
