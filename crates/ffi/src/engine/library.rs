//! Everyday note features for the screen (M12, E5): full-text search,
//! deleting for good, emptying the trash and its 30-day purge, and fetching
//! full images ahead on unmetered networks. None of it runs in a push-woken
//! job: they start from the screen's calls.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tracing::info;

use super::screen::UiEvent;
use super::{Engine, EngineError, lock};

/// Notes in the trash longer than this are deleted for good (owner, 11 Nov 2026).
const PURGE_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);
/// The purge runs when the screen first lists notes, then at most this often.
const PURGE_EVERY: Duration = Duration::from_secs(24 * 3600);
/// Notes read into the search index per step; the database is free between.
const INDEX_STEP: usize = 25;
/// Most results a search returns.
const MAX_RESULTS: usize = 500;

/// A note a search found, with the words around the first match in its text
/// (matches between `\u{2}` and `\u{3}`); empty when only its title matched.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SearchResult {
    pub id: String,
    pub snippet: String,
}

impl Engine {
    /// Notes matching every word of `query`, best first. Catches the search
    /// index up first, a few notes at a time.
    pub fn search_notes(&self, query: &str) -> Result<Vec<SearchResult>, EngineError> {
        while lock(&self.replica).update_search(INDEX_STEP)? {}
        Ok(lock(&self.replica)
            .search(query, MAX_RESULTS)?
            .into_iter()
            .map(|hit| SearchResult {
                id: hit.id,
                snippet: hit.snippet,
            })
            .collect())
    }

    /// Deletes a note for good, on every device.
    pub fn delete_note(&self, id: &str) -> Result<(), EngineError> {
        lock(&self.replica).delete_note(id)?;
        self.notify(UiEvent::NotesChanged);
        Ok(())
    }

    /// Deletes every note in the trash for good. Returns how many.
    pub fn empty_trash(&self) -> Result<u32, EngineError> {
        let deleted = lock(&self.replica).purge_trash(None)?;
        if deleted > 0 {
            self.notify(UiEvent::NotesChanged);
        }
        Ok(deleted as u32)
    }

    /// Deletes for good the notes trashed more than 30 days ago, if a day has
    /// passed since the last purge. Called as the screen lists notes.
    pub(super) fn purge_if_due(&self) -> anyhow::Result<()> {
        {
            let mut last = lock(&self.last_purge);
            if last.is_some_and(|at| at.elapsed() < PURGE_EVERY) {
                return Ok(());
            }
            *last = Some(Instant::now());
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let before = now - PURGE_AFTER.as_millis() as i64;
        let purged = lock(&self.replica).purge_trash(Some(before))?;
        if purged > 0 {
            info!("deleted {purged} notes trashed over 30 days ago");
        }
        Ok(())
    }

    /// Applies the prefetch rule to the live node: full images ahead while the
    /// screen is in use on an unmetered network (always, on a desktop).
    pub(super) fn apply_prefetch(&self) {
        if let Some(node) = lock(&self.node).as_ref() {
            node.set_prefetch(self.unmetered.load(Ordering::Acquire));
        }
    }
}

/// Meta key: "0" when finding devices on the same network is turned off.
const LOCAL_DISCOVERY: &str = "local_discovery";

impl Engine {
    /// Whether this device looks for the account's devices on the same
    /// network, with no internet needed (mDNS): on unless turned off (M12).
    pub fn local_discovery_on(&self) -> bool {
        !matches!(
            lock(&self.replica).meta(LOCAL_DISCOVERY),
            Ok(Some(v)) if v == b"0"
        )
    }

    /// Turns finding devices on the same network on or off, at once if live
    /// sync is running.
    pub fn set_local_discovery(&self, on: bool) -> Result<(), EngineError> {
        lock(&self.replica).set_meta(LOCAL_DISCOVERY, if on { b"1" } else { b"0" })?;
        if self.live.load(Ordering::Acquire)
            && let Some(node) = lock(&self.node).as_ref()
        {
            node.set_local_discovery(on);
        }
        Ok(())
    }
}

impl Engine {
    /// Starts live sync again on a fresh network endpoint, if it is running
    /// (M12, E8): Android applies a newly granted nearby-devices permission
    /// only to sockets opened after it (M4 finding 1).
    pub async fn restart_network(self: std::sync::Arc<Self>) -> Result<(), EngineError> {
        if !self.live.load(Ordering::Acquire) {
            return Ok(());
        }
        let this = self.clone();
        self.on_runtime(async move {
            this.stop_live().await?;
            this.start_live().await
        })
        .await?;
        info!("restarted live sync on a new endpoint");
        Ok(())
    }
}

/// For Android's own code.
#[uniffi::export]
impl Engine {
    /// Whether the phone is on an unmetered network (Wi-Fi), from Android's
    /// connectivity callback: full images are fetched ahead only then (M12).
    pub fn set_unmetered(&self, unmetered: bool) {
        if self.unmetered.swap(unmetered, Ordering::AcqRel) != unmetered {
            info!(
                "{} network: full images {}",
                if unmetered { "unmetered" } else { "metered" },
                if unmetered {
                    "fetched ahead"
                } else {
                    "fetched when opened"
                }
            );
        }
        if self.live.load(Ordering::Acquire) {
            self.apply_prefetch();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mirrorleaf_net::device::DeviceDir;

    #[test]
    fn searching_deleting_and_the_purge() {
        let dir = std::env::temp_dir().join(format!("mirrorleaf-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = Engine::start(DeviceDir::new(&dir), "Test").unwrap();
        let market = engine
            .create_note(None, Some("Vegetable market".into()))
            .unwrap();
        {
            let mut replica = lock(&engine.replica);
            replica
                .append_paragraph(&market, "Vegetable market")
                .unwrap();
            replica
                .append_paragraph(&market, "Onions and garlic")
                .unwrap();
        }
        let hits = engine.search_notes("garl").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, market);
        assert!(hits[0].snippet.contains("\u{2}garlic\u{3}"));

        // The purge leaves a note trashed today; Empty trash takes it.
        engine.trash_note(&market).unwrap();
        engine.purge_if_due().unwrap();
        assert_eq!(engine.note_list().unwrap().notes.len(), 1);
        assert_eq!(engine.empty_trash().unwrap(), 1);
        assert!(engine.note_list().unwrap().notes.is_empty());
        assert!(engine.search_notes("garlic").unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
