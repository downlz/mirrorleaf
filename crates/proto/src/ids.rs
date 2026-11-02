use std::fmt;
use std::str::FromStr;

use data_encoding::HEXLOWER;
use serde::{Deserialize, Serialize};

use crate::ProtoError;

macro_rules! key_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub [u8; 32]);

        impl $name {
            /// The first 8 hex characters, for logs and the CLI.
            pub fn short(&self) -> String {
                HEXLOWER.encode(&self.0[..4])
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&HEXLOWER.encode(&self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.short())
            }
        }

        impl FromStr for $name {
            type Err = ProtoError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let bytes = HEXLOWER
                    .decode(s.as_bytes())
                    .map_err(|_| ProtoError::Invalid(stringify!($name)))?;
                let bytes = bytes
                    .try_into()
                    .map_err(|_| ProtoError::Invalid(stringify!($name)))?;
                Ok(Self(bytes))
            }
        }
    };
}

key_id!(
    /// A device's public key. It is also the device's Iroh endpoint ID, so a peer
    /// authenticated by Iroh is authenticated as this device.
    DeviceId
);

key_id!(
    /// The account's public key, which signs the device list.
    AccountId
);

key_id!(
    /// The BLAKE3 hash of a blob's content, which names it in the blob store
    /// (plan §5.5).
    BlobHash
);
