//! The ledger's HTTP API (implementation plan §5.8): request signing, device
//! approval, and the JSON bodies, shared by the devices and the ledger.
//!
//! Every request is signed with the device's key, which is also its Iroh key,
//! so the ledger knows which device sent it. The ledger stores sync status only:
//! counters, presence and push tokens. Device names stay on the devices.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use data_encoding::HEXLOWER;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{AccountId, AccountKey, DeviceId, ProtoError, VersionVector};

/// Request headers carrying the signature.
pub const DEVICE_HEADER: &str = "x-mirrorleaf-device";
pub const TIME_HEADER: &str = "x-mirrorleaf-time";
pub const SIGNATURE_HEADER: &str = "x-mirrorleaf-signature";

/// Requests stamped further than this from the ledger's clock are refused (plan §5.8).
pub const MAX_CLOCK_SKEW_SECS: i64 = 5 * 60;

/// Longest a device may say it stays online for: the heartbeat is every 5
/// minutes and each one keeps the device online for 6 (ADR-004, gap 1).
pub const MAX_ONLINE_SECS: u32 = 10 * 60;

const REQUEST_CONTEXT: &[u8] = b"mirrorleaf/ledger-request/1";
const ADD_DEVICE_CONTEXT: &[u8] = b"mirrorleaf/approve-device/1";
const REMOVE_DEVICE_CONTEXT: &[u8] = b"mirrorleaf/remove-device/1";

/// What a device signs: the method, path, time and the body's BLAKE3 hash.
fn request_message(method: &str, path: &str, time: i64, body: &[u8]) -> Vec<u8> {
    let mut msg = REQUEST_CONTEXT.to_vec();
    for part in [method, path, &time.to_string()] {
        msg.push(b'\n');
        msg.extend(part.as_bytes());
    }
    msg.push(b'\n');
    msg.extend(blake3::hash(body).as_bytes());
    msg
}

/// A device's private key, as used for ledger requests. It is the same key as
/// the device's Iroh key.
pub struct DeviceKey(SigningKey);

impl DeviceKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&bytes))
    }

    pub fn id(&self) -> DeviceId {
        DeviceId(self.0.verifying_key().to_bytes())
    }

    /// The signature header's value for this request.
    pub fn sign_request(&self, method: &str, path: &str, time: i64, body: &[u8]) -> String {
        let message = request_message(method, path, time, body);
        HEXLOWER.encode(&self.0.sign(&message).to_bytes())
    }
}

/// Checks a request's signature against the device that claims to have sent it.
pub fn verify_request(
    device: &DeviceId,
    method: &str,
    path: &str,
    time: i64,
    body: &[u8],
    signature: &str,
) -> Result<(), ProtoError> {
    verify(
        &device.0,
        &request_message(method, path, time, body),
        signature,
    )
}

/// Why the account key signed something about a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    /// The device may join the account on the ledger.
    AddDevice,
    /// The device may be removed from the account on the ledger.
    RemoveDevice,
}

impl Approval {
    fn message(self, account: &AccountId, device: &DeviceId) -> Vec<u8> {
        let mut msg = match self {
            Approval::AddDevice => ADD_DEVICE_CONTEXT,
            Approval::RemoveDevice => REMOVE_DEVICE_CONTEXT,
        }
        .to_vec();
        msg.extend(account.0);
        msg.extend(device.0);
        msg
    }

    /// The account key's signature, in hex. The ledger is given this rather
    /// than the signed device list, which carries device names (ADR-004).
    pub fn sign(self, key: &AccountKey, device: &DeviceId) -> String {
        let message = self.message(&key.id(), device);
        HEXLOWER.encode(&key.signing_key().sign(&message).to_bytes())
    }

    pub fn verify(
        self,
        account: &AccountId,
        device: &DeviceId,
        signature: &str,
    ) -> Result<(), ProtoError> {
        verify(&account.0, &self.message(account, device), signature)
    }
}

fn verify(public_key: &[u8; 32], message: &[u8], signature_hex: &str) -> Result<(), ProtoError> {
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| ProtoError::BadSignature)?;
    let signature: [u8; 64] = HEXLOWER
        .decode(signature_hex.as_bytes())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(ProtoError::BadSignature)?;
    key.verify_strict(message, &Signature::from_bytes(&signature))
        .map_err(|_| ProtoError::BadSignature)
}

/// What kind of device this is: it decides how the ledger can wake it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Android,
    Macos,
    Linux,
    Windows,
    Ios,
}

impl Platform {
    /// The platform this program was built for.
    pub fn this() -> Self {
        if cfg!(target_os = "android") {
            Platform::Android
        } else if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "ios") {
            Platform::Ios
        } else {
            Platform::Linux
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Platform::Android => "android",
            Platform::Macos => "macos",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
            Platform::Ios => "ios",
        })
    }
}

impl FromStr for Platform {
    type Err = ProtoError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "android" => Platform::Android,
            "macos" => Platform::Macos,
            "linux" => Platform::Linux,
            "windows" => Platform::Windows,
            "ios" => Platform::Ios,
            _ => return Err(ProtoError::Invalid("platform")),
        })
    }
}

/// `POST /v1/accounts` (a new account with this device) and `POST /v1/devices`
/// (this device joins an account). The device is the request's signer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Register {
    #[serde(with = "hex_id")]
    pub account: AccountId,
    /// [`Approval::AddDevice`] for this device, signed by the account key.
    pub approval: String,
    pub platform: Platform,
}

/// `PUT /v1/devices/me/status`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusUpdate {
    /// The highest change this device holds from each device, its own included.
    #[serde(with = "hex_vector")]
    pub version_vector: VersionVector,
    /// How long to count the device as online from now: the heartbeat's 6
    /// minutes, 0 when it is going offline, or `None` to leave it as it is.
    pub online_for_secs: Option<u32>,
    pub platform: Platform,
    /// The FCM token, for devices that can be woken by a push.
    pub push_token: Option<String>,
    /// Whether the device may show notifications (Android 13+ asks the user).
    pub notifications: bool,
}

/// One device of the account, as the ledger sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    #[serde(with = "hex_id")]
    pub device: DeviceId,
    pub platform: Platform,
    #[serde(with = "hex_vector")]
    pub version_vector: VersionVector,
    /// How many changes it lacks that some device of the account holds.
    pub behind: u64,
    pub online: bool,
    /// Unix time to the minute.
    pub last_seen: i64,
}

/// The ledger's answer to a status update: every device of the account, so the
/// caller can show "behind by N" and decide whom to dial (plan §5.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub devices: Vec<DeviceStatus>,
}

/// The ledger's answer to `POST /v1/wake` (plan §5.9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum WakeResponse {
    /// Nothing to fetch: no device holds changes the caller lacks.
    UpToDate,
    /// A device holding the changes is online: dial it.
    HolderOnline {
        #[serde(with = "hex_id")]
        device: DeviceId,
    },
    /// The device holding the changes was sent a push.
    Woken {
        #[serde(with = "hex_id")]
        device: DeviceId,
        changes: u64,
        high_priority: bool,
    },
    /// It was already woken in the last two minutes.
    AlreadyWaking {
        #[serde(with = "hex_id")]
        device: DeviceId,
    },
    /// No device holding the changes can be woken: they are offline desktops,
    /// phones without a push token, or the push failed. `device` is the
    /// offline device holding the most of what the caller lacks, for the
    /// fallback prompt "Open Mirrorleaf on <name> to sync" (plan §5.9).
    NoWakeableHolder {
        #[serde(with = "hex_id")]
        device: DeviceId,
    },
}

/// `DELETE /v1/devices/{id}` for a device other than the signer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveDevice {
    /// [`Approval::RemoveDevice`] for that device, signed by the account key.
    pub approval: String,
}

/// An error from the ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerError {
    pub error: String,
    pub message: String,
}

/// Error codes a device acts on.
pub mod error_code {
    /// The signer isn't registered: register, then retry.
    pub const UNKNOWN_DEVICE: &str = "unknown_device";
    /// The account doesn't exist yet: create it with `POST /v1/accounts`.
    pub const UNKNOWN_ACCOUNT: &str = "unknown_account";
}

/// Keys as hex strings in JSON. (Their serde form elsewhere is the raw bytes
/// postcard signs, which must not change.)
pub mod hex_id {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<T: fmt::Display, S: Serializer>(id: &T, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(id)
    }

    pub fn deserialize<'de, T: FromStr, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
        let s = String::deserialize(d)?;
        s.parse()
            .map_err(|_| serde::de::Error::custom("invalid hex key"))
    }
}

/// A version vector as a JSON object from hex device ID to change number.
pub mod hex_vector {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(vv: &VersionVector, s: S) -> Result<S::Ok, S::Error> {
        let map: BTreeMap<String, u64> = vv.0.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        map.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VersionVector, D::Error> {
        let map = BTreeMap::<String, u64>::deserialize(d)?;
        map.into_iter()
            .map(|(k, v)| {
                Ok((
                    k.parse()
                        .map_err(|_| serde::de::Error::custom("invalid device ID"))?,
                    v,
                ))
            })
            .collect::<Result<_, _>>()
            .map(VersionVector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_verify_only_as_signed() {
        let key = DeviceKey::from_bytes([3; 32]);
        let sig = key.sign_request("PUT", "/v1/devices/me/status", 1000, b"{}");
        let device = key.id();
        verify_request(&device, "PUT", "/v1/devices/me/status", 1000, b"{}", &sig).unwrap();
        for (method, path, time, body) in [
            ("POST", "/v1/devices/me/status", 1000, &b"{}"[..]),
            ("PUT", "/v1/wake", 1000, b"{}"),
            ("PUT", "/v1/devices/me/status", 1001, b"{}"),
            ("PUT", "/v1/devices/me/status", 1000, b"{ }"),
        ] {
            assert!(verify_request(&device, method, path, time, body, &sig).is_err());
        }
        let other = DeviceKey::from_bytes([4; 32]).id();
        assert!(verify_request(&other, "PUT", "/v1/devices/me/status", 1000, b"{}", &sig).is_err());
    }

    #[test]
    fn approvals_are_bound_to_their_purpose_and_device() {
        let account = AccountKey::from_bytes([5; 32]);
        let device = DeviceId([6; 32]);
        let approval = Approval::AddDevice.sign(&account, &device);
        Approval::AddDevice
            .verify(&account.id(), &device, &approval)
            .unwrap();
        assert!(
            Approval::RemoveDevice
                .verify(&account.id(), &device, &approval)
                .is_err()
        );
        assert!(
            Approval::AddDevice
                .verify(&account.id(), &DeviceId([7; 32]), &approval)
                .is_err()
        );
    }

    #[test]
    fn json_uses_hex_keys() {
        let update = StatusUpdate {
            version_vector: VersionVector([(DeviceId([1; 32]), 4)].into()),
            online_for_secs: Some(360),
            platform: Platform::Android,
            push_token: None,
            notifications: true,
        };
        let json = serde_json::to_string(&update).unwrap();
        assert!(
            json.contains(&format!("\"{}\":4", DeviceId([1; 32]))),
            "{json}"
        );
        assert!(json.contains("\"platform\":\"android\""), "{json}");
        assert_eq!(serde_json::from_str::<StatusUpdate>(&json).unwrap(), update);

        let wake = WakeResponse::Woken {
            device: DeviceId([2; 32]),
            changes: 3,
            high_priority: true,
        };
        let json = serde_json::to_string(&wake).unwrap();
        assert!(json.starts_with("{\"result\":\"woken\""), "{json}");
        assert_eq!(serde_json::from_str::<WakeResponse>(&json).unwrap(), wake);

        let none = WakeResponse::NoWakeableHolder {
            device: DeviceId([3; 32]),
        };
        let json = serde_json::to_string(&none).unwrap();
        assert_eq!(
            json,
            format!(
                "{{\"result\":\"no_wakeable_holder\",\"device\":\"{}\"}}",
                DeviceId([3; 32])
            )
        );
        assert_eq!(serde_json::from_str::<WakeResponse>(&json).unwrap(), none);
    }
}
