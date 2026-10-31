use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{AccountId, DeviceId, ProtoError};

/// Domain separator so a device-list signature can't be replayed as anything else.
const DEVICE_LIST_CONTEXT: &[u8] = b"mirrorleaf/device-list/1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub id: DeviceId,
    pub name: String,
}

/// The account's devices. A peer is accepted only if it is on this list (plan §5.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    pub account: AccountId,
    pub version: u64,
    /// Sorted by device ID, no duplicates.
    pub devices: Vec<DeviceEntry>,
}

impl DeviceList {
    fn normalised(mut self) -> Self {
        self.devices.sort_by_key(|d| d.id);
        self.devices.dedup_by_key(|d| d.id);
        self
    }

    fn signing_message(&self) -> Vec<u8> {
        let mut msg = DEVICE_LIST_CONTEXT.to_vec();
        msg.extend(postcard::to_stdvec(self).expect("device list always serialises"));
        msg
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDeviceList {
    pub list: DeviceList,
    pub signature: Vec<u8>,
}

impl SignedDeviceList {
    /// Checks the list was signed by `account`.
    pub fn verify(&self, account: &AccountId) -> Result<(), ProtoError> {
        if self.list.account != *account {
            return Err(ProtoError::WrongAccount);
        }
        let key = VerifyingKey::from_bytes(&account.0).map_err(|_| ProtoError::BadSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ProtoError::BadSignature)?;
        key.verify_strict(
            &self.list.signing_message(),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| ProtoError::BadSignature)
    }

    pub fn contains(&self, device: &DeviceId) -> bool {
        self.list
            .devices
            .binary_search_by_key(device, |d| d.id)
            .is_ok()
    }
}

/// The account key. In Phase 0 every device of a developer account holds it
/// (plan §5.4); real pairing and approval come in Phase 1 (E1).
pub struct AccountKey(SigningKey);

impl AccountKey {
    pub fn generate() -> Self {
        Self::from_bytes(rand::random())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&bytes))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn id(&self) -> AccountId {
        AccountId(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, list: DeviceList) -> SignedDeviceList {
        let list = list.normalised();
        let signature = self.0.sign(&list.signing_message()).to_bytes().to_vec();
        SignedDeviceList { list, signature }
    }

    /// The first device list of a new account.
    pub fn first_list(&self, device: DeviceEntry) -> SignedDeviceList {
        self.sign(DeviceList {
            account: self.id(),
            version: 1,
            devices: vec![device],
        })
    }

    /// Adds `device` to `current` as a new version.
    pub fn add_device(&self, current: &SignedDeviceList, device: DeviceEntry) -> SignedDeviceList {
        let mut list = current.list.clone();
        list.version += 1;
        list.devices.push(device);
        self.sign(list)
    }
}

/// The outcome of comparing our device list with a peer's.
#[derive(Debug, PartialEq, Eq)]
pub enum DeviceListMerge {
    Same,
    /// Ours is complete and newest; the peer should adopt it.
    KeepOurs,
    /// Theirs is complete and newest.
    AdoptTheirs,
    /// Neither holds every device: a new version covering both, signed by us.
    Resigned(SignedDeviceList),
}

impl DeviceListMerge {
    /// Merges two lists that have both been verified against the same account.
    /// Devices are only ever added in Phase 0, so the result is the union. Two peers
    /// that both re-sign produce the same list and signature (Ed25519 signing is
    /// deterministic), so they converge.
    pub fn of(
        ours: &SignedDeviceList,
        theirs: &SignedDeviceList,
        key: Option<&AccountKey>,
    ) -> Self {
        if ours.list == theirs.list {
            return Self::Same;
        }
        let mut union = ours.list.devices.clone();
        union.extend(theirs.list.devices.iter().cloned());
        let union = DeviceList {
            account: ours.list.account,
            version: ours.list.version.max(theirs.list.version),
            devices: union,
        }
        .normalised();
        let ids = |l: &DeviceList| l.devices.iter().map(|d| d.id).collect::<Vec<_>>();
        let theirs_complete = ids(&union) == ids(&theirs.list);
        let ours_complete = ids(&union) == ids(&ours.list);

        // Theirs holds every device and is the newest, or the only complete one.
        let theirs_wins = theirs.list.version > ours.list.version
            || (theirs.list.version == union.version && !ours_complete);
        if theirs_complete && theirs_wins {
            Self::AdoptTheirs
        } else if ours_complete && ours.list.version == union.version {
            Self::KeepOurs
        } else if let Some(key) = key {
            Self::Resigned(key.sign(DeviceList {
                version: union.version + 1,
                ..union
            }))
        } else if theirs.list.version > ours.list.version {
            Self::AdoptTheirs
        } else {
            Self::KeepOurs
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(n: u8) -> DeviceEntry {
        DeviceEntry {
            id: DeviceId([n; 32]),
            name: format!("device {n}"),
        }
    }

    #[test]
    fn signed_list_verifies_and_rejects_tampering() {
        let key = AccountKey::generate();
        let list = key.add_device(&key.first_list(device(1)), device(2));
        list.verify(&key.id()).unwrap();
        assert!(list.contains(&DeviceId([2; 32])));
        assert!(!list.contains(&DeviceId([3; 32])));

        let mut tampered = list.clone();
        tampered.list.devices.push(device(3));
        assert!(matches!(
            tampered.verify(&key.id()),
            Err(ProtoError::BadSignature)
        ));

        let other = AccountKey::generate();
        assert!(matches!(
            list.verify(&other.id()),
            Err(ProtoError::WrongAccount)
        ));
    }

    #[test]
    fn newer_superset_is_adopted() {
        let key = AccountKey::generate();
        let v1 = key.first_list(device(1));
        let v2 = key.add_device(&v1, device(2));
        assert_eq!(
            DeviceListMerge::of(&v1, &v2, None),
            DeviceListMerge::AdoptTheirs
        );
        assert_eq!(
            DeviceListMerge::of(&v2, &v1, None),
            DeviceListMerge::KeepOurs
        );
        assert_eq!(DeviceListMerge::of(&v2, &v2, None), DeviceListMerge::Same);
    }

    #[test]
    fn concurrent_additions_converge_on_one_signed_union() {
        let key = AccountKey::generate();
        let v1 = key.first_list(device(1));
        let with_2 = key.add_device(&v1, device(2));
        let with_3 = key.add_device(&v1, device(3));

        let DeviceListMerge::Resigned(a) = DeviceListMerge::of(&with_2, &with_3, Some(&key)) else {
            panic!("expected a re-signed union");
        };
        let DeviceListMerge::Resigned(b) = DeviceListMerge::of(&with_3, &with_2, Some(&key)) else {
            panic!("expected a re-signed union");
        };
        assert_eq!(a, b);
        assert_eq!(a.list.version, 3);
        assert_eq!(a.list.devices.len(), 3);
        a.verify(&key.id()).unwrap();
    }
}
