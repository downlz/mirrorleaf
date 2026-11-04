package app.mirrorleaf

import android.app.Application
import android.os.Build
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.mirrorleaf.engine.Engine
import com.google.firebase.messaging.FirebaseMessaging
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Opens the engine in every process, before the screen or a push-woken job can need it, so one
 * engine serves both (implementation plan §5.7).
 */
class MirrorleafApp : Application() {
    override fun onCreate() {
        super.onCreate()
        System.loadLibrary("mirrorleaf_app_lib")
        MirrorleafNative.init(this)
        engine = Engine.open(File(filesDir, "mirrorleaf").path, deviceName())
        Notifications.createChannels(this)
        engine.setNotifications(Notifications.allowed(this))
        registerForPushes()
        ProcessLifecycleOwner.get().lifecycle.addObserver(Visibility())
    }

    /** Live sync while the app is visible; going offline when it isn't. */
    private inner class Visibility : DefaultLifecycleObserver {
        private val order = Mutex()

        override fun onStart(owner: LifecycleOwner) {
            runCatching { engine.setNotifications(Notifications.allowed(this@MirrorleafApp)) }
            scope.launch {
                order.withLock {
                    runCatching { engine.goLive() }
                        .onFailure { Log.i(TAG, "live sync: ${it.message}") }
                }
            }
        }

        override fun onStop(owner: LifecycleOwner) {
            scope.launch {
                order.withLock {
                    runCatching { engine.goOffline() }
                        .onFailure { Log.i(TAG, "going offline: ${it.message}") }
                }
            }
        }
    }

    /**
     * Gives the engine this phone's FCM token. Without Google Play services, or if registration
     * fails, there is none: the phone then syncs when the app is opened (plan §5.9).
     */
    private fun registerForPushes() {
        try {
            FirebaseMessaging.getInstance().token.addOnCompleteListener { task ->
                val token = if (task.isSuccessful) task.result else null
                if (token == null) Log.i(TAG, "no push token: ${task.exception?.message}")
                runCatching { engine.setPushToken(token) }
            }
        } catch (e: Exception) {
            Log.i(TAG, "no push token: ${e.message}")
            runCatching { engine.setPushToken(null) }
        }
    }

    private fun deviceName(): String =
        Build.MODEL?.takeIf { it.isNotBlank() } ?: "Android phone"

    companion object {
        const val TAG = "mirrorleaf"

        /** The process's engine, opened in [onCreate]. */
        lateinit var engine: Engine
            private set

        val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    }
}
