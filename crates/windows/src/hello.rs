//! Windows Hello for the app lock (M11's lock on Windows): the PC's PIN,
//! fingerprint or face, as Touch ID or the Mac's password are on the Mac.
//! There is no Mirrorleaf PIN (owner, 9 Nov 2026).

use windows::Security::Credentials::UI::{
    UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
use windows::core::{HSTRING, factory};
use windows_future::IAsyncOperation;

/// Whether Windows Hello is set up for this user (a PIN at least).
pub fn available() -> bool {
    UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|operation| operation.join())
        .is_ok_and(|availability| availability == UserConsentVerifierAvailability::Available)
}

/// Asks for Windows Hello over the window `window` (its raw HWND), showing
/// `message`. Blocks until the person answers: call it off the main thread.
/// `true` once they passed it.
pub fn verify(window: isize, message: &str) -> windows::core::Result<bool> {
    let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
    // SAFETY: `window` is the app's own window handle.
    let operation: IAsyncOperation<UserConsentVerificationResult> = unsafe {
        interop.RequestVerificationForWindowAsync(
            HWND(window as *mut core::ffi::c_void),
            &HSTRING::from(message),
        )?
    };
    Ok(operation.join()? == UserConsentVerificationResult::Verified)
}
