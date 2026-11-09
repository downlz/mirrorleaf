use serde::{Deserialize, Serialize};

use crate::ledger::Platform;
use crate::{AccountId, Change, DeviceId, ProtoError, SignedDeviceList, VersionVector};

/// The Iroh ALPN for device-to-device sync (plan §5.3). Each protocol version
/// has its own ALPN, so devices that share none fail when the connection
/// opens, before any message is misread. Version 2 (M9) added the device list
/// v2, the change's content type, sync class and time, and the app version in
/// `Hello`. Version 3 (M10) added `Handover`, which gives a device the
/// account's new key.
pub const ALPN: &[u8] = b"mirrorleaf/sync/3";

/// Protocol versions this build speaks, oldest first.
pub const PROTOCOL_VERSIONS: &[u16] = &[3];

/// Largest message accepted from a peer. Change batches stay well below this.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// Sent by both sides when a connection opens (plan §5.3, step 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub versions: Vec<u16>,
    pub device: DeviceId,
    pub account: AccountId,
    pub device_list: SignedDeviceList,
    /// The app's version, so a device can say which one needs updating.
    pub app_version: String,
    pub platform: Platform,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// Boxed: the device list makes it far larger than the other messages.
    Hello(Box<Hello>),
    /// Opens a sync round: what the sender already holds.
    VersionVector(VersionVector),
    /// A batch of changes the receiver lacks, in the sender's log order.
    Changes(Vec<Change>),
    /// The sender has nothing more to send in this round.
    End,
    /// The sender has stored everything the receiver sent before its `End`, so
    /// the round is complete on both sides.
    Ack,
    /// The account's key was replaced (E3): the new key and device list, for
    /// a device on the new list that still uses the old key. Sent right after
    /// `Hello`, over the connection Iroh encrypts between the two device keys.
    Handover(Box<Handover>),
}

/// The account's new key, handed to a device that missed its replacement (E3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handover {
    pub account_secret: [u8; 32],
    /// The new account's list, linked from the old account by the old key's
    /// signature (`SignedDeviceList::verify_replacement_of`).
    pub devices: SignedDeviceList,
}

pub fn encode(message: &Message) -> Vec<u8> {
    postcard::to_stdvec(message).expect("messages always serialise")
}

pub fn decode(bytes: &[u8]) -> Result<Message, ProtoError> {
    if bytes.len() > MAX_FRAME_LEN {
        return Err(ProtoError::TooLarge(bytes.len()));
    }
    Ok(postcard::from_bytes(bytes)?)
}

/// The highest protocol version both sides speak, if any. With none, the older
/// device must show "update needed" (plan §5.3).
pub fn choose_version(theirs: &[u16]) -> Option<u16> {
    PROTOCOL_VERSIONS
        .iter()
        .rev()
        .find(|v| theirs.contains(v))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let change = Change::yjs(DeviceId([7; 32]), 3, "index", vec![1, 2, 3], 1_794_160_119);
        let msg = Message::Changes(vec![change]);
        assert_eq!(decode(&encode(&msg)).unwrap(), msg);
    }

    #[test]
    fn version_choice_needs_overlap() {
        assert_eq!(choose_version(&[2, 3]), Some(3));
        assert_eq!(choose_version(&[2]), None);
    }
}
