-- Ledger schema version 2 (Phase 1, M9): pairing numbers (E1). A device showing
-- a word code ("7-acoustic-planet-river") gets a short number that points to
-- it for 10 minutes, so a new device can find it. The words never reach the
-- ledger. No account is stored: the device implies it.
CREATE TABLE pairing (
    nameplate  INTEGER PRIMARY KEY,
    device_id  BYTEA NOT NULL REFERENCES devices (device_id) ON DELETE CASCADE,
    expires_at BIGINT NOT NULL
);

CREATE INDEX pairing_by_device ON pairing (device_id);
