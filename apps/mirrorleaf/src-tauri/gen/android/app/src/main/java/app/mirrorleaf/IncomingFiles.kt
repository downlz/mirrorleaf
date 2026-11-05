package app.mirrorleaf

import android.content.Context
import android.net.Uri
import android.util.Log
import android.webkit.MimeTypeMap
import java.io.File
import java.util.UUID

/**
 * Copies photos picked or shared from other apps into the app's cache, so the engine can read them
 * as files (it removes each copy once imported). Called off the main thread.
 */
object IncomingFiles {
    /** Larger files are skipped: no phone photo comes close. */
    private const val MAX_BYTES = 100L * 1024 * 1024

    fun copy(context: Context, uris: List<Uri>, prefix: String): List<String> {
        val dir = File(context.cacheDir, "incoming").apply { mkdirs() }
        return uris.mapNotNull { uri ->
            try {
                val type = context.contentResolver.getType(uri)
                val ext = MimeTypeMap.getSingleton().getExtensionFromMimeType(type) ?: "img"
                val file = File(dir, "$prefix-${UUID.randomUUID()}.$ext")
                val stream = context.contentResolver.openInputStream(uri) ?: return@mapNotNull null
                stream.use { input ->
                    file.outputStream().use { output ->
                        val copied = input.copyTo(output)
                        if (copied > MAX_BYTES) error("over 100 MB")
                    }
                }
                file.path
            } catch (e: Exception) {
                Log.w(MirrorleafApp.TAG, "couldn't read $uri: ${e.message}")
                null
            }
        }
    }

    /** Whether a shared item is a photo (by its declared type). */
    fun isImage(context: Context, uri: Uri): Boolean =
        try {
            context.contentResolver.getType(uri)?.startsWith("image/") == true
        } catch (e: SecurityException) {
            Log.w(MirrorleafApp.TAG, "share: no permission to read $uri")
            false
        }
}
