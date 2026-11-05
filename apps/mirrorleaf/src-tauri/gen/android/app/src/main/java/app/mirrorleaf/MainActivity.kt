package app.mirrorleaf

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.drawable.ColorDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.View
import android.widget.Toast
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.content.IntentCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class MainActivity : TauriActivity() {
    private val askPermissions =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            runCatching { MirrorleafApp.engine.setNotifications(Notifications.allowed(this)) }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        keepClearOfBarsAndKeyboard()
        val missing = wanted().filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isNotEmpty()) askPermissions.launch(missing.toTypedArray())
        // A share that started the app; not again when the activity is recreated.
        if (savedInstanceState == null) receiveShare(intent)
    }

    /**
     * Dark mode switched while the app runs: the activity isn't recreated (Tauri's template handles
     * `uiMode` itself, so the web view isn't reloaded), so the background behind the system bars
     * and the bars' icon colours are set again here. The screen follows by itself.
     */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        window.setBackgroundDrawable(ColorDrawable(ContextCompat.getColor(this, R.color.mirrorleaf_background)))
        enableEdgeToEdge()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        receiveShare(intent)
    }

    /**
     * The window draws edge to edge, so the screen is kept clear of the status and navigation bars,
     * any display cutout and, while it is open, the keyboard: the web view shrinks to the space
     * left and the editor can keep the cursor in view (Android test 2, plan M6). The space behind
     * the bars shows the window's background, which matches the screen's (themes.xml).
     */
    private fun keepClearOfBarsAndKeyboard() {
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val clear = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout() or
                    WindowInsetsCompat.Type.ime()
            )
            view.setPadding(clear.left, clear.top, clear.right, clear.bottom)
            WindowInsetsCompat.CONSUMED
        }
    }

    /**
     * "Share → Mirrorleaf" from another app makes a note (plan M6): the shared subject or the
     * text's first line is its title, then the text, then any photos. The engine stores it and
     * tells the screen to open it.
     */
    private fun receiveShare(intent: Intent?) {
        val share = Shared.from(this, intent) ?: return
        intent?.action = null
        MirrorleafApp.scope.launch {
            val photos = IncomingFiles.copy(this@MainActivity, share.photos, "shared")
            try {
                MirrorleafApp.engine.shareIntoNote(share.subject, share.text, photos)
            } catch (e: Exception) {
                Log.w(MirrorleafApp.TAG, "share: ${e.message}")
                withContext(Dispatchers.Main) {
                    Toast.makeText(this@MainActivity, getString(R.string.share_failed), Toast.LENGTH_LONG).show()
                }
            }
        }
    }

    /**
     * Phase 0 asks at first start; Phase 1 moves this into onboarding and pairing (plan §5.10,
     * item 4). Without notifications, only normal-priority pushes are used. Without the local
     * network permission, sync goes over the internet instead (assessment risk #9).
     */
    private fun wanted(): List<String> = buildList {
        if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        if (Build.VERSION.SDK_INT >= 37) add(Manifest.permission.ACCESS_LOCAL_NETWORK)
    }
}

/** What another app shared: text, a subject, and photos. */
private class Shared(val subject: String?, val text: String?, val photos: List<Uri>) {
    companion object {
        fun from(activity: MainActivity, intent: Intent?): Shared? {
            if (intent == null) return null
            val streams = when (intent.action) {
                Intent.ACTION_SEND ->
                    listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
                Intent.ACTION_SEND_MULTIPLE ->
                    IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
                else -> return null
            }
            val photos = streams.filter { IncomingFiles.isImage(activity, it) }
            if (photos.size < streams.size) {
                Log.w(MirrorleafApp.TAG, "share: ${streams.size - photos.size} item(s) skipped (not a photo, or not readable)")
            }
            val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
            val subject = intent.getStringExtra(Intent.EXTRA_SUBJECT)
                ?: intent.getStringExtra(Intent.EXTRA_TITLE)
            if (text.isNullOrBlank() && photos.isEmpty()) {
                Log.w(MirrorleafApp.TAG, "share: nothing Mirrorleaf can use")
                return null
            }
            return Shared(subject, text, photos)
        }
    }
}
