//! mirrorleaf-ledger: the sync-status service (implementation plan §5.8–§5.9).
//! It holds which devices an account has, how many changes each holds, whether
//! each is online and how to wake it. Never note content, titles or photos.
//!
//! No IP addresses are logged: nothing here records a peer address, and
//! request logging stays off.

pub mod db;
pub mod limits;
pub mod push;
pub mod rules;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use mirrorleaf_proto::ledger::{
    self, Approval, DeviceStatus, LedgerError, PairingLookup, PairingOffer, Register, Rekey,
    RemoveDevice, StatusResponse, StatusUpdate, WakeResponse,
};
use mirrorleaf_proto::pair::CODE_LIFETIME_SECS;
use mirrorleaf_proto::{AccountId, DeviceId};
use sqlx_postgres::PgPool;

use crate::limits::Limits;
use crate::push::{PushError, Pusher};
use crate::rules::{Device, Push, WakeDecision};

/// The current Unix time. Tests replace it to move through the rules' timeouts.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    })
}

/// Largest request body accepted.
const MAX_BODY: usize = 64 * 1024;

/// How long `/health` waits for the database.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct Ledger {
    pub db: PgPool,
    pub pusher: Pusher,
    pub clock: Clock,
    pub limits: Arc<Limits>,
}

impl Ledger {
    pub fn router(self) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/v1/accounts", post(create_account))
            .route("/v1/accounts/me", delete(delete_account))
            .route("/v1/accounts/rekey", post(rekey))
            .route("/v1/devices", post(add_device))
            .route("/v1/devices/me/status", put(status))
            .route("/v1/devices/{id}", delete(remove_device))
            .route("/v1/wake", post(wake))
            .route("/v1/pairing", post(offer_pairing))
            .route(
                "/v1/pairing/{nameplate}",
                get(lookup_pairing).delete(cancel_pairing),
            )
            .with_state(self)
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Sends pushes decided in a committed transaction.
    async fn send(&self, pushes: &[Push]) {
        for push in pushes {
            self.send_one(push).await;
        }
    }

    /// Sends one push and says whether FCM took it (log-only mode counts as
    /// taken). A token FCM rejects is forgotten; other failures are logged.
    async fn send_one(&self, push: &Push) -> bool {
        tracing::info!("push {}", push::describe(push));
        match self.pusher.send(push).await {
            Ok(()) => true,
            Err(PushError::Unregistered) => {
                tracing::info!(
                    "forgetting {}'s push token: FCM refuses it",
                    push.to.short()
                );
                if let Err(e) = db::forget_token(&self.db, &push.to, &push.token).await {
                    tracing::warn!("couldn't forget a push token: {e:#}");
                }
                false
            }
            Err(PushError::Other(e)) => {
                tracing::warn!("push to {} failed: {e:#}", push.to.short());
                false
            }
        }
    }

    /// Rule 4 (plan §5.9): follows up wake-ups that went unanswered, and
    /// settles answered ones. Run every few seconds.
    pub async fn tick(&self) -> anyhow::Result<()> {
        for account in db::accounts_with_wakes(&self.db).await? {
            let mut tx = self.db.begin().await?;
            let now = self.now();
            let rows = db::lock_account(&mut tx, &account).await?;
            let devices: Vec<Device> = rows.into_iter().map(|r| r.device).collect();
            let mut pushes = Vec::new();
            for holder in devices.iter().filter(|d| d.wake.is_some()) {
                let (push, settled) = rules::fallback(&devices, holder, now);
                if settled {
                    db::clear_wake(&mut tx, &holder.id).await?;
                }
                pushes.extend(push);
            }
            tx.commit().await?;
            self.send(&pushes).await;
        }
        Ok(())
    }

    /// Removes devices not seen for 90 days (assessment §7).
    pub async fn prune(&self) -> anyhow::Result<u64> {
        db::prune(&self.db, self.now()).await
    }
}

/// An error answer: a status and a code the device can act on.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn unauthorised(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "bad_signature", message)
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            ledger::error_code::RATE_LIMITED,
            message,
        )
    }

    fn unknown_device() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            ledger::error_code::UNKNOWN_DEVICE,
            "this device isn't registered",
        )
    }

    fn account_moved(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            ledger::error_code::ACCOUNT_MOVED,
            message,
        )
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("{e:#}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
}

impl From<sqlx_core::Error> for ApiError {
    fn from(e: sqlx_core::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = LedgerError {
            error: self.code.to_owned(),
            message: self.message,
        };
        (self.status, Json(body)).into_response()
    }
}

/// A request whose signature checked out: the device that signed it and the
/// body it signed (plan §5.8).
pub struct Signed {
    pub device: DeviceId,
    pub body: Bytes,
}

impl Signed {
    fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, ApiError> {
        serde_json::from_slice(&self.body).map_err(|e| ApiError::bad_request(e.to_string()))
    }
}

impl FromRequest<Ledger> for Signed {
    type Rejection = ApiError;

    async fn from_request(req: Request, ledger: &Ledger) -> Result<Self, Self::Rejection> {
        let method = req.method().as_str().to_owned();
        let path = req.uri().path().to_owned();
        let (device, time, signature) = {
            let header = |name: &str| {
                req.headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
                    .ok_or_else(|| ApiError::unauthorised(format!("missing {name}")))
            };
            let device: DeviceId = header(ledger::DEVICE_HEADER)?
                .parse()
                .map_err(|_| ApiError::unauthorised("malformed device ID"))?;
            let time: i64 = header(ledger::TIME_HEADER)?
                .parse()
                .map_err(|_| ApiError::unauthorised("malformed time"))?;
            (device, time, header(ledger::SIGNATURE_HEADER)?)
        };
        if (time - ledger.now()).abs() > ledger::MAX_CLOCK_SKEW_SECS {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "clock_skew",
                "the request's time is more than 5 minutes off; check the device's clock",
            ));
        }
        let body = axum::body::to_bytes(req.into_body(), MAX_BODY)
            .await
            .map_err(|_| ApiError::bad_request("body too large"))?;
        ledger::verify_request(&device, &method, &path, time, &body, &signature)
            .map_err(|_| ApiError::unauthorised("the signature doesn't match"))?;
        // Counted only once the signature checks out, so nobody can use up
        // another device's allowance.
        if !ledger.limits.device_request(&device) {
            return Err(ApiError::rate_limited(
                "too many requests from this device; try again in a minute",
            ));
        }
        Ok(Signed { device, body })
    }
}

/// `POST /v1/pairing` (E1): a number pointing to the signer for 10 minutes, to
/// start a word code with. Only a registered device may ask: it is already on
/// an account.
async fn offer_pairing(
    State(l): State<Ledger>,
    req: Signed,
) -> Result<Json<PairingOffer>, ApiError> {
    if db::account_of(&l.db, &req.device).await?.is_none() {
        return Err(ApiError::unknown_device());
    }
    let now = l.now();
    let expires_at = now + CODE_LIFETIME_SECS as i64;
    match db::offer_pairing(&l.db, &req.device, now, expires_at).await? {
        Some(nameplate) => Ok(Json(PairingOffer {
            nameplate,
            expires_at,
        })),
        None => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "too many devices are pairing right now; try again in a few minutes",
        )),
    }
}

/// `GET /v1/pairing/{nameplate}` (E1): the device a number points to. Any
/// signer may ask, registered or not: the new device hasn't joined yet. The
/// answer is a device ID only; the word code's words, the PAKE password, never
/// reach the ledger.
async fn lookup_pairing(
    State(l): State<Ledger>,
    Path(nameplate): Path<u32>,
    _req: Signed,
) -> Result<Json<PairingLookup>, ApiError> {
    match db::lookup_pairing(&l.db, nameplate, l.now()).await? {
        Some(device) => Ok(Json(PairingLookup { device })),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            ledger::error_code::NOT_FOUND,
            "no device is showing a code with that number",
        )),
    }
}

/// `DELETE /v1/pairing/{nameplate}` (E1): the signer withdraws its number.
async fn cancel_pairing(
    State(l): State<Ledger>,
    Path(nameplate): Path<u32>,
    req: Signed,
) -> Result<StatusCode, ApiError> {
    db::cancel_pairing(&l.db, &req.device, nameplate).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /health`: "ok" while the database answers, 503 otherwise. Unsigned,
/// for uptime monitors and the reverse proxy.
async fn health(State(l): State<Ledger>) -> (StatusCode, &'static str) {
    match tokio::time::timeout(HEALTH_TIMEOUT, db::ping(&l.db)).await {
        Ok(Ok(())) => (StatusCode::OK, "ok"),
        Ok(Err(e)) => {
            tracing::warn!("health check: the database didn't answer: {e:#}");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
        Err(_) => {
            tracing::warn!("health check: the database took over {HEALTH_TIMEOUT:?}");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}

/// `POST /v1/accounts`: a new account, with the signer as a device.
async fn create_account(State(l): State<Ledger>, req: Signed) -> Result<StatusCode, ApiError> {
    register(&l, req, true).await
}

/// `POST /v1/devices`: the signer joins an existing account.
async fn add_device(State(l): State<Ledger>, req: Signed) -> Result<StatusCode, ApiError> {
    register(&l, req, false).await
}

async fn register(l: &Ledger, req: Signed, create: bool) -> Result<StatusCode, ApiError> {
    let body: Register = req.json()?;
    Approval::AddDevice
        .verify(&body.account, &req.device, &body.approval)
        .map_err(|_| ApiError::unauthorised("the account key didn't approve this device"))?;
    // A replaced key, and the recovery kit it came from, stays retired (E3).
    if db::moved_to(&l.db, &body.account).await?.is_some() {
        return Err(ApiError::account_moved(
            "this account's key was replaced: use the newest recovery kit, or add this device from one of your devices",
        ));
    }
    let mut tx = l.db.begin().await?;
    let now = l.now();
    let exists = db::account_exists(&mut tx, &body.account).await?;
    if !exists && !create {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            ledger::error_code::UNKNOWN_ACCOUNT,
            "no such account",
        ));
    }
    if !exists {
        // Only accounts the ledger doesn't hold yet count: devices repeat
        // this request harmlessly, for example after the database was replaced.
        if !l.limits.new_account() {
            return Err(ApiError::rate_limited(
                "too many new accounts right now; try again in a minute",
            ));
        }
        db::create_account(&mut tx, &body.account, now).await?;
    }
    match db::upsert_device(&mut tx, &body.account, &req.device, body.platform, now).await? {
        db::Upsert::Added | db::Upsert::Refreshed => {}
        db::Upsert::OtherAccount => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "other_account",
                "this device belongs to another account",
            ));
        }
        db::Upsert::AccountFull => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                ledger::error_code::TOO_MANY_DEVICES,
                format!(
                    "this account already has {} devices; remove one first",
                    ledger::MAX_DEVICES_PER_ACCOUNT
                ),
            ));
        }
    }
    tx.commit().await?;
    tracing::info!("device {} registered", req.device.short());
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /v1/devices/me/status`: counters, presence and push token. Evaluates
/// rules 1 and 2, and answers with every device of the account.
async fn status(State(l): State<Ledger>, req: Signed) -> Result<Json<StatusResponse>, ApiError> {
    let update: StatusUpdate = req.json()?;
    let account = db::account_of(&l.db, &req.device)
        .await?
        .ok_or_else(ApiError::unknown_device)?;
    let mut tx = l.db.begin().await?;
    let now = l.now();
    let mut rows = db::lock_account(&mut tx, &account).await?;
    let Some(mine) = rows.iter_mut().find(|r| r.device.id == req.device) else {
        return Err(ApiError::unknown_device());
    };
    let before = mine.device.clone();
    let me = &mut mine.device;
    me.vv = update.version_vector;
    me.platform = update.platform;
    me.push_token = update.push_token;
    me.notifications = update.notifications;
    if let Some(secs) = update.online_for_secs {
        me.online_until = now + i64::from(secs.min(ledger::MAX_ONLINE_SECS));
    }
    mine.last_seen = now - now.rem_euclid(60);
    db::write_status(&mut tx, &mine.device, now).await?;

    let devices: Vec<Device> = rows.iter().map(|r| r.device.clone()).collect();
    let pushes = rules::after_status(&devices, req.device, Some(&before), now);
    for push in &pushes {
        db::mark_pushed(&mut tx, &push.to, now).await?;
    }
    tx.commit().await?;
    l.send(&pushes).await;

    let response = StatusResponse {
        devices: rows
            .iter()
            .map(|r| DeviceStatus {
                device: r.device.id,
                platform: r.device.platform,
                version_vector: r.device.vv.clone(),
                behind: rules::behind(&devices, &r.device),
                online: r.device.online(now),
                last_seen: r.last_seen,
            })
            .collect(),
        account: Some(account),
    };
    Ok(Json(response))
}

/// `POST /v1/accounts/rekey` (E3): the signer's account moves to a new key.
/// The old key must have signed the new account ID; the devices on the new
/// list move with it and the rest go. Of two devices replacing the key at
/// once, the first wins; the second is told the account moved.
async fn rekey(State(l): State<Ledger>, req: Signed) -> Result<StatusCode, ApiError> {
    let body: Rekey = req.json()?;
    let account = db::account_of(&l.db, &req.device)
        .await?
        .ok_or_else(ApiError::unknown_device)?;
    if account == body.new_account {
        return Ok(StatusCode::NO_CONTENT);
    }
    if body.verify_link(account).is_err() {
        // A second device replacing the key it knew, after the first already
        // moved the account (and this device with it).
        if let Some(old) = db::moved_from(&l.db, &account).await?
            && body.verify_link(old).is_ok()
        {
            return Err(ApiError::account_moved(format!(
                "another device replaced this account's key first (account {})",
                account.short()
            )));
        }
        return Err(ApiError::unauthorised(
            "the old account key didn't sign the new account",
        ));
    }
    if !body.devices.contains(&req.device) {
        return Err(ApiError::bad_request(
            "the new account's devices must include the device asking",
        ));
    }
    if body.devices.len() as i64 > ledger::MAX_DEVICES_PER_ACCOUNT {
        return Err(ApiError::bad_request("too many devices for one account"));
    }
    match db::rekey(
        &l.db,
        &req.device,
        &account,
        &body.new_account,
        &body.devices,
        l.now(),
    )
    .await?
    {
        db::Rekeyed::Moved => {
            tracing::info!("an account moved to a new key");
            Ok(StatusCode::NO_CONTENT)
        }
        db::Rekeyed::AlreadyDone => Ok(StatusCode::NO_CONTENT),
        db::Rekeyed::MovedElsewhere(to) => Err(ApiError::account_moved(format!(
            "another device replaced this account's key first (account {})",
            to.short()
        ))),
        db::Rekeyed::UnknownDevice => Err(ApiError::unknown_device()),
        db::Rekeyed::WrongAccount => Err(ApiError::account_moved(
            "this device's account changed while the request waited",
        )),
    }
}

/// `POST /v1/wake`: the signer was opened while behind (rule 3). The push goes
/// out before the answer, so a push FCM refuses is answered as "no wakeable
/// holder" and the device in use can prompt the user at once (plan §5.9).
async fn wake(State(l): State<Ledger>, req: Signed) -> Result<Json<WakeResponse>, ApiError> {
    let account = db::account_of(&l.db, &req.device)
        .await?
        .ok_or_else(ApiError::unknown_device)?;
    let mut tx = l.db.begin().await?;
    let now = l.now();
    let devices: Vec<Device> = db::lock_account(&mut tx, &account)
        .await?
        .into_iter()
        .map(|r| r.device)
        .collect();
    let (response, push) = match rules::on_wake(&devices, req.device, now) {
        WakeDecision::UpToDate => (WakeResponse::UpToDate, None),
        WakeDecision::HolderOnline(device) => (WakeResponse::HolderOnline { device }, None),
        WakeDecision::AlreadyWaking(device) => (WakeResponse::AlreadyWaking { device }, None),
        WakeDecision::NoWakeableHolder(device) => (WakeResponse::NoWakeableHolder { device }, None),
        WakeDecision::Wake(push) => {
            db::set_wake(&mut tx, &push.to, &req.device, now).await?;
            let changes = match push.kind {
                rules::PushKind::Deliver { changes, .. } => changes,
                _ => 0,
            };
            let response = WakeResponse::Woken {
                device: push.to,
                changes,
                high_priority: push.priority == rules::Priority::High,
            };
            (response, Some(push))
        }
    };
    tx.commit().await?;
    if let Some(push) = push
        && !l.send_one(&push).await
    {
        // Nothing is on its way, so rule 4 has nothing to follow up. The
        // holder's last_push_at stays, so a failing FCM isn't asked again
        // for two minutes.
        let mut tx = l.db.begin().await?;
        db::clear_wake(&mut tx, &push.to).await?;
        tx.commit().await?;
        return Ok(Json(WakeResponse::NoWakeableHolder { device: push.to }));
    }
    Ok(Json(response))
}

/// `DELETE /v1/devices/{id}`: by the device itself, or by another device of
/// the account with the account key's approval.
async fn remove_device(
    State(l): State<Ledger>,
    Path(id): Path<String>,
    req: Signed,
) -> Result<StatusCode, ApiError> {
    let target: DeviceId = id
        .parse()
        .map_err(|_| ApiError::bad_request("malformed device ID"))?;
    let account: AccountId = db::account_of(&l.db, &req.device)
        .await?
        .ok_or_else(ApiError::unknown_device)?;
    if target != req.device {
        let body: RemoveDevice = req.json()?;
        Approval::RemoveDevice
            .verify(&account, &target, &body.approval)
            .map_err(|_| ApiError::unauthorised("the account key didn't approve this removal"))?;
    }
    if !db::delete_device(&l.db, &account, &target).await? {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such device in this account",
        ));
    }
    tracing::info!("device {} removed", target.short());
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v1/accounts/me`: the signer's account and all its rows.
async fn delete_account(State(l): State<Ledger>, req: Signed) -> Result<StatusCode, ApiError> {
    let account = db::account_of(&l.db, &req.device)
        .await?
        .ok_or_else(ApiError::unknown_device)?;
    db::delete_account(&l.db, &account).await?;
    tracing::info!("an account was deleted");
    Ok(StatusCode::NO_CONTENT)
}
