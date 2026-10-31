use std::ops::AddAssign;

use anyhow::{Result, bail};
use iroh::endpoint::{RecvStream, SendStream};
use mirrorleaf_proto::{MAX_FRAME_LEN, Message, ProtoError, decode, encode};

/// Bytes of protocol messages sent and received, framing included. These are the
/// core's byte counters for the "≤ 5 KB per text edit" target (plan §7.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    pub sent: u64,
    pub received: u64,
}

impl Traffic {
    pub fn total(&self) -> u64 {
        self.sent + self.received
    }
}

impl AddAssign for Traffic {
    fn add_assign(&mut self, other: Self) {
        self.sent += other.sent;
        self.received += other.received;
    }
}

/// Writes one message as a 4-byte big-endian length and a postcard body.
pub(crate) async fn write(send: &mut SendStream, message: &Message, sent: &mut u64) -> Result<()> {
    let body = encode(message);
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    send.write_all(&frame).await?;
    *sent += frame.len() as u64;
    Ok(())
}

pub(crate) async fn read(recv: &mut RecvStream, received: &mut u64) -> Result<Message> {
    let mut len = [0; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME_LEN {
        bail!(ProtoError::TooLarge(len));
    }
    let mut body = vec![0; len];
    recv.read_exact(&mut body).await?;
    *received += 4 + len as u64;
    Ok(decode(&body)?)
}
