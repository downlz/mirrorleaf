//! mirrorleaf-core: SQLite storage, Yjs (yrs) documents, the change log, the sync
//! engine and photos (implementation plan §5.1–§5.2, §5.5).
//!
//! The change log is the source of truth. Syncing only moves log rows between
//! devices and never loads a Yjs document, so a background sync stays small; a
//! document is rebuilt from its rows when it is opened.

mod batch;
mod blobs;
mod heif;
#[cfg(target_os = "macos")]
mod imageio;
mod notes;
pub mod photos;
mod replica;

pub use batch::{BATCH_WINDOW, Batcher};
pub use blobs::{BLOB_SCHEME, BlobKind, PhotoRef, PhotoStatus};
pub use mirrorleaf_proto::{BlobHash, Change, DeviceId, VersionVector};
pub use notes::{Block, CONTENT_FRAGMENT, INDEX_DOC, NoteMeta};
pub use replica::Replica;
pub use yrs;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("invalid Yjs update: {0}")]
    Update(String),
    #[error("change {seq} from device {origin} skips ahead: only {held} held")]
    Gap {
        origin: DeviceId,
        seq: u64,
        held: u64,
    },
    #[error("this database belongs to device {stored}, not {given}")]
    WrongDevice { stored: String, given: String },
    #[error("no note matches {0}")]
    NoSuchNote(String),
    #[error("{0}")]
    Invalid(String),
    #[error("can't import this photo: {0}")]
    Photo(String),
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
