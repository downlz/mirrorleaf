//! mirrorleaf-relay: Mirrorleaf's own Iroh relay (M13, E10), built on n0's
//! `iroh-relay` server. It forwards encrypted traffic between devices that
//! can't reach each other directly and stores nothing.
//!
//! Three things differ from n0's ready-made relay binary:
//!
//! - **Only Mirrorleaf devices get in**: each connecting device is checked
//!   with the ledger ([`LedgerAccess`]), which knows the registered devices and
//!   the short passes of devices that are joining an account.
//! - **No IP addresses in the logs**: the relay's own log lines carry the
//!   client's address in their context, so [`NoSpans`] prints events without
//!   it.
//! - **One server with the ledger**: Caddy serves the relay's HTTPS and
//!   passes the WebSocket through to plain HTTP here; the QUIC address check
//!   on UDP port 7842 reads Caddy's certificate files ([`find_certificate`]).

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh_relay::server::{
    Access, AccessControl, ClientRateLimit, ClientRequest, DynAccessControl, QuicConfig,
    RelayConfig, ServerConfig,
};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// How long the ledger's "yes" for a device is reused before asking again.
const ALLOW_FOR: Duration = Duration::from_secs(10 * 60);
/// How long its "no" is reused: short, so a device that has just registered
/// or been given a pass gets in on its next try (Iroh retries within 16 s).
const DENY_FOR: Duration = Duration::from_secs(10);
/// While the ledger doesn't answer, devices it let in within this time still
/// get in: a ledger restart mustn't cut everyone off.
const KEEP_ALLOWED_FOR: Duration = Duration::from_secs(24 * 3600);
/// How long to wait for the ledger's answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(3);

/// Lets in the devices the ledger knows (M13): registered devices of accounts
/// that aren't blocked, and devices joining an account with a pass. The
/// ledger is asked at `{url}/{device ID in hex}`: 204 lets the device in,
/// anything else keeps it out.
#[derive(Debug)]
pub struct LedgerAccess {
    url: String,
    client: reqwest::Client,
    answers: Mutex<HashMap<[u8; 32], (Instant, bool)>>,
}

impl LedgerAccess {
    pub fn new(url: impl Into<String>) -> Self {
        // reqwest wants a TLS provider even for plain HTTP.
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            url: url.into().trim_end_matches('/').to_owned(),
            client: reqwest::Client::builder()
                .timeout(ASK_TIMEOUT)
                .build()
                .expect("a plain HTTP client"),
            answers: Mutex::default(),
        }
    }

    async fn ask(&self, device: &[u8; 32]) -> anyhow::Result<bool> {
        let url = format!("{}/{}", self.url, data_encoding::HEXLOWER.encode(device));
        let status = self.client.get(url).send().await?.status();
        match status.as_u16() {
            204 => Ok(true),
            403 | 404 => Ok(false),
            other => anyhow::bail!("the ledger answered {other}"),
        }
    }

    fn remember(&self, device: [u8; 32], allowed: bool) {
        let mut answers = self.answers.lock().unwrap();
        if answers.len() > 100_000 {
            answers.retain(|_, (at, ok)| *ok && at.elapsed() < KEEP_ALLOWED_FOR);
        }
        answers.insert(device, (Instant::now(), allowed));
    }
}

impl AccessControl for LedgerAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let device = *request.endpoint_id().as_bytes();
        let cached = self.answers.lock().unwrap().get(&device).copied();
        if let Some((at, allowed)) = cached {
            let fresh_for = if allowed { ALLOW_FOR } else { DENY_FOR };
            if at.elapsed() < fresh_for {
                return access(allowed);
            }
        }
        match self.ask(&device).await {
            Ok(allowed) => {
                self.remember(device, allowed);
                access(allowed)
            }
            Err(e) => {
                tracing::warn!("the ledger didn't answer an access check: {e:#}");
                access(matches!(cached, Some((at, true)) if at.elapsed() < KEEP_ALLOWED_FOR))
            }
        }
    }
}

fn access(allowed: bool) -> Access {
    if allowed {
        Access::Allow
    } else {
        Access::Deny { reason: None }
    }
}

/// What the relay serves.
pub struct Settings {
    /// Plain HTTP: the relay itself, behind Caddy's HTTPS.
    pub http: SocketAddr,
    /// QUIC address discovery, with its TLS configuration. None without a
    /// certificate (development).
    pub quic: Option<(SocketAddr, rustls::ServerConfig)>,
    pub access: Arc<dyn DynAccessControl>,
    /// Bytes per second each device may send, and its burst.
    pub client_rate: Option<(NonZeroU32, NonZeroU32)>,
    /// Where to serve the relay's counters (Prometheus text, `/metrics`).
    pub metrics: Option<SocketAddr>,
}

impl Settings {
    pub fn server_config(self) -> ServerConfig {
        let mut relay = RelayConfig::new(self.http);
        relay.access = self.access;
        relay.key_cache_capacity = Some(16_384);
        if let Some((rate, burst)) = self.client_rate {
            let mut limit = ClientRateLimit::new(rate);
            limit.max_burst_bytes = Some(burst);
            relay.limits.client_rx = Some(limit);
        }
        let mut config = ServerConfig::default();
        config.relay = Some(relay);
        config.quic = self.quic.map(|(addr, tls)| {
            let mut quic = QuicConfig::new(addr);
            quic.server_config = Some(tls);
            quic
        });
        config.metrics_addr = self.metrics;
        config
    }
}

/// Finds the certificate and key Caddy obtained for `domain` under its
/// certificate folder (`<dir>/<issuer>/<domain>/<domain>.crt` and `.key`).
pub fn find_certificate(dir: &Path, domain: &str) -> Option<(PathBuf, PathBuf)> {
    let mut issuers: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    // Let's Encrypt first, if Caddy ever used a second issuer.
    issuers.sort_by_key(|p| !p.to_string_lossy().contains("letsencrypt"));
    issuers.into_iter().find_map(|issuer| {
        let cert = issuer.join(domain).join(format!("{domain}.crt"));
        let key = issuer.join(domain).join(format!("{domain}.key"));
        (cert.is_file() && key.is_file()).then_some((cert, key))
    })
}

/// TLS for the QUIC address check, from certificate files read again every
/// hour, so Caddy's renewals are picked up.
pub async fn quic_tls(cert: PathBuf, key: PathBuf) -> anyhow::Result<rustls::ServerConfig> {
    let provider = rustls::crypto::ring::default_provider();
    let resolver =
        iroh_relay::server::reloading_resolver(&provider, cert, key, Duration::from_secs(3600))
            .await
            .map_err(|e| anyhow::anyhow!("can't read the certificate: {e}"))?;
    Ok(
        rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_cert_resolver(resolver),
    )
}

/// Log lines without their span context, which for the relay's connections
/// holds the client's IP address (plan §5.8: no IP addresses in logs).
pub struct NoSpans;

impl<S, N> FormatEvent<S, N> for NoSpans
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        SystemTime.format_time(&mut writer)?;
        let meta = event.metadata();
        write!(writer, " {:>5} {}: ", meta.level(), meta.target())?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}
