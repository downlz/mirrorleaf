-- Ledger schema version 3 (Phase 1, M10): accounts that moved to a new key
-- (E3). When a device replaces the account key, its rows move to the new
-- account and the old account ID is remembered here, so the old key, and the
-- recovery kit it came from, can't register again, and a second replacement
-- of the same account is refused. Kept for good: a row is two keys and a time.
CREATE TABLE account_moves (
    old_account BYTEA PRIMARY KEY,
    new_account BYTEA NOT NULL,
    moved_at    BIGINT NOT NULL
);
