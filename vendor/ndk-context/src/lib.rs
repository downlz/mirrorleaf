//! Handles for calling Android APIs from Rust: the Java VM and an
//! `android.content.Context`.
//!
//! A patched copy of ndk-context 0.1.1 with the same public API. The changes:
//! initialising it a second time keeps the first context instead of panicking,
//! releasing it when it isn't set does nothing, and the state is kept behind a
//! lock rather than in a `static mut`. In Mirrorleaf both callers (our startup
//! code and Tauri's) pass the same application context, so keeping the first
//! loses nothing (ADR-005).

use std::ffi::c_void;
use std::sync::Mutex;

/// The pointers needed to call into the JVM on Android.
#[derive(Clone, Copy, Debug)]
pub struct AndroidContext {
    java_vm: *mut c_void,
    context_jobject: *mut c_void,
}

// SAFETY: the pointers are a `JavaVM*`, valid for the whole process, and a JNI
// global reference, valid on any thread until it is deleted.
unsafe impl Send for AndroidContext {}
// SAFETY: as above; the pointers are only copied, never written through here.
unsafe impl Sync for AndroidContext {}

impl AndroidContext {
    /// A handle to the `JavaVM` object.
    pub fn vm(self) -> *mut c_void {
        self.java_vm
    }

    /// A handle to an `android.content.Context` (here, the application).
    pub fn context(self) -> *mut c_void {
        self.context_jobject
    }
}

static ANDROID_CONTEXT: Mutex<Option<AndroidContext>> = Mutex::new(None);

fn slot() -> std::sync::MutexGuard<'static, Option<AndroidContext>> {
    ANDROID_CONTEXT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The context set by [`initialize_android_context`].
///
/// # Panics
///
/// If it hasn't been set, as in the original crate.
pub fn android_context() -> AndroidContext {
    slot().expect("android context was not initialized")
}

/// Sets the context. If it is already set, the first one is kept.
///
/// # Safety
///
/// The pointers must be a valid `JavaVM*` and a JNI global reference to a
/// `Context` that stays valid for as long as it may be used.
pub unsafe fn initialize_android_context(java_vm: *mut c_void, context_jobject: *mut c_void) {
    slot().get_or_insert(AndroidContext {
        java_vm,
        context_jobject,
    });
}

/// Removes the context. Does nothing if it isn't set.
///
/// # Safety
///
/// No code may still be using the context returned earlier.
pub unsafe fn release_android_context() {
    slot().take();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_initialisation_keeps_the_first_context() {
        // Stand-in pointers: never dereferenced, only compared.
        let first = std::ptr::without_provenance_mut::<c_void>(16);
        let second = std::ptr::without_provenance_mut::<c_void>(32);
        unsafe {
            release_android_context();
            initialize_android_context(first, first);
            initialize_android_context(second, second);
        }
        assert_eq!(android_context().context(), first);
        unsafe {
            release_android_context();
            release_android_context();
        }
        assert!(std::panic::catch_unwind(android_context).is_err());
    }
}
