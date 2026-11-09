//! Adding a device (E1, assessment §8.2): the messages two devices exchange
//! over the pairing ALPN, and what both derive from the key their PAKE agrees.
//!
//! The new device (the joiner) dials the device showing the code (the
//! inviter). Both run SPAKE2 with the code's secret as the password and their
//! device IDs as identities, prove to each other that they derived the same
//! key, and show the same four-digit number. Only once the person allows it on
//! the inviter does the inviter send the account key.

use serde::{Deserialize, Serialize};

use crate::ledger::Platform;
use crate::{ProtoError, SignedDeviceList};

/// The Iroh ALPN for pairing.
pub const PAIR_ALPN: &[u8] = b"mirrorleaf/pair/1";

/// How long a pairing code works (assessment §8.2).
pub const CODE_LIFETIME_SECS: u64 = 10 * 60;

/// Largest pairing message accepted.
pub const MAX_PAIR_MESSAGE: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairMessage {
    /// Joiner to inviter: which code it is using, and its SPAKE2 message.
    Start { via: CodeKind, pake: Vec<u8> },
    /// Inviter to joiner: its SPAKE2 message.
    Reply { pake: Vec<u8> },
    /// Proof that the sender derived the same key. Each side has its own value
    /// ([`confirmation`]), so one can't simply echo the other's.
    Confirm { proof: [u8; 32] },
    /// Joiner to inviter, once both proofs check out: who is asking to join.
    Request {
        name: String,
        platform: Platform,
        app_version: String,
    },
    /// Inviter to joiner, once the person allowed it: the account key, the
    /// device list with the joiner on it, the account key's approval of the
    /// joiner for the ledger (kept so it can register again after the
    /// ledger's database is lost), and the ledger the inviter uses.
    Welcome {
        account_secret: [u8; 32],
        /// Boxed: far larger than the other messages.
        devices: Box<SignedDeviceList>,
        approval: String,
        ledger: Option<String>,
    },
    /// The pairing ends without joining, and why.
    Refused { reason: Refusal },
    /// Joiner to inviter: the joiner stored everything; the inviter may hang up.
    Done,
}

/// Which of the inviter's two codes the joiner is using: the QR code's secret
/// or the typed words. Each is a different PAKE password.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeKind {
    Qr,
    Words,
}

/// Why a pairing ended without the device joining.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// The two sides didn't derive the same key: the code was typed wrongly,
    /// or someone guessed. The code stops working.
    WrongCode,
    /// The code ran out, was used already, or was cancelled.
    CodeExpired,
    /// The person said no on the inviting device.
    Denied,
    /// The person didn't answer in time.
    NoAnswer,
    /// The account already has the most devices it may hold.
    AccountFull,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::WrongCode => "the code didn't match; make a new one on your other device",
            Refusal::CodeExpired => {
                "the code has run out or was used already; make a new one on your other device"
            }
            Refusal::Denied => "your other device didn't allow this one to join",
            Refusal::NoAnswer => "nobody answered on your other device in time",
            Refusal::AccountFull => "your account already has the most devices it can hold",
        })
    }
}

pub fn encode(message: &PairMessage) -> Vec<u8> {
    postcard::to_stdvec(message).expect("messages always serialise")
}

pub fn decode(bytes: &[u8]) -> Result<PairMessage, ProtoError> {
    if bytes.len() > MAX_PAIR_MESSAGE {
        return Err(ProtoError::TooLarge(bytes.len()));
    }
    Ok(postcard::from_bytes(bytes)?)
}

/// The proof one side sends that it derived `key`. `joiner` says whose proof:
/// the two differ, so neither side can send back what it received.
pub fn confirmation(key: &[u8], joiner: bool) -> [u8; 32] {
    let context = if joiner {
        "mirrorleaf pair 1: joiner confirms the key"
    } else {
        "mirrorleaf pair 1: inviter confirms the key"
    };
    blake3::derive_key(context, key)
}

/// Whether `proof` is the other side's proof of `key`, compared in constant
/// time.
pub fn check_confirmation(key: &[u8], from_joiner: bool, proof: &[u8; 32]) -> bool {
    blake3::Hash::from(confirmation(key, from_joiner)) == blake3::Hash::from(*proof)
}

/// The four-digit number both screens show (assessment §8.2).
pub fn confirmation_number(key: &[u8]) -> String {
    let bytes = blake3::derive_key("mirrorleaf pair 1: confirmation number", key);
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 10_000;
    format!("{n:04}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proofs_differ_by_side_and_by_key() {
        let key = [7u8; 32];
        let joiner = confirmation(&key, true);
        let inviter = confirmation(&key, false);
        assert_ne!(joiner, inviter);
        assert!(check_confirmation(&key, true, &joiner));
        assert!(!check_confirmation(&key, false, &joiner));
        assert!(!check_confirmation(&[8u8; 32], true, &joiner));
    }

    #[test]
    fn the_confirmation_number_has_four_digits() {
        for k in 0..50u8 {
            let n = confirmation_number(&[k; 32]);
            assert_eq!(n.len(), 4, "{n}");
            assert!(n.chars().all(|c| c.is_ascii_digit()));
        }
        assert_eq!(confirmation_number(&[1; 32]), confirmation_number(&[1; 32]));
    }

    #[test]
    fn messages_round_trip() {
        let m = PairMessage::Refused {
            reason: Refusal::WrongCode,
        };
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }
}
