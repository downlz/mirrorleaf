package app.mirrorleaf

import android.util.Log
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * Pushes from the ledger (plan §5.9). Each carries only its type: `pull` or `deliver` start the
 * sync job; `fallback` is a notification the system shows by itself when the app isn't running,
 * and that the app shows when it is.
 */
class SyncMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        runCatching { MirrorleafApp.engine.setPushToken(token) }
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val type = message.data["type"]
        val high = message.priority == RemoteMessage.PRIORITY_HIGH
        // The message ID and FCM's send time let the push-to-wake matrix
        // (tools/push-matrix.py) match and time each trial. Neither says
        // anything about the user's notes or devices.
        val sentAs = if (message.originalPriority == RemoteMessage.PRIORITY_HIGH) "high" else "normal"
        Log.i(
            MirrorleafApp.TAG,
            "push: $type, ${if (high) "high" else "normal"} priority (sent as $sentAs), " +
                "id=${message.messageId}, sent at ${message.sentTime}",
        )
        when {
            type == "pull" || type == "deliver" -> SyncWorker.enqueue(this, high)
            type == "fallback" || message.notification != null -> Notifications.showFallback(this)
        }
    }
}
