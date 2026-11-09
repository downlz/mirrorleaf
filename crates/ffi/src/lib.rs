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

pub use engine::{
    DecodedPixels, DeviceSummary, Engine, EngineError, EngineStatus, FolderItem, Invitation,
    NoteItem, NoteList, PairingStatus, PhotoDecoder, PhotoInfo, SyncListener, SyncSummary, UiEvent,
};
