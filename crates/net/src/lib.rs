//! mirrorleaf-net: the Iroh endpoint, the sync and blob protocols, and peer
//! authentication. All Iroh use stays inside this crate (implementation plan §5.3).

mod account;
pub mod backup;
mod blobs;
pub mod device;
mod frame;
pub mod kit;
pub mod ledger;
mod node;
pub mod pair;
pub mod recovery;
mod session;
mod ticket;

pub use account::Account;
pub use blobs::{BlobStorage, PhotoStore, read_blob};
pub use frame::Traffic;
pub use iroh::SecretKey;
pub use node::{
    BlobFailure, BlobReport, Event, FetchRound, Network, PathKind, State, SyncNode, endpoint_addr,
    path_kind,
};
pub use session::RoundReport;
pub use ticket::DevTicket;

static APP_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Sets the app's version, which devices tell each other when they connect
/// (`Hello`) and when pairing. Set once, at startup; later calls are ignored.
pub fn set_app_version(version: &str) {
    let _ = APP_VERSION.set(version.to_owned());
}

/// The app's version, or this crate's if the app didn't set one (the CLI).
pub fn app_version() -> &'static str {
    APP_VERSION
        .get()
        .map(String::as_str)
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// An error and its causes on one line. Iroh often repeats a cause's text in
/// the error that wraps it, so causes already shown are left out.
pub fn describe(error: &anyhow::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for cause in error.chain() {
        let text = cause.to_string();
        if !parts.iter().any(|shown| shown.contains(&text)) {
            parts.push(text);
        }
    }
    parts.join(": ")
}

/// Why a peer was refused (plan §5.3, step 2).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("the peer speaks no protocol version we know: one side needs an update")]
    UpdateNeeded,
    #[error("the peer claims a device ID that isn't its connection key")]
    WrongKey,
    #[error("the peer belongs to another account")]
    OtherAccount,
    #[error("the peer isn't on this account's device list")]
    NotOnList,
    #[error("this device was removed from the account")]
    Removed,
    #[error("the peer's device list isn't validly signed: {0}")]
    BadList(mirrorleaf_proto::ProtoError),
}
