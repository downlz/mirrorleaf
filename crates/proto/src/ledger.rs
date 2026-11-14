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
/// The ledger API version a device speaks (M13, E11). Requests without it
/// are version 1 (builds before M13).
pub const API_HEADER: &str = "x-mirrorleaf-api";
/// A random value in every version-2 request, signed with it: no two
/// requests are alike, so the ledger can refuse one sent again (M13, P4).
pub const NONCE_HEADER: &str = "x-mirrorleaf-nonce";
/// The API version this build speaks.
pub const API_VERSION: u32 = 2;
/// The oldest version a ledger still serves: one before its own (N−1).
pub const OLDEST_API_VERSION: u32 = 1;

/// Requests stamped further than this from the ledger's clock are refused (plan §5.8).
pub const MAX_CLOCK_SKEW_SECS: i64 = 5 * 60;

/// Longest a device may say it stays online for: the heartbeat is every 5
/// minutes and each one keeps the device online for 6 (ADR-004, gap 1).
pub const MAX_ONLINE_SECS: u32 = 10 * 60;

/// Most devices one account may register (E0). A device removed with
/// `DELETE /v1/devices/{id}`, or pruned after 90 days unseen, frees its place.
pub const MAX_DEVICES_PER_ACCOUNT: i64 = 10;

const REQUEST_CONTEXT: &[u8] = b"mirrorleaf/ledger-request/1";
const ADD_DEVICE_CONTEXT: &[u8] = b"mirrorleaf/approve-device/1";
const REMOVE_DEVICE_CONTEXT: &[u8] = b"mirrorleaf/remove-device/1";

/// What a device signs: the method, path, time and the body's BLAKE3 hash.
fn request_message(
    method: &str,
    path: &str,
    time: i64,
    nonce: Option<&str>,
    body: &[u8],
) -> Vec<u8> {
    let mut msg = REQUEST_CONTEXT.to_vec();
    for part in [method, path, &time.to_string()] {
        msg.push(b'\n');
        msg.extend(part.as_bytes());
    }
    msg.push(b'\n');
    msg.extend(blake3::hash(body).as_bytes());
    // Version 2 (M13): the nonce follows, so version-1 signatures still check.
    if let Some(nonce) = nonce {
        msg.push(b'\n');
        msg.extend(nonce.as_bytes());
    }
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

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// The signature header's value for this request.
    pub fn sign_request(&self, method: &str, path: &str, time: i64, body: &[u8]) -> String {
        let message = request_message(method, path, time, None, body);
        HEXLOWER.encode(&self.0.sign(&message).to_bytes())
    }

    /// The signature header's value for a version-2 request, with its nonce.
    pub fn sign_request_v2(
        &self,
        method: &str,
        path: &str,
        time: i64,
        nonce: &str,
        body: &[u8],
    ) -> String {
        let message = request_message(method, path, time, Some(nonce), body);
        HEXLOWER.encode(&self.0.sign(&message).to_bytes())
    }
}

/// Checks a request's signature against the device that claims to have sent it.
/// Checks a request's signature against the device that claims to have sent
/// it. `nonce` is the version-2 nonce, `None` for version 1.
pub fn verify_request(
    device: &DeviceId,
    method: &str,
    path: &str,
    time: i64,
    nonce: Option<&str>,
    body: &[u8],
    signature: &str,
) -> Result<(), ProtoError> {
    verify(
        &device.0,
        &request_message(method, path, time, nonce, body),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    /// The sign-up puzzle, solved for this account (M13): needed only to
    /// create an account the ledger doesn't hold yet. Absent before M13.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub puzzle: Option<crate::puzzle::Solution>,
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
    /// Counts since the last report, for the beta's daily totals (M13, E12).
    /// Absent when the person turned them off, and before M13.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<SyncMetrics>,
}

/// What a device counted since its last report (M13, E12). Only counts: no
/// note, title, size or time of day. The ledger adds them to the day's
/// totals and keeps nothing per device.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncMetrics {
    /// Changes from other devices that arrived, by how long after they were
    /// made: under 5 s, 30 s, 5 min, 1 h, 24 h, and later.
    #[serde(default)]
    pub arrivals: [u32; 6],
    /// Connections to other devices made directly, and through the relay.
    #[serde(default)]
    pub direct: u32,
    #[serde(default)]
    pub relayed: u32,
    /// Push-woken jobs (Android) that synced, and that didn't.
    #[serde(default)]
    pub woken_synced: u32,
    #[serde(default)]
    pub woken_failed: u32,
    /// The phone's maker, for wake-ups by maker ("motorola"); none on desktops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maker: Option<String>,
    /// The app's version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl SyncMetrics {
    /// Upper bounds of the arrival bands, in seconds; the last band is open.
    pub const ARRIVAL_BANDS: [u64; 5] = [5, 30, 300, 3600, 86_400];

    /// Counts a change that arrived `secs` after it was made.
    pub fn arrived_after(&mut self, secs: u64) {
        let band = Self::ARRIVAL_BANDS
            .iter()
            .position(|limit| secs < *limit)
            .unwrap_or(5);
        self.arrivals[band] = self.arrivals[band].saturating_add(1);
    }

    pub fn is_empty(&self) -> bool {
        self.arrivals.iter().all(|n| *n == 0)
            && self.direct == 0
            && self.relayed == 0
            && self.woken_synced == 0
            && self.woken_failed == 0
    }

    /// Adds counts kept from a report that didn't go through.
    pub fn add(&mut self, other: &SyncMetrics) {
        for (a, b) in self.arrivals.iter_mut().zip(other.arrivals) {
            *a = a.saturating_add(b);
        }
        self.direct = self.direct.saturating_add(other.direct);
        self.relayed = self.relayed.saturating_add(other.relayed);
        self.woken_synced = self.woken_synced.saturating_add(other.woken_synced);
        self.woken_failed = self.woken_failed.saturating_add(other.woken_failed);
    }
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
    /// The account the ledger files the device under (M10). It differs from
    /// the device's own when another device replaced the account's key and
    /// this one hasn't received the new key yet. Absent from older ledgers.
    #[serde(default, with = "hex_opt_id", skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountId>,
    /// The relay this ledger's devices use (M13). Absent from older ledgers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
}

/// `GET /v1/info` (M13): what this ledger offers. Unsigned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerInfo {
    /// The API versions it serves.
    pub api: Vec<u32>,
    /// The relay its devices use; none when it has no relay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
}

/// `POST /v1/relay-pass` (M13): a short pass to the ledger's relay for a
/// device joining an account by QR code, before it is registered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayPassRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub puzzle: Option<crate::puzzle::Solution>,
}

/// The pass's end, as Unix seconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayPass {
    pub expires_at: i64,
}

/// `POST /v1/accounts/rekey` (E3): the signer's account moves to a new
/// account key. The ledger keeps the rows of `devices` under the new account
/// and drops the account's other devices, and remembers the move, so the old
/// key (and the recovery kit it came from) can't register again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rekey {
    #[serde(with = "hex_id")]
    pub new_account: AccountId,
    /// The old account key's signature over the new account ID, in hex
    /// (`RotationLink`).
    pub link: String,
    /// The devices on the new account's list.
    #[serde(with = "hex_ids")]
    pub devices: Vec<DeviceId>,
}

impl Rekey {
    pub fn new(link: &crate::RotationLink, new_account: AccountId, devices: Vec<DeviceId>) -> Self {
        Self {
            new_account,
            link: HEXLOWER.encode(&link.signature),
            devices,
        }
    }

    /// The link from `previous`, checked against the new account.
    pub fn verify_link(&self, previous: AccountId) -> Result<(), ProtoError> {
        let signature = HEXLOWER
            .decode(self.link.as_bytes())
            .map_err(|_| ProtoError::BadSignature)?;
        crate::RotationLink {
            previous,
            signature,
        }
        .verify(&self.new_account)
    }
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

/// The ledger's answer to `POST /v1/pairing` (E1): a short number that, for
/// the next 10 minutes, points to the inviting device. The number starts a
/// word code ("7-acoustic-planet-river"); the words are the pairing secret and
/// never reach the ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingOffer {
    pub nameplate: u32,
    /// Unix time when the number stops working.
    pub expires_at: i64,
}

/// The ledger's answer to `GET /v1/pairing/{nameplate}`: which device to dial.
/// The signer needn't be registered: a new device asks before it has joined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingLookup {
    #[serde(with = "hex_id")]
    pub device: DeviceId,
}

/// An error from the ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerError {
    pub error: String,
    pub message: String,
    /// With [`error_code::PUZZLE_NEEDED`]: how many leading zero bits the
    /// puzzle's answer needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub puzzle_bits: Option<u32>,
}

/// Error codes a device acts on.
pub mod error_code {
    /// The signer isn't registered: register, then retry.
    pub const UNKNOWN_DEVICE: &str = "unknown_device";
    /// The account doesn't exist yet: create it with `POST /v1/accounts`.
    pub const UNKNOWN_ACCOUNT: &str = "unknown_account";
    /// The account already has [`super::MAX_DEVICES_PER_ACCOUNT`] devices:
    /// remove one first.
    pub const TOO_MANY_DEVICES: &str = "too_many_devices";
    /// Too many requests: try again later (the app's reports are spaced far
    /// enough apart never to hit this).
    pub const RATE_LIMITED: &str = "rate_limited";
    /// No pairing offer has that number, or it ran out.
    pub const NOT_FOUND: &str = "not_found";
    /// The account's key was replaced (E3): the old key can't register, and a
    /// second replacement of the same account is refused.
    pub const ACCOUNT_MOVED: &str = "account_moved";
    /// Solve the puzzle ([`super::LedgerError::puzzle_bits`]) and send the
    /// request again with its answer (M13).
    pub const PUZZLE_NEEDED: &str = "puzzle_needed";
    /// The ledger blocked this account (M13).
    pub const BLOCKED: &str = "blocked";
    /// The ledger doesn't speak this API version: update the app (M13).
    pub const UNSUPPORTED_API: &str = "unsupported_api";
    /// The same signed request came twice (M13).
    pub const REPLAYED: &str = "replayed";
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

/// An optional key as a hex string in JSON.
pub mod hex_opt_id {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<T: fmt::Display, S: Serializer>(
        id: &Option<T>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match id {
            Some(id) => s.collect_str(id),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: FromStr, D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| {
                s.parse()
                    .map_err(|_| serde::de::Error::custom("invalid hex key"))
            })
            .transpose()
    }
}

/// A list of keys as hex strings in JSON.
pub mod hex_ids {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<T: fmt::Display, S: Serializer>(ids: &[T], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(ids.iter().map(ToString::to_string))
    }

    pub fn deserialize<'de, T: FromStr, D: Deserializer<'de>>(d: D) -> Result<Vec<T>, D::Error> {
        Vec::<String>::deserialize(d)?
            .into_iter()
            .map(|s| {
                s.parse()
                    .map_err(|_| serde::de::Error::custom("invalid hex key"))
            })
            .collect()
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
        verify_request(
            &device,
            "PUT",
            "/v1/devices/me/status",
            1000,
            None,
            b"{}",
            &sig,
        )
        .unwrap();
        for (method, path, time, body) in [
            ("POST", "/v1/devices/me/status", 1000, &b"{}"[..]),
            ("PUT", "/v1/wake", 1000, b"{}"),
            ("PUT", "/v1/devices/me/status", 1001, b"{}"),
            ("PUT", "/v1/devices/me/status", 1000, b"{ }"),
        ] {
            assert!(verify_request(&device, method, path, time, None, body, &sig).is_err());
        }
        let other = DeviceKey::from_bytes([4; 32]).id();
        assert!(
            verify_request(
                &other,
                "PUT",
                "/v1/devices/me/status",
                1000,
                None,
                b"{}",
                &sig
            )
            .is_err()
        );

        // Version 2 signs the nonce too, and a version-1 check refuses it.
        let v2 = key.sign_request_v2("PUT", "/v1/devices/me/status", 1000, "ab12", b"{}");
        let check = |nonce| {
            verify_request(
                &device,
                "PUT",
                "/v1/devices/me/status",
                1000,
                nonce,
                b"{}",
                &v2,
            )
        };
        check(Some("ab12")).unwrap();
        assert!(check(Some("ab13")).is_err());
        assert!(check(None).is_err());
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
            metrics: None,
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

    #[test]
    fn rekeying_carries_a_checkable_link() {
        let old = AccountKey::from_bytes([1; 32]);
        let new = AccountKey::from_bytes([2; 32]);
        let rekey = Rekey::new(
            &old.rotation_link(&new.id()),
            new.id(),
            vec![DeviceId([3; 32])],
        );
        let json = serde_json::to_string(&rekey).unwrap();
        assert!(json.contains(&DeviceId([3; 32]).to_string()), "{json}");
        let back: Rekey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rekey);
        back.verify_link(old.id()).unwrap();
        assert!(back.verify_link(new.id()).is_err());

        // Older ledgers send no account in their status reply.
        let status: StatusResponse = serde_json::from_str(r#"{"devices":[]}"#).unwrap();
        assert_eq!(status.account, None);
        let status = StatusResponse {
            devices: Vec::new(),
            account: Some(new.id()),
            relay: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(
            serde_json::from_str::<StatusResponse>(&json).unwrap(),
            status
        );
    }
}
