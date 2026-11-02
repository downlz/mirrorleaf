//! mirrorleaf-proto: wire messages, protocol versioning and request signing, shared
//! by the devices and the ledger (implementation plan §5.3, §5.8).

mod change;
mod devices;
mod ids;
mod wire;

pub use change::{Change, VersionVector};
pub use devices::{AccountKey, DeviceEntry, DeviceList, DeviceListMerge, SignedDeviceList};
pub use ids::{AccountId, BlobHash, DeviceId};
pub use wire::{
    ALPN, Hello, MAX_FRAME_LEN, Message, PROTOCOL_VERSIONS, choose_version, decode, encode,
};

/// Errors from decoding or verifying protocol data.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("malformed message: {0}")]
    Malformed(#[from] postcard::Error),
    #[error("message of {0} bytes is over the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(usize),
    #[error("device list signature doesn't match the account key")]
    BadSignature,
    #[error("device list belongs to a different account")]
    WrongAccount,
    #[error("invalid {0}")]
    Invalid(&'static str),
}
