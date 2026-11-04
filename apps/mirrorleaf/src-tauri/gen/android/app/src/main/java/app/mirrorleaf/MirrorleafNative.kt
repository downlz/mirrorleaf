package app.mirrorleaf

import android.content.Context

/** The engine's Android startup, in the app's native library (ADR-005). */
object MirrorleafNative {
    /**
     * Hands the JVM and the application context to the Rust code that needs them: Iroh's
     * network code and HTTPS certificate checks. Must run before the engine is used.
     */
    @JvmStatic external fun init(context: Context)
}
