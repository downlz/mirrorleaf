//! Ledger integration tests (implementation plan, M3): the real service on a
//! local port, against Postgres, with a clock the tests move forward.
//!
//! Set `MIRRORLEAF_TEST_DATABASE_URL` to run them; without it they pass without
//! doing anything. Each test works in a schema of its own, dropped at the end.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use mirrorleaf_ledger::push::{FALLBACK_BODY_KEY, Fcm, Pusher, http_client};
use mirrorleaf_ledger::rules::{Priority, Push, PushKind, WAKE_INTERVAL_SECS};
use mirrorleaf_ledger::{Ledger, db};
use mirrorleaf_proto::ledger::{Approval, DeviceKey, Platform};
use mirrorleaf_proto::{AccountKey, DeviceId};
use serde_json::{Value, json};
use sqlx_core::query::query;
use sqlx_core::query_scalar::query_scalar;
use sqlx_core::sql_str::AssertSqlSafe;
use sqlx_postgres::{PgConnectOptions, PgPool, PgPoolOptions};

const START: i64 = 1_800_000_000;

struct TestLedger {
    url: String,
    pushes: Arc<Mutex<Vec<Push>>>,
    clock: Arc<AtomicI64>,
    ledger: Ledger,
    admin: PgPool,
    schema: String,
}

impl TestLedger {
    /// Starts a ledger in a fresh schema, or returns `None` if no test
    /// database is configured.
    async fn start() -> Option<Self> {
        Self::start_with(None).await
    }

    async fn start_with(pusher: Option<Pusher>) -> Option<Self> {
        let Ok(url) = std::env::var("MIRRORLEAF_TEST_DATABASE_URL") else {
            eprintln!(
                "skipped: set MIRRORLEAF_TEST_DATABASE_URL to run the ledger's database tests"
            );
            return None;
        };
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("can't connect to the test database");
        drop_stale_schemas(&admin).await;
        let schema = format!("mlt_{}_{:08x}", wall_clock(), rand::random::<u32>());
        query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await
            .unwrap();
        let options: PgConnectOptions = url.parse().unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.options([("search_path", schema.as_str())]))
            .await
            .unwrap();
        db::migrate(&pool).await.unwrap();
        db::migrate(&pool)
            .await
            .expect("migrating twice is harmless");

        let pushes = Arc::new(Mutex::new(Vec::new()));
        let clock = Arc::new(AtomicI64::new(START));
        let ledger = Ledger {
            db: pool,
            pusher: pusher.unwrap_or_else(|| Pusher::Record(pushes.clone())),
            clock: {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::SeqCst))
            },
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = ledger.clone().router();
        tokio::spawn(async move { axum::serve(listener, app).await });
        Some(Self {
            url,
            pushes,
            clock,
            ledger,
            admin,
            schema,
        })
    }

    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    fn take_pushes(&self) -> Vec<Push> {
        std::mem::take(&mut *self.pushes.lock().unwrap())
    }

    async fn finish(self) {
        self.ledger.db.close().await;
        query(AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .unwrap();
    }
}

/// Removes schemas left by test runs that stopped early, after an hour.
async fn drop_stale_schemas(admin: &PgPool) {
    let names: Vec<String> =
        query_scalar("SELECT nspname::text FROM pg_namespace WHERE nspname LIKE 'mlt\\_%'")
            .fetch_all(admin)
            .await
            .unwrap();
    for name in names {
        let started: i64 = name
            .split('_')
            .nth(1)
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        if wall_clock() - started > 3600 {
            let _ = query(AssertSqlSafe(format!(
                "DROP SCHEMA IF EXISTS {name} CASCADE"
            )))
            .execute(admin)
            .await;
        }
    }
}

fn wall_clock() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A device of a developer account, signing its own requests.
struct Dev {
    key: DeviceKey,
    account: Arc<AccountKey>,
    platform: Platform,
    token: Option<String>,
    notifications: bool,
}

impl Dev {
    fn new(account: &Arc<AccountKey>, platform: Platform) -> Self {
        let key = DeviceKey::from_bytes(rand::random());
        let token = (platform == Platform::Android).then(|| format!("token-{}", key.id().short()));
        Self {
            key,
            account: account.clone(),
            platform,
            token,
            notifications: true,
        }
    }

    fn id(&self) -> DeviceId {
        self.key.id()
    }

    async fn call(
        &self,
        t: &TestLedger,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        self.call_at(t, t.now(), method, path, body).await
    }

    async fn call_at(
        &self,
        t: &TestLedger,
        time: i64,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let response = http_client()
            .unwrap()
            .request(method.parse().unwrap(), format!("{}{path}", t.url))
            .header("x-mirrorleaf-device", self.id().to_string())
            .header("x-mirrorleaf-time", time.to_string())
            .header(
                "x-mirrorleaf-signature",
                self.key.sign_request(method, path, time, &body),
            )
            .body(body)
            .send()
            .await
            .unwrap();
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
        let bytes = response.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn register(&self, t: &TestLedger, path: &str) -> (StatusCode, Value) {
        let body = json!({
            "account": self.account.id().to_string(),
            "approval": Approval::AddDevice.sign(&self.account, &self.id()),
            "platform": self.platform,
        });
        self.call(t, "POST", path, body).await
    }

    /// Reports holding `held` (device → change number), online for `online` seconds.
    async fn report(&self, t: &TestLedger, held: &[(&Dev, u64)], online: Option<u32>) -> Value {
        let vv: serde_json::Map<String, Value> = held
            .iter()
            .map(|(d, n)| (d.id().to_string(), json!(n)))
            .collect();
        let body = json!({
            "version_vector": vv,
            "online_for_secs": online,
            "platform": self.platform,
            "push_token": self.token,
            "notifications": self.notifications,
        });
        let (status, response) = self.call(t, "PUT", "/v1/devices/me/status", body).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        response
    }

    async fn wake(&self, t: &TestLedger) -> Value {
        let (status, response) = self.call(t, "POST", "/v1/wake", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        response
    }
}

/// A laptop, a desktop and a phone on one account, all registered.
async fn account(t: &TestLedger) -> (Dev, Dev, Dev) {
    let key = Arc::new(AccountKey::generate());
    let laptop = Dev::new(&key, Platform::Macos);
    let desktop = Dev::new(&key, Platform::Linux);
    let phone = Dev::new(&key, Platform::Android);
    assert_eq!(
        laptop.register(t, "/v1/accounts").await.0,
        StatusCode::NO_CONTENT
    );
    for d in [&desktop, &phone] {
        assert_eq!(d.register(t, "/v1/devices").await.0, StatusCode::NO_CONTENT);
    }
    (laptop, desktop, phone)
}

fn row<'a>(response: &'a Value, d: &Dev) -> &'a Value {
    response["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["device"] == d.id().to_string())
        .expect("device listed")
}

#[tokio::test]
async fn registration_needs_signatures_and_the_account_keys_approval() {
    let Some(t) = TestLedger::start().await else {
        return;
    };
    let key = Arc::new(AccountKey::generate());
    let laptop = Dev::new(&key, Platform::Macos);

    // Joining an account that doesn't exist yet.
    let (status, body) = laptop.register(&t, "/v1/devices").await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("unknown_account"))
    );
    assert_eq!(
        laptop.register(&t, "/v1/accounts").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        laptop.register(&t, "/v1/accounts").await.0,
        StatusCode::NO_CONTENT,
        "idempotent"
    );

    // Approved by a different account key.
    let impostor = Dev::new(&Arc::new(AccountKey::generate()), Platform::Android);
    let body = json!({
        "account": key.id().to_string(),
        "approval": Approval::AddDevice.sign(&impostor.account, &impostor.id()),
        "platform": "android",
    });
    let (status, _) = impostor.call(&t, "POST", "/v1/devices", body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Not registered, so no status.
    let (status, body) = impostor
        .call(
            &t,
            "PUT",
            "/v1/devices/me/status",
            json!({
                "version_vector": {}, "online_for_secs": 360, "platform": "android",
                "push_token": null, "notifications": false,
            }),
        )
        .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("unknown_device"))
    );

    // A stale timestamp, and a body that doesn't match its signature.
    let (status, body) = laptop
        .call_at(&t, t.now() - 301, "POST", "/v1/wake", json!({}))
        .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("clock_skew"))
    );
    let response = http_client()
        .unwrap()
        .post(format!("{}/v1/wake", t.url))
        .header("x-mirrorleaf-device", laptop.id().to_string())
        .header("x-mirrorleaf-time", t.now().to_string())
        .header(
            "x-mirrorleaf-signature",
            laptop.key.sign_request("POST", "/v1/wake", t.now(), b"{}"),
        )
        .body("{\"tampered\":true}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    t.finish().await;
}

/// M3 "done when": the ledger marks the right devices as stale.
#[tokio::test]
async fn the_ledger_marks_the_right_devices_as_stale() {
    let Some(t) = TestLedger::start().await else {
        return;
    };
    let (laptop, desktop, phone) = account(&t).await;
    laptop.report(&t, &[(&laptop, 1284)], Some(360)).await;
    desktop
        .report(&t, &[(&laptop, 1284), (&desktop, 912)], Some(360))
        .await;
    t.advance(30);
    let response = phone
        .report(
            &t,
            &[(&laptop, 1280), (&desktop, 900), (&phone, 5)],
            Some(0),
        )
        .await;

    let behind = |d: &Dev| row(&response, d)["behind"].as_u64().unwrap();
    assert_eq!(
        behind(&phone),
        4 + 12,
        "behind the laptop by 4 and the desktop by 12"
    );
    assert_eq!(behind(&laptop), 912 + 5);
    assert_eq!(behind(&desktop), 5);
    assert_eq!(row(&response, &laptop)["online"], true);
    assert_eq!(
        row(&response, &phone)["online"],
        false,
        "it reported going offline"
    );
    let last_seen = row(&response, &phone)["last_seen"].as_i64().unwrap();
    assert_eq!(last_seen % 60, 0, "presence is kept to the minute");

    // The heartbeat runs out: a device that stops reporting counts as offline.
    t.advance(361);
    let response = phone.report(&t, &[(&phone, 5)], None).await;
    assert_eq!(row(&response, &laptop)["online"], false);
    t.finish().await;
}

/// M3 "done when", rule 1: new changes wake offline phones that lack them.
#[tokio::test]
async fn rule_1_new_changes_wake_offline_phones_at_normal_priority() {
    let Some(t) = TestLedger::start().await else {
        return;
    };
    let (laptop, desktop, phone) = account(&t).await;
    phone.report(&t, &[(&laptop, 10)], Some(0)).await;
    desktop.report(&t, &[(&laptop, 10)], Some(0)).await;
    laptop.report(&t, &[(&laptop, 10)], Some(360)).await;
    assert!(t.take_pushes().is_empty(), "nobody lacks anything yet");

    laptop.report(&t, &[(&laptop, 11)], Some(360)).await;
    let pushes = t.take_pushes();
    assert_eq!(
        pushes.len(),
        1,
        "the phone, not the desktop: desktops can't be woken"
    );
    assert_eq!(pushes[0].to, phone.id());
    assert_eq!(pushes[0].token, phone.token.clone().unwrap());
    assert_eq!(pushes[0].priority, Priority::Normal);
    assert_eq!(pushes[0].kind, PushKind::Pull { from: laptop.id() });

    // At most one wake-up per two minutes.
    laptop.report(&t, &[(&laptop, 12)], Some(360)).await;
    assert!(t.take_pushes().is_empty());
    t.advance(WAKE_INTERVAL_SECS);
    laptop.report(&t, &[(&laptop, 13)], Some(360)).await;
    assert_eq!(t.take_pushes().len(), 1);
    t.finish().await;
}

/// M3 "done when", rule 2: a device coming online behind gets the phone that
/// holds its changes woken to deliver them.
#[tokio::test]
async fn rule_2_a_device_coming_online_behind_wakes_the_holder() {
    let Some(t) = TestLedger::start().await else {
        return;
    };
    let (laptop, _desktop, phone) = account(&t).await;
    laptop.report(&t, &[(&laptop, 5)], Some(0)).await;
    phone
        .report(&t, &[(&laptop, 5), (&phone, 3)], Some(0))
        .await;
    assert!(t.take_pushes().is_empty());

    laptop.report(&t, &[(&laptop, 5)], Some(360)).await;
    let pushes = t.take_pushes();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        (pushes[0].to, pushes[0].priority),
        (phone.id(), Priority::Normal)
    );
    assert_eq!(
        pushes[0].kind,
        PushKind::Deliver {
            to: laptop.id(),
            changes: 3
        }
    );

    // Still online on its heartbeat: not "coming online" again.
    t.advance(300);
    laptop.report(&t, &[(&laptop, 5)], Some(360)).await;
    assert!(t.take_pushes().is_empty());
    t.finish().await;
}

/// M3 "done when", rules 3 and 4: opening the app while behind wakes the
/// holder at high priority, and an unanswered wake-up gets the fallback.
#[tokio::test]
async fn rules_3_and_4_wake_at_high_priority_then_fall_back_to_a_notification() {
    let Some(t) = TestLedger::start().await else {
        return;
    };
    let (laptop, _desktop, phone) = account(&t).await;
    phone
        .report(&t, &[(&laptop, 5), (&phone, 3)], Some(0))
        .await;
    laptop.report(&t, &[(&laptop, 5)], Some(360)).await;
    t.take_pushes();
    t.advance(WAKE_INTERVAL_SECS);

    let response = laptop.wake(&t).await;
    assert_eq!(response["result"], "woken", "{response}");
    assert_eq!(response["device"], phone.id().to_string());
    assert_eq!(
        (
            response["changes"].as_u64(),
            response["high_priority"].as_bool()
        ),
        (Some(3), Some(true))
    );
    let pushes = t.take_pushes();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        (pushes[0].to, pushes[0].priority),
        (phone.id(), Priority::High)
    );
    assert_eq!(laptop.wake(&t).await["result"], "already_waking");

    // Not answered within 60 s: the fallback notification, once.
    t.advance(59);
    t.ledger.tick().await.unwrap();
    assert!(t.take_pushes().is_empty(), "not due yet");
    t.advance(1);
    t.ledger.tick().await.unwrap();
    let pushes = t.take_pushes();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        (pushes[0].to, pushes[0].priority, &pushes[0].kind),
        (phone.id(), Priority::High, &PushKind::Fallback)
    );
    t.ledger.tick().await.unwrap();
    assert!(t.take_pushes().is_empty(), "only once");

    // A wake-up that is answered gets no fallback.
    t.advance(WAKE_INTERVAL_SECS);
    phone
        .report(&t, &[(&laptop, 5), (&phone, 4)], Some(0))
        .await;
    t.take_pushes();
    assert_eq!(laptop.wake(&t).await["result"], "woken");
    t.take_pushes();
    laptop
        .report(&t, &[(&laptop, 5), (&phone, 4)], Some(360))
        .await;
    t.advance(60);
    t.ledger.tick().await.unwrap();
    assert!(t.take_pushes().is_empty());
    t.finish().await;
}

#[tokio::test]
async fn wake_requests_answer_without_a_push_when_none_helps() {
    let Some(t) = TestLedger::start().await else {
        return;
    };
    let (laptop, desktop, mut phone) = account(&t).await;
    laptop.report(&t, &[(&laptop, 5)], Some(360)).await;
    assert_eq!(laptop.wake(&t).await["result"], "up_to_date");

    desktop
        .report(&t, &[(&laptop, 5), (&desktop, 2)], Some(360))
        .await;
    let response = laptop.wake(&t).await;
    assert_eq!(
        (response["result"].as_str(), response["device"].as_str()),
        (
            Some("holder_online"),
            Some(desktop.id().to_string().as_str())
        )
    );

    desktop
        .report(&t, &[(&laptop, 5), (&desktop, 2)], Some(0))
        .await;
    let response = laptop.wake(&t).await;
    assert_eq!(
        (response["result"].as_str(), response["device"].as_str()),
        (
            Some("no_wakeable_holder"),
            Some(desktop.id().to_string().as_str())
        ),
        "a sleeping desktop can't be woken, and the answer names it for the prompt"
    );

    // A phone that may not show notifications is woken at normal priority only.
    phone.notifications = false;
    phone
        .report(&t, &[(&laptop, 5), (&desktop, 2), (&phone, 1)], Some(0))
        .await;
    t.take_pushes();
    let response = laptop.wake(&t).await;
    assert_eq!(
        (
            response["result"].as_str(),
            response["high_priority"].as_bool()
        ),
        (Some("woken"), Some(false))
    );
    assert_eq!(t.take_pushes()[0].priority, Priority::Normal);
    t.finish().await;
}

#[tokio::test]
async fn devices_can_be_removed_and_unseen_ones_are_pruned() {
    let Some(t) = TestLedger::start().await else {
        return;
    };
    let (laptop, desktop, phone) = account(&t).await;
    for d in [&laptop, &desktop, &phone] {
        d.report(&t, &[], Some(0)).await;
    }

    // Removing another device needs the account key's approval.
    let path = format!("/v1/devices/{}", phone.id());
    let wrong = json!({ "approval": Approval::AddDevice.sign(&laptop.account, &phone.id()) });
    assert_eq!(
        laptop.call(&t, "DELETE", &path, wrong).await.0,
        StatusCode::UNAUTHORIZED
    );
    let right = json!({ "approval": Approval::RemoveDevice.sign(&laptop.account, &phone.id()) });
    assert_eq!(
        laptop.call(&t, "DELETE", &path, right).await.0,
        StatusCode::NO_CONTENT
    );
    let response = laptop.report(&t, &[], Some(0)).await;
    assert_eq!(response["devices"].as_array().unwrap().len(), 2);

    // A device not seen for 90 days is pruned.
    t.advance(89 * 24 * 3600);
    laptop.report(&t, &[], Some(0)).await;
    t.advance(2 * 24 * 3600);
    assert_eq!(t.ledger.prune().await.unwrap(), 1, "the desktop");
    let response = laptop.report(&t, &[], Some(0)).await;
    assert_eq!(response["devices"].as_array().unwrap().len(), 1);

    // Deleting the account removes every row.
    assert_eq!(
        laptop
            .call(&t, "DELETE", "/v1/accounts/me", json!({}))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let devices: i64 = query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&t.ledger.db)
        .await
        .unwrap();
    assert_eq!(devices, 0);
    t.finish().await;
}

/// What the fake FCM received.
#[derive(Default)]
struct FakeFcm {
    tokens_issued: usize,
    messages: Vec<(String, Value)>,
}

/// The real FCM client against a fake FCM: the service-account token exchange,
/// the message format, and forgetting a token FCM no longer knows.
#[tokio::test]
async fn the_fcm_client_sends_http_v1_messages_and_drops_dead_tokens() {
    let Some(pem) = test_rsa_key() else {
        eprintln!("skipped: openssl isn't available to make a test RSA key");
        return;
    };
    let fake = Arc::new(Mutex::new(FakeFcm::default()));
    let app = Router::new()
        .route(
            "/token",
            post(|State(f): State<Arc<Mutex<FakeFcm>>>, body: String| async move {
                assert!(body.starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion="));
                assert_eq!(body.rsplit('=').next().unwrap().split('.').count(), 3, "a signed JWT");
                f.lock().unwrap().tokens_issued += 1;
                axum::Json(json!({ "access_token": "fake-access", "expires_in": 3600 }))
            }),
        )
        .route(
            "/v1/projects/{project}/messages:send",
            post(
                |State(f): State<Arc<Mutex<FakeFcm>>>,
                 Path(project): Path<String>,
                 headers: HeaderMap,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(project, "mirrorleaf-test");
                    assert_eq!(headers["authorization"], "Bearer fake-access");
                    let message = body["message"].clone();
                    let answer = match message["token"].as_str() {
                        Some("gone") => StatusCode::NOT_FOUND,
                        Some("broken") => StatusCode::INTERNAL_SERVER_ERROR,
                        _ => StatusCode::OK,
                    };
                    f.lock().unwrap().messages.push((project, message));
                    answer
                },
            ),
        )
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    let key_file = json!({
        "project_id": "mirrorleaf-test",
        "client_email": "ledger@mirrorleaf-test.iam.gserviceaccount.com",
        "private_key": pem,
        "token_uri": format!("{api}/token"),
    });
    let fcm = Fcm::new(&key_file.to_string(), Some(api)).unwrap();

    let Some(t) = TestLedger::start_with(Some(Pusher::Fcm(fcm.into()))).await else {
        return;
    };
    let (laptop, _desktop, mut phone) = account(&t).await;
    phone
        .report(&t, &[(&laptop, 5), (&phone, 3)], Some(0))
        .await;
    laptop.report(&t, &[(&laptop, 5)], Some(360)).await;
    t.advance(WAKE_INTERVAL_SECS);
    assert_eq!(laptop.wake(&t).await["result"], "woken");
    t.advance(60);
    t.ledger.tick().await.unwrap();
    {
        let fake = fake.lock().unwrap();
        assert_eq!(fake.tokens_issued, 1, "one access token, reused");
        let kinds: Vec<_> = fake.messages.iter().map(|(_, m)| m.clone()).collect();
        assert_eq!(
            kinds.len(),
            3,
            "rule 2 (normal), rule 3 (high), rule 4 (fallback): {kinds:#?}"
        );
        assert_eq!(kinds[0]["android"]["priority"], "NORMAL");
        assert_eq!(kinds[0]["data"], json!({ "type": "deliver" }));
        assert_eq!(kinds[1]["android"]["priority"], "HIGH");
        assert_eq!(kinds[1]["data"], json!({ "type": "deliver" }));
        assert_eq!(kinds[2]["data"], json!({ "type": "fallback" }));
        assert_eq!(
            kinds[2]["android"]["notification"]["body_loc_key"],
            FALLBACK_BODY_KEY
        );
        assert!(
            kinds
                .iter()
                .all(|m| m["token"] == phone.token.clone().unwrap())
        );
        // Only the type reaches Google: no device IDs or change counts (plan §5.9).
        for m in &kinds {
            let text = m.to_string();
            assert!(!text.contains(&laptop.id().to_string()), "{text}");
            assert!(!text.contains(&phone.id().to_string()), "{text}");
        }
    }

    // FCM answers 404 for a token it no longer knows: the ledger forgets it,
    // and the device in use hears at once that the phone can't be woken.
    phone.token = Some("gone".into());
    phone
        .report(&t, &[(&laptop, 5), (&phone, 4)], Some(0))
        .await;
    t.advance(WAKE_INTERVAL_SECS);
    let response = laptop.wake(&t).await;
    assert_eq!(
        (response["result"].as_str(), response["device"].as_str()),
        (
            Some("no_wakeable_holder"),
            Some(phone.id().to_string().as_str())
        )
    );
    let token: Option<String> = query_scalar("SELECT push_token FROM devices WHERE device_id = $1")
        .bind(&phone.id().0[..])
        .fetch_one(&t.ledger.db)
        .await
        .unwrap();
    assert_eq!(token, None);

    // Any other failure (here a 500): the same answer, the token is kept, and
    // no fallback notification follows, because nothing is on its way.
    phone.token = Some("broken".into());
    phone
        .report(&t, &[(&laptop, 5), (&phone, 5)], Some(0))
        .await;
    t.advance(WAKE_INTERVAL_SECS);
    let response = laptop.wake(&t).await;
    assert_eq!(
        (response["result"].as_str(), response["device"].as_str()),
        (
            Some("no_wakeable_holder"),
            Some(phone.id().to_string().as_str())
        )
    );
    let sent = fake.lock().unwrap().messages.len();
    t.advance(60);
    t.ledger.tick().await.unwrap();
    assert_eq!(fake.lock().unwrap().messages.len(), sent, "no fallback");
    let (token, wake_at): (Option<String>, Option<i64>) = sqlx_core::query_as::query_as(
        "SELECT push_token, wake_at FROM devices WHERE device_id = $1",
    )
    .bind(&phone.id().0[..])
    .fetch_one(&t.ledger.db)
    .await
    .unwrap();
    assert_eq!((token.as_deref(), wake_at), (Some("broken"), None));
    t.finish().await;
}

/// A throwaway 2048-bit RSA key in PKCS #8 PEM, made with openssl, so no
/// private key needs to live in the repository.
fn test_rsa_key() -> Option<String> {
    let output = std::process::Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
        ])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

/// The device side: `mirrorleaf-net`'s ledger client and `watch` reporting,
/// against the real ledger, on the real clock.
#[tokio::test(flavor = "multi_thread")]
async fn devices_register_report_and_go_offline_through_the_client() {
    use mirrorleaf_core::Replica;
    use mirrorleaf_net::ledger::{DeviceProfile, LedgerClient};
    use mirrorleaf_net::{Account, BlobStorage, Event, Network, SecretKey, State, SyncNode};
    use mirrorleaf_proto::DeviceEntry;

    let Some(t) = TestLedger::start().await else {
        return;
    };
    t.clock.store(wall_clock(), Ordering::SeqCst);
    let device = |secret: &SecretKey, name: &str| DeviceEntry {
        id: DeviceId(*secret.public().as_bytes()),
        name: name.into(),
    };
    let (laptop_key, phone_key) = (SecretKey::generate(), SecretKey::generate());
    let laptop_account = Account::create(device(&laptop_key, "laptop"));
    let phone_account = Account::join(
        AccountKey::from_bytes(laptop_account.key.as_ref().unwrap().to_bytes()),
        &laptop_account.devices,
        device(&phone_key, "phone"),
    )
    .unwrap();
    let desktop = DeviceProfile {
        platform: Platform::Macos,
        push_token: None,
        notifications: false,
    };
    let phone_profile = DeviceProfile {
        platform: Platform::Android,
        push_token: Some("phone-token".into()),
        notifications: true,
    };
    let phone = LedgerClient::new(&t.url, &phone_key, &phone_account, phone_profile).unwrap();

    // The phone never registered: its first report registers it (and creates
    // the account), as after a ledger database is replaced.
    phone.report(&Default::default(), Some(0)).await.unwrap();

    // The laptop reports through `watch`, at once on starting.
    let laptop_id = DeviceId(*laptop_key.public().as_bytes());
    let mut replica = Replica::open_in_memory(laptop_id).unwrap();
    let note = replica.create_note("Reported").unwrap();
    replica.append_paragraph(&note, "two changes").unwrap();
    let client = LedgerClient::new(&t.url, &laptop_key, &laptop_account, desktop).unwrap();
    let node = SyncNode::bind(
        State {
            replica,
            account: laptop_account,
        },
        laptop_key,
        Network::Local,
        BlobStorage::Memory,
    )
    .await
    .unwrap();
    node.use_ledger(client);
    let (events, mut log) = tokio::sync::mpsc::unbounded_channel();
    {
        let node = node.clone();
        tokio::spawn(async move { node.watch(events).await });
    }
    let response = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            match log.recv().await {
                Some(Event::LedgerReported { response }) => break response,
                Some(Event::LedgerFailed { error }) => panic!("{error}"),
                _ => {}
            }
        }
    })
    .await
    .expect("watch reports within seconds");
    let me = response
        .devices
        .iter()
        .find(|d| d.device == laptop_id)
        .unwrap();
    assert!(me.online);
    let phone_row = response
        .devices
        .iter()
        .find(|d| d.platform == Platform::Android)
        .unwrap();
    assert_eq!(
        phone_row.behind, 3,
        "the note, its paragraph and the index entry's update"
    );

    // Rule 1 woke the phone to pull from the laptop.
    let pushes = t.take_pushes();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].kind, PushKind::Pull { from: laptop_id });

    // With the laptop online, the phone is told to dial it rather than wake anything.
    let wake = phone.wake().await.unwrap();
    assert_eq!(
        wake,
        mirrorleaf_proto::ledger::WakeResponse::HolderOnline { device: laptop_id }
    );

    // Going offline is reported at once.
    node.go_offline().await.unwrap();
    let response = phone.report(&Default::default(), Some(0)).await.unwrap();
    assert!(response.devices.iter().all(|d| !d.online));
    phone.leave().await.unwrap();
    node.close().await;
    t.finish().await;
}
