use std::cmp::Reverse;
use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::ledger::Platform;
use crate::{AccountId, DeviceId, ProtoError};

/// Domain separator so a device-list signature can't be replayed as anything
/// else. Version 2 (Phase 1, M9): entries say what platform a device is and
/// when it joined, and the list carries removed devices, account settings and
/// a link to the account key it replaced.
const DEVICE_LIST_CONTEXT: &[u8] = b"mirrorleaf/device-list/2";
/// The Phase 0 list's separator, kept only to read lists stored before M9.
const LEGACY_CONTEXT: &[u8] = b"mirrorleaf/device-list/1";
/// The old account key's signature over the new account ID, when a key is
/// replaced (E3).
const ROTATION_CONTEXT: &[u8] = b"mirrorleaf/rotate-account/1";
/// BLAKE3 key-derivation context: the account key from the recovery secret
/// (E2, ADR-010). Fixed for good: changing it would make every kit useless.
const RECOVERY_KEY_CONTEXT: &str = "mirrorleaf 2026-11-09 account key from recovery secret";

/// The account setting saying when the account's recovery kit was made, in
/// Unix seconds (E2). Absent for accounts made before M10, which have none.
pub const KIT_MADE_AT: &str = "kit_made_at";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub id: DeviceId,
    pub name: String,
    /// `None` for devices that joined before M9: Phase 0's list didn't say.
    pub platform: Option<Platform>,
    /// When the device joined, in Unix seconds; 0 if unknown.
    pub added_at: u64,
}

impl DeviceEntry {
    /// Which of two entries for the same device a merge keeps: the one that
    /// knows more, then a fixed order, so every device keeps the same one.
    fn rank(&self) -> (bool, u64, Vec<u8>) {
        (
            self.platform.is_some(),
            self.added_at,
            postcard::to_stdvec(self).expect("entries always serialise"),
        )
    }
}

/// Proof that the account moved to a new key: the old key signed the new
/// account ID (E3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotationLink {
    pub previous: AccountId,
    pub signature: Vec<u8>,
}

impl RotationLink {
    pub fn verify(&self, new: &AccountId) -> Result<(), ProtoError> {
        let key =
            VerifyingKey::from_bytes(&self.previous.0).map_err(|_| ProtoError::BadSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ProtoError::BadSignature)?;
        key.verify_strict(&rotation_message(new), &Signature::from_bytes(&signature))
            .map_err(|_| ProtoError::BadSignature)
    }
}

fn rotation_message(new: &AccountId) -> Vec<u8> {
    let mut msg = ROTATION_CONTEXT.to_vec();
    msg.extend_from_slice(&new.0);
    msg
}

/// The account's devices. A peer is accepted only if it is on this list (plan §5.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    pub account: AccountId,
    pub version: u64,
    /// Sorted by device ID, no duplicates, none of them removed.
    pub devices: Vec<DeviceEntry>,
    /// Devices removed from the account, sorted. Kept for good, so that merging
    /// with an older list can't bring one back (E3).
    pub removed: Vec<DeviceId>,
    /// Settings that apply to the whole account, such as how it is hosted
    /// (E10). Keys are sorted; none are used yet.
    pub settings: BTreeMap<String, String>,
    /// Set when this account's key replaced an earlier one (E3).
    pub rotated_from: Option<RotationLink>,
}

impl DeviceList {
    fn normalised(mut self) -> Self {
        self.removed.sort();
        self.removed.dedup();
        let removed = &self.removed;
        self.devices
            .retain(|d| removed.binary_search(&d.id).is_err());
        self.devices
            .sort_by_cached_key(|d| (d.id, Reverse(d.rank())));
        self.devices.dedup_by_key(|d| d.id);
        self
    }

    fn signing_message(&self) -> Vec<u8> {
        let mut msg = DEVICE_LIST_CONTEXT.to_vec();
        msg.extend(postcard::to_stdvec(self).expect("device list always serialises"));
        msg
    }

    /// Everything but the version number.
    fn same_content(&self, other: &DeviceList) -> bool {
        self.devices == other.devices
            && self.removed == other.removed
            && self.settings == other.settings
            && self.rotated_from == other.rotated_from
    }

    pub fn entry(&self, device: &DeviceId) -> Option<&DeviceEntry> {
        self.devices
            .binary_search_by_key(device, |d| d.id)
            .ok()
            .map(|i| &self.devices[i])
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
        verify(account, &self.list.signing_message(), &self.signature)
    }

    pub fn contains(&self, device: &DeviceId) -> bool {
        self.list.entry(device).is_some()
    }

    pub fn was_removed(&self, device: &DeviceId) -> bool {
        self.list.removed.binary_search(device).is_ok()
    }

    /// When the account's current recovery kit was made, if it has one (E2).
    pub fn kit_made_at(&self) -> Option<u64> {
        self.list.settings.get(KIT_MADE_AT)?.parse().ok()
    }

    /// Checks this list belongs to an account that replaced `previous` (E3):
    /// signed by its own account key, and linked from `previous` by the old
    /// key's signature.
    pub fn verify_replacement_of(&self, previous: &AccountId) -> Result<(), ProtoError> {
        self.verify(&self.list.account)?;
        match &self.list.rotated_from {
            Some(link) if link.previous == *previous => link.verify(&self.list.account),
            _ => Err(ProtoError::WrongAccount),
        }
    }

    /// Reads a list stored before M9 (device list 1, which held only IDs and
    /// names), checks its signature, and signs the same devices as a version 2
    /// list under the same version number. The conversion is fixed, so every
    /// device converting the same list produces the same signed list (Ed25519
    /// signatures are deterministic), and they agree when they next meet.
    pub fn upgrade_legacy(bytes: &[u8], key: &AccountKey) -> Result<Self, ProtoError> {
        let legacy: LegacySignedList = postcard::from_bytes(bytes)?;
        let account = key.id();
        if legacy.list.account != account {
            return Err(ProtoError::WrongAccount);
        }
        let mut msg = LEGACY_CONTEXT.to_vec();
        msg.extend(postcard::to_stdvec(&legacy.list).expect("device list always serialises"));
        verify(&account, &msg, &legacy.signature)?;
        Ok(key.sign(DeviceList {
            account,
            version: legacy.list.version,
            devices: legacy
                .list
                .devices
                .into_iter()
                .map(|d| DeviceEntry {
                    id: d.id,
                    name: d.name,
                    platform: None,
                    added_at: 0,
                })
                .collect(),
            removed: Vec::new(),
            settings: BTreeMap::new(),
            rotated_from: None,
        }))
    }
}

fn verify(account: &AccountId, msg: &[u8], signature: &[u8]) -> Result<(), ProtoError> {
    let key = VerifyingKey::from_bytes(&account.0).map_err(|_| ProtoError::BadSignature)?;
    let signature: [u8; 64] = signature.try_into().map_err(|_| ProtoError::BadSignature)?;
    key.verify_strict(msg, &Signature::from_bytes(&signature))
        .map_err(|_| ProtoError::BadSignature)
}

/// Phase 0's device list, as stored before M9.
#[derive(Serialize, Deserialize)]
struct LegacyEntry {
    id: DeviceId,
    name: String,
}

#[derive(Serialize, Deserialize)]
struct LegacyList {
    account: AccountId,
    version: u64,
    devices: Vec<LegacyEntry>,
}

#[derive(Serialize, Deserialize)]
struct LegacySignedList {
    list: LegacyList,
    signature: Vec<u8>,
}

/// The account key. Every device of the account holds it: pairing hands it to
/// a new device over the encrypted connection (assessment §8.2), so any device
/// can add or remove others.
pub struct AccountKey(SigningKey);

impl AccountKey {
    pub fn generate() -> Self {
        Self::from_bytes(rand::random())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&bytes))
    }

    /// The account key the 128-bit recovery secret stands for (E2): the
    /// recovery kit's 12 words recreate the same key, so a recovered device
    /// joins the same account, with the same ID on the ledger.
    pub fn from_recovery(secret: &[u8; 16]) -> Self {
        Self::from_bytes(blake3::derive_key(RECOVERY_KEY_CONTEXT, secret))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn id(&self) -> AccountId {
        AccountId(self.0.verifying_key().to_bytes())
    }

    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.0
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
            removed: Vec::new(),
            settings: BTreeMap::new(),
            rotated_from: None,
        })
    }

    /// Adds `device` to `current` as a new version. A device that was removed
    /// stays removed: it must start again with a new device key.
    pub fn add_device(&self, current: &SignedDeviceList, device: DeviceEntry) -> SignedDeviceList {
        let mut list = current.list.clone();
        list.version += 1;
        list.devices.push(device);
        self.sign(list)
    }

    /// Removes `device` from `current` for good, as a new version (E3).
    pub fn remove_device(&self, current: &SignedDeviceList, device: DeviceId) -> SignedDeviceList {
        let mut list = current.list.clone();
        list.version += 1;
        list.removed.push(device);
        self.sign(list)
    }

    /// This key's signature over the account that replaces it (E3).
    pub fn rotation_link(&self, new: &AccountId) -> RotationLink {
        RotationLink {
            previous: self.id(),
            signature: self.0.sign(&rotation_message(new)).to_bytes().to_vec(),
        }
    }

    /// The first device list of the account `new` that replaces this key (E3):
    /// the same devices less `remove`, with the removed devices remembered, the
    /// same settings plus `kit_made_at`, and this key's signature linking the
    /// two accounts. Signed by `new`.
    pub fn replacement_list(
        &self,
        current: &SignedDeviceList,
        new: &AccountKey,
        remove: &[DeviceId],
        kit_made_at: u64,
    ) -> SignedDeviceList {
        let mut list = current.list.clone();
        list.account = new.id();
        list.version += 1;
        list.removed.extend_from_slice(remove);
        list.settings
            .insert(KIT_MADE_AT.to_owned(), kit_made_at.to_string());
        list.rotated_from = Some(self.rotation_link(&new.id()));
        new.sign(list)
    }

    /// A recovered device's first list (E2): itself only, at version 0, so the
    /// list of any surviving device is newer and its settings win the merge.
    pub fn recovery_list(&self, me: DeviceEntry) -> SignedDeviceList {
        self.sign(DeviceList {
            account: self.id(),
            version: 0,
            devices: vec![me],
            removed: Vec::new(),
            settings: BTreeMap::new(),
            rotated_from: None,
        })
    }

    /// Adds devices the ledger knows but this device can't name yet (a
    /// recovery with no other device reachable): no name, no platform, so the
    /// device's own entry wins once it is seen (`DeviceEntry::rank`).
    pub fn add_unnamed(
        &self,
        current: &SignedDeviceList,
        devices: &[DeviceId],
    ) -> SignedDeviceList {
        let mut list = current.list.clone();
        list.version += 1;
        list.devices.extend(devices.iter().map(|id| DeviceEntry {
            id: *id,
            name: String::new(),
            platform: None,
            added_at: 0,
        }));
        self.sign(list)
    }

    /// `current` with the account settings in `settings` changed.
    pub fn with_settings(
        &self,
        current: &SignedDeviceList,
        settings: &[(&str, String)],
    ) -> SignedDeviceList {
        let mut list = current.list.clone();
        list.version += 1;
        for (key, value) in settings {
            list.settings.insert((*key).to_owned(), value.clone());
        }
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
    /// Neither holds everything: a new version covering both, signed by us.
    Resigned(SignedDeviceList),
}

impl DeviceListMerge {
    /// Merges two lists that have both been verified against the same account.
    /// The result holds every device either list holds, minus every device
    /// either list removed; settings and the rotation link come from the newer
    /// list. Two peers that both re-sign produce the same list and signature
    /// (the merge is symmetric and Ed25519 signing is deterministic), so they
    /// converge.
    pub fn of(
        ours: &SignedDeviceList,
        theirs: &SignedDeviceList,
        key: Option<&AccountKey>,
    ) -> Self {
        if ours.list == theirs.list {
            return Self::Same;
        }
        // The newer list, by version, then by a fixed order of its bytes.
        let newer = std::cmp::max_by_key(&ours.list, &theirs.list, |l| {
            (l.version, l.signing_message())
        });
        let mut devices = ours.list.devices.clone();
        devices.extend(theirs.list.devices.iter().cloned());
        let mut removed = ours.list.removed.clone();
        removed.extend(theirs.list.removed.iter().copied());
        let union = DeviceList {
            account: ours.list.account,
            version: ours.list.version.max(theirs.list.version),
            devices,
            removed,
            settings: newer.settings.clone(),
            rotated_from: newer.rotated_from.clone(),
        }
        .normalised();
        let theirs_complete = union.same_content(&theirs.list);
        let ours_complete = union.same_content(&ours.list);

        // Theirs holds everything and is the newest, or the only complete one.
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
            platform: Some(Platform::Android),
            added_at: 1_794_000_000 + u64::from(n),
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
        let mut tampered = list.clone();
        tampered
            .list
            .settings
            .insert("hosting".into(), "elsewhere".into());
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

    #[test]
    fn a_removed_device_stays_removed_after_merging_an_older_list() {
        let key = AccountKey::generate();
        let both = key.add_device(&key.first_list(device(1)), device(2));
        let without_2 = key.remove_device(&both, DeviceId([2; 32]));
        assert!(!without_2.contains(&DeviceId([2; 32])));
        assert!(without_2.was_removed(&DeviceId([2; 32])));

        // A device that missed the removal adds a third device meanwhile.
        let with_3 = key.add_device(&both, device(3));
        let DeviceListMerge::Resigned(merged) =
            DeviceListMerge::of(&with_3, &without_2, Some(&key))
        else {
            panic!("expected a re-signed union");
        };
        let ids: Vec<u8> = merged.list.devices.iter().map(|d| d.id.0[0]).collect();
        assert_eq!(ids, [1, 3]);
        assert!(merged.was_removed(&DeviceId([2; 32])));
        // Adding it again doesn't bring it back: it needs a new device key.
        assert!(
            !key.add_device(&merged, device(2))
                .contains(&DeviceId([2; 32]))
        );
    }

    #[test]
    fn the_better_entry_for_a_device_wins_whichever_side_merges() {
        let key = AccountKey::generate();
        let v1 = key.first_list(device(1));
        let bare = DeviceEntry {
            platform: None,
            added_at: 0,
            ..device(2)
        };
        let a = key.add_device(&v1, bare);
        let b = key.add_device(&v1, device(2));
        // Both are version 2; `b` already holds the better entry, so both sides
        // settle on `b` without signing anything new.
        assert_eq!(
            DeviceListMerge::of(&a, &b, Some(&key)),
            DeviceListMerge::AdoptTheirs
        );
        assert_eq!(
            DeviceListMerge::of(&b, &a, Some(&key)),
            DeviceListMerge::KeepOurs
        );
        assert_eq!(b.list.entry(&DeviceId([2; 32])), Some(&device(2)));

        // With a third device on the bare side, neither is complete: both
        // re-sign the same union, keeping the better entry.
        let a = key.add_device(&a, device(3));
        let DeviceListMerge::Resigned(x) = DeviceListMerge::of(&a, &b, Some(&key)) else {
            panic!("expected a re-signed union");
        };
        let DeviceListMerge::Resigned(y) = DeviceListMerge::of(&b, &a, Some(&key)) else {
            panic!("expected a re-signed union");
        };
        assert_eq!(x, y);
        assert_eq!(x.list.entry(&DeviceId([2; 32])), Some(&device(2)));
        assert!(x.contains(&DeviceId([3; 32])));
    }

    #[test]
    fn phase_0_lists_upgrade_to_the_same_signed_list_everywhere() {
        let key = AccountKey::generate();
        let legacy = LegacyList {
            account: key.id(),
            version: 4,
            devices: vec![
                LegacyEntry {
                    id: DeviceId([1; 32]),
                    name: "mac".into(),
                },
                LegacyEntry {
                    id: DeviceId([2; 32]),
                    name: "phone".into(),
                },
            ],
        };
        let mut msg = LEGACY_CONTEXT.to_vec();
        msg.extend(postcard::to_stdvec(&legacy).unwrap());
        let signature = key.signing_key().sign(&msg).to_bytes().to_vec();
        let bytes = postcard::to_stdvec(&LegacySignedList {
            list: legacy,
            signature,
        })
        .unwrap();

        let a = SignedDeviceList::upgrade_legacy(&bytes, &key).unwrap();
        let b = SignedDeviceList::upgrade_legacy(&bytes, &AccountKey::from_bytes(key.to_bytes()))
            .unwrap();
        assert_eq!(a, b, "two devices converting the same list agree");
        a.verify(&key.id()).unwrap();
        assert_eq!(a.list.version, 4);
        assert_eq!(a.list.entry(&DeviceId([2; 32])).unwrap().name, "phone");
        assert_eq!(a.list.entry(&DeviceId([2; 32])).unwrap().platform, None);

        // A tampered or foreign list is refused.
        let mut broken = bytes.clone();
        let last = broken.len() - 1;
        broken[last] ^= 1;
        assert!(SignedDeviceList::upgrade_legacy(&broken, &key).is_err());
        assert!(matches!(
            SignedDeviceList::upgrade_legacy(&bytes, &AccountKey::generate()),
            Err(ProtoError::WrongAccount)
        ));
    }

    #[test]
    fn the_recovery_secret_always_makes_the_same_account_key() {
        let secret = [9u8; 16];
        assert_eq!(
            AccountKey::from_recovery(&secret).id(),
            AccountKey::from_recovery(&secret).id()
        );
        assert_ne!(
            AccountKey::from_recovery(&secret).id(),
            AccountKey::from_recovery(&[8u8; 16]).id()
        );
        // Pinned: a kit made today must open the same account in every later
        // version of the app.
        assert_eq!(
            AccountKey::from_recovery(&[0u8; 16]).id().to_string()[..16],
            *"44ce0cc369631239"
        );
    }

    #[test]
    fn a_replacement_list_moves_the_devices_to_the_new_key() {
        let old = AccountKey::generate();
        let current = old.add_device(&old.first_list(device(1)), device(2));
        let new = AccountKey::from_recovery(&[1u8; 16]);
        let replaced = old.replacement_list(&current, &new, &[DeviceId([2; 32])], 1_794_200_000);
        replaced.verify(&new.id()).unwrap();
        replaced.verify_replacement_of(&old.id()).unwrap();
        assert!(replaced.contains(&DeviceId([1; 32])));
        assert!(!replaced.contains(&DeviceId([2; 32])));
        assert!(replaced.was_removed(&DeviceId([2; 32])));
        assert_eq!(replaced.kit_made_at(), Some(1_794_200_000));
        // Not a replacement of some other account.
        assert!(
            replaced
                .verify_replacement_of(&AccountKey::generate().id())
                .is_err()
        );
        // Someone holding only the new key can't claim the old account.
        let mut forged = replaced.clone();
        forged.list.rotated_from = Some(new.rotation_link(&new.id()));
        let forged = new.sign(forged.list);
        assert!(forged.verify_replacement_of(&old.id()).is_err());
    }

    #[test]
    fn a_recovered_device_takes_the_names_and_settings_its_account_already_has() {
        let key = AccountKey::from_recovery(&[2u8; 16]);
        let held = key.with_settings(
            &key.add_device(&key.first_list(device(1)), device(2)),
            &[(KIT_MADE_AT, "1794200000".into())],
        );
        // The recovered device knows the others only from the ledger.
        let recovered = key.add_unnamed(&key.recovery_list(device(3)), &[DeviceId([1; 32])]);
        let DeviceListMerge::Resigned(merged) = DeviceListMerge::of(&recovered, &held, Some(&key))
        else {
            panic!("expected a re-signed union");
        };
        assert_eq!(merged.list.devices.len(), 3);
        assert_eq!(merged.list.entry(&DeviceId([1; 32])), Some(&device(1)));
        assert_eq!(merged.kit_made_at(), Some(1_794_200_000));
    }

    #[test]
    fn a_rotation_link_proves_the_old_key_signed_the_new_account() {
        let old = AccountKey::generate();
        let new = AccountKey::generate();
        let link = old.rotation_link(&new.id());
        link.verify(&new.id()).unwrap();
        assert!(link.verify(&AccountKey::generate().id()).is_err());
    }
}
