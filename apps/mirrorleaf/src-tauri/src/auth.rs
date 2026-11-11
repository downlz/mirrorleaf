//! The device's own unlock, for the app lock (M11, E4): Touch ID or the Mac's
//! password on the Mac; the phone's passcode, fingerprint or face on Android
//! (`LockPlugin.kt`). There is no Mirrorleaf PIN to forget (owner, 9 Nov 2026).

use tauri::{AppHandle, Runtime};

/// Completes macOS's "Mirrorleaf is trying to …".
#[cfg(target_os = "macos")]
const REASON: &str = "unlock your notes";

/// Whether the device has an unlock the lock can use (on Android, a screen
/// lock must be set).
pub async fn available<R: Runtime>(app: &AppHandle<R>) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        mac::available()
    }
    #[cfg(target_os = "android")]
    {
        crate::device::lock_available(app).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        let _ = app;
        false
    }
}

/// Asks for the device's own unlock. `true` once the person passed it.
pub async fn authenticate<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        if !mac::available() {
            return Err("Set a login password on this Mac first.".to_owned());
        }
        tokio::task::spawn_blocking(|| mac::authenticate(REASON))
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "android")]
    {
        crate::device::authenticate(app).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        let _ = app;
        Err("the app lock isn't available on this computer".to_owned())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    /// Touch ID or the account password: a Mac with a login password has it.
    pub fn available() -> bool {
        // SAFETY: a fresh context, used on this thread only.
        let context = unsafe { LAContext::new() };
        unsafe { context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication) }.is_ok()
    }

    /// Shows Touch ID or the password sheet and waits for the answer, so it
    /// runs on a thread of its own.
    pub fn authenticate(reason: &str) -> bool {
        // SAFETY: a fresh context, kept alive until the reply has come.
        let context = unsafe { LAContext::new() };
        let (sent, answered) = mpsc::channel();
        let reply = RcBlock::new(move |success: Bool, _error: *mut NSError| {
            let _ = sent.send(success.as_bool());
        });
        // SAFETY: the reply block only sends on a channel, from any thread.
        unsafe {
            context.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthentication,
                &NSString::from_str(reason),
                &reply,
            );
        }
        answered.recv().unwrap_or(false)
    }
}
