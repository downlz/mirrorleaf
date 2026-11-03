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
//! | `RUST_LOG` | Log filter (default `info`) |

use std::time::Duration;

use anyhow::{Context, Result};
use mirrorleaf_ledger::push::{Fcm, Pusher};
use mirrorleaf_ledger::{Ledger, db, system_clock};
use sqlx_postgres::PgPoolOptions;

/// How often unanswered wake-ups are checked (rule 4).
const TICK: Duration = Duration::from_secs(15);
const PRUNE_EVERY: Duration = Duration::from_secs(6 * 3600);

#[tokio::main]
async fn main() -> Result<()> {
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
    let ledger = Ledger {
        db: pool,
        pusher,
        clock: system_clock(),
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
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
