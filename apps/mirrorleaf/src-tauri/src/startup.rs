//! Cold start timing (plan §7.3: "time from launch to the first note being
//! editable, using app-side timestamps"). The screen reports once when it is
//! ready to use; the time is measured from when the operating system started
//! the process, so it includes loading the app, the engine and the web view.

use std::time::Duration;

/// Logs how long the process took to reach a usable screen, the first time
/// the screen reports.
pub fn report(what: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| match process_age() {
        Some(age) => tracing::info!(
            "cold start: {} ms from process start to the {what}",
            age.as_millis()
        ),
        None => tracing::info!("cold start: the {what} is ready (process start time unknown)"),
    });
}

/// Time since the process started (10 ms resolution): its start time from
/// `/proc/self/stat`, against the boot clock it is counted on. On Android this
/// is when Zygote forked the app's process. (Android apps may not read
/// `/proc/uptime`, so the boot clock is read directly.)
#[cfg(any(target_os = "linux", target_os = "android"))]
fn process_age() -> Option<Duration> {
    // Clock ticks per second: 100 on every Linux and Android kernel we build for.
    const TICKS: f64 = 100.0;
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command name is in brackets and may hold spaces; `starttime` is the
    // 22nd field, the 20th after the name.
    let after_name = stat.get(stat.rfind(')')? + 1..)?;
    let started: f64 = after_name.split_whitespace().nth(19)?.parse().ok()?;
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid timespec for the call to fill.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut now) } != 0 {
        return None;
    }
    let now = now.tv_sec as f64 + now.tv_nsec as f64 / 1e9;
    Some(Duration::from_secs_f64((now - started / TICKS).max(0.0)))
}

/// Time since the process started, from the kernel's process table.
#[cfg(target_os = "macos")]
fn process_age() -> Option<Duration> {
    use std::time::{SystemTime, UNIX_EPOCH};
    // SAFETY: `proc_bsdinfo` is plain data, valid when zeroed; the kernel fills
    // at most `size` bytes of it and returns how many it wrote.
    let info = unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        );
        (written == size).then_some(info)?
    };
    let started =
        Duration::from_secs(info.pbi_start_tvsec) + Duration::from_micros(info.pbi_start_tvusec);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .checked_sub(started)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn process_age() -> Option<Duration> {
    None
}
