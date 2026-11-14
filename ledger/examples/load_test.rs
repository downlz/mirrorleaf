//! The ledger's load test at the 10,000-user scale (M13, E10, P1): about
//! 30,000 devices (three per account), each sending signed status reports.
//! At launch scale the ledger sees about 65 requests a second (a heartbeat
//! every 5 minutes per online device, plus reports after edits).
//!
//! Run it against a ledger started for testing, with the puzzle and the
//! sign-up limits off (registering 30,000 devices is not the steady state):
//!
//! ```text
//! LEDGER_PUZZLE_BITS=0 LEDGER_NEW_ACCOUNTS_PER_MINUTE=0 LEDGER_SIGN_UPS_PER_HOUR_PER_IP=0 …
//! cargo run --release -p mirrorleaf-ledger --example load_test -- \
//!     --url http://127.0.0.1:18080 --devices 30000 --rate 65 --secs 120
//! ```
//!
//! It prints the latency of the reports (p50, p95, p99, slowest) and any
//! errors; watch the ledger and Postgres with `docker stats` meanwhile.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use mirrorleaf_proto::AccountKey;
use mirrorleaf_proto::ledger::{Approval, DeviceKey};
use serde_json::json;
use tokio::sync::{Mutex, Semaphore};

struct Device {
    key: DeviceKey,
    /// This device's own change counter, raised as it "edits".
    counter: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    };
    let url = arg("--url", "http://127.0.0.1:18080");
    let devices: usize = arg("--devices", "30000").parse()?;
    let rate: f64 = arg("--rate", "65").parse()?;
    let secs: u64 = arg("--secs", "120").parse()?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(256)
        .build()?;

    // Accounts of three devices, all registered.
    println!("registering {devices} devices…");
    let started = Instant::now();
    let fleet: Arc<Vec<Device>> = Arc::new(
        (0..devices)
            .map(|_| Device {
                key: DeviceKey::from_bytes(rand::random()),
                counter: AtomicU64::new(0),
            })
            .collect(),
    );
    let accounts: Vec<AccountKey> = (0..devices.div_ceil(3))
        .map(|_| AccountKey::generate())
        .collect();
    let limit = Arc::new(Semaphore::new(64));
    let failures = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for (i, account) in accounts.iter().enumerate() {
        let members: Vec<usize> = (i * 3..(i * 3 + 3).min(devices)).collect();
        let approvals: Vec<String> = members
            .iter()
            .map(|d| Approval::AddDevice.sign(account, &fleet[*d].key.id()))
            .collect();
        let (http, url, fleet, limit, failures) = (
            http.clone(),
            url.clone(),
            fleet.clone(),
            limit.clone(),
            failures.clone(),
        );
        let account_id = account.id().to_string();
        tasks.push(tokio::spawn(async move {
            let _permit = limit.acquire().await;
            for (n, (d, approval)) in members.iter().zip(approvals).enumerate() {
                let path = if n == 0 {
                    "/v1/accounts"
                } else {
                    "/v1/devices"
                };
                let body =
                    json!({ "account": account_id, "approval": approval, "platform": "android" });
                if let Err(e) = send(&http, &url, &fleet[*d].key, "POST", path, &body).await {
                    failures.lock().await.push(format!("{e:#}"));
                }
            }
        }));
    }
    for task in tasks {
        task.await?;
    }
    let failed = failures.lock().await.len();
    println!(
        "registered in {:.1} s ({failed} failed)",
        started.elapsed().as_secs_f64()
    );
    if failed > devices / 100 {
        bail!(
            "too many registrations failed: {:?}",
            failures.lock().await.first()
        );
    }

    // The steady state: status reports at `rate` a second, from devices
    // picked at random; one in four after an "edit".
    println!("reporting at {rate} a second for {secs} s…");
    let latencies = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(AtomicU64::new(0));
    let interval = Duration::from_secs_f64(1.0 / rate);
    let end = Instant::now() + Duration::from_secs(secs);
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
    let mut tasks = Vec::new();
    while Instant::now() < end {
        tick.tick().await;
        let i = rand::random_range(0..devices);
        let (http, url, fleet, latencies, errors) = (
            http.clone(),
            url.clone(),
            fleet.clone(),
            latencies.clone(),
            errors.clone(),
        );
        tasks.push(tokio::spawn(async move {
            let base = i - i % 3;
            if rand::random_range(0..4) == 0 {
                fleet[i].counter.fetch_add(1, Ordering::Relaxed);
            }
            // What this device holds: its own changes and its account's.
            let vv: serde_json::Map<String, serde_json::Value> = (base..(base + 3)
                .min(fleet.len()))
                .map(|d| {
                    (
                        fleet[d].key.id().to_string(),
                        json!(fleet[d].counter.load(Ordering::Relaxed)),
                    )
                })
                .collect();
            let body = json!({
                "version_vector": vv, "online_for_secs": 360, "platform": "android",
                "push_token": null, "notifications": false,
            });
            let started = Instant::now();
            match send(
                &http,
                &url,
                &fleet[i].key,
                "PUT",
                "/v1/devices/me/status",
                &body,
            )
            .await
            {
                Ok(()) => latencies.lock().await.push(started.elapsed()),
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for task in tasks {
        task.await?;
    }
    let mut latencies = latencies.lock().await.clone();
    latencies.sort();
    let at = |q: f64| {
        latencies
            .get(((latencies.len() as f64 - 1.0) * q) as usize)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    };
    println!(
        "{} reports, {} errors: p50 {:.1} ms, p95 {:.1} ms, p99 {:.1} ms, slowest {:.1} ms",
        latencies.len(),
        errors.load(Ordering::Relaxed),
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0)
    );
    Ok(())
}

/// A signed version-2 request; errors unless it succeeded.
async fn send(
    http: &reqwest::Client,
    url: &str,
    key: &DeviceKey,
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<()> {
    let body = serde_json::to_vec(body)?;
    let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let nonce = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 16]>());
    let response = http
        .request(method.parse()?, format!("{url}{path}"))
        .header("x-mirrorleaf-device", key.id().to_string())
        .header("x-mirrorleaf-time", time.to_string())
        .header("x-mirrorleaf-api", "2")
        .header("x-mirrorleaf-nonce", &nonce)
        .header(
            "x-mirrorleaf-signature",
            key.sign_request_v2(method, path, time, &nonce, &body),
        )
        .body(body)
        .send()
        .await
        .context("no answer")?;
    if !response.status().is_success() {
        bail!(
            "{} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }
    Ok(())
}
