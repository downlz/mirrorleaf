use std::time::Duration;

use anyhow::{Context, Result, bail};
use iroh::SecretKey;
use mirrorleaf_proto::ledger::{
    Approval, DEVICE_HEADER, DeviceKey, LedgerError, Platform, Register, SIGNATURE_HEADER,
    StatusResponse, StatusUpdate, TIME_HEADER, WakeResponse, error_code,
};
use mirrorleaf_proto::{AccountId, VersionVector};

use crate::Account;

/// How often an online device checks in (ADR-004, gap 1).
pub const HEARTBEAT: Duration = Duration::from_secs(5 * 60);
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
    /// A client for the ledger at `url`. In Phase 0 every device of a developer
    /// account holds the account key, so the device approves itself; real
    /// pairing (E1) hands the new device an approval instead.
    pub fn new(
        url: &str,
        secret: &SecretKey,
        account: &Account,
        profile: DeviceProfile,
    ) -> Result<Self> {
        let key = DeviceKey::from_bytes(secret.to_bytes());
        let account_key = account
            .key
            .as_ref()
            .context("this device doesn't hold the account key, so it can't register itself")?;
        let approval = Approval::AddDevice.sign(account_key, &key.id());
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
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

    /// Sends a signed request and returns the response body.
    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<Vec<u8>> {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let signature = self.key.sign_request(method, path, time, &body);
        let response = self
            .http
            .request(method.parse()?, format!("{}{path}", self.base))
            .header(DEVICE_HEADER, self.key.id().to_string())
            .header(TIME_HEADER, time.to_string())
            .header(SIGNATURE_HEADER, signature)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .with_context(|| format!("can't reach the ledger at {}", self.base))?;
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
