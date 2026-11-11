//! The encrypted database (M11, E4): SQLCipher with a random 256-bit key that
//! the platform keeps (the Keychain on the Mac, the Android Keystore, a key
//! file for the CLI). A database written before M11 is converted once, in
//! place.

use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::{CoreError, Result};

/// The database's key. Its `Debug` form hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseKey([u8; 32]);

impl fmt::Debug for DatabaseKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DatabaseKey(…)")
    }
}

impl DatabaseKey {
    /// A new random key (the operating system's randomness, through `rand`'s
    /// cryptographically secure generator).
    pub fn generate() -> Self {
        Self(rand::random())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// From a key the platform kept: exactly 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CoreError::Invalid("a database key is 32 bytes".into()))?;
        Ok(Self(bytes))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// SQLCipher's raw-key form, `x'…'`, which skips its key derivation: the
    /// key is random, not a password.
    fn pragma(&self) -> String {
        let hex: String = self.0.iter().map(|b| format!("{b:02x}")).collect();
        format!("x'{hex}'")
    }
}

/// Unlocks a connection with `key`, before anything else touches it, and
/// checks the key fits.
pub(crate) fn unlock(conn: &Connection, key: &DatabaseKey) -> Result<()> {
    conn.pragma_update(None, "key", key.pragma())?;
    readable(conn).map(drop).map_err(|_| {
        CoreError::Invalid(
            "the database can't be opened with this device's key (or it is damaged)".into(),
        )
    })
}

/// Whether the connection can read the database (with the key it was given).
fn readable(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))
}

/// Whether the file at `path` is a plain SQLite database (written before
/// M11): it starts with SQLite's own header, which SQLCipher encrypts.
pub fn is_plain_database(path: &Path) -> bool {
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut header))
        .is_ok_and(|()| &header == b"SQLite format 3\0")
}

/// Encrypts the database at `path` with `key` if it is still a plain one
/// (written before M11). Returns whether it converted anything. The plain file
/// is kept until the encrypted copy opens with the key, and a conversion cut
/// short is undone at the next try.
pub fn encrypt_in_place(path: &Path, key: &DatabaseKey) -> Result<bool> {
    let plain_copy = suffixed(path, "plain");
    let encrypting = suffixed(path, "encrypting");
    // A conversion stopped between its two renames: put the plain file back.
    if !path.exists() && plain_copy.exists() {
        std::fs::rename(&plain_copy, path).map_err(io)?;
    }
    if !path.exists() {
        return Ok(false);
    }
    let version = {
        let conn = Connection::open(path)?;
        if readable(&conn).is_err() {
            // Not plain: already encrypted (or not a database at all).
            return Ok(false);
        }
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        // Everything into the main file, so the copy misses nothing.
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .optional()?;
        let _ = std::fs::remove_file(&encrypting);
        conn.execute(
            "ATTACH DATABASE ?1 AS encrypted KEY ?2",
            rusqlite::params![encrypting.to_string_lossy(), key.pragma()],
        )?;
        conn.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
        conn.pragma_update(Some("encrypted"), "user_version", version)?;
        conn.execute("DETACH DATABASE encrypted", [])?;
        version
    };
    // The encrypted copy must open with the key before the plain file goes.
    {
        let conn = Connection::open(&encrypting)?;
        unlock(&conn, key)?;
        let copied: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if copied != version {
            return Err(CoreError::Invalid(
                "the encrypted copy of the database didn't match".into(),
            ));
        }
    }
    std::fs::rename(path, &plain_copy).map_err(io)?;
    for side in ["wal", "shm"] {
        let _ = std::fs::remove_file(suffixed_raw(path, side));
    }
    std::fs::rename(&encrypting, path).map_err(io)?;
    std::fs::remove_file(&plain_copy).map_err(io)?;
    Ok(true)
}

/// `mirrorleaf.db` → `mirrorleaf.db.<suffix>`.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{suffix}"));
    PathBuf::from(name)
}

/// SQLite's side files: `mirrorleaf.db-wal`, `mirrorleaf.db-shm`.
fn suffixed_raw(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!("-{suffix}"));
    PathBuf::from(name)
}

fn io(e: std::io::Error) -> CoreError {
    CoreError::Invalid(format!("couldn't convert the database: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceId, Replica};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mirrorleaf-enc-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("mirrorleaf.db")
    }

    #[test]
    fn only_the_right_key_opens_the_database() {
        let path = temp("keys");
        let key = DatabaseKey::generate();
        let me = DeviceId([1; 32]);
        {
            let mut replica = Replica::open_with_key(&path, Some(me), Some(&key)).unwrap();
            replica.create_note("Private").unwrap();
        }
        // Not plain SQLite any more: the bytes on disk don't say "SQLite".
        let head = std::fs::read(&path).unwrap();
        assert!(!head.starts_with(b"SQLite format 3"));
        assert!(Replica::open(&path, me).is_err(), "no key");
        assert!(
            Replica::open_with_key(&path, Some(me), Some(&DatabaseKey::generate())).is_err(),
            "a wrong key"
        );
        // The right key, without even saying which device: the database knows.
        let replica = Replica::open_with_key(&path, None, Some(&key)).unwrap();
        assert_eq!(replica.device(), me);
        assert_eq!(replica.list_notes().unwrap().len(), 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_plain_database_is_encrypted_in_place_once() {
        let path = temp("convert");
        let me = DeviceId([2; 32]);
        let note = {
            let mut replica = Replica::open(&path, me).unwrap();
            replica.create_note("Written before M11").unwrap()
        };
        let key = DatabaseKey::generate();
        assert!(is_plain_database(&path));
        assert!(encrypt_in_place(&path, &key).unwrap());
        assert!(!is_plain_database(&path));
        assert!(!encrypt_in_place(&path, &key).unwrap(), "already encrypted");
        let replica = Replica::open_with_key(&path, Some(me), Some(&key)).unwrap();
        assert!(replica.list_notes().unwrap().iter().any(|n| n.id == note));
        let version: i64 = replica
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        drop(replica);
        assert!(Replica::open(&path, me).is_err(), "no plain copy left");
        let dir = path.parent().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("plain") || n.contains("encrypting"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_conversion_cut_short_is_finished_next_time() {
        let path = temp("interrupted");
        let me = DeviceId([3; 32]);
        {
            let mut replica = Replica::open(&path, me).unwrap();
            replica.create_note("Kept").unwrap();
        }
        // As if the app stopped between moving the plain file aside and
        // putting the encrypted one in its place.
        std::fs::rename(&path, suffixed(&path, "plain")).unwrap();
        let key = DatabaseKey::generate();
        assert!(encrypt_in_place(&path, &key).unwrap());
        let replica = Replica::open_with_key(&path, Some(me), Some(&key)).unwrap();
        assert_eq!(replica.list_notes().unwrap().len(), 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
