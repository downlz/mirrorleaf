//! mirrorleaf-ffi: the engine the apps drive (implementation plan §5.7).
//!
//! One engine runs per process. On Android the Kotlin background code (the
//! push-woken sync job) and the Tauri screen share it, so a single sync engine
//! owns the database and the photo store whichever of them started the process.
//! The Kotlin side reaches it through uniffi; the Tauri side calls it directly.

uniffi::setup_scaffolding!();

#[cfg(target_os = "android")]
mod android;
mod engine;

/// The app's version, which devices tell each other when they connect and
/// when one joins the account. Call it once, before opening the engine.
#[uniffi::export]
pub fn set_app_version(version: String) {
    mirrorleaf_net::set_app_version(&version);
}

/// A new random database key (32 bytes), for the platform to keep (M11).
#[uniffi::export]
pub fn new_database_key() -> Vec<u8> {
    mirrorleaf_core::DatabaseKey::generate().to_bytes().to_vec()
}

/// The database key kept in a private file in `data_dir`, made on first use:
/// for desktops with no keychain (Linux; Windows until DPAPI).
pub fn database_key_file(data_dir: &str) -> Result<Vec<u8>, EngineError> {
    let path = std::path::Path::new(data_dir).join(mirrorleaf_net::device::DB_KEY_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = new_database_key();
            std::fs::create_dir_all(data_dir).map_err(|e| EngineError::Failed(e.to_string()))?;
            write_private(&path, &key).map_err(|e| EngineError::Failed(e.to_string()))?;
            Ok(key)
        }
        Err(e) => Err(EngineError::Failed(e.to_string())),
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

pub use engine::{
    BackupDone, BackupInfo, BackupSettings, DecodedPixels, DeviceSummary, Engine, EngineError,
    EngineStatus, FolderItem, Invitation, KitView, LockSettings, NoteItem, NoteList, PairingStatus,
    PhotoDecoder, PhotoInfo, SearchResult, SyncListener, SyncSummary, UiEvent,
};
/// A backup file's extension, without the dot.
pub use mirrorleaf_net::backup::EXTENSION as BACKUP_EXTENSION;
/// The database key file of CLI and Linux devices (`database.key`).
pub use mirrorleaf_net::device::DB_KEY_FILE;
