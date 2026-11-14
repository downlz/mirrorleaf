//! The sign-up puzzle (M13, E10): a small proof of work a device solves before
//! the ledger creates an account for it, or gives it a pass to the relay.
//! About a second on a phone, once; it makes creating accounts by the
//! thousand slow without asking anyone for an email address or an invitation
//! (owner, 12 Nov 2026). The ledger raises the difficulty when sign-ups spike
//! instead of refusing people.
//!
//! The answer is a nonce such that BLAKE3 over the subject (the account, or
//! "relay pass"), the device, the time and the nonce starts with the number
//! of zero bits the ledger asks for. It is bound to the device and the time,
//! so an answer can't be reused by another device or kept for later.

use serde::{Deserialize, Serialize};

use crate::{AccountId, DeviceId};

/// How far an answer's time may be from the ledger's clock.
pub const MAX_AGE_SECS: i64 = 10 * 60;

const CONTEXT: &str = "mirrorleaf 2026-11 sign-up puzzle";

/// What a puzzle answer is for.
#[derive(Clone, Copy, Debug)]
pub enum Subject<'a> {
    /// Creating this account.
    Account(&'a AccountId),
    /// A pass to the relay before joining an account.
    RelayPass,
}

/// A puzzle's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Solution {
    /// When it was solved, as Unix seconds.
    pub time: i64,
    pub nonce: u64,
}

fn key() -> [u8; 32] {
    blake3::derive_key(CONTEXT, b"")
}

/// Everything hashed but the nonce, which fills the last 8 bytes.
fn input(subject: Subject<'_>, device: &DeviceId, time: i64) -> Vec<u8> {
    let mut input = Vec::with_capacity(90);
    match subject {
        Subject::Account(account) => {
            input.push(1);
            input.extend(account.0);
        }
        Subject::RelayPass => input.push(2),
    }
    input.extend(device.0);
    input.extend(time.to_le_bytes());
    input.extend([0; 8]);
    input
}

fn zero_bits(hash: &[u8; 32]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        if *byte == 0 {
            bits += 8;
        } else {
            return bits + byte.leading_zeros();
        }
    }
    bits
}

/// Solves the puzzle at difficulty `bits`: about 2^bits hashes, which is
/// about a second at 21 bits on a phone. Run it off the async runtime.
pub fn solve(subject: Subject<'_>, device: &DeviceId, time: i64, bits: u32) -> Solution {
    let key = key();
    let mut input = input(subject, device, time);
    let at = input.len() - 8;
    let mut nonce: u64 = rand_start(device, time);
    loop {
        input[at..].copy_from_slice(&nonce.to_le_bytes());
        if zero_bits(blake3::keyed_hash(&key, &input).as_bytes()) >= bits {
            return Solution { time, nonce };
        }
        nonce = nonce.wrapping_add(1);
    }
}

/// Where a search starts: anywhere does; this keeps the crate free of a
/// random number generator.
fn rand_start(device: &DeviceId, time: i64) -> u64 {
    let hash = blake3::hash(&[&device.0[..], &time.to_le_bytes()].concat());
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap_or_default())
}

impl Solution {
    /// Whether this answers the puzzle for `subject` and `device` at
    /// difficulty `bits`, and is recent enough at `now`.
    pub fn checks(&self, subject: Subject<'_>, device: &DeviceId, bits: u32, now: i64) -> bool {
        if (self.time - now).abs() > MAX_AGE_SECS {
            return false;
        }
        let mut input = input(subject, device, self.time);
        let at = input.len() - 8;
        input[at..].copy_from_slice(&self.nonce.to_le_bytes());
        zero_bits(blake3::keyed_hash(&key(), &input).as_bytes()) >= bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_checks_only_for_its_account_device_and_time() {
        let (account, device, other) = (AccountId([1; 32]), DeviceId([2; 32]), DeviceId([3; 32]));
        let now = 1_800_000_000;
        let started = std::time::Instant::now();
        let solution = solve(Subject::Account(&account), &device, now, 16);
        assert!(started.elapsed().as_secs() < 5);
        assert!(solution.checks(Subject::Account(&account), &device, 16, now));
        assert!(solution.checks(Subject::Account(&account), &device, 16, now + 60));
        assert!(!solution.checks(Subject::Account(&account), &other, 16, now));
        assert!(!solution.checks(Subject::Account(&AccountId([9; 32])), &device, 16, now));
        assert!(!solution.checks(Subject::RelayPass, &device, 16, now));
        assert!(!solution.checks(Subject::Account(&account), &device, 16, now + 11 * 60));
        // Zero bits always checks; far more than solved for almost never does.
        assert!(solution.checks(Subject::Account(&account), &device, 0, now));
        assert!(!solution.checks(Subject::Account(&account), &device, 40, now));
    }

    #[test]
    fn zero_bits_are_counted_from_the_start() {
        let mut hash = [0u8; 32];
        hash[0] = 0b0001_0000;
        assert_eq!(zero_bits(&hash), 3);
        hash[0] = 0;
        hash[1] = 1;
        assert_eq!(zero_bits(&hash), 15);
    }
}
