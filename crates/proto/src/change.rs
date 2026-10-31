use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::DeviceId;

/// One entry in the change log: a Yjs update made on `origin`, numbered `seq`
/// (plan §5.2). Sequence numbers start at 1 and have no gaps per origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub origin: DeviceId,
    pub seq: u64,
    pub doc_id: String,
    pub update: Vec<u8>,
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
