use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use data_encoding::BASE32_NOPAD;
use iroh::EndpointAddr;
use mirrorleaf_proto::{AccountKey, SignedDeviceList};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "mlt1";

/// A Phase 0 join ticket (plan §5.4): the account key, the current device list
/// and how to reach the inviting device. It carries the account key, so treat it
/// like a password. Real QR pairing with approval replaces it in Phase 1 (E1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevTicket {
    pub account_secret: [u8; 32],
    pub devices: SignedDeviceList,
    pub inviter: EndpointAddr,
}

impl DevTicket {
    pub fn account_key(&self) -> AccountKey {
        AccountKey::from_bytes(self.account_secret)
    }
}

impl fmt::Display for DevTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = postcard::to_stdvec(self).map_err(|_| fmt::Error)?;
        write!(f, "{PREFIX}{}", BASE32_NOPAD.encode(&bytes).to_lowercase())
    }
}

impl FromStr for DevTicket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let Some(body) = s.trim().strip_prefix(PREFIX) else {
            bail!("not a Mirrorleaf join ticket");
        };
        let bytes = BASE32_NOPAD
            .decode(body.to_uppercase().as_bytes())
            .context("the ticket is damaged")?;
        postcard::from_bytes(&bytes).context("the ticket is damaged")
    }
}
