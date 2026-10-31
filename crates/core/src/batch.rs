use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mirrorleaf_proto::Change;

use crate::{CoreError, Replica, Result};

/// How long the editor's updates are collected before they become one change
/// (plan §5.2: "one change per note per ~1–2 seconds").
pub const BATCH_WINDOW: Duration = Duration::from_millis(1500);

/// Collects the many small Yjs updates typing produces and merges them into one
/// change per document. Updates waiting here are in memory only, so a crash can
/// lose up to one window of typing; flush-on-close covers a normal exit.
pub struct Batcher {
    window: Duration,
    pending: BTreeMap<String, Vec<Vec<u8>>>,
    oldest: Option<Instant>,
}

impl Batcher {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            pending: BTreeMap::new(),
            oldest: None,
        }
    }

    pub fn push(&mut self, doc_id: &str, update: Vec<u8>, now: Instant) {
        self.pending
            .entry(doc_id.to_owned())
            .or_default()
            .push(update);
        self.oldest.get_or_insert(now);
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.oldest
            .is_some_and(|oldest| now.duration_since(oldest) >= self.window)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Writes one merged change per document to the log.
    pub fn flush(&mut self, replica: &mut Replica) -> Result<Vec<Change>> {
        self.oldest = None;
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(doc_id, updates)| {
                let merged = yrs::merge_updates_v1(&updates)
                    .map_err(|e| CoreError::Update(e.to_string()))?;
                replica.record_local(&doc_id, merged)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use mirrorleaf_proto::DeviceId;
    use yrs::{Doc, GetString, ReadTxn, Text, Transact, WriteTxn};

    use super::*;

    #[test]
    fn typing_becomes_one_small_change() {
        let typed = "The quick brown fox jumps over the lazy dog. ".repeat(4);
        let editor = Doc::new();
        let mut batcher = Batcher::new(BATCH_WINDOW);
        let start = Instant::now();
        let mut separate_bytes = 0;
        for (i, ch) in typed.chars().enumerate() {
            let mut txn = editor.transact_mut();
            txn.get_or_insert_text("t")
                .insert(&mut txn, i as u32, &ch.to_string());
            let update = txn.encode_update_v1();
            separate_bytes += update.len();
            batcher.push("note", update, start);
        }
        assert!(!batcher.is_due(start));
        assert!(batcher.is_due(start + BATCH_WINDOW));

        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let changes = batcher.flush(&mut replica).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(batcher.is_empty());
        // yrs keeps typed characters as separate items (about 12 bytes each), so
        // merging saves the per-update overhead rather than folding the text. Even
        // this 180-character burst stays under the 5 KB per-edit target (plan §7.3).
        let merged = changes[0].update.len();
        assert!(
            merged < separate_bytes && merged < 5 * 1024,
            "merged {merged} bytes vs {separate_bytes} sent separately"
        );

        let doc = replica.load_doc("note").unwrap();
        let txn = doc.transact();
        assert_eq!(txn.get_text("t").unwrap().get_string(&txn), typed);
    }
}
