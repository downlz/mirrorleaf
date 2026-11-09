package app.mirrorleaf

import android.app.Activity
import android.content.Intent
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class SaveArgs {
    /** The file the engine wrote, in the app's own storage. Deleted once copied. */
    lateinit var path: String
    /** The name to suggest. */
    lateinit var name: String
    lateinit var mime: String
}

/**
 * Saving the recovery kit (E2) where the person chooses, through Android's own "Save to" picker
 * (the Storage Access Framework): the engine writes the kit to the app's storage, and this copies
 * it to the place picked, so the kit never passes through the web view (ADR-007). Also restarts
 * the app after signing out (E3), so the engine opens afresh.
 */
@TauriPlugin
class DocumentsPlugin(private val activity: Activity) : Plugin(activity) {
    /** The file waiting for the picker's answer. */
    private var pending: File? = null

    @Command
    fun save(invoke: Invoke) {
        val args = invoke.parseArgs(SaveArgs::class.java)
        pending = File(args.path)
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = args.mime
            putExtra(Intent.EXTRA_TITLE, args.name)
        }
        startActivityForResult(invoke, intent, "picked")
    }

    @ActivityCallback
    private fun picked(invoke: Invoke, result: ActivityResult) {
        val file = pending
        pending = null
        try {
            val uri = result.data?.data
            if (result.resultCode != Activity.RESULT_OK || uri == null || file == null) {
                invoke.resolve(JSObject().put("saved", false))
                return
            }
            val out = activity.contentResolver.openOutputStream(uri, "wt")
                ?: throw IllegalStateException("can't write to the place chosen")
            out.use { stream -> file.inputStream().use { it.copyTo(stream) } }
            invoke.resolve(JSObject().put("saved", true))
        } catch (e: Exception) {
            invoke.reject(e.message ?: "couldn't save the kit")
        } finally {
            file?.delete()
        }
    }

    /** Starts the app again in a fresh process. */
    @Command
    fun restart(invoke: Invoke) {
        invoke.resolve()
        val intent = activity.packageManager.getLaunchIntentForPackage(activity.packageName)
            ?: return
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        activity.startActivity(intent)
        Runtime.getRuntime().exit(0)
    }
}
