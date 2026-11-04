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

pub use engine::{
    DeviceSummary, Engine, EngineError, EngineStatus, NoteSummary, SyncListener, SyncSummary,
};
