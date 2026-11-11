//! Backups (E9) and the app lock (E4) as the screen drives them (M11). Both
//! are settings of this device, off by default (owner, 9 Nov 2026).
//!
//! **Encrypt backups** off: backups are plain files anyone holding one can
//! read; on: encrypted with a key derived from the account key, which the
//! recovery kit recreates. **App lock** off: no lock; on: the device's own
//! unlock (passcode, fingerprint, face; Touch ID or the Mac's password), asked
//! when the app opens and when it comes back after the chosen time away. The
//! lock covers the screen only: background sync carries on while locked.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use mirrorleaf_net::Account;
use mirrorleaf_net::backup::{BackupReader, EXTENSION, key_for};
use mirrorleaf_net::kit::long_date;
use tracing::info;

use super::screen::UiEvent;
use super::{Engine, EngineError, lock};

/// Settings and records kept in the database's `meta` table.
const BACKUP_ENCRYPT: &str = "backup_encrypt";
const LAST_BACKUP: &str = "last_backup_at";
const FIRST_OPENED: &str = "first_opened_at";
const BACKUP_SNOOZED: &str = "backup_snoozed_until";
const APP_LOCK: &str = "app_lock_minutes";

/// A backup is due this long after the last one (or after the device first
/// opened, if there has been none).
const BACKUP_EVERY: u64 = 30 * 24 * 3600;
/// "Later" on the reminder puts it off this long.
const SNOOZE: u64 = 7 * 24 * 3600;
/// The lock's choices of time away before it locks, in minutes.
pub const LOCK_CHOICES: [u32; 4] = [0, 1, 5, 15];
const DEFAULT_LOCK_MINUTES: u32 = 5;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the screen shows about backups.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BackupSettings {
    pub encrypt: bool,
    /// "9 November 2026", if there has been a backup.
    pub last_backup: Option<String>,
    /// The main screen's reminder, when one is due.
    pub due: Option<String>,
}

/// What a backup file says about itself, before restoring it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BackupInfo {
    pub made: String,
    pub encrypted: bool,
    /// Encrypted with an earlier account key: restoring needs that kit's words.
    pub needs_kit: bool,
}

/// What a backup or a restore moved.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BackupDone {
    pub changes: u64,
    pub photos: u64,
}

/// The app lock's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LockSettings {
    pub enabled: bool,
    /// Minutes away before it locks: 0 (at once), 1, 5 or 15.
    pub after_minutes: u32,
}

/// Whether the screen is locked, kept for the life of the process: the Mac
/// window can close and open again while the app keeps running.
#[derive(Debug, Default)]
pub(super) struct LockState {
    locked: bool,
    left_at: Option<LeftAt>,
    started: bool,
    /// The screen is in sight (between `screen_returned` and `screen_left`).
    shown: bool,
    /// The device's own unlock is showing: Android's passcode screen covers
    /// the app, which must not count as time away.
    authenticating: bool,
}

/// When the screen went out of sight, on both clocks: the monotonic clock
/// stops while the computer or phone sleeps (as the ledger's token timing
/// found in M8), and the wall clock can be moved back. The time away is the
/// longer of the two.
#[derive(Debug, Clone, Copy)]
struct LeftAt {
    monotonic: Instant,
    wall: SystemTime,
}

impl LeftAt {
    fn now() -> Self {
        Self {
            monotonic: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    fn away(&self) -> Duration {
        let wall = SystemTime::now()
            .duration_since(self.wall)
            .unwrap_or_default();
        self.monotonic.elapsed().max(wall)
    }
}

impl Engine {
    fn meta_text(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(lock(&self.replica)
            .meta(key)?
            .map(|v| String::from_utf8_lossy(&v).into_owned()))
    }

    fn meta_number(&self, key: &str) -> anyhow::Result<Option<u64>> {
        Ok(self.meta_text(key)?.and_then(|v| v.parse().ok()))
    }

    fn set_meta_text(&self, key: &str, value: &str) -> anyhow::Result<()> {
        lock(&self.replica).set_meta(key, value.as_bytes())?;
        Ok(())
    }

    /// Notes when this device first opened, for the first backup reminder.
    pub(super) fn note_first_open(&self) -> anyhow::Result<()> {
        if self.meta_number(FIRST_OPENED)?.is_none() {
            self.set_meta_text(FIRST_OPENED, &now().to_string())?;
        }
        Ok(())
    }

    // --- Backups ---

    pub fn backup_settings(&self) -> Result<BackupSettings, EngineError> {
        let last = self.meta_number(LAST_BACKUP)?;
        Ok(BackupSettings {
            encrypt: self.meta_text(BACKUP_ENCRYPT)?.as_deref() == Some("1"),
            last_backup: last.map(long_date),
            due: self.backup_due(last)?,
        })
    }

    /// The reminder, if one is due: 30 days after the last backup (or after
    /// this device first opened), or after a new recovery kit since the last
    /// backup. "Later" puts it off for a week.
    fn backup_due(&self, last: Option<u64>) -> anyhow::Result<Option<String>> {
        let now = now();
        if self
            .meta_number(BACKUP_SNOOZED)?
            .is_some_and(|until| until > now)
        {
            return Ok(None);
        }
        let kit_made = Account::load(&lock(&self.replica))?.and_then(|a| a.devices.kit_made_at());
        if let Some(last) = last {
            if kit_made.is_some_and(|k| k > last) {
                return Ok(Some(
                    "You made a new recovery kit. Back up your notes again.".into(),
                ));
            }
            let days = now.saturating_sub(last) / 86_400;
            return Ok((now.saturating_sub(last) >= BACKUP_EVERY)
                .then(|| format!("Your last backup was {days} days ago.")));
        }
        let first = self.meta_number(FIRST_OPENED)?.unwrap_or(now);
        if now.saturating_sub(first) < BACKUP_EVERY {
            return Ok(None);
        }
        let has_notes = lock(&self.replica).list_notes()?.iter().any(|n| !n.trashed);
        Ok(has_notes.then(|| "You haven't backed up your notes yet.".into()))
    }

    pub fn set_backup_encryption(&self, on: bool) -> Result<(), EngineError> {
        self.set_meta_text(BACKUP_ENCRYPT, if on { "1" } else { "0" })?;
        Ok(())
    }

    /// Puts the reminder off for a week.
    pub fn snooze_backup_reminder(&self) -> Result<(), EngineError> {
        self.set_meta_text(BACKUP_SNOOZED, &(now() + SNOOZE).to_string())?;
        self.notify(UiEvent::StatusChanged);
        Ok(())
    }

    /// "Mirrorleaf backup 2026-11-09.mirrorleaf-backup".
    pub fn backup_file_name(&self) -> String {
        let date = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let days = date / 86_400;
        // A sortable date; `long_date` gives the words, this the numbers.
        let (y, m, d) = civil(days as i64);
        format!("Mirrorleaf backup {y:04}-{m:02}-{d:02}.{EXTENSION}")
    }

    /// Writes a backup of everything this device holds to `path`, encrypted
    /// if Encrypt backups is on. The caller calls `backup_saved` once the file
    /// is where the person chose (on Android it is copied there afterwards,
    /// and they can still cancel).
    pub async fn write_backup(self: Arc<Self>, path: String) -> Result<BackupDone, EngineError> {
        let encrypt = self.backup_settings()?.encrypt;
        let this = self.clone();
        self.on_runtime(async move {
            let partial = format!("{path}.partial");
            let node = this.borrow_node().await?;
            let written = async {
                let file = std::fs::File::create(&partial)?;
                let (file, counts) = node
                    .write_backup(std::io::BufWriter::new(file), encrypt)
                    .await?;
                file.into_inner()
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?
                    .sync_all()?;
                std::fs::rename(&partial, &path)?;
                anyhow::Ok(counts)
            }
            .await;
            this.return_node().await;
            let counts = written.inspect_err(|_| {
                let _ = std::fs::remove_file(&partial);
            })?;
            info!(
                "backed up {} changes and {} photos ({})",
                counts.changes,
                counts.photos,
                if encrypt { "encrypted" } else { "plain" }
            );
            Ok(BackupDone {
                changes: counts.changes,
                photos: counts.photos,
            })
        })
        .await
    }

    /// The backup written is saved where the person chose: it counts as the
    /// last backup, and the reminder goes.
    pub fn backup_saved(&self) -> Result<(), EngineError> {
        self.set_meta_text(LAST_BACKUP, &now().to_string())?;
        self.notify(UiEvent::StatusChanged);
        Ok(())
    }

    /// What the backup at `path` says about itself.
    pub fn backup_info(&self, path: &str) -> Result<BackupInfo, EngineError> {
        let reader = BackupReader::open(std::io::BufReader::new(
            std::fs::File::open(path).map_err(|e| EngineError::Failed(e.to_string()))?,
        ))?;
        let header = reader.header().clone();
        let current = Account::load(&lock(&self.replica))?.and_then(|a| a.key);
        let needs_kit =
            header.encrypted && current.as_ref().is_none_or(|k| k.id() != header.account);
        Ok(BackupInfo {
            made: long_date(header.made_at),
            encrypted: header.encrypted,
            needs_kit,
        })
    }

    /// Restores the backup at `path` into this device: its notes and photos
    /// merge with what is here, and reach the other devices as sync does.
    /// `words` are the backup's own recovery kit, for an encrypted backup
    /// made with an earlier account key.
    pub async fn restore_backup(
        self: Arc<Self>,
        path: String,
        words: Option<String>,
    ) -> Result<BackupDone, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let mut reader = BackupReader::open(std::io::BufReader::new(
                std::fs::File::open(&path).context("can't open the backup")?,
            ))?;
            let current = Account::load(&lock(&this.replica))?
                .context("join or start an account first")?
                .key;
            let key = key_for(reader.header(), current.as_ref(), words.as_deref())?;
            if let Some(key) = key {
                reader.unlock(key);
            }
            let node = this.borrow_node().await?;
            let restored = node.read_backup(reader).await;
            if restored.is_ok() {
                // Pushes what arrived to the connected devices at once.
                node.nudge();
            }
            this.return_node().await;
            let counts = restored?;
            info!(
                "restored {} changes and {} photos",
                counts.changes, counts.photos
            );
            this.notify(UiEvent::NotesChanged);
            this.notify(UiEvent::StatusChanged);
            Ok(BackupDone {
                changes: counts.changes,
                photos: counts.photos,
            })
        })
        .await
    }

    // --- The app lock ---

    pub fn lock_settings(&self) -> Result<LockSettings, EngineError> {
        let minutes = self
            .meta_text(APP_LOCK)?
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|m| LOCK_CHOICES.contains(m));
        Ok(LockSettings {
            enabled: minutes.is_some(),
            after_minutes: minutes.unwrap_or(DEFAULT_LOCK_MINUTES),
        })
    }

    /// Turns the lock on (after `after_minutes` away) or off. The screen asks
    /// the device's own unlock first, so only its owner changes it.
    pub fn set_lock(&self, enabled: bool, after_minutes: u32) -> Result<(), EngineError> {
        if !LOCK_CHOICES.contains(&after_minutes) {
            return Err(EngineError::Failed(
                "choose at once, 1, 5 or 15 minutes".into(),
            ));
        }
        let value = if enabled {
            after_minutes.to_string()
        } else {
            String::new()
        };
        self.set_meta_text(APP_LOCK, &value)?;
        if !enabled {
            lock(&self.lock).locked = false;
        }
        Ok(())
    }

    /// Whether the screen must show the lock now. The first time in a
    /// process, it locks if the lock is on.
    pub fn is_locked(&self) -> Result<bool, EngineError> {
        let settings = self.lock_settings()?;
        let mut state = lock(&self.lock);
        if !state.started {
            state.started = true;
            state.locked = settings.enabled;
        }
        if !settings.enabled {
            state.locked = false;
        }
        Ok(state.locked)
    }

    /// The screen went out of sight: the time away starts.
    pub fn screen_left(&self) {
        let mut state = lock(&self.lock);
        state.shown = false;
        if state.left_at.is_none() && !state.authenticating {
            state.left_at = Some(LeftAt::now());
        }
    }

    /// The computer is going to sleep: if the screen is in sight, its time
    /// away starts now.
    pub fn device_sleeping(&self) {
        let mut state = lock(&self.lock);
        if state.shown && state.left_at.is_none() {
            state.left_at = Some(LeftAt::now());
        }
    }

    /// The computer woke: a screen still in sight is back, and locks if it
    /// slept long enough (`LockChanged`). A screen out of sight keeps
    /// counting from when it left.
    pub fn device_woke(&self) {
        let shown = lock(&self.lock).shown;
        if shown {
            let _ = self.screen_returned();
        }
    }

    /// The screen is back: locks if it was away at least the chosen time.
    pub fn screen_returned(&self) -> Result<bool, EngineError> {
        let settings = self.lock_settings()?;
        let newly = {
            let mut state = lock(&self.lock);
            state.shown = true;
            let away = state.left_at.take().map(|t| t.away());
            let due = settings.enabled
                && away.is_some_and(|a| {
                    a >= Duration::from_secs(u64::from(settings.after_minutes) * 60)
                });
            let newly = due && !state.locked && state.started;
            if due {
                state.locked = true;
            }
            newly
        };
        if newly {
            self.notify(UiEvent::LockChanged);
        }
        self.is_locked()
    }

    /// The device's own unlock is showing (`true`) or has answered.
    pub fn authenticating(&self, on: bool) {
        let mut state = lock(&self.lock);
        state.authenticating = on;
        if on {
            state.left_at = None;
        }
    }

    /// The device's own unlock succeeded.
    pub fn unlocked(&self) {
        let mut state = lock(&self.lock);
        state.locked = false;
        state.left_at = None;
    }
}

/// Days since 1970 to a civil date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_files_are_named_by_date() {
        assert_eq!(civil(20_401), (2025, 11, 9));
        assert_eq!(civil(0), (1970, 1, 1));
    }

    #[test]
    fn the_lock_follows_its_settings_and_the_time_away() {
        let dir = std::env::temp_dir().join(format!("mirrorleaf-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = Engine::start(mirrorleaf_net::device::DeviceDir::new(&dir), "Test").unwrap();
        // Off by default.
        assert_eq!(
            engine.lock_settings().unwrap(),
            LockSettings {
                enabled: false,
                after_minutes: 5
            }
        );
        assert!(!engine.is_locked().unwrap());
        // On, locking at once: leaving and coming back locks.
        engine.set_lock(true, 0).unwrap();
        engine.screen_left();
        assert!(engine.screen_returned().unwrap());
        engine.unlocked();
        assert!(!engine.is_locked().unwrap());
        // After 5 minutes: a short absence doesn't lock.
        engine.set_lock(true, 5).unwrap();
        engine.screen_left();
        assert!(!engine.screen_returned().unwrap());
        assert!(engine.set_lock(true, 7).is_err());
        // Sleep and wake count only while the screen is in sight: a closed
        // window keeps counting from when it closed.
        engine.set_lock(true, 0).unwrap();
        engine.device_sleeping();
        engine.device_woke();
        assert!(engine.is_locked().unwrap());
        engine.unlocked();
        engine.set_lock(true, 5).unwrap();
        engine.screen_left();
        engine.device_sleeping();
        engine.device_woke();
        assert!(lock(&engine.lock).left_at.is_some());
        lock(&engine.lock).left_at = Some(LeftAt {
            monotonic: Instant::now(),
            wall: SystemTime::now() - Duration::from_secs(6 * 60),
        });
        assert!(engine.screen_returned().unwrap());
        engine.unlocked();
        // Android's passcode screen covering the app isn't time away.
        engine.set_lock(true, 0).unwrap();
        engine.authenticating(true);
        engine.screen_left();
        engine.authenticating(false);
        engine.unlocked();
        assert!(!engine.screen_returned().unwrap());
        // Turning it off unlocks.
        engine.set_lock(true, 0).unwrap();
        engine.screen_left();
        assert!(engine.screen_returned().unwrap());
        engine.set_lock(false, 5).unwrap();
        assert!(!engine.is_locked().unwrap());

        // Backups: plain by default, no reminder for a new device.
        let settings = engine.backup_settings().unwrap();
        assert!(!settings.encrypt);
        assert_eq!(settings.last_backup, None);
        assert_eq!(settings.due, None);
        engine.set_backup_encryption(true).unwrap();
        assert!(engine.backup_settings().unwrap().encrypt);
        assert!(engine.backup_file_name().ends_with(".mirrorleaf-backup"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_backup_reminder_comes_after_30_days_and_waits_after_later() {
        let dir = std::env::temp_dir().join(format!("mirrorleaf-remind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = Engine::start(mirrorleaf_net::device::DeviceDir::new(&dir), "Test").unwrap();
        let days_ago = |days: u64| (now() - days * 86_400).to_string();
        let due = || engine.backup_settings().unwrap().due;
        // Never backed up: nothing until 30 days, and only with notes.
        engine.set_meta_text(FIRST_OPENED, &days_ago(31)).unwrap();
        assert_eq!(due(), None);
        engine
            .create_note(None, Some("Packing list".into()))
            .unwrap();
        assert_eq!(
            due().as_deref(),
            Some("You haven't backed up your notes yet.")
        );
        // A backup 29 days ago: not yet; 31 days ago: due.
        engine.set_meta_text(LAST_BACKUP, &days_ago(29)).unwrap();
        assert_eq!(due(), None);
        engine.set_meta_text(LAST_BACKUP, &days_ago(31)).unwrap();
        assert_eq!(due().as_deref(), Some("Your last backup was 31 days ago."));
        // "Later" waits a week; a saved backup clears it.
        engine.snooze_backup_reminder().unwrap();
        assert_eq!(due(), None);
        engine.set_meta_text(BACKUP_SNOOZED, "0").unwrap();
        engine.backup_saved().unwrap();
        assert_eq!(due(), None);
        assert!(engine.backup_settings().unwrap().last_backup.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
