//! Sleep and wake (M5 on macOS; the Windows app). Before the computer sleeps
//! the agent stores the editor's edits, lets live sync send them and tells
//! the ledger it is going offline, so other devices stop counting on it at
//! once. On waking it goes live again and redials its devices.

use mirrorleaf_ffi::Engine;

/// The computer is going to sleep.
fn sleeping() {
    tracing::info!("the computer is going to sleep");
    if let Some(engine) = Engine::get() {
        // The app lock's time away starts, if the window is in sight (E4).
        engine.device_sleeping();
        tauri::async_runtime::spawn(async move {
            let _ = engine.go_offline().await;
        });
    }
}

/// The computer woke up.
fn woke() {
    tracing::info!("the computer woke up");
    if let Some(engine) = Engine::get() {
        // Locks the screen if it slept long enough (`LockChanged`).
        engine.device_woke();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = engine.go_live().await {
                tracing::info!("live sync didn't restart: {e}");
            }
        });
    }
}

/// Windows tells a callback (`mirrorleaf_windows::power`).
#[cfg(windows)]
pub fn watch_sleep() {
    use mirrorleaf_windows::power::{Power, on_sleep_and_wake};
    if let Err(e) = on_sleep_and_wake(|power| match power {
        Power::Sleeping => sleeping(),
        Power::Woke => woke(),
    }) {
        tracing::warn!("couldn't watch for sleep and wake: {e}");
    }
}

#[cfg(target_os = "macos")]
pub use mac::watch_sleep;

#[cfg(target_os = "macos")]
mod mac {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
    };
    use objc2_foundation::NSNotification;

    pub fn watch_sleep() {
        let sleeping = RcBlock::new(|_: NonNull<NSNotification>| super::sleeping());
        let woken = RcBlock::new(|_: NonNull<NSNotification>| super::woke());
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the names are AppKit's own constants, no object filter or queue
        // is given (the blocks run on the posting thread), and the blocks only
        // capture nothing and hand work to Tauri's runtime, so they are sendable.
        let observers = unsafe {
            [
                center.addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceWillSleepNotification),
                    None,
                    None,
                    &sleeping,
                ),
                center.addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceDidWakeNotification),
                    None,
                    None,
                    &woken,
                ),
            ]
        };
        // The observers last for the life of the process.
        std::mem::forget(observers);
    }
}
