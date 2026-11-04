//! Postgres storage (plan §5.8). Queries are checked at run time rather than
//! compile time, so building the ledger needs no database.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use mirrorleaf_proto::ledger::Platform;
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
const MIGRATIONS: &[(i32, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

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

/// Adds a device to an account, or refreshes it if it is already there.
/// Returns `false` if the device belongs to another account.
pub async fn upsert_device(
    tx: &mut Transaction<'_, Postgres>,
    account: &AccountId,
    device: &DeviceId,
    platform: Platform,
    now: i64,
) -> Result<bool> {
    let existing: Option<Vec<u8>> =
        query_scalar("SELECT account_id FROM devices WHERE device_id = $1 FOR UPDATE")
            .bind(&device.0[..])
            .fetch_optional(&mut **tx)
            .await?;
    match existing {
        Some(other) if other != account.0 => return Ok(false),
        Some(_) => {
            query("UPDATE devices SET platform = $2, last_seen = $3 WHERE device_id = $1")
                .bind(&device.0[..])
                .bind(platform.to_string())
                .bind(to_minute(now))
                .execute(&mut **tx)
                .await?;
        }
        None => {
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
        }
    }
    Ok(true)
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

/// Removes devices not seen for 90 days, and accounts left with none. Returns
/// how many devices went.
pub async fn prune(pool: &PgPool, now: i64) -> Result<u64> {
    let mut tx = pool.begin().await?;
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
