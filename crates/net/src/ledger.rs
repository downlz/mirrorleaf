use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use iroh::SecretKey;
use mirrorleaf_proto::ledger::{
    Approval, DEVICE_HEADER, DeviceKey, LedgerError, PairingLookup, PairingOffer, Platform,
    Register, SIGNATURE_HEADER, StatusResponse, StatusUpdate, TIME_HEADER, WakeResponse,
    error_code,
};
use mirrorleaf_proto::{AccountId, DeviceId, VersionVector};

use crate::Account;

/// How often an online device checks in (ADR-004, gap 1).
pub const HEARTBEAT: Duration = Duration::from_secs(5 * 60);
/// How long the device in use waits for a woken holder before asking the user
/// to open Mirrorleaf on it (plan §5.9). The ledger's fallback notification on
/// the holder (rule 4) goes out after 60 s, so it gets its chance first.
pub const PROMPT_AFTER: Duration = Duration::from_secs(90);
/// How long each check-in keeps the device online: a little longer than the
/// heartbeat, so one late check-in doesn't drop it.
pub const ONLINE_FOR_SECS: u32 = 6 * 60;
/// Status updates are sent at most this often (plan M3).
pub const MIN_REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// How this device presents itself to the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProfile {
    pub platform: Platform,
    /// The FCM token, on devices a push can wake.
    pub push_token: Option<String>,
    /// Whether the device may show notifications.
    pub notifications: bool,
}

/// The device's ledger client (plan §5.8): registration, status reports and
/// wake requests, each signed with the device key.
pub struct LedgerClient {
    base: String,
    http: reqwest::Client,
    key: DeviceKey,
    account: AccountId,
    approval: String,
    profile: DeviceProfile,
}

impl LedgerClient {
    /// A client for the ledger at `url`. The device registers with the
    /// account key's approval: the one pairing handed over (E1), or one the
    /// device signs itself with the account key it holds.
    pub fn new(
        url: &str,
        secret: &SecretKey,
        account: &Account,
        profile: DeviceProfile,
    ) -> Result<Self> {
        let key = DeviceKey::from_bytes(secret.to_bytes());
        let approval = match (&account.approval, &account.key) {
            (Some(approval), _) => approval.clone(),
            (None, Some(account_key)) => Approval::AddDevice.sign(account_key, &key.id()),
            (None, None) => bail!(
                "this device has neither the account key nor its approval, so it can't register"
            ),
        };
        let http = http_client()?;
        Ok(Self {
            base: url.trim_end_matches('/').to_owned(),
            http,
            key,
            account: account.id(),
            approval,
            profile,
        })
    }

    pub fn url(&self) -> &str {
        &self.base
    }

    pub fn profile(&self) -> &DeviceProfile {
        &self.profile
    }

    /// Adds this device to its account on the ledger, creating the account if
    /// the ledger doesn't know it yet. Harmless to repeat.
    pub async fn register(&self) -> Result<()> {
        let body = serde_json::to_vec(&Register {
            account: self.account,
            approval: self.approval.clone(),
            platform: self.profile.platform,
        })?;
        match self.send("POST", "/v1/devices", body.clone()).await {
            Err(e) if is(&e, error_code::UNKNOWN_ACCOUNT) => {
                self.send("POST", "/v1/accounts", body).await?;
                Ok(())
            }
            other => other.map(drop),
        }
    }

    /// Reports what this device holds. `online_for` is how long to count it as
    /// online (0 when going offline; `None` leaves it as it is). If the ledger
    /// doesn't know the device, for example after its database was replaced,
    /// the device registers again and reports once more.
    pub async fn report(
        &self,
        vv: &VersionVector,
        online_for: Option<u32>,
    ) -> Result<StatusResponse> {
        let body = serde_json::to_vec(&StatusUpdate {
            version_vector: vv.clone(),
            online_for_secs: online_for,
            platform: self.profile.platform,
            push_token: self.profile.push_token.clone(),
            notifications: self.profile.notifications,
        })?;
        let response = match self
            .send("PUT", "/v1/devices/me/status", body.clone())
            .await
        {
            Err(e) if is(&e, error_code::UNKNOWN_DEVICE) => {
                self.register().await?;
                self.send("PUT", "/v1/devices/me/status", body).await?
            }
            other => other?,
        };
        Ok(serde_json::from_slice(&response)?)
    }

    /// Asks the ledger to wake the device holding what this one lacks (plan
    /// §5.9, rule 3): called when the app is opened while behind.
    pub async fn wake(&self) -> Result<WakeResponse> {
        let response = self.send("POST", "/v1/wake", b"{}".to_vec()).await?;
        Ok(serde_json::from_slice(&response)?)
    }

    /// Removes this device from the ledger.
    pub async fn leave(&self) -> Result<()> {
        let path = format!("/v1/devices/{}", self.key.id());
        self.send("DELETE", &path, b"{}".to_vec()).await.map(drop)
    }

    /// Asks for a short number pointing to this device for the next 10
    /// minutes, to start a word code with (E1). Replaces this device's earlier
    /// number, if any.
    pub async fn offer_pairing(&self) -> Result<PairingOffer> {
        let response = self.send("POST", "/v1/pairing", b"{}".to_vec()).await?;
        Ok(serde_json::from_slice(&response)?)
    }

    /// Withdraws this device's pairing number once it has been used or
    /// cancelled.
    pub async fn cancel_pairing(&self, nameplate: u32) -> Result<()> {
        let path = format!("/v1/pairing/{nameplate}");
        self.send("DELETE", &path, b"{}".to_vec()).await.map(drop)
    }

    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<Vec<u8>> {
        send(&self.http, &self.base, &self.key, method, path, body).await
    }
}

/// Which device a word code's number points to (E1). A new device asks this
/// before it belongs to any account: the request is signed with its own key,
/// which the ledger needn't know.
pub async fn lookup_pairing(url: &str, secret: &SecretKey, nameplate: u32) -> Result<DeviceId> {
    let key = DeviceKey::from_bytes(secret.to_bytes());
    let base = url.trim_end_matches('/');
    let path = format!("/v1/pairing/{nameplate}");
    match send(&http_client()?, base, &key, "GET", &path, Vec::new()).await {
        Ok(body) => Ok(serde_json::from_slice::<PairingLookup>(&body)?.device),
        Err(e) if is(&e, error_code::NOT_FOUND) => {
            bail!(
                "no device is showing a code starting with {nameplate}: check the number, or make a new code"
            )
        }
        Err(e) => Err(e),
    }
}

fn http_client() -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?)
}

/// Sends a request signed with `key` and returns the response body.
async fn send(
    http: &reqwest::Client,
    base: &str,
    key: &DeviceKey,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<Vec<u8>> {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let signature = key.sign_request(method, path, time, &body);
    let response = http
        .request(method.parse()?, format!("{base}{path}"))
        .header(DEVICE_HEADER, key.id().to_string())
        .header(TIME_HEADER, time.to_string())
        .header(SIGNATURE_HEADER, signature)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .with_context(|| format!("can't reach the ledger at {base}"))?;
    let status = response.status();
    let bytes = response.bytes().await?.to_vec();
    if status.is_success() {
        return Ok(bytes);
    }
    match serde_json::from_slice::<LedgerError>(&bytes) {
        Ok(error) => Err(Refused(error).into()),
        Err(_) => bail!("the ledger answered {status}"),
    }
}

/// The fallback that needs no push (plan §5.9): after a wake request, when the
/// device in use should show "Open Mirrorleaf on <name> to sync", with the
/// name from its own device list. It works the same whether or not FCM can
/// reach the holder, so it covers phones without Google Play services, failed
/// pushes and FCM outages.
///
/// Apps make one with [`Fallback::after_wake`], then call [`Fallback::due`]
/// as changes arrive or on a timer. It never contacts the ledger again: it
/// compares what the holder held with what this device holds now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fallback {
    /// The device to name in the prompt.
    pub holder: DeviceId,
    /// What the holder held when the ledger last reported it, if it did.
    held: Option<VersionVector>,
    /// When to prompt if the holder's changes haven't arrived: at once if no
    /// push could reach it, otherwise after [`PROMPT_AFTER`].
    at: Instant,
}

impl Fallback {
    /// The fallback for the ledger's answer to a wake request, or `None` when
    /// there is nothing to wait for (up to date, or the holder is online and
    /// gets dialled). `status` is the ledger's reply to the report sent just
    /// before the wake request.
    pub fn after_wake(wake: &WakeResponse, status: &StatusResponse, now: Instant) -> Option<Self> {
        let (holder, at) = match wake {
            WakeResponse::UpToDate | WakeResponse::HolderOnline { .. } => return None,
            WakeResponse::NoWakeableHolder { device } => (*device, now),
            WakeResponse::Woken { device, .. } | WakeResponse::AlreadyWaking { device } => {
                (*device, now + PROMPT_AFTER)
            }
        };
        let held = status
            .devices
            .iter()
            .find(|d| d.device == holder)
            .map(|d| d.version_vector.clone());
        Some(Self { holder, held, at })
    }

    /// Whether everything the holder held has arrived here.
    pub fn arrived(&self, mine: &VersionVector) -> bool {
        self.held
            .as_ref()
            .is_some_and(|held| held.ahead_of(mine) == 0)
    }

    /// Whether to show the prompt now, given what this device holds.
    pub fn due(&self, mine: &VersionVector, now: Instant) -> bool {
        !self.arrived(mine) && now >= self.at
    }

    /// When the prompt becomes due if nothing arrives.
    pub fn at(&self) -> Instant {
        self.at
    }
}

/// The ledger refused a request.
#[derive(Debug, thiserror::Error)]
#[error("the ledger refused: {} ({})", .0.message, .0.error)]
pub struct Refused(pub LedgerError);

fn is(error: &anyhow::Error, code: &str) -> bool {
    error
        .downcast_ref::<Refused>()
        .is_some_and(|r| r.0.error == code)
}

#[cfg(test)]
mod tests {
    use mirrorleaf_proto::ledger::DeviceStatus;

    use super::*;

    fn id(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    /// A vector holding `held[i]` changes from device i+1.
    fn vv(held: &[u64]) -> VersionVector {
        VersionVector(
            held.iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(i, n)| (id(i as u8 + 1), *n))
                .collect(),
        )
    }

    /// The ledger's view: the laptop (1) holds 5 of its own; the phone (2)
    /// also holds 3 of its own.
    fn status() -> StatusResponse {
        let row = |n: u8, platform, held: &[u64]| DeviceStatus {
            device: id(n),
            platform,
            version_vector: vv(held),
            behind: 0,
            online: false,
            last_seen: 0,
        };
        StatusResponse {
            devices: vec![
                row(1, Platform::Macos, &[5]),
                row(2, Platform::Android, &[5, 3]),
            ],
        }
    }

    #[test]
    fn nothing_to_wait_for_when_up_to_date_or_the_holder_is_online() {
        let now = Instant::now();
        let online = WakeResponse::HolderOnline { device: id(2) };
        assert_eq!(Fallback::after_wake(&online, &status(), now), None);
        assert_eq!(
            Fallback::after_wake(&WakeResponse::UpToDate, &status(), now),
            None
        );
    }

    #[test]
    fn a_holder_no_push_can_reach_is_named_at_once() {
        let now = Instant::now();
        let none = WakeResponse::NoWakeableHolder { device: id(2) };
        let fallback = Fallback::after_wake(&none, &status(), now).unwrap();
        assert_eq!(fallback.holder, id(2));
        assert!(fallback.due(&vv(&[5]), now));
        // The user opened the phone and its changes arrived: no prompt.
        assert!(!fallback.due(&vv(&[5, 3]), now));
    }

    #[test]
    fn a_woken_holder_gets_ninety_seconds_before_the_prompt() {
        let now = Instant::now();
        for wake in [
            WakeResponse::Woken {
                device: id(2),
                changes: 3,
                high_priority: true,
            },
            WakeResponse::AlreadyWaking { device: id(2) },
        ] {
            let fallback = Fallback::after_wake(&wake, &status(), now).unwrap();
            let mine = vv(&[5]);
            assert!(!fallback.due(&mine, now + PROMPT_AFTER - Duration::from_secs(1)));
            assert!(fallback.due(&mine, now + PROMPT_AFTER));
            // Only part arrived: still prompt. All of it: no prompt.
            assert!(fallback.due(&vv(&[5, 2]), now + PROMPT_AFTER));
            assert!(fallback.arrived(&vv(&[5, 3])));
            assert!(!fallback.due(&vv(&[5, 3]), now + PROMPT_AFTER));
        }
    }
}
