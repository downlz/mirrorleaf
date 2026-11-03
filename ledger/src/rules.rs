//! Staleness (assessment §7) and the push rules (implementation plan §5.9), as
//! plain functions over one account's device rows, so they can be tested
//! without a database.

use mirrorleaf_proto::ledger::Platform;
use mirrorleaf_proto::{DeviceId, VersionVector};

/// A device gets at most one ordinary wake-up per this many seconds (plan §5.9).
pub const WAKE_INTERVAL_SECS: i64 = 120;
/// How long a high-priority wake-up may go unanswered before the fallback
/// notification is sent (ADR-004).
pub const FALLBACK_AFTER_SECS: i64 = 60;

/// One device row, as the rules need it.
#[derive(Clone, Debug)]
pub struct Device {
    pub id: DeviceId,
    pub platform: Platform,
    pub push_token: Option<String>,
    pub notifications: bool,
    /// The highest change it holds from each device, its own included.
    pub vv: VersionVector,
    pub online_until: i64,
    pub last_push_at: i64,
    pub wake: Option<Wake>,
}

/// A high-priority wake-up waiting for an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wake {
    /// The device that asked for it, because it was opened while behind.
    pub for_device: DeviceId,
    pub at: i64,
}

impl Device {
    pub fn online(&self, now: i64) -> bool {
        self.online_until > now
    }

    /// Whether a push can reach it: an Android device with a token, offline.
    /// Desktops can't be woken; while awake they are online anyway.
    fn wakeable(&self, now: i64) -> bool {
        self.platform == Platform::Android && self.push_token.is_some() && !self.online(now)
    }

    fn may_push(&self, now: i64) -> bool {
        self.wakeable(now) && now - self.last_push_at >= WAKE_INTERVAL_SECS
    }

    /// How many changes it holds that `other` lacks.
    fn ahead_of(&self, other: &Device) -> u64 {
        self.vv.ahead_of(&other.vv)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Normal,
    High,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushKind {
    /// Start a background job that pulls the new changes from `from`.
    Pull { from: DeviceId },
    /// Start a background job that delivers `changes` changes to `to`. At high
    /// priority the phone shows "Sending N changes to <name>", composed from
    /// its own device list.
    Deliver { to: DeviceId, changes: u64 },
    /// The human fallback: a notification the system shows by itself.
    Fallback,
}

/// A push to send to one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub to: DeviceId,
    pub token: String,
    pub priority: Priority,
    pub kind: PushKind,
}

impl Push {
    fn to(device: &Device, priority: Priority, kind: PushKind) -> Option<Self> {
        Some(Self {
            to: device.id,
            token: device.push_token.clone()?,
            priority,
            kind,
        })
    }
}

/// How many changes some device of the account holds that `device` lacks.
pub fn behind(devices: &[Device], device: &Device) -> u64 {
    devices
        .iter()
        .fold(VersionVector::default(), |all, d| all.merged(&d.vv))
        .ahead_of(&device.vv)
}

/// Rules 1 and 2, after device `me` reported its status. `devices` holds every
/// row as it is now; `before` is `me`'s row as it was (`None` for a first
/// report).
pub fn after_status(
    devices: &[Device],
    me: DeviceId,
    before: Option<&Device>,
    now: i64,
) -> Vec<Push> {
    let Some(mine) = devices.iter().find(|d| d.id == me) else {
        return Vec::new();
    };
    let others = || devices.iter().filter(move |d| d.id != me);
    let mut pushes: Vec<Push> = Vec::new();

    let came_online = mine.online(now) && before.is_none_or(|b| !b.online(now));

    // Rule 2: I've just come online behind, and no online device holds what I
    // lack, so an offline Android device that holds it should deliver it.
    // Checked first: a device that both lacks my changes and holds ones I lack
    // gets this push, which says more than rule 1's.
    if came_online && !others().any(|d| d.online(now) && d.ahead_of(mine) > 0) {
        let holder = best_holder(devices, mine, now).filter(|h| h.may_push(now));
        if let Some(h) = holder {
            let kind = PushKind::Deliver {
                to: me,
                changes: h.ahead_of(mine),
            };
            pushes.extend(Push::to(h, Priority::Normal, kind));
        }
    }

    // Rule 1: I'm online with changes that offline Android devices lack, either
    // new ones or ones I made while offline and have just come online with, so
    // they can pull them from me. One push per device.
    let gained = mine
        .vv
        .ahead_of(&before.map(|b| b.vv.clone()).unwrap_or_default());
    if (gained > 0 || came_online) && mine.online(now) {
        for y in others().filter(|y| y.may_push(now) && mine.ahead_of(y) > 0) {
            if !pushes.iter().any(|p| p.to == y.id) {
                pushes.extend(Push::to(y, Priority::Normal, PushKind::Pull { from: me }));
            }
        }
    }
    pushes
}

/// What to do when device `me` is opened while behind and asks the ledger to
/// wake the device holding its changes (rule 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WakeDecision {
    UpToDate,
    HolderOnline(DeviceId),
    AlreadyWaking(DeviceId),
    Wake(Push),
    NoWakeableHolder,
}

pub fn on_wake(devices: &[Device], me: DeviceId, now: i64) -> WakeDecision {
    let Some(mine) = devices.iter().find(|d| d.id == me) else {
        return WakeDecision::UpToDate;
    };
    if behind(devices, mine) == 0 {
        return WakeDecision::UpToDate;
    }
    if let Some(online) = devices
        .iter()
        .find(|d| d.id != me && d.online(now) && d.ahead_of(mine) > 0)
    {
        return WakeDecision::HolderOnline(online.id);
    }
    let Some(holder) = best_holder(devices, mine, now) else {
        return WakeDecision::NoWakeableHolder;
    };
    if holder.wake.is_some_and(|w| now - w.at < WAKE_INTERVAL_SECS) {
        return WakeDecision::AlreadyWaking(holder.id);
    }
    // FCM downgrades high-priority messages that can't show anything, so a
    // device that may not show notifications gets normal priority (plan §5.9).
    let priority = if holder.notifications {
        Priority::High
    } else {
        Priority::Normal
    };
    let kind = PushKind::Deliver {
        to: me,
        changes: holder.ahead_of(mine),
    };
    match Push::to(holder, priority, kind) {
        Some(push) => WakeDecision::Wake(push),
        None => WakeDecision::NoWakeableHolder,
    }
}

/// Rule 4, for a device with a pending wake-up: the fallback notification if
/// the wake-up went unanswered for `FALLBACK_AFTER_SECS`. Returns the push, if
/// any, and whether the wake-up is settled (answered, or followed up).
pub fn fallback(devices: &[Device], holder: &Device, now: i64) -> (Option<Push>, bool) {
    let Some(wake) = holder.wake else {
        return (None, false);
    };
    let answered = devices
        .iter()
        .find(|d| d.id == wake.for_device)
        .is_none_or(|asker| holder.ahead_of(asker) == 0);
    if answered {
        return (None, true);
    }
    if now - wake.at < FALLBACK_AFTER_SECS {
        return (None, false);
    }
    let push = holder
        .notifications
        .then(|| Push::to(holder, Priority::High, PushKind::Fallback))
        .flatten();
    (push, true)
}

/// The wakeable device holding the most of what `me` lacks.
fn best_holder<'a>(devices: &'a [Device], me: &Device, now: i64) -> Option<&'a Device> {
    devices
        .iter()
        .filter(|d| d.id != me.id && d.wakeable(now) && d.ahead_of(me) > 0)
        .max_by_key(|d| (d.ahead_of(me), d.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn id(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    /// A device holding `held[i]` changes from device i+1.
    fn device(n: u8, platform: Platform, held: &[u64]) -> Device {
        Device {
            id: id(n),
            platform,
            push_token: (platform == Platform::Android).then(|| format!("token-{n}")),
            notifications: true,
            vv: VersionVector(
                held.iter()
                    .enumerate()
                    .filter(|(_, n)| **n > 0)
                    .map(|(i, n)| (id(i as u8 + 1), *n))
                    .collect(),
            ),
            online_until: 0,
            last_push_at: 0,
            wake: None,
        }
    }

    fn online(mut d: Device) -> Device {
        d.online_until = NOW + 360;
        d
    }

    #[test]
    fn staleness_counts_what_any_device_holds() {
        let laptop = device(1, Platform::Macos, &[1284, 0, 0]);
        let desktop = device(2, Platform::Macos, &[1284, 912, 0]);
        let phone = device(3, Platform::Android, &[1280, 900, 5]);
        let all = [laptop.clone(), desktop.clone(), phone.clone()];
        assert_eq!(behind(&all, &phone), 4 + 12);
        assert_eq!(behind(&all, &laptop), 912 + 5);
        assert_eq!(behind(&all, &desktop), 5);
    }

    #[test]
    fn rule_1_wakes_offline_phones_that_lack_new_changes() {
        let before = online(device(1, Platform::Macos, &[10, 0, 0]));
        let laptop = online(device(1, Platform::Macos, &[11, 0, 0]));
        let phone = device(2, Platform::Android, &[10, 0, 0]);
        let synced_phone = device(3, Platform::Android, &[11, 0, 0]);
        let desktop = device(4, Platform::Macos, &[10, 0, 0]);
        let all = [laptop, phone, synced_phone, desktop];
        assert_eq!(
            after_status(&all, id(1), Some(&before), NOW),
            [Push {
                to: id(2),
                token: "token-2".into(),
                priority: Priority::Normal,
                kind: PushKind::Pull { from: id(1) }
            }]
        );
    }

    #[test]
    fn rule_1_also_wakes_phones_when_a_device_comes_online_with_older_changes() {
        // Reported while offline, so nobody could pull them then.
        let before = device(1, Platform::Macos, &[11, 0]);
        let laptop = online(before.clone());
        let phone = device(2, Platform::Android, &[10, 0]);
        let all = [laptop, phone];
        let pushes = after_status(&all, id(1), Some(&before), NOW);
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].kind, PushKind::Pull { from: id(1) });
    }

    #[test]
    fn a_device_both_rules_apply_to_gets_one_deliver_push() {
        let before = device(1, Platform::Macos, &[11, 0]);
        let laptop = online(before.clone());
        let phone = device(2, Platform::Android, &[10, 3]);
        let all = [laptop, phone];
        let pushes = after_status(&all, id(1), Some(&before), NOW);
        assert_eq!(pushes.len(), 1);
        assert_eq!(
            pushes[0].kind,
            PushKind::Deliver {
                to: id(1),
                changes: 3
            }
        );
    }

    #[test]
    fn rule_1_skips_online_phones_recent_pushes_and_offline_reporters() {
        let before = online(device(1, Platform::Macos, &[10, 0]));
        let laptop = online(device(1, Platform::Macos, &[11, 0]));
        let phone = device(2, Platform::Android, &[10, 0]);

        let all = [laptop.clone(), online(phone.clone())];
        assert!(after_status(&all, id(1), Some(&before), NOW).is_empty());

        let mut pushed = phone.clone();
        pushed.last_push_at = NOW - WAKE_INTERVAL_SECS + 1;
        let all = [laptop.clone(), pushed];
        assert!(after_status(&all, id(1), Some(&before), NOW).is_empty());

        // Going offline with new changes: nobody could pull them from me.
        let mut leaving = laptop.clone();
        leaving.online_until = NOW;
        let all = [leaving, phone];
        assert!(after_status(&all, id(1), Some(&before), NOW).is_empty());
    }

    #[test]
    fn rule_2_wakes_the_phone_holding_what_a_returning_device_lacks() {
        let before = device(1, Platform::Macos, &[5, 0]);
        let laptop = online(before.clone());
        let phone = device(2, Platform::Android, &[5, 3]);
        let all = [laptop.clone(), phone.clone()];
        assert_eq!(
            after_status(&all, id(1), Some(&before), NOW),
            [Push {
                to: id(2),
                token: "token-2".into(),
                priority: Priority::Normal,
                kind: PushKind::Deliver {
                    to: id(1),
                    changes: 3
                }
            }]
        );

        // Already online before: not "coming online", so no push.
        assert!(after_status(&all, id(1), Some(&laptop), NOW).is_empty());
        // Another online device holds the changes: dial it instead.
        let desktop = online(device(3, Platform::Macos, &[5, 3]));
        let all = [laptop, phone, desktop];
        assert!(after_status(&all, id(1), Some(&before), NOW).is_empty());
    }

    #[test]
    fn rule_3_wakes_the_holder_at_high_priority_only_if_it_can_show_notifications() {
        let laptop = online(device(1, Platform::Macos, &[5, 0]));
        let mut phone = device(2, Platform::Android, &[5, 3]);
        let all = [laptop.clone(), phone.clone()];
        let WakeDecision::Wake(push) = on_wake(&all, id(1), NOW) else {
            panic!("expected a wake-up");
        };
        assert_eq!((push.to, push.priority), (id(2), Priority::High));
        assert_eq!(
            push.kind,
            PushKind::Deliver {
                to: id(1),
                changes: 3
            }
        );

        phone.notifications = false;
        let all = [laptop.clone(), phone.clone()];
        let WakeDecision::Wake(push) = on_wake(&all, id(1), NOW) else {
            panic!("expected a wake-up");
        };
        assert_eq!(push.priority, Priority::Normal);

        phone.wake = Some(Wake {
            for_device: id(1),
            at: NOW - 30,
        });
        let all = [laptop.clone(), phone];
        assert_eq!(
            on_wake(&all, id(1), NOW),
            WakeDecision::AlreadyWaking(id(2))
        );
    }

    #[test]
    fn rule_3_answers_without_a_push_when_none_is_needed_or_possible() {
        let laptop = online(device(1, Platform::Macos, &[5, 0]));
        let up_to_date = [laptop.clone(), device(2, Platform::Android, &[5, 0])];
        assert_eq!(on_wake(&up_to_date, id(1), NOW), WakeDecision::UpToDate);

        let online_holder = [
            laptop.clone(),
            online(device(2, Platform::Android, &[5, 3])),
        ];
        assert_eq!(
            on_wake(&online_holder, id(1), NOW),
            WakeDecision::HolderOnline(id(2))
        );

        let sleeping_desktop = [laptop, device(2, Platform::Macos, &[5, 3])];
        assert_eq!(
            on_wake(&sleeping_desktop, id(1), NOW),
            WakeDecision::NoWakeableHolder
        );
    }

    #[test]
    fn rule_4_follows_an_unanswered_wake_up_with_a_notification() {
        let laptop = online(device(1, Platform::Macos, &[5, 0]));
        let mut phone = device(2, Platform::Android, &[5, 3]);
        phone.wake = Some(Wake {
            for_device: id(1),
            at: NOW - FALLBACK_AFTER_SECS,
        });
        let all = [laptop.clone(), phone.clone()];
        let (push, settled) = fallback(&all, &phone, NOW);
        assert!(settled);
        let push = push.expect("a fallback notification");
        assert_eq!(
            (push.to, push.priority, push.kind),
            (id(2), Priority::High, PushKind::Fallback)
        );

        // Not yet due.
        assert_eq!(fallback(&all, &phone, NOW - 1), (None, false));

        // Answered: the laptop now holds what the phone had.
        let caught_up = online(device(1, Platform::Macos, &[5, 3]));
        let all = [caught_up, phone.clone()];
        assert_eq!(fallback(&all, &phone, NOW), (None, true));

        // No notification permission: nothing can be shown, so it just settles.
        phone.notifications = false;
        let all = [laptop, phone.clone()];
        assert_eq!(fallback(&all, &phone, NOW), (None, true));
    }
}
