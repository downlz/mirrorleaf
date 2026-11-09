//! A device's data folder: its key, its database and its photo store, plus the
//! settings kept in the database. Shared by the CLI and the apps, so every
//! device keeps the same layout (M4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use iroh::SecretKey;
use mirrorleaf_core::Replica;
use mirrorleaf_proto::ledger::Platform;
use mirrorleaf_proto::{DeviceEntry, DeviceId};

use crate::Account;
use crate::blobs::BlobStorage;
use crate::ledger::{DeviceProfile, LedgerClient};

/// The device's secret key, readable by this user only.
pub const KEY_FILE: &str = "device.key";
pub const DB_FILE: &str = "mirrorleaf.db";
/// The photo store's folder.
pub const BLOB_DIR: &str = "blobs";
/// Left by signing out (E3): the next start erases this device's key, notes
/// and photos, and sets it up afresh.
pub const ERASE_FILE: &str = "erase-on-start";

// Settings kept in the database's `meta` table.
pub const DEVICE_NAME: &str = "device_name";
pub const LEDGER_URL: &str = "ledger_url";
pub const LEDGER_PLATFORM: &str = "ledger_platform";
pub const LEDGER_PUSH_TOKEN: &str = "ledger_push_token";
pub const LEDGER_NOTIFICATIONS: &str = "ledger_notifications";

/// One device's data folder.
#[derive(Clone, Debug)]
pub struct DeviceDir {
    path: PathBuf,
}

impl DeviceDir {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_set_up(&self) -> bool {
        self.path.join(KEY_FILE).exists()
    }

    /// Creates the device's key and database. `name` is what other devices show
    /// for this one; without it, "device <short ID>".
    pub fn init(&self, name: Option<&str>) -> Result<(SecretKey, Replica)> {
        let key_path = self.path.join(KEY_FILE);
        if key_path.exists() {
            bail!("{} is already set up", self.path.display());
        }
        std::fs::create_dir_all(&self.path)?;
        let secret = SecretKey::generate();
        write_private(&key_path, &secret.to_bytes())?;
        let device = device_of(&secret);
        let mut replica = Replica::open(self.path.join(DB_FILE), device)?;
        let name = name
            .map(str::to_owned)
            .unwrap_or_else(|| format!("device {}", device.short()));
        replica.set_meta(DEVICE_NAME, name.as_bytes())?;
        Ok((secret, replica))
    }

    /// Opens the device's key and database.
    pub fn open(&self) -> Result<(SecretKey, Replica)> {
        let bytes = std::fs::read(self.path.join(KEY_FILE))
            .with_context(|| format!("{} isn't set up", self.path.display()))?;
        let secret = SecretKey::from_bytes(
            &bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("{KEY_FILE} is corrupt"))?,
        );
        let mut replica = Replica::open(self.path.join(DB_FILE), device_of(&secret))?;
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
