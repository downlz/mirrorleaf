package app.mirrorleaf

import android.app.Activity
import androidx.activity.result.ActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import kotlinx.coroutines.launch

/**
 * Android's photo picker for the editor (plan M6). The chosen photos are copied into the app's
 * cache and only their paths go back to Rust, which imports them: photo bytes never cross the web
 * view, where Tauri would send them as JSON (ADR-007). The picker needs no storage permission.
 */
@TauriPlugin
class PhotoPickerPlugin(private val activity: Activity) : Plugin(activity) {
    private val contract = ActivityResultContracts.PickMultipleVisualMedia(MAX_PHOTOS)

    @Command
    fun pick(invoke: Invoke) {
        val request = PickVisualMediaRequest.Builder()
            .setMediaType(ActivityResultContracts.PickVisualMedia.ImageOnly)
            .build()
        startActivityForResult(invoke, contract.createIntent(activity, request), "picked")
    }

    @ActivityCallback
    private fun picked(invoke: Invoke, result: ActivityResult) {
        val uris = contract.parseResult(result.resultCode, result.data)
        MirrorleafApp.scope.launch {
            val paths = IncomingFiles.copy(activity, uris, "picked")
            invoke.resolve(JSObject().put("paths", JSArray(paths)))
        }
    }

    companion object {
        const val MAX_PHOTOS = 20
    }
}
