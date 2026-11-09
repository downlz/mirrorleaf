//! Postgres storage (plan §5.8). Queries are checked at run time rather than
//! compile time, so building the ledger needs no database.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use mirrorleaf_proto::ledger::{MAX_DEVICES_PER_ACCOUNT, Platform};
use mirrorleaf_proto::{AccountId, DeviceId, VersionVector};
use sqlx_core::query::query;
use sqlx_core::query_scalar::query_scalar;
use sqlx_core::raw_sql::raw_sql;
use sqlx_core::row::Row;
use sqlx_core::transaction::Transaction;
use sqlx_core::types::Json;
use sqlx_postgres::{PgPool, PgRow, Postgres};

use crate::rules::{Device, Wake};

/// The migrations, in order. Each runs once, in a transaction.
const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_pairing.sql")),
    (3, include_str!("../migrations/0003_account_moves.sql")),
];

/// The highest pairing number handed out. Numbers are reused once free, so
/// they stay short; this many devices pairing at once fills them.
const MAX_NAMEPLATE: i32 = 9999;

/// An arbitrary constant for Postgres' advisory lock, so two ledgers starting
/// at once don't both migrate.
const MIGRATION_LOCK: i64 = 0x6d6c_6c65_6467;

/// Days after which a device that hasn't been seen is pruned (assessment §7).
pub const PRUNE_AFTER_SECS: i64 = 90 * 24 * 3600;

pub async fn migrate(pool: &PgPool) -> Result<()> {
    let mut tx = pool.begin().await?;
    query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK)
        .execute(&mut *tx)
        .await?;
    query("CREATE TABLE IF NOT EXISTS ledger_migrations (version INTEGER PRIMARY KEY)")
        .execute(&mut *tx)
        .await?;
    let applied: Option<i32> = query_scalar("SELECT max(version) FROM ledger_migrations")
        .fetch_one(&mut *tx)
        .await?;
    for (version, sql) in MIGRATIONS {
        if applied.is_some_and(|v| v >= *version) {
            continue;
        }
        raw_sql(*sql)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("migration {version} failed"))?;
        query("INSERT INTO ledger_migrations (version) VALUES ($1)")
            .bind(version)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Checks the database answers (`/health`).
pub async fn ping(pool: &PgPool) -> Result<()> {
    query("SELECT 1").execute(pool).await?;
    Ok(())
}

/// The account a device belongs to, if it is registered.
pub async fn account_of(pool: &PgPool, device: &DeviceId) -> Result<Option<AccountId>> {
    let row: Option<Vec<u8>> = query_scalar("SELECT account_id FROM devices WHERE device_id = $1")
        .bind(&device.0[..])
        .fetch_optional(pool)
        .await?;
    row.map(|bytes| key(bytes).map(AccountId)).transpose()
}

pub async fn account_exists(
    tx: &mut Transaction<'_, Postgres>,
    account: &AccountId,
) -> Result<bool> {
    Ok(
        query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE account_id = $1)")
            .bind(&account.0[..])
            .fetch_one(&mut **tx)
            .await?,
    )
}

pub async fn create_account(
    tx: &mut Transaction<'_, Postgres>,
    account: &AccountId,
    now: i64,
) -> Result<()> {
    query(
        "INSERT INTO accounts (account_id, created_at) VALUES ($1, $2)
         ON CONFLICT (account_id) DO NOTHING",
    )
    .bind(&account.0[..])
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// What [`upsert_device`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upsert {
    Added,
    Refreshed,
    /// The device is registered to another account; nothing changed.
    OtherAccount,
    /// The account already has [`MAX_DEVICES_PER_ACCOUNT`] devices; nothing changed.
    AccountFull,
}

/// Adds a device to an account, or refreshes it if it is already there. The
/// account must exist.
pub async fn upsert_device(
    tx: &mut Transaction<'_, Postgres>,
    account: &AccountId,
    device: &DeviceId,
    platform: Platform,
    now: i64,
) -> Result<Upsert> {
    let existing: Option<Vec<u8>> =
        query_scalar("SELECT account_id FROM devices WHERE device_id = $1 FOR UPDATE")
            .bind(&device.0[..])
            .fetch_optional(&mut **tx)
            .await?;
    match existing {
        Some(other) if other != account.0 => Ok(Upsert::OtherAccount),
        Some(_) => {
            query("UPDATE devices SET platform = $2, last_seen = $3 WHERE device_id = $1")
                .bind(&device.0[..])
                .bind(platform.to_string())
                .bind(to_minute(now))
                .execute(&mut **tx)
                .await?;
            Ok(Upsert::Refreshed)
        }
        None => {
            // Locking the account's row makes two devices joining at once
            // count one after the other, so neither can pass the cap.
            query("SELECT 1 FROM accounts WHERE account_id = $1 FOR UPDATE")
                .bind(&account.0[..])
                .execute(&mut **tx)
                .await?;
            let count: i64 = query_scalar("SELECT count(*) FROM devices WHERE account_id = $1")
                .bind(&account.0[..])
                .fetch_one(&mut **tx)
                .await?;
            if count >= MAX_DEVICES_PER_ACCOUNT {
                return Ok(Upsert::AccountFull);
            }
            query(
                "INSERT INTO devices (device_id, account_id, platform, last_seen)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&device.0[..])
            .bind(&account.0[..])
            .bind(platform.to_string())
            .bind(to_minute(now))
            .execute(&mut **tx)
            .await?;
            Ok(Upsert::Added)
        }
    }
}

/// A device row with what the rules don't need.
pub struct StoredDevice {
    pub device: Device,
    pub last_seen: i64,
}

/// Every device of the account, locked until the transaction ends, so that
/// concurrent reports from one account are evaluated one at a time. Rows are
/// locked in key order to avoid deadlocks.
pub async fn lock_account(
    tx: &mut Transaction<'_, Postgres>,
    account: &AccountId,
) -> Result<Vec<StoredDevice>> {
    let rows = query(
        "SELECT device_id, platform, push_token, notifications, seen_counters,
                online_until, last_push_at, wake_for, wake_at, last_seen
         FROM devices WHERE account_id = $1 ORDER BY device_id FOR UPDATE",
    )
    .bind(&account.0[..])
    .fetch_all(&mut **tx)
    .await?;
    rows.iter().map(device_row).collect()
}

fn device_row(row: &PgRow) -> Result<StoredDevice> {
    let Json(seen): Json<BTreeMap<String, u64>> = row.try_get("seen_counters")?;
    let vv = seen
        .into_iter()
        .map(|(k, v)| Ok((k.parse()?, v)))
        .collect::<Result<_>>()
        .map(VersionVector)?;
    let wake = match (
        row.try_get::<Option<Vec<u8>>, _>("wake_for")?,
        row.try_get::<Option<i64>, _>("wake_at")?,
    ) {
        (Some(for_device), Some(at)) => Some(Wake {
            for_device: DeviceId(key(for_device)?),
            at,
        }),
        _ => None,
    };
    Ok(StoredDevice {
        device: Device {
            id: DeviceId(key(row.try_get("device_id")?)?),
            platform: row.try_get::<String, _>("platform")?.parse()?,
            push_token: row.try_get("push_token")?,
            notifications: row.try_get("notifications")?,
            vv,
            online_until: row.try_get("online_until")?,
            last_push_at: row.try_get("last_push_at")?,
            wake,
        },
        last_seen: row.try_get("last_seen")?,
    })
}

/// Writes a status report over the device's row.
pub async fn write_status(
    tx: &mut Transaction<'_, Postgres>,
    device: &Device,
    now: i64,
) -> Result<()> {
    let seen: BTreeMap<String, u64> = device
        .vv
        .0
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect();
    query(
        "UPDATE devices SET platform = $2, push_token = $3, notifications = $4,
                own_counter = $5, seen_counters = $6, online_until = $7, last_seen = $8
         WHERE device_id = $1",
    )
    .bind(&device.id.0[..])
    .bind(device.platform.to_string())
    .bind(&device.push_token)
    .bind(device.notifications)
    .bind(device.vv.get(&device.id) as i64)
    .bind(Json(seen))
    .bind(device.online_until)
    .bind(to_minute(now))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn mark_pushed(
    tx: &mut Transaction<'_, Postgres>,
    device: &DeviceId,
    now: i64,
) -> Result<()> {
    query("UPDATE devices SET last_push_at = $2 WHERE device_id = $1")
        .bind(&device.0[..])
        .bind(now)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Records a high-priority wake-up of `holder`, asked for by `asker`.
pub async fn set_wake(
    tx: &mut Transaction<'_, Postgres>,
    holder: &DeviceId,
    asker: &DeviceId,
    now: i64,
) -> Result<()> {
    query("UPDATE devices SET wake_for = $2, wake_at = $3, last_push_at = $3 WHERE device_id = $1")
        .bind(&holder.0[..])
        .bind(&asker.0[..])
        .bind(now)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn clear_wake(tx: &mut Transaction<'_, Postgres>, holder: &DeviceId) -> Result<()> {
    query("UPDATE devices SET wake_for = NULL, wake_at = NULL WHERE device_id = $1")
        .bind(&holder.0[..])
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Accounts with a wake-up waiting for an answer.
pub async fn accounts_with_wakes(pool: &PgPool) -> Result<Vec<AccountId>> {
    let rows: Vec<Vec<u8>> =
        query_scalar("SELECT DISTINCT account_id FROM devices WHERE wake_at IS NOT NULL")
            .fetch_all(pool)
            .await?;
    rows.into_iter().map(|b| key(b).map(AccountId)).collect()
}

/// The devices whose ID starts with `prefix` (lower-case hex), with their push
/// tokens. For the operator's `test-push` command (plan §7.2).
pub async fn find_devices(pool: &PgPool, prefix: &str) -> Result<Vec<(DeviceId, Option<String>)>> {
    anyhow::ensure!(
        prefix.chars().all(|c| c.is_ascii_hexdigit()),
        "a device ID prefix is hex digits only"
    );
    let rows = query(
        "SELECT device_id, push_token FROM devices WHERE encode(device_id, 'hex') LIKE $1 || '%'",
    )
    .bind(prefix.to_ascii_lowercase())
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                DeviceId(key(row.try_get("device_id")?)?),
                row.try_get("push_token")?,
            ))
        })
        .collect()
}

/// Forgets a push token FCM no longer accepts.
pub async fn forget_token(pool: &PgPool, device: &DeviceId, token: &str) -> Result<()> {
    query("UPDATE devices SET push_token = NULL WHERE device_id = $1 AND push_token = $2")
        .bind(&device.0[..])
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

/// Removes a device, and its account once it has no devices left.
pub async fn delete_device(pool: &PgPool, account: &AccountId, device: &DeviceId) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let removed = query("DELETE FROM devices WHERE device_id = $1 AND account_id = $2")
        .bind(&device.0[..])
        .bind(&account.0[..])
        .execute(&mut *tx)
        .await?
        .rows_affected();
    query(
        "DELETE FROM accounts WHERE account_id = $1
         AND NOT EXISTS (SELECT 1 FROM devices WHERE account_id = $1)",
    )
    .bind(&account.0[..])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(removed > 0)
}

pub async fn delete_account(pool: &PgPool, account: &AccountId) -> Result<()> {
    query("DELETE FROM accounts WHERE account_id = $1")
        .bind(&account.0[..])
        .execute(pool)
        .await?;
    Ok(())
}

/// Where an account moved when its key was replaced, if it did (E3).
pub async fn moved_to(pool: &PgPool, account: &AccountId) -> Result<Option<AccountId>> {
    let row: Option<Vec<u8>> =
        query_scalar("SELECT new_account FROM account_moves WHERE old_account = $1")
            .bind(&account.0[..])
            .fetch_optional(pool)
            .await?;
    row.map(|bytes| key(bytes).map(AccountId)).transpose()
}

/// The account that moved to `account`, if one did (E3).
pub async fn moved_from(pool: &PgPool, account: &AccountId) -> Result<Option<AccountId>> {
    let row: Option<Vec<u8>> = query_scalar(
        "SELECT old_account FROM account_moves WHERE new_account = $1 ORDER BY moved_at DESC LIMIT 1",
    )
    .bind(&account.0[..])
    .fetch_optional(pool)
    .await?;
    row.map(|bytes| key(bytes).map(AccountId)).transpose()
}

/// What [`rekey`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rekeyed {
    /// The account moved to the new key.
    Moved,
    /// The signer is already on the new account: a repeat, nothing to do.
    AlreadyDone,
    /// Another device moved the account to a different key first.
    MovedElsewhere(AccountId),
    /// The signer isn't registered (any more).
    UnknownDevice,
    /// The signer's account isn't the one the link comes from.
    WrongAccount,
}

/// Moves the signer's account, `old`, to `new` (E3): the rows of `keep` move
/// with it, the account's other devices go, and the move is remembered. The
/// account's row is locked first, so of two devices replacing the key at
/// once, the first wins and the second hears where the account went.
pub async fn rekey(
    pool: &PgPool,
    signer: &DeviceId,
    old: &AccountId,
    new: &AccountId,
    keep: &[DeviceId],
    now: i64,
) -> Result<Rekeyed> {
    let mut tx = pool.begin().await?;
    query("SELECT 1 FROM accounts WHERE account_id = $1 FOR UPDATE")
        .bind(&old.0[..])
        .execute(&mut *tx)
        .await?;
    let current: Option<Vec<u8>> =
        query_scalar("SELECT account_id FROM devices WHERE device_id = $1")
            .bind(&signer.0[..])
            .fetch_optional(&mut *tx)
            .await?;
    let Some(current) = current else {
        return Ok(Rekeyed::UnknownDevice);
    };
    let current = AccountId(key(current)?);
    if current == *new {
        return Ok(Rekeyed::AlreadyDone);
    }
    if current != *old {
        // The account moved while this request waited for the lock.
        let moved: Option<Vec<u8>> =
            query_scalar("SELECT new_account FROM account_moves WHERE old_account = $1")
                .bind(&old.0[..])
                .fetch_optional(&mut *tx)
                .await?;
        return Ok(match moved {
            Some(to) => Rekeyed::MovedElsewhere(AccountId(key(to)?)),
            None => Rekeyed::WrongAccount,
        });
    }
    query(
        "INSERT INTO accounts (account_id, created_at) VALUES ($1, $2)
         ON CONFLICT (account_id) DO NOTHING",
    )
    .bind(&new.0[..])
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let keep: Vec<Vec<u8>> = keep.iter().map(|d| d.0.to_vec()).collect();
    query("UPDATE devices SET account_id = $2 WHERE account_id = $1 AND device_id = ANY($3)")
        .bind(&old.0[..])
        .bind(&new.0[..])
        .bind(&keep)
        .execute(&mut *tx)
        .await?;
    // Whatever is left on the old account goes with it (the cascade).
    query("DELETE FROM accounts WHERE account_id = $1")
        .bind(&old.0[..])
        .execute(&mut *tx)
        .await?;
    query(
        "INSERT INTO account_moves (old_account, new_account, moved_at) VALUES ($1, $2, $3)
         ON CONFLICT (old_account) DO UPDATE SET new_account = excluded.new_account,
                                                 moved_at = excluded.moved_at",
    )
    .bind(&old.0[..])
    .bind(&new.0[..])
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Rekeyed::Moved)
}

/// Gives `device` a pairing number valid until `expires_at` (E1): the lowest
/// one free, so word codes stay short. Replaces the device's earlier number.
/// `None` if every number is taken.
pub async fn offer_pairing(
    pool: &PgPool,
    device: &DeviceId,
    now: i64,
    expires_at: i64,
) -> Result<Option<u32>> {
    // Two devices asking at once may pick the same number; the loser tries again.
    for _ in 0..5 {
        let mut tx = pool.begin().await?;
        query("DELETE FROM pairing WHERE device_id = $1 OR expires_at <= $2")
            .bind(&device.0[..])
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let inserted: Result<Option<i32>, sqlx_core::Error> = query_scalar(
            "INSERT INTO pairing (nameplate, device_id, expires_at)
             SELECT n, $1, $2 FROM generate_series(1, $3) AS n
             WHERE NOT EXISTS (SELECT 1 FROM pairing p WHERE p.nameplate = n)
             ORDER BY n LIMIT 1
             RETURNING nameplate",
        )
        .bind(&device.0[..])
        .bind(expires_at)
        .bind(MAX_NAMEPLATE)
        .fetch_optional(&mut *tx)
        .await;
        match inserted {
            Ok(nameplate) => {
                tx.commit().await?;
                return Ok(nameplate.map(|n| n as u32));
            }
            Err(e) if is_unique_violation(&e) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("couldn't find a free pairing number")
}

/// The device a pairing number points to, unless it ran out.
pub async fn lookup_pairing(pool: &PgPool, nameplate: u32, now: i64) -> Result<Option<DeviceId>> {
    let row: Option<Vec<u8>> =
        query_scalar("SELECT device_id FROM pairing WHERE nameplate = $1 AND expires_at > $2")
            .bind(nameplate as i32)
            .bind(now)
            .fetch_optional(pool)
            .await?;
    row.map(|bytes| key(bytes).map(DeviceId)).transpose()
}

/// Withdraws `device`'s pairing number. Harmless if it is already gone.
pub async fn cancel_pairing(pool: &PgPool, device: &DeviceId, nameplate: u32) -> Result<()> {
    query("DELETE FROM pairing WHERE nameplate = $1 AND device_id = $2")
        .bind(nameplate as i32)
        .bind(&device.0[..])
        .execute(pool)
        .await?;
    Ok(())
}

fn is_unique_violation(e: &sqlx_core::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|code| code == "23505")
}

/// Removes devices not seen for 90 days, accounts left with none, and pairing
/// numbers that ran out. Returns how many devices went.
pub async fn prune(pool: &PgPool, now: i64) -> Result<u64> {
    let mut tx = pool.begin().await?;
    query("DELETE FROM pairing WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let pruned = query("DELETE FROM devices WHERE last_seen < $1")
        .bind(now - PRUNE_AFTER_SECS)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    query(
        "DELETE FROM accounts a WHERE NOT EXISTS (SELECT 1 FROM devices d WHERE d.account_id = a.account_id)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(pruned)
}

/// Times the ledger keeps about presence are kept to the minute (assessment §7).
fn to_minute(t: i64) -> i64 {
    t - t.rem_euclid(60)
}

fn key(bytes: Vec<u8>) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("stored key isn't 32 bytes"))
}
