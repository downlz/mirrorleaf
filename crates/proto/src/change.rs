use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::DeviceId;

/// What a change's bytes are, so each goes to the code that merges it
/// (reuse assessment, change 1). A device stores and passes on content types it
/// doesn't know, so a newer device's changes can hop through an older one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentType(pub u8);

impl ContentType {
    /// A Yjs update (`encode_update_v1`) to the document `doc_id`: every note
    /// and the index.
    pub const YJS: Self = Self(0);
    /// Reserved: whole values that replace older ones field by field.
    pub const FIELDS: Self = Self(1);
    /// Reserved: the manifest of a file stored as blobs.
    pub const MANIFEST: Self = Self(2);
}

/// How soon a change should reach the other devices (reuse assessment, change
/// 2), for the push rules. Notes are all `NORMAL` for now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SyncClass(pub u8);

impl SyncClass {
    pub const NORMAL: Self = Self(0);
    /// Reserved: push at once.
    pub const URGENT: Self = Self(1);
    /// Reserved: no push; merged into the next change.
    pub const LAZY: Self = Self(2);
}

/// One entry in the change log: an update made on `origin`, numbered `seq`
/// (plan §5.2). Sequence numbers start at 1 and have no gaps per origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub origin: DeviceId,
    pub seq: u64,
    pub doc_id: String,
    pub content: ContentType,
    pub class: SyncClass,
    /// When the change was made on its origin, in Unix seconds, so a device
    /// receiving it can tell how long it took to arrive (E12). 0 for changes
    /// made before this field existed. Travels only between the account's
    /// devices, never to the ledger.
    pub made_at: u64,
    pub update: Vec<u8>,
}

impl Change {
    /// A note change as this version makes them: a Yjs update, normal class.
    pub fn yjs(
        origin: DeviceId,
        seq: u64,
        doc_id: impl Into<String>,
        update: Vec<u8>,
        made_at: u64,
    ) -> Self {
        Self {
            origin,
            seq,
            doc_id: doc_id.into(),
            content: ContentType::YJS,
            class: SyncClass::NORMAL,
            made_at,
            update,
        }
    }
}

/// The highest sequence number held from each origin device. These are also the
/// ledger's counters (plan §5.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionVector(pub BTreeMap<DeviceId, u64>);

impl VersionVector {
    pub fn get(&self, origin: &DeviceId) -> u64 {
        self.0.get(origin).copied().unwrap_or(0)
    }

    /// Whether this vector already holds `change`.
    pub fn contains(&self, change: &Change) -> bool {
        change.seq <= self.get(&change.origin)
    }

    /// The changes held by either vector: what both hold after a full sync.
    pub fn merged(&self, other: &VersionVector) -> VersionVector {
        let mut merged = self.clone();
        for (origin, seq) in &other.0 {
            let held = merged.0.entry(*origin).or_default();
            *held = (*held).max(*seq);
        }
        merged
    }

    /// Total changes held minus those `other` holds, counting only origins where
    /// this vector is ahead: how far `other` is behind this one.
    pub fn ahead_of(&self, other: &VersionVector) -> u64 {
        self.0
            .iter()
            .map(|(origin, seq)| seq.saturating_sub(other.get(origin)))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ahead_of_counts_only_missing_changes() {
        let (a, b) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let ours = VersionVector([(a, 5), (b, 2)].into());
        let theirs = VersionVector([(a, 3), (b, 4)].into());
        assert_eq!(ours.ahead_of(&theirs), 2);
        assert_eq!(theirs.ahead_of(&ours), 2);
        assert_eq!(ours.ahead_of(&ours), 0);
        let both = ours.merged(&theirs);
        assert_eq!((both.get(&a), both.get(&b)), (5, 4));
    }
}
