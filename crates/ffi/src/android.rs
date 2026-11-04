//! Android startup (ADR-005). `MirrorleafNative.init(context)` runs first in
//! every process, from `Application.onCreate`, before the screen or a
//! push-woken job can start the engine.

use std::sync::atomic::{AtomicBool, Ordering};

use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject};
use jni::{EnvUnowned, jni_mangle};

static CONTEXT_SET: AtomicBool = AtomicBool::new(false);

/// Hands the JVM and the application context to the Rust code that needs them:
/// Iroh's network and DNS lookups read them through `ndk-context`, and HTTPS
/// certificate checks call Android's verifier. A push-woken job has no screen,
/// so Tauri hasn't set the context; ours comes first, and the patched
/// `ndk-context` ignores Tauri's later call when the screen opens.
#[jni_mangle("app.mirrorleaf.MirrorleafNative")]
pub fn init<'caller>(
    mut unowned_env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    context: JObject<'caller>,
) {
    unowned_env
        .with_env(|env| -> jni::errors::Result<()> {
            logcat::install();
            if !CONTEXT_SET.swap(true, Ordering::AcqRel) {
                let vm = env.get_java_vm()?;
                // The application context lives as long as the process, so this
                // global reference is never deleted.
                let context = env.new_global_ref(&context)?;
                // SAFETY: a valid JavaVM pointer and a global reference that
                // stays valid for the life of the process.
                unsafe {
                    ndk_context::initialize_android_context(
                        vm.get_raw().cast(),
                        context.into_raw().cast(),
                    );
                }
            }
            rustls_platform_verifier::android::init_with_env(env, context)
        })
        .resolve::<ThrowRuntimeExAndDefault>();
}

/// Sends the engine's logs to logcat under the tag `mirrorleaf`, so a
/// push-woken job, which has no screen, can be followed with `adb logcat`.
mod logcat {
    use std::ffi::{CString, c_char, c_int};
    use std::io;

    use tracing_subscriber::EnvFilter;

    const INFO: c_int = 4;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    struct Logcat;

    impl io::Write for Logcat {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let text = String::from_utf8_lossy(buf).trim_end().replace('\0', "");
            if let Ok(text) = CString::new(text) {
                // SAFETY: both strings are NUL-terminated and outlive the call.
                unsafe { __android_log_write(INFO, c"mirrorleaf".as_ptr(), text.as_ptr()) };
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(super) fn install() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(
                "warn,mirrorleaf_ffi=info,mirrorleaf_net=info,iroh::net_report=error",
            ))
            .with_ansi(false)
            .without_time()
            .with_writer(|| Logcat)
            .try_init();
    }
}
