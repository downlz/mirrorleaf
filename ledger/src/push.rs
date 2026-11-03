//! Sending pushes: FCM HTTP v1 (plan §5.8), or only logging them when no
//! Firebase credentials are configured, or recording them for tests.
//!
//! A push carries only its type (`pull`, `deliver` or `fallback`; plan §5.9):
//! no device IDs, change counts or names, so Google can't link a user's
//! devices or see how much they edit. The woken phone reads the rest from the
//! ledger's status reply. Only the fallback is a notification message, with
//! fixed generic text (ADR-004, gap 3). The ledger's own log keeps the details,
//! because it never leaves our server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use data_encoding::{BASE64, BASE64URL_NOPAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::rules::{Priority, Push, PushKind};

/// The fallback notification (ADR-004, gap 3). It is shown by the system when
/// the app can't run, so it can't name devices: the ledger can't read their names.
pub const FALLBACK_TITLE: &str = "Mirrorleaf";
pub const FALLBACK_BODY: &str = "You have changes waiting to sync. Open Mirrorleaf to send them.";

const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const DEFAULT_FCM_API: &str = "https://fcm.googleapis.com";

/// Why a push wasn't sent.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// FCM no longer knows the token (the app was uninstalled or its data
    /// cleared): the ledger forgets it.
    #[error("the push token is no longer valid")]
    Unregistered,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Clone)]
pub enum Pusher {
    Fcm(Arc<Fcm>),
    /// No Firebase credentials: pushes are only logged (Phase 0 without FCM).
    LogOnly,
    /// Tests: pushes are kept for inspection.
    Record(Arc<Mutex<Vec<Push>>>),
}

impl Pusher {
    pub async fn send(&self, push: &Push) -> Result<(), PushError> {
        match self {
            Pusher::Fcm(fcm) => fcm.send(push).await,
            Pusher::LogOnly => {
                tracing::info!("push (not sent, FCM not configured): {}", describe(push));
                Ok(())
            }
            Pusher::Record(sent) => {
                sent.lock().unwrap().push(push.clone());
                Ok(())
            }
        }
    }
}

/// A push for the log: never the token.
pub fn describe(push: &Push) -> String {
    let priority = match push.priority {
        Priority::Normal => "normal",
        Priority::High => "high",
    };
    let what = match &push.kind {
        PushKind::Pull { from } => format!("pull from {}", from.short()),
        PushKind::Deliver { to, changes } => format!("deliver {changes} changes to {}", to.short()),
        PushKind::Fallback => "fallback notification".to_owned(),
    };
    format!("{} {priority}: {what}", push.to.short())
}

/// The FCM message for a push (HTTP v1 format). The kind's device and change
/// count stay out of it (plan §5.9).
pub fn message(push: &Push) -> Value {
    let priority = match push.priority {
        Priority::Normal => "NORMAL",
        Priority::High => "HIGH",
    };
    let kind = match push.kind {
        PushKind::Pull { .. } => "pull",
        PushKind::Deliver { .. } => "deliver",
        PushKind::Fallback => "fallback",
    };
    let data = json!({ "type": kind });
    match push.kind {
        PushKind::Fallback => json!({
            "token": push.token,
            "notification": { "title": FALLBACK_TITLE, "body": FALLBACK_BODY },
            "data": data,
            "android": { "priority": priority },
        }),
        // One collapse key: if the phone is unreachable, FCM keeps only the
        // latest wake-up rather than a queue of them.
        _ => json!({
            "token": push.token,
            "data": data,
            "android": { "priority": priority, "collapse_key": "sync" },
        }),
    }
}

/// The parts of a Firebase service-account key the ledger uses.
#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

/// An FCM HTTP v1 client, authorised with a service-account key.
pub struct Fcm {
    http: reqwest::Client,
    api: String,
    account: ServiceAccount,
    key: RsaKeyPair,
    token: tokio::sync::Mutex<Option<(String, std::time::Instant)>>,
}

impl Fcm {
    /// `service_account` is the key file's JSON. `api` replaces FCM's address
    /// (for the fake FCM in tests).
    pub fn new(service_account: &str, api: Option<String>) -> Result<Self> {
        let account: ServiceAccount =
            serde_json::from_str(service_account).context("not a service-account key file")?;
        let key = rsa_key(&account.private_key)?;
        Ok(Self {
            http: http_client()?,
            api: api.unwrap_or_else(|| DEFAULT_FCM_API.to_owned()),
            account,
            key,
            token: tokio::sync::Mutex::new(None),
        })
    }

    pub fn project(&self) -> &str {
        &self.account.project_id
    }

    async fn send(&self, push: &Push) -> Result<(), PushError> {
        let token = self.access_token().await?;
        let url = format!(
            "{}/v1/projects/{}/messages:send",
            self.api, self.account.project_id
        );
        let body = serde_json::to_vec(&json!({ "message": message(push) }))
            .map_err(anyhow::Error::from)?;
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(anyhow::Error::from)?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            404 => Err(PushError::Unregistered),
            status => {
                let text = response.text().await.unwrap_or_default();
                Err(anyhow::anyhow!("FCM answered {status}: {text}").into())
            }
        }
    }

    /// An OAuth access token, from Google's token endpoint with a signed JWT
    /// (the service-account flow), reused until shortly before it expires.
    async fn access_token(&self) -> Result<String> {
        let mut cached = self.token.lock().await;
        if let Some((token, expires)) = cached.as_ref()
            && *expires > std::time::Instant::now()
        {
            return Ok(token.clone());
        }
        let assertion = self.jwt()?;
        let body = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
        );
        let response = self
            .http
            .post(&self.account.token_uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            bail!(
                "token request failed ({status}): {}",
                response.text().await.unwrap_or_default()
            );
        }
        #[derive(Deserialize)]
        struct Token {
            access_token: String,
            expires_in: u64,
        }
        let token: Token = serde_json::from_slice(&response.bytes().await?)?;
        let lifetime = Duration::from_secs(token.expires_in.saturating_sub(300));
        *cached = Some((
            token.access_token.clone(),
            std::time::Instant::now() + lifetime,
        ));
        Ok(token.access_token)
    }

    /// The JWT asking for an FCM access token, signed RS256 with the key.
    fn jwt(&self) -> Result<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let header = json!({ "alg": "RS256", "typ": "JWT" });
        let claims = json!({
            "iss": self.account.client_email,
            "scope": FCM_SCOPE,
            "aud": self.account.token_uri,
            "iat": now,
            "exp": now + 3600,
        });
        let signed = format!(
            "{}.{}",
            BASE64URL_NOPAD.encode(header.to_string().as_bytes()),
            BASE64URL_NOPAD.encode(claims.to_string().as_bytes())
        );
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            )
            .map_err(|_| anyhow::anyhow!("couldn't sign the token request"))?;
        Ok(format!("{signed}.{}", BASE64URL_NOPAD.encode(&signature)))
    }
}

/// An HTTPS client using the ring crypto provider and the system's trust store.
pub fn http_client() -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?)
}

/// The RSA key from a service-account file's PEM `private_key` (PKCS #8).
fn rsa_key(pem: &str) -> Result<RsaKeyPair> {
    let base64: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .flat_map(|line| line.trim().chars())
        .collect();
    let der = BASE64
        .decode(base64.as_bytes())
        .context("the service-account private key isn't valid PEM")?;
    RsaKeyPair::from_pkcs8(&der)
        .map_err(|e| anyhow::anyhow!("the service-account private key is unusable: {e}"))
}

#[cfg(test)]
mod tests {
    use mirrorleaf_proto::DeviceId;

    use super::*;

    #[test]
    fn pushes_carry_only_their_type_and_the_fallback_is_generic() {
        let peer = DeviceId([2; 32]);
        let deliver = Push {
            to: DeviceId([1; 32]),
            token: "tok".into(),
            priority: Priority::High,
            kind: PushKind::Deliver {
                to: peer,
                changes: 3,
            },
        };
        let m = message(&deliver);
        assert_eq!(m["android"]["priority"], "HIGH");
        assert_eq!(m["data"], json!({ "type": "deliver" }));
        assert!(m.get("notification").is_none());

        let pull = Push {
            priority: Priority::Normal,
            kind: PushKind::Pull { from: peer },
            ..deliver.clone()
        };
        let m = message(&pull);
        assert_eq!(m["android"]["priority"], "NORMAL");
        assert_eq!(m["data"], json!({ "type": "pull" }));

        let fallback = Push {
            kind: PushKind::Fallback,
            ..deliver
        };
        let m = message(&fallback);
        assert_eq!(m["data"], json!({ "type": "fallback" }));
        assert_eq!(m["notification"]["body"], FALLBACK_BODY);
        assert_eq!(m["android"]["priority"], "HIGH");

        // Nothing anywhere in the messages names a device.
        let deliver = Push {
            kind: PushKind::Deliver {
                to: peer,
                changes: 3,
            },
            ..fallback.clone()
        };
        for m in [message(&deliver), message(&pull), message(&fallback)] {
            let text = m.to_string();
            assert!(!text.contains(&peer.to_string()), "{text}");
            assert!(!text.contains(&DeviceId([1; 32]).to_string()), "{text}");
        }
    }
}
