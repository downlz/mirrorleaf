//! A device's data folder: its key, its database and its photo store, plus the
//! settings kept in the database. Shared by the CLI and the apps, so every
//! device keeps the same layout (M4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use iroh::SecretKey;
use mirrorleaf_core::{DatabaseKey, Replica, encrypt_in_place};
use mirrorleaf_proto::ledger::Platform;
use mirrorleaf_proto::{DeviceEntry, DeviceId};

use crate::Account;
use crate::blobs::BlobStorage;
use crate::ledger::{DeviceProfile, LedgerClient};

/// The device's secret key, readable by this user only. Since M11 it lives
/// inside the encrypted database instead (`DEVICE_SECRET`); a file left by an
/// earlier build moves there the first time the device opens.
pub const KEY_FILE: &str = "device.key";
pub const DB_FILE: &str = "mirrorleaf.db";
/// The database key, for devices with nowhere safer to keep it: the CLI, and
/// the desktop app on Linux and Windows (M11, E4). Readable by this user only.
pub const DB_KEY_FILE: &str = "database.key";
/// The photo store's folder.
pub const BLOB_DIR: &str = "blobs";
/// Left by signing out (E3): the next start erases this device's key, notes
/// and photos, and sets it up afresh.
pub const ERASE_FILE: &str = "erase-on-start";

// Settings kept in the database's `meta` table.
pub const DEVICE_NAME: &str = "device_name";
/// The device's secret key, inside the encrypted database (M11).
pub const DEVICE_SECRET: &str = "device_secret";
pub const LEDGER_URL: &str = "ledger_url";
pub const LEDGER_PLATFORM: &str = "ledger_platform";
pub const LEDGER_PUSH_TOKEN: &str = "ledger_push_token";
pub const LEDGER_NOTIFICATIONS: &str = "ledger_notifications";

/// One device's data folder.
#[derive(Clone, Debug)]
pub struct DeviceDir {
    path: PathBuf,
    /// The database's key, if it is encrypted (M11, E4).
    key: Option<DatabaseKey>,
}

impl DeviceDir {
    /// A device folder whose database isn't encrypted (tests).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            key: None,
        }
    }

    /// A device folder whose database is encrypted with `key`, kept by the
    /// platform (the Keychain, the Android Keystore).
    pub fn with_key(path: impl Into<PathBuf>, key: DatabaseKey) -> Self {
        Self {
            path: path.into(),
            key: Some(key),
        }
    }

    /// A device folder whose database key is kept in a private file beside
    /// it, made on first use: the CLI, and desktops with no keychain.
    pub fn with_key_file(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = path.join(DB_KEY_FILE);
        let key = match std::fs::read(&file) {
            Ok(bytes) => DatabaseKey::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let db = path.join(DB_FILE);
                if db.exists() && !mirrorleaf_core::is_plain_database(&db) {
                    // A new key couldn't open it, and must not be left here.
                    bail!(
                        "{} is encrypted with a key that isn't in this folder \
                         (the Mac app keeps its key in the Keychain): open it with its own app",
                        db.display()
                    );
                }
                std::fs::create_dir_all(&path)?;
                let key = DatabaseKey::generate();
                write_private(&file, &key.to_bytes())?;
                key
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Self::with_key(path, key))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_set_up(&self) -> bool {
        self.path.join(KEY_FILE).exists() || self.path.join(DB_FILE).exists()
    }

    /// Creates the device's key and database. `name` is what other devices show
    /// for this one; without it, "device <short ID>".
    pub fn init(&self, name: Option<&str>) -> Result<(SecretKey, Replica)> {
        if self.is_set_up() {
            bail!("{} is already set up", self.path.display());
        }
        std::fs::create_dir_all(&self.path)?;
        let secret = SecretKey::generate();
        let device = device_of(&secret);
        let mut replica =
            Replica::open_with_key(self.path.join(DB_FILE), Some(device), self.key.as_ref())?;
        if self.key.is_some() {
            replica.set_meta(DEVICE_SECRET, &secret.to_bytes())?;
        } else {
            write_private(&self.path.join(KEY_FILE), &secret.to_bytes())?;
        }
        let name = name
            .map(str::to_owned)
            .unwrap_or_else(|| format!("device {}", device.short()));
        replica.set_meta(DEVICE_NAME, name.as_bytes())?;
        Ok((secret, replica))
    }

    /// Opens the device's key and database. A database written before M11 is
    /// encrypted the first time, and the device's key moves into it.
    pub fn open(&self) -> Result<(SecretKey, Replica)> {
        if !self.is_set_up() {
            bail!("{} isn't set up", self.path.display());
        }
        let db = self.path.join(DB_FILE);
        let key_file = self.path.join(KEY_FILE);
        let file_secret = match std::fs::read(&key_file) {
            Ok(bytes) => Some(SecretKey::from_bytes(
                &bytes
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("{KEY_FILE} is corrupt"))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(key) = &self.key
            && encrypt_in_place(&db, key)?
        {
            tracing::info!("encrypted this device's database");
        }
        let mut replica =
            Replica::open_with_key(&db, file_secret.as_ref().map(device_of), self.key.as_ref())?;
        let secret = match file_secret {
            Some(secret) => {
                if self.key.is_some() {
                    // Into the encrypted database, then the plain file goes.
                    replica.set_meta(DEVICE_SECRET, &secret.to_bytes())?;
                    std::fs::remove_file(&key_file)?;
                }
                secret
            }
            None => {
                let bytes = replica
                    .meta(DEVICE_SECRET)?
                    .context("this device's key is missing from its database")?;
                let secret = SecretKey::from_bytes(
                    &bytes
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("the stored device key is corrupt"))?,
                );
                if device_of(&secret) != replica.device() {
                    bail!("the stored device key doesn't match this database");
                }
                secret
            }
        };
        // Phase 0's device list becomes version 2 the first time M9 opens it.
        if Account::upgrade(&mut replica)? {
            tracing::info!("converted the account's device list to version 2");
        }
        Ok((secret, replica))
    }

    /// Erases the device on its next start (signing out, E3): done then
    /// rather than now, because the database and photo store are open.
    pub fn erase_on_next_start(&self) -> Result<()> {
        std::fs::write(self.path.join(ERASE_FILE), b"")?;
        Ok(())
    }

    /// Erases this device's key, database and photo store, leaving anything
    /// else in the folder (the app's own files) alone.
    pub fn erase(&self) -> Result<()> {
        // The database key file stays: the next database is made with the same
        // key, which the app already holds (like the Keychain's).
        for file in [
            KEY_FILE.to_owned(),
            DB_FILE.to_owned(),
            format!("{DB_FILE}-wal"),
            format!("{DB_FILE}-shm"),
            ERASE_FILE.to_owned(),
        ] {
            match std::fs::remove_file(self.path.join(file)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        match std::fs::remove_dir_all(self.path.join(BLOB_DIR)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Opens the device, setting it up first if this is its first start, or
    /// afresh after signing out.
    pub fn open_or_init(&self, name: &str) -> Result<(SecretKey, Replica)> {
        if self.path.join(ERASE_FILE).exists() {
            self.erase()?;
            tracing::info!("erased this device after signing out");
        }
        if self.is_set_up() {
            self.open()
        } else {
            self.init(Some(name))
        }
    }

    pub fn blob_storage(&self) -> BlobStorage {
        BlobStorage::Disk(self.path.join(BLOB_DIR))
    }
}

pub fn device_of(secret: &SecretKey) -> DeviceId {
    DeviceId(*secret.public().as_bytes())
}

/// The name this device gave itself.
pub fn device_name(replica: &Replica) -> Result<String> {
    Ok(replica
        .meta(DEVICE_NAME)?
        .map(|n| String::from_utf8_lossy(&n).into_owned())
        .unwrap_or_default())
}

/// This device's entry for the account's device list.
pub fn me(replica: &Replica, secret: &SecretKey) -> Result<DeviceEntry> {
    Ok(DeviceEntry {
        id: device_of(secret),
        name: device_name(replica)?,
        platform: Some(Platform::this()),
        added_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    })
}

/// Each device's name, from the account's signed device list.
pub fn device_names(account: &Account) -> HashMap<DeviceId, String> {
    account
        .devices
        .list
        .devices
        .iter()
        .map(|d| (d.id, d.name.clone()))
        .collect()
}

/// The ledger this device reports to and how it presents itself, if one is set.
pub fn ledger_settings(replica: &Replica) -> Result<Option<(String, DeviceProfile)>> {
    let text = |key| -> Result<String> {
        Ok(replica
            .meta(key)?
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default())
    };
    let url = text(LEDGER_URL)?;
    if url.is_empty() {
        return Ok(None);
    }
    let token = text(LEDGER_PUSH_TOKEN)?;
    let profile = DeviceProfile {
        platform: text(LEDGER_PLATFORM)?
            .parse()
            .unwrap_or_else(|_| Platform::this()),
        push_token: (!token.is_empty()).then_some(token),
        notifications: replica.meta(LEDGER_NOTIFICATIONS)?.as_deref() == Some(&[1]),
    };
    Ok(Some((url, profile)))
}

/// A client for the ledger this device reports to, if one is set.
pub fn ledger_client(
    replica: &Replica,
    secret: &SecretKey,
    account: &Account,
) -> Result<Option<LedgerClient>> {
    match ledger_settings(replica)? {
        Some((url, profile)) => Ok(Some(LedgerClient::new(&url, secret, account, profile)?)),
        None => Ok(None),
    }
}

/// Remembers the ledger and how this device presents itself to it.
pub fn save_ledger(replica: &mut Replica, url: &str, profile: &DeviceProfile) -> Result<()> {
    replica.set_meta(LEDGER_URL, url.as_bytes())?;
    replica.set_meta(LEDGER_PLATFORM, profile.platform.to_string().as_bytes())?;
    replica.set_meta(
        LEDGER_PUSH_TOKEN,
        profile.push_token.as_deref().unwrap_or("").as_bytes(),
    )?;
    replica.set_meta(LEDGER_NOTIFICATIONS, &[u8::from(profile.notifications)])?;
    Ok(())
}

/// Stops using the ledger.
pub fn forget_ledger(replica: &mut Replica) -> Result<()> {
    replica.set_meta(LEDGER_URL, b"")?;
    Ok(())
}

/// Writes the device key readable by this user only.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_from_before_m11_is_encrypted_and_keeps_its_identity() {
        let tmp = std::env::temp_dir().join(format!("mirrorleaf-m11-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // As an M10 build left it: a plain database and a key file.
        let (secret, mut replica) = DeviceDir::new(&tmp).init(Some("laptop")).unwrap();
        replica.create_note("Before M11").unwrap();
        drop(replica);
        assert!(tmp.join(KEY_FILE).exists());

        let dir = DeviceDir::with_key_file(&tmp).unwrap();
        let (again, replica) = dir.open().unwrap();
        assert_eq!(device_of(&again), device_of(&secret), "same device");
        assert_eq!(replica.list_notes().unwrap().len(), 1);
        drop(replica);
        assert!(
            !tmp.join(KEY_FILE).exists(),
            "the key moved into the database"
        );
        assert!(
            !std::fs::read(tmp.join(DB_FILE))
                .unwrap()
                .starts_with(b"SQLite format 3"),
            "the database is encrypted"
        );
        // It opens again from its database alone, but not without the key.
        let (third, _) = DeviceDir::with_key_file(&tmp).unwrap().open().unwrap();
        assert_eq!(device_of(&third), device_of(&secret));
        assert!(DeviceDir::new(&tmp).open().is_err());
        // Without its key file (as the Mac app's folder, whose key is in the
        // Keychain), the CLI refuses rather than leaving a key that can't fit.
        let kept = std::fs::read(tmp.join(DB_KEY_FILE)).unwrap();
        std::fs::remove_file(tmp.join(DB_KEY_FILE)).unwrap();
        let refused = DeviceDir::with_key_file(&tmp).unwrap_err();
        assert!(refused.to_string().contains("Keychain"), "{refused}");
        assert!(!tmp.join(DB_KEY_FILE).exists());
        write_private(&tmp.join(DB_KEY_FILE), &kept).unwrap();

        // Erasing keeps the database key, so a fresh device opens with it.
        dir.erase().unwrap();
        let (fresh, _) = DeviceDir::with_key_file(&tmp)
            .unwrap()
            .open_or_init("new")
            .unwrap();
        assert_ne!(device_of(&fresh), device_of(&secret));
        let _ = DeviceDir::with_key_file(&tmp).unwrap().open().unwrap();
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn a_device_folder_keeps_its_name_and_ledger_settings() {
        let tmp = std::env::temp_dir().join(format!("mirrorleaf-device-{}", std::process::id()));
        let dir = DeviceDir::new(&tmp);
        assert!(!dir.is_set_up());
        let (secret, mut replica) = dir.open_or_init("phone").unwrap();
        assert_eq!(device_name(&replica).unwrap(), "phone");
        assert!(dir.init(None).is_err(), "a second init must refuse");
        assert!(ledger_settings(&replica).unwrap().is_none());

        let profile = DeviceProfile {
            platform: Platform::Android,
            push_token: Some("token".into()),
            notifications: true,
        };
        save_ledger(&mut replica, "http://ledger", &profile).unwrap();
        drop(replica);

        let (again, replica) = dir.open_or_init("ignored").unwrap();
        assert_eq!(device_of(&again), device_of(&secret));
        assert_eq!(device_name(&replica).unwrap(), "phone");
        assert_eq!(
            ledger_settings(&replica).unwrap(),
            Some(("http://ledger".to_owned(), profile))
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
