//! What this device counts for the beta's daily totals (M13, E12; owner,
//! 15 Nov 2026: on by default for testers, with an off switch in Settings).
//! Only counts, sent with the next status report: how long changes took to
//! arrive, whether connections went direct, whether push-woken jobs synced.
//! Never a note, a title, a size or a time of day.
//!
//! One tally per process: the app has one node at a time, and a push-woken
//! job's counts wait in the database for the next report if the process ends
//! first ([`save`], [`load`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use mirrorleaf_core::Replica;
use mirrorleaf_proto::ledger::SyncMetrics;

use crate::node::PathKind;

/// Meta key: counts not yet sent when the process last ended.
const PENDING: &str = "metrics_pending";

static TALLY: Mutex<Option<SyncMetrics>> = Mutex::new(None);
static SHARING: AtomicBool = AtomicBool::new(true);
static MAKER: OnceLock<String> = OnceLock::new();

/// Turns counting on or off for this process. Off drops what was counted.
pub fn set_sharing(on: bool) {
    SHARING.store(on, Ordering::Release);
    if !on {
        *TALLY.lock().unwrap() = None;
    }
}

pub fn sharing() -> bool {
    SHARING.load(Ordering::Acquire)
}

/// The phone's maker (Android's `Build.MANUFACTURER`), for wake-ups by maker.
pub fn set_device_maker(maker: &str) {
    let _ = MAKER.set(maker.trim().to_ascii_lowercase());
}

fn record(f: impl FnOnce(&mut SyncMetrics)) {
    if sharing() {
        f(TALLY.lock().unwrap().get_or_insert_with(Default::default));
    }
}

/// A change from another device arrived; `made_at` and `now` in Unix seconds.
pub(crate) fn arrived(made_at: u64, now: u64) {
    record(|m| m.arrived_after(now.saturating_sub(made_at)));
}

/// A connection to another device was made.
pub(crate) fn connected(path: PathKind) {
    record(|m| match path {
        PathKind::Direct => m.direct = m.direct.saturating_add(1),
        PathKind::Relay => m.relayed = m.relayed.saturating_add(1),
        PathKind::Unknown => {}
    });
}

/// A push-woken job ended: whether it synced with the device it was woken for.
pub fn woken(synced: bool) {
    record(|m| {
        if synced {
            m.woken_synced = m.woken_synced.saturating_add(1);
        } else {
            m.woken_failed = m.woken_failed.saturating_add(1);
        }
    });
}

/// What to send with a status report, taking it from the tally: `None` when
/// the person turned counting off. Always carries the app version and maker.
pub(crate) fn take() -> Option<SyncMetrics> {
    if !sharing() {
        return None;
    }
    let mut m = TALLY.lock().unwrap().take().unwrap_or_default();
    m.version = Some(crate::app_version().to_owned()).filter(|v| !v.is_empty());
    m.maker = MAKER.get().cloned();
    Some(m)
}

/// Puts back counts whose report didn't go through.
pub(crate) fn put_back(m: &SyncMetrics) {
    if !m.is_empty() {
        record(|tally| tally.add(m));
    }
}

/// Keeps unsent counts in the database when the node closes.
pub(crate) fn save(replica: &mut Replica) {
    let pending = TALLY.lock().unwrap().take();
    if let Some(m) = pending.filter(|m| !m.is_empty() && sharing())
        && let Ok(json) = serde_json::to_vec(&m)
    {
        let _ = replica.set_meta(PENDING, &json);
    }
}

/// Takes back the counts an earlier process left in the database.
pub(crate) fn load(replica: &mut Replica) {
    let Ok(Some(json)) = replica.meta(PENDING) else {
        return;
    };
    let _ = replica.set_meta(PENDING, b"");
    if let Ok(m) = serde_json::from_slice::<SyncMetrics>(&json) {
        put_back(&m);
    }
}
