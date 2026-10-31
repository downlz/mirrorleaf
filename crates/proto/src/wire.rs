use serde::{Deserialize, Serialize};

use crate::{AccountId, Change, DeviceId, ProtoError, SignedDeviceList, VersionVector};

/// The Iroh ALPN for device-to-device sync (plan §5.3).
pub const ALPN: &[u8] = b"mirrorleaf/sync/1";

/// Protocol versions this build speaks, oldest first.
pub const PROTOCOL_VERSIONS: &[u16] = &[1];

/// Largest message accepted from a peer. Change batches stay well below this.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// Sent by both sides when a connection opens (plan §5.3, step 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub versions: Vec<u16>,
    pub device: DeviceId,
    pub account: AccountId,
    pub device_list: SignedDeviceList,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    Hello(Hello),
    /// Opens a sync round: what the sender already holds.
    VersionVector(VersionVector),
    /// A batch of changes the receiver lacks, in the sender's log order.
    Changes(Vec<Change>),
    /// The sender has nothing more to send in this round.
    End,
    /// The sender has stored everything the receiver sent before its `End`, so
    /// the round is complete on both sides.
    Ack,
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
        let change = Change {
            origin: DeviceId([7; 32]),
            seq: 3,
            doc_id: "index".into(),
            update: vec![1, 2, 3],
        };
        let msg = Message::Changes(vec![change]);
        assert_eq!(decode(&encode(&msg)).unwrap(), msg);
    }

    #[test]
    fn version_choice_needs_overlap() {
        assert_eq!(choose_version(&[1, 2]), Some(1));
        assert_eq!(choose_version(&[2]), None);
    }
}
