//! Request limits that need no IP address (E0). Every request is signed, so the
//! ledger limits each signing device; and because anyone can make new keys,
//! new accounts are limited across the whole ledger as well.
//!
//! Each limit is a token bucket: it holds up to a minute's worth of requests
//! and refills evenly. A rate of 0 turns a limit off, for example to let every
//! device register again quickly after the database was replaced.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use mirrorleaf_proto::DeviceId;

/// Signed requests per device per minute. The app reports at most every 30
/// seconds, with a heartbeat every 5 minutes, so it stays far below this.
pub const DEFAULT_DEVICE_REQUESTS_PER_MINUTE: u32 = 30;

/// New accounts per minute across the ledger.
pub const DEFAULT_NEW_ACCOUNTS_PER_MINUTE: u32 = 20;

/// Requests per minute; 0 means no limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub per_minute: u32,
}

impl Rate {
    pub const UNLIMITED: Rate = Rate { per_minute: 0 };

    pub fn per_minute(per_minute: u32) -> Self {
        Self { per_minute }
    }

    fn capacity(self) -> f64 {
        f64::from(self.per_minute)
    }
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(rate: Rate, now: Instant) -> Self {
        Self {
            tokens: rate.capacity(),
            at: now,
        }
    }

    fn refill(&mut self, rate: Rate, now: Instant) {
        let earned = now.saturating_duration_since(self.at).as_secs_f64() * rate.capacity() / 60.0;
        self.tokens = (self.tokens + earned).min(rate.capacity());
        self.at = now;
    }

    /// Takes one request's token, if there is one.
    fn take(&mut self, rate: Rate, now: Instant) -> bool {
        self.refill(rate, now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub struct Limits {
    device_rate: Rate,
    account_rate: Rate,
    devices: Mutex<HashMap<DeviceId, Bucket>>,
    accounts: Mutex<Option<Bucket>>,
}

impl Limits {
    pub fn new(device_rate: Rate, account_rate: Rate) -> Self {
        Self {
            device_rate,
            account_rate,
            devices: Mutex::default(),
            accounts: Mutex::default(),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(Rate::UNLIMITED, Rate::UNLIMITED)
    }

    /// Counts one signed request from `device`; `false` if it is over its limit.
    pub fn device_request(&self, device: &DeviceId) -> bool {
        self.device_request_at(device, Instant::now())
    }

    /// Counts one new account; `false` if the ledger is over its limit.
    pub fn new_account(&self) -> bool {
        self.new_account_at(Instant::now())
    }

    /// Drops the buckets that have filled up again: they hold nothing a new
    /// bucket wouldn't, so the map only keeps devices that were busy lately.
    pub fn forget_idle(&self) {
        let (rate, now) = (self.device_rate, Instant::now());
        self.devices.lock().unwrap().retain(|_, bucket| {
            bucket.refill(rate, now);
            bucket.tokens < rate.capacity()
        });
    }

    fn device_request_at(&self, device: &DeviceId, now: Instant) -> bool {
        let rate = self.device_rate;
        if rate == Rate::UNLIMITED {
            return true;
        }
        self.devices
            .lock()
            .unwrap()
            .entry(*device)
            .or_insert_with(|| Bucket::full(rate, now))
            .take(rate, now)
    }

    fn new_account_at(&self, now: Instant) -> bool {
        let rate = self.account_rate;
        if rate == Rate::UNLIMITED {
            return true;
        }
        self.accounts
            .lock()
            .unwrap()
            .get_or_insert_with(|| Bucket::full(rate, now))
            .take(rate, now)
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.devices.lock().unwrap().len()
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(
            Rate::per_minute(DEFAULT_DEVICE_REQUESTS_PER_MINUTE),
            Rate::per_minute(DEFAULT_NEW_ACCOUNTS_PER_MINUTE),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn device(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    #[test]
    fn a_device_gets_a_minutes_worth_then_waits_for_the_refill() {
        let limits = Limits::new(Rate::per_minute(6), Rate::UNLIMITED);
        let start = Instant::now();
        for _ in 0..6 {
            assert!(limits.device_request_at(&device(1), start));
        }
        assert!(!limits.device_request_at(&device(1), start));
        // Another device has a bucket of its own.
        assert!(limits.device_request_at(&device(2), start));
        // 6 a minute is one every 10 seconds.
        assert!(!limits.device_request_at(&device(1), start + Duration::from_secs(9)));
        assert!(limits.device_request_at(&device(1), start + Duration::from_secs(10)));
        // Refilling stops at a minute's worth.
        let later = start + Duration::from_secs(3600);
        for _ in 0..6 {
            assert!(limits.device_request_at(&device(1), later));
        }
        assert!(!limits.device_request_at(&device(1), later));
    }

    #[test]
    fn new_accounts_share_one_bucket() {
        let limits = Limits::new(Rate::UNLIMITED, Rate::per_minute(2));
        let start = Instant::now();
        assert!(limits.new_account_at(start));
        assert!(limits.new_account_at(start));
        assert!(!limits.new_account_at(start));
        assert!(limits.new_account_at(start + Duration::from_secs(30)));
    }

    #[test]
    fn a_rate_of_zero_is_no_limit() {
        let limits = Limits::unlimited();
        let now = Instant::now();
        for _ in 0..1000 {
            assert!(limits.device_request_at(&device(1), now));
            assert!(limits.new_account_at(now));
        }
        assert_eq!(limits.tracked(), 0, "nothing is tracked without a limit");
    }

    #[test]
    fn idle_devices_are_forgotten() {
        let limits = Limits::new(Rate::per_minute(60_000), Rate::UNLIMITED);
        assert!(limits.device_request(&device(1)));
        assert_eq!(limits.tracked(), 1);
        // At 1,000 a second the single token is back within a few milliseconds.
        std::thread::sleep(Duration::from_millis(20));
        limits.forget_idle();
        assert_eq!(limits.tracked(), 0);
    }
}
