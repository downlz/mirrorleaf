package app.mirrorleaf

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.result.ActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class RequestArgs {
    /** "notifications" and/or "nearby". */
    var which: Array<String> = arrayOf()
}

/**
 * The two permissions sync uses, asked for by the screen at the moment they matter (E1, E8),
 * no longer at every start. Notifications let the ledger's high-priority pushes and the fallback
 * prompt reach the person (plan §5.9); nearby devices (`ACCESS_LOCAL_NETWORK`, Android 17) lets sync
 * use the local network, otherwise it goes over the internet (assessment risk #9). On Android
 * versions that don't have a permission, it counts as granted.
 */
@TauriPlugin
class PermissionsPlugin(private val activity: Activity) : Plugin(activity) {
    private val contract = ActivityResultContracts.RequestMultiplePermissions()

    @Command
    fun status(invoke: Invoke) {
        invoke.resolve(state())
    }

    @Command
    fun request(invoke: Invoke) {
        val args = invoke.parseArgs(RequestArgs::class.java)
        val missing = args.which.mapNotNull(::permissionFor).filter {
            ContextCompat.checkSelfPermission(activity, it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isEmpty()) {
            invoke.resolve(state())
            return
        }
        startActivityForResult(invoke, contract.createIntent(activity, missing.toTypedArray()), "answered")
    }

    @ActivityCallback
    private fun answered(invoke: Invoke, @Suppress("UNUSED_PARAMETER") result: ActivityResult) {
        runCatching { MirrorleafApp.engine.setNotifications(Notifications.allowed(activity)) }
        invoke.resolve(state())
    }

    private fun state(): JSObject = JSObject()
        .put("notifications", granted("notifications"))
        .put("nearby", granted("nearby"))

    private fun granted(name: String): Boolean {
        val permission = permissionFor(name) ?: return true
        return ContextCompat.checkSelfPermission(activity, permission) == PackageManager.PERMISSION_GRANTED
    }

    /** The Android permission behind a name, or null where this Android version has none. */
    private fun permissionFor(name: String): String? = when (name) {
        "notifications" -> if (Build.VERSION.SDK_INT >= 33) Manifest.permission.POST_NOTIFICATIONS else null
        "nearby" -> if (Build.VERSION.SDK_INT >= 37) Manifest.permission.ACCESS_LOCAL_NETWORK else null
        else -> null
    }
}
