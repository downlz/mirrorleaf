//! The ledger's internal port (M13): never published to the internet.
//!
//! - `GET /internal/relay/{device}`: the relay asks whether a device may use
//!   it (204 yes, 403 no). Over the server's internal network only.
//! - `GET /dashboard` and `/dashboard/data`: the beta's daily totals (E12),
//!   for the owner over Tailscale or an SSH tunnel.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use mirrorleaf_proto::DeviceId;

use crate::stats::Report;
use crate::{Ledger, db, stats};

/// Days the dashboard shows.
const DAYS: i32 = 30;

impl Ledger {
    pub fn internal_router(self) -> Router {
        Router::new()
            .route("/internal/relay/{device}", get(relay_check))
            .route("/dashboard", get(dashboard))
            .route("/dashboard/data", get(dashboard_data))
            .route("/", get(|| async { Redirect::to("/dashboard") }))
            .with_state(self)
    }

    /// The dashboard's figures for the last `days` days.
    pub async fn report(&self, days: i32) -> anyhow::Result<Report> {
        let now = self.now();
        let relay = match &self.settings.relay_metrics_url {
            Some(url) => match fetch(url).await {
                Ok(text) => stats::relay_counters(&text),
                Err(e) => {
                    tracing::warn!("the relay's counters didn't load: {e:#}");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        Ok(Report {
            generated_at: now,
            now: db::snapshot(&self.db, now).await?,
            versions: db::versions(&self.db).await?,
            puzzle_bits: self.guard.puzzle_bits(),
            sign_ups_last_hour: self.guard.sign_ups_last_hour(),
            days: Report::from_counts(
                db::daily_counts(&self.db, stats::day(now) - days + 1).await?,
            ),
            relay,
        })
    }
}

async fn fetch(url: &str) -> anyhow::Result<String> {
    // reqwest wants a TLS provider even for plain HTTP; without Firebase
    // nothing else installs one.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

async fn relay_check(State(l): State<Ledger>, Path(device): Path<String>) -> StatusCode {
    let Ok(device) = device.parse::<DeviceId>() else {
        return StatusCode::BAD_REQUEST;
    };
    match db::relay_allowed(&l.db, &device, l.now()).await {
        Ok(true) => {
            l.counts.bump("relay_allowed");
            StatusCode::NO_CONTENT
        }
        Ok(false) => {
            l.counts.bump("relay_denied");
            StatusCode::FORBIDDEN
        }
        Err(e) => {
            tracing::warn!("relay check failed: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

async fn dashboard(State(l): State<Ledger>) -> Response {
    match l.report(DAYS).await {
        Ok(report) => Html(report.html()).into_response(),
        Err(e) => {
            tracing::warn!("dashboard failed: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the dashboard failed: see the ledger's log",
            )
                .into_response()
        }
    }
}

async fn dashboard_data(State(l): State<Ledger>) -> Response {
    match l.report(DAYS).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => {
            tracing::warn!("dashboard failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
