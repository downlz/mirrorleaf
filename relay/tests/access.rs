//! The relay lets in only the devices the ledger allows (M13), here with a
//! stand-in for the ledger's access check and devices that can reach each
//! other only through the relay.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey};
use iroh_relay::server::Server;
use mirrorleaf_relay::{LedgerAccess, Settings};

const ALPN: &[u8] = b"mirrorleaf/relay-test";

async fn fake_ledger(allowed: HashSet<String>) -> String {
    async fn check(
        State(allowed): State<Arc<HashSet<String>>>,
        Path(device): Path<String>,
    ) -> StatusCode {
        if allowed.contains(&device) {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::FORBIDDEN
        }
    }
    let app = Router::new()
        .route("/internal/relay/{device}", get(check))
        .with_state(Arc::new(allowed));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}/internal/relay")
}

async fn device(secret: SecretKey, relay: &RelayUrl) -> Endpoint {
    let map: RelayMap = RelayConfig::new(relay.clone(), None).into();
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(map))
        // Only the relay: no direct path to find.
        .clear_ip_transports()
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap()
}

#[tokio::test]
async fn only_devices_the_ledger_knows_use_the_relay() {
    let (a, b, stranger) = (
        SecretKey::generate(),
        SecretKey::generate(),
        SecretKey::generate(),
    );
    let hex = |k: &SecretKey| data_encoding::HEXLOWER.encode(k.public().as_bytes());
    let url = fake_ledger([hex(&a), hex(&b)].into_iter().collect()).await;
    let settings = Settings {
        http: "127.0.0.1:0".parse().unwrap(),
        quic: None,
        access: Arc::new(LedgerAccess::new(url)),
        client_rate: None,
        metrics: None,
    };
    let server = Server::spawn(settings.server_config()).await.unwrap();
    let relay: RelayUrl = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();

    let b_id = b.public();
    let (a, b, stranger) = (
        device(a, &relay).await,
        device(b, &relay).await,
        device(stranger, &relay).await,
    );
    tokio::time::timeout(Duration::from_secs(10), b.online())
        .await
        .expect("an allowed device connects to the relay");
    let accepting = tokio::spawn(async move {
        let conn = b.accept().await.unwrap().await.unwrap();
        let mut recv = conn.accept_uni().await.unwrap();
        let got = recv.read_to_end(64).await.unwrap();
        (b, got)
    });
    let target = EndpointAddr::new(b_id).with_relay_url(relay.clone());
    let conn = tokio::time::timeout(Duration::from_secs(10), a.connect(target.clone(), ALPN))
        .await
        .expect("allowed devices meet through the relay")
        .unwrap();
    let mut send = conn.open_uni().await.unwrap();
    send.write_all(b"hello").await.unwrap();
    send.finish().unwrap();
    let (_b, got) = accepting.await.unwrap();
    assert_eq!(got, b"hello");

    // A device the ledger doesn't know never gets through.
    let refused =
        tokio::time::timeout(Duration::from_secs(5), stranger.connect(target, ALPN)).await;
    assert!(
        !matches!(refused, Ok(Ok(_))),
        "a stranger reached the device through the relay"
    );
}
