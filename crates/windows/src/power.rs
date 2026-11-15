//! Sleep and wake on Windows, as `power.rs` does on the Mac (M5): before the
//! PC sleeps the agent stores the editor's edits and tells the ledger it is
//! going offline; on waking it goes live again.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
};

/// What the PC is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Sleeping,
    Woke,
}

type Handler = Box<dyn Fn(Power) + Send + Sync>;

static HANDLER: OnceLock<Handler> = OnceLock::new();

/// Calls `handler` as the PC goes to sleep and wakes, for the rest of the
/// process. Only the first call registers.
pub fn on_sleep_and_wake(
    handler: impl Fn(Power) + Send + Sync + 'static,
) -> windows::core::Result<()> {
    if HANDLER.set(Box::new(handler)).is_err() {
        return Ok(());
    }
    // Windows keeps a pointer to this for as long as the registration lasts:
    // the life of the process.
    let parameters = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(callback),
        Context: std::ptr::null_mut(),
    }));
    let mut registration: *mut c_void = std::ptr::null_mut();
    // SAFETY: `parameters` lives for the process; `registration` is written once.
    unsafe {
        PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(std::ptr::from_mut(parameters).cast()),
            &mut registration,
        )
        .ok()
    }
}

unsafe extern "system" fn callback(
    _context: *const c_void,
    kind: u32,
    _setting: *const c_void,
) -> u32 {
    let power = match kind {
        PBT_APMSUSPEND => Some(Power::Sleeping),
        // Woken by the user, or by a timer (the second comes first).
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(Power::Woke),
        _ => None,
    };
    if let (Some(power), Some(handler)) = (power, HANDLER.get()) {
        handler(power);
    }
    0
}
