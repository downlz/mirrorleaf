//! Sleep and wake on macOS (M5). Before the Mac sleeps the agent stores the
//! editor's edits, lets live sync send them and tells the ledger it is going
//! offline, so other devices stop counting on it at once. On waking it goes
//! live again and redials its devices.

use std::ptr::NonNull;

use block2::RcBlock;
use mirrorleaf_ffi::Engine;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::NSNotification;

pub fn watch_sleep() {
    let sleeping = RcBlock::new(|_: NonNull<NSNotification>| {
        tracing::info!("the Mac is going to sleep");
        if let Some(engine) = Engine::get() {
            tauri::async_runtime::spawn(async move {
                let _ = engine.go_offline().await;
            });
        }
    });
    let woken = RcBlock::new(|_: NonNull<NSNotification>| {
        tracing::info!("the Mac woke up");
        if let Some(engine) = Engine::get() {
            tauri::async_runtime::spawn(async move {
                if let Err(e) = engine.go_live().await {
                    tracing::info!("live sync didn't restart: {e}");
                }
            });
        }
    });
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
