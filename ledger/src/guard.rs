//! Protections for a ledger anyone may use (M13, E10). The owner wants no
//! sign-up restriction (12 Nov 2026: no invite codes), so abuse is made slow
//! and visible instead:
//!
//! - **The sign-up puzzle** ([`mirrorleaf_proto::puzzle`]) before a new
//!   account or a relay pass, harder while sign-ups spike;
//! - **limits by IP address, in memory only**, on the three requests a new
//!   key can make (creating an account, looking up a word code, a relay
//!   pass). Generous, as mobile networks put many people behind one address.
//!   Addresses are never logged or stored, and leave memory within the hour;
//! - **a replayed request is refused**: every signature seen in the last few
//!   minutes is kept, and a request whose time is older than that is refused
//!   anyway (P4).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mirrorleaf_proto::ledger::MAX_CLOCK_SKEW_SECS;

/// Which request a per-address limit counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IpKind {
    SignUp,
    Lookup,
    Pass,
}

/// Requests per hour from one address; 0 for no limit.
#[derive(Clone, Copy, Debug)]
pub struct IpRates {
    pub sign_ups: u32,
    pub lookups: u32,
    pub passes: u32,
}

impl Default for IpRates {
    fn default() -> Self {
        Self {
            sign_ups: 30,
            lookups: 120,
            passes: 30,
        }
    }
}

impl IpRates {
    pub const OFF: IpRates = IpRates {
        sign_ups: 0,
        lookups: 0,
        passes: 0,
    };

    fn of(&self, kind: IpKind) -> u32 {
        match kind {
            IpKind::SignUp => self.sign_ups,
            IpKind::Lookup => self.lookups,
            IpKind::Pass => self.passes,
        }
    }
}

/// The default puzzle difficulty: about a second on a phone.
pub const DEFAULT_PUZZLE_BITS: u32 = 21;
const MAX_PUZZLE_BITS: u32 = 30;

#[derive(Clone, Copy, Debug)]
struct Window {
    tokens: f64,
    at: Instant,
}

/// An address as limited: IPv4 whole, IPv6 by its /64 (one household or
/// phone gets a whole /64).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Address {
    V4([u8; 4]),
    V6([u8; 8]),
}

fn address(ip: IpAddr) -> Address {
    match ip {
        IpAddr::V4(v4) => Address::V4(v4.octets()),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => Address::V4(v4.octets()),
            None => Address::V6(v6.octets()[..8].try_into().unwrap_or_default()),
        },
    }
}

pub struct Guard {
    rates: IpRates,
    windows: Mutex<HashMap<(IpKind, Address), Window>>,
    seen: Mutex<HashMap<String, Instant>>,
    sign_ups: Mutex<VecDeque<Instant>>,
    /// The puzzle's difficulty while sign-ups are quiet; 0 turns it off.
    puzzle_bits: u32,
}

impl Guard {
    pub fn new(puzzle_bits: u32, rates: IpRates) -> Self {
        Self {
            rates,
            windows: Mutex::default(),
            seen: Mutex::default(),
            sign_ups: Mutex::default(),
            puzzle_bits: puzzle_bits.min(MAX_PUZZLE_BITS),
        }
    }

    /// No puzzle and no limits by address (tests); replays are still refused.
    pub fn off() -> Self {
        Self::new(0, IpRates::OFF)
    }

    /// Counts a request of `kind` from `ip`; `false` if over the hour's limit.
    /// Without an address (no proxy header and no connection info) nothing
    /// is counted.
    pub fn ip_request(&self, kind: IpKind, ip: Option<IpAddr>) -> bool {
        let per_hour = self.rates.of(kind);
        let Some(ip) = ip.filter(|_| per_hour > 0) else {
            return true;
        };
        let capacity = f64::from(per_hour);
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap();
        let window = windows.entry((kind, address(ip))).or_insert(Window {
            tokens: capacity,
            at: now,
        });
        let earned = now.saturating_duration_since(window.at).as_secs_f64() * capacity / 3600.0;
        window.tokens = (window.tokens + earned).min(capacity);
        window.at = now;
        if window.tokens >= 1.0 {
            window.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Whether this signature is new; `false` for a request sent again.
    pub fn first_time(&self, signature: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if seen.contains_key(signature) {
            return false;
        }
        seen.insert(signature.to_owned(), Instant::now());
        true
    }

    /// Notes a new account, for the puzzle's difficulty.
    pub fn sign_up(&self) {
        let mut sign_ups = self.sign_ups.lock().unwrap();
        sign_ups.push_back(Instant::now());
        if sign_ups.len() > 100_000 {
            sign_ups.pop_front();
        }
    }

    pub fn sign_ups_last_hour(&self) -> usize {
        let mut sign_ups = self.sign_ups.lock().unwrap();
        while sign_ups
            .front()
            .is_some_and(|at| at.elapsed() > Duration::from_secs(3600))
        {
            sign_ups.pop_front();
        }
        sign_ups.len()
    }

    /// The puzzle's difficulty now: two more bits (four times the work) at
    /// each step of sign-ups in the last hour. 0 when the puzzle is off.
    pub fn puzzle_bits(&self) -> u32 {
        if self.puzzle_bits == 0 {
            return 0;
        }
        let extra = match self.sign_ups_last_hour() {
            0..100 => 0,
            100..500 => 2,
            500..2000 => 4,
            _ => 6,
        };
        (self.puzzle_bits + extra).min(MAX_PUZZLE_BITS)
    }

    /// Drops what no longer matters: full windows, and signatures older than
    /// any request the clock check still lets through.
    pub fn forget_old(&self) {
        let keep = Duration::from_secs(MAX_CLOCK_SKEW_SECS as u64 * 2 + 60);
        self.seen
            .lock()
            .unwrap()
            .retain(|_, at| at.elapsed() < keep);
        self.windows
            .lock()
            .unwrap()
            .retain(|_, w| w.at.elapsed() < Duration::from_secs(3600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_limited_per_hour_and_ipv6_by_its_64() {
        let guard = Guard::new(
            0,
            IpRates {
                sign_ups: 2,
                ..IpRates::OFF
            },
        );
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let same_64: IpAddr = "2001:db8:1:2::99".parse().unwrap();
        let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert!(guard.ip_request(IpKind::SignUp, Some(a)));
        assert!(guard.ip_request(IpKind::SignUp, Some(same_64)));
        assert!(!guard.ip_request(IpKind::SignUp, Some(a)));
        assert!(guard.ip_request(IpKind::SignUp, Some(other)));
        // Other kinds are off here, and no address means no limit.
        assert!(guard.ip_request(IpKind::Lookup, Some(a)));
        assert!(guard.ip_request(IpKind::SignUp, None));
    }

    #[test]
    fn a_signature_is_accepted_once_and_the_puzzle_hardens_with_sign_ups() {
        let guard = Guard::new(DEFAULT_PUZZLE_BITS, IpRates::OFF);
        assert!(guard.first_time("ab"));
        assert!(!guard.first_time("ab"));
        assert_eq!(guard.puzzle_bits(), DEFAULT_PUZZLE_BITS);
        for _ in 0..100 {
            guard.sign_up();
        }
        assert_eq!(guard.puzzle_bits(), DEFAULT_PUZZLE_BITS + 2);
        assert_eq!(Guard::off().puzzle_bits(), 0);
    }
}
