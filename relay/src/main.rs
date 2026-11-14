//! mirrorleaf-relay (M13, E10): see the library's notes.
//!
//! Configuration, from the environment:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `RELAY_HTTP_LISTEN` | The relay's plain HTTP address, behind Caddy (default `0.0.0.0:3340`) |
//! | `RELAY_ACCESS_URL` | The ledger's access check, e.g. `http://ledger:8081/internal/relay` (required, unless `RELAY_ACCESS=everyone` for development) |
//! | `RELAY_CERT_FILE`, `RELAY_KEY_FILE` | The certificate (full chain) and key for the relay's domain, as PEM files, e.g. certbot's `fullchain.pem` and `privkey.pem` behind nginx |
//! | `RELAY_DOMAIN` | Otherwise, the relay's domain, e.g. `relay.mirrorleaf.app`, to find Caddy's certificate for it |
//! | `RELAY_CERT_DIR` | Caddy's certificate folder (default `/certs`) |
//! | `RELAY_QUIC_LISTEN` | The QUIC address check (default `0.0.0.0:7842`); off without a certificate |
//! | `RELAY_CLIENT_BYTES_PER_SECOND` | What each device may send per second (default 1,000,000; 0 for no limit) |
//! | `RELAY_CLIENT_BURST_BYTES` | Its burst (default 4,000,000) |
//! | `RELAY_METRICS_LISTEN` | The relay's counters, for the ledger's dashboard (default `0.0.0.0:9090`; empty for none) |
//! | `RUST_LOG` | Log filter (default `info`). Debug lines can name addresses: keep it at `info` in production |

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use iroh_relay::server::{AllowAll, DynAccessControl, Server};
use mirrorleaf_relay::{LedgerAccess, NoSpans, Settings, find_certificate, quic_tls};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .event_format(NoSpans)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let access: Arc<dyn DynAccessControl> = match env("RELAY_ACCESS_URL") {
        Some(url) => {
            tracing::info!("devices are checked with the ledger");
            Arc::new(LedgerAccess::new(url))
        }
        None if env("RELAY_ACCESS").as_deref() == Some("everyone") => {
            tracing::warn!("RELAY_ACCESS=everyone: any device may use this relay");
            Arc::new(AllowAll)
        }
        None => bail!("set RELAY_ACCESS_URL (or RELAY_ACCESS=everyone for development)"),
    };
    let http: SocketAddr = env("RELAY_HTTP_LISTEN")
        .unwrap_or_else(|| "0.0.0.0:3340".into())
        .parse()
        .context("RELAY_HTTP_LISTEN")?;
    let quic_addr = || -> Result<SocketAddr> {
        env("RELAY_QUIC_LISTEN")
            .unwrap_or_else(|| "0.0.0.0:7842".into())
            .parse()
            .context("RELAY_QUIC_LISTEN")
    };
    let files = env("RELAY_CERT_FILE").zip(env("RELAY_KEY_FILE"));
    let quic = match (files, env("RELAY_DOMAIN")) {
        // Certificate files named outright (certbot behind nginx).
        (Some((cert, key)), _) => {
            let (cert, key) = (PathBuf::from(cert), PathBuf::from(key));
            wait_for_files(&cert, &key).await;
            tracing::info!("using the certificate in {}", cert.display());
            Some((quic_addr()?, quic_tls(cert, key).await?))
        }
        (None, Some(domain)) => {
            let dir = PathBuf::from(env("RELAY_CERT_DIR").unwrap_or_else(|| "/certs".into()));
            let (cert, key) = wait_for_certificate(&dir, &domain).await;
            Some((quic_addr()?, quic_tls(cert, key).await?))
        }
        (None, None) => {
            tracing::warn!(
                "no certificate (RELAY_CERT_FILE or RELAY_DOMAIN): no QUIC address check"
            );
            None
        }
    };
    let rate = number("RELAY_CLIENT_BYTES_PER_SECOND", 1_000_000)?;
    let burst = number("RELAY_CLIENT_BURST_BYTES", 4_000_000)?;
    let client_rate = NonZeroU32::new(rate)
        .map(|rate| (rate, NonZeroU32::new(burst.max(rate.get())).unwrap_or(rate)));
    let metrics = match env("RELAY_METRICS_LISTEN").as_deref() {
        None => Some("0.0.0.0:9090".parse()?),
        Some("") => None,
        Some(addr) => Some(addr.parse().context("RELAY_METRICS_LISTEN")?),
    };

    let settings = Settings {
        http,
        quic,
        access,
        client_rate,
        metrics,
    };
    let server = Server::spawn(settings.server_config()).await?;
    tracing::info!(
        "mirrorleaf-relay {} on {http}{}",
        env!("CARGO_PKG_VERSION"),
        server
            .quic_addr()
            .map(|a| format!(", QUIC address check on {a}"))
            .unwrap_or_default()
    );
    shutdown_signal().await;
    tracing::info!("stopping");
    server.shutdown().await?;
    Ok(())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn number(name: &str, default: u32) -> Result<u32> {
    match env(name) {
        Some(v) => v
            .parse()
            .with_context(|| format!("{name} must be a number")),
        None => Ok(default),
    }
}

/// Caddy obtains the certificate when it starts, which may be after the relay.
async fn wait_for_certificate(dir: &std::path::Path, domain: &str) -> (PathBuf, PathBuf) {
    let mut said = false;
    loop {
        if let Some(found) = find_certificate(dir, domain) {
            tracing::info!("using the certificate in {}", found.0.display());
            return found;
        }
        if !said {
            tracing::info!(
                "waiting for Caddy's certificate for {domain} in {}",
                dir.display()
            );
            said = true;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

/// Waits for certificate files that may not exist yet.
async fn wait_for_files(cert: &std::path::Path, key: &std::path::Path) {
    let mut said = false;
    while !(cert.is_file() && key.is_file()) {
        if !said {
            tracing::info!("waiting for {} and its key", cert.display());
            said = true;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
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
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}
