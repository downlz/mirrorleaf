//! mirrorleaf-ledger: the sync-status service (axum and Postgres). It holds device
//! lists, change counters and push tokens, never note content (implementation
//! plan §5.8–§5.9).
//!
//! Configuration, from the environment:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `DATABASE_URL` | Postgres connection string (required) |
//! | `LEDGER_LISTEN` | Address to listen on (default `0.0.0.0:8080`) |
//! | `FCM_SERVICE_ACCOUNT` | Path to a Firebase service-account key file. Without one, pushes are only logged. |
//! | `FCM_API` | Replaces FCM's address (for a fake FCM in tests) |
//! | `LEDGER_DEVICE_REQUESTS_PER_MINUTE` | Signed requests allowed per device per minute (default 30; 0 for no limit) |
//! | `LEDGER_NEW_ACCOUNTS_PER_MINUTE` | New accounts allowed per minute across the ledger (default 20; 0 for no limit) |
//! | `RUST_LOG` | Log filter (default `info`) |
//!
//! `mirrorleaf-ledger test-push <device ID prefix> <normal|high>` sends one test
//! push instead of starting the service (see [`test_push`]).
//!
//! It stops cleanly on SIGTERM (what `docker stop` sends) or Ctrl-C: it stops
//! accepting connections and lets requests in progress finish.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use mirrorleaf_ledger::limits::{self, Limits, Rate};
use mirrorleaf_ledger::push::{Fcm, Pusher, test_message};
use mirrorleaf_ledger::rules::{Priority, Push, PushKind};
use mirrorleaf_ledger::{Ledger, db, system_clock};
use sqlx_postgres::PgPoolOptions;

/// How often unanswered wake-ups are checked (rule 4).
const TICK: Duration = Duration::from_secs(15);
const PRUNE_EVERY: Duration = Duration::from_secs(6 * 3600);

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("test-push") {
        return test_push(&args[1..]).await;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    let url = std::env::var("DATABASE_URL").context("set DATABASE_URL")?;
    let listen = std::env::var("LEDGER_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let fcm_key = std::env::var("FCM_SERVICE_ACCOUNT")
        .ok()
        .filter(|p| !p.is_empty());
    let pusher = match fcm_key {
        Some(path) => {
            let key =
                std::fs::read_to_string(&path).with_context(|| format!("can't read {path}"))?;
            let fcm = Fcm::new(&key, std::env::var("FCM_API").ok())?;
            tracing::info!("sending pushes with FCM for project {}", fcm.project());
            Pusher::Fcm(fcm.into())
        }
        None => {
            tracing::warn!("FCM_SERVICE_ACCOUNT isn't set: pushes are only logged");
            Pusher::LogOnly
        }
    };

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .context("can't connect to the database")?;
    db::migrate(&pool).await?;
    let limits = Limits::new(
        rate_from_env(
            "LEDGER_DEVICE_REQUESTS_PER_MINUTE",
            limits::DEFAULT_DEVICE_REQUESTS_PER_MINUTE,
        )?,
        rate_from_env(
            "LEDGER_NEW_ACCOUNTS_PER_MINUTE",
            limits::DEFAULT_NEW_ACCOUNTS_PER_MINUTE,
        )?,
    );
    let ledger = Ledger {
        db: pool,
        pusher,
        clock: system_clock(),
        limits: Arc::new(limits),
    };

    let background = ledger.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        let mut last_prune: Option<tokio::time::Instant> = None;
        loop {
            tick.tick().await;
            if let Err(e) = background.tick().await {
                tracing::warn!("checking wake-ups failed: {e:#}");
            }
            background.limits.forget_idle();
            if last_prune.is_none_or(|t| t.elapsed() >= PRUNE_EVERY) {
                last_prune = Some(tokio::time::Instant::now());
                match background.prune().await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!("pruned {n} devices not seen for 90 days"),
                    Err(e) => tracing::warn!("pruning failed: {e:#}"),
                }
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("can't listen on {listen}"))?;
    tracing::info!(
        "mirrorleaf-ledger {} listening on {listen}",
        env!("CARGO_PKG_VERSION")
    );
    axum::serve(listener, ledger.router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("stopped");
    Ok(())
}

/// Resolves on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(e) => {
                tracing::warn!("can't listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutting down: finishing requests in progress");
}

/// A per-minute rate from the environment: unset or empty gives `default`.
fn rate_from_env(name: &str, default: u32) -> Result<Rate> {
    let per_minute = match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse()
            .with_context(|| format!("{name} must be a whole number, not {value:?}"))?,
        _ => default,
    };
    if per_minute == 0 {
        tracing::warn!("{name} is 0: that limit is off");
    }
    Ok(Rate::per_minute(per_minute))
}

/// `test-push <device ID prefix> <normal|high>`: sends one `pull` push to a
/// registered device, outside the push rules and their limits and without a
/// collapse key, for the
/// push-to-wake matrix (plan §7.2, `tools/push-matrix.py`). It has no HTTP
/// endpoint: only someone with a shell on the ledger's host can run it, with
/// `docker compose exec ledger mirrorleaf-ledger test-push …`.
///
/// Prints `sent <Unix time in ms> <FCM message name>`; the app logs the same
/// message ID when the push arrives.
async fn test_push(args: &[String]) -> Result<()> {
    let [prefix, priority] = args else {
        bail!("usage: mirrorleaf-ledger test-push <device ID prefix> <normal|high>");
    };
    let priority = match priority.as_str() {
        "normal" => Priority::Normal,
        "high" => Priority::High,
        other => bail!("the priority is normal or high, not {other}"),
    };
    ensure!(
        prefix.len() >= 4,
        "give at least 4 hex digits of the device ID"
    );
    let url = std::env::var("DATABASE_URL").context("set DATABASE_URL")?;
    let key_path = std::env::var("FCM_SERVICE_ACCOUNT")
        .ok()
        .filter(|p| !p.is_empty())
        .context("FCM_SERVICE_ACCOUNT isn't set: test pushes need Firebase")?;
    let key =
        std::fs::read_to_string(&key_path).with_context(|| format!("can't read {key_path}"))?;
    let fcm = Fcm::new(&key, std::env::var("FCM_API").ok())?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .context("can't connect to the database")?;
    let found = db::find_devices(&pool, prefix).await?;
    let (device, token) = match found.as_slice() {
        [one] => one.clone(),
        [] => bail!("no registered device's ID starts with {prefix}"),
        many => bail!(
            "{} devices' IDs start with {prefix}: give more digits",
            many.len()
        ),
    };
    let token = token.context("that device has no push token")?;
    let push = Push {
        to: device,
        token,
        priority,
        kind: PushKind::Pull { from: device },
    };
    let sent = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    // Without the collapse key: FCM throttles collapsible messages after a
    // burst of 20 per device, which the matrix's trials would hit.
    let name = fcm
        .send_message(test_message(&push))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("sent {sent} {name}");
    Ok(())
}
