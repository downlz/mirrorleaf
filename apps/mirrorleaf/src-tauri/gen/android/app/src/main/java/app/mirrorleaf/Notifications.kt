package app.mirrorleaf

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

/**
 * The notifications behind the push rules (plan §5.9): "Syncing…" and "Sending N changes to
 * <name>" during a high-priority job, and the fallback prompt.
 */
object Notifications {
    const val SYNC_CHANNEL = "sync"
    /** Must match the channel the ledger names in the fallback push. */
    const val FALLBACK_CHANNEL = "fallback"
    const val SYNC_ID = 1
    private const val FALLBACK_ID = 2

    fun createChannels(context: Context) {
        if (Build.VERSION.SDK_INT < 26) return
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                SYNC_CHANNEL,
                context.getString(R.string.channel_sync),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
        manager.createNotificationChannel(
            NotificationChannel(
                FALLBACK_CHANNEL,
                context.getString(R.string.channel_fallback),
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
    }

    /** False when the user has denied notifications (Android 13+) or turned them off. */
    fun allowed(context: Context): Boolean =
        NotificationManagerCompat.from(context).areNotificationsEnabled()

    fun syncing(context: Context): Notification =
        build(context, SYNC_CHANNEL, context.getString(R.string.syncing), null)

    fun showSyncing(context: Context) = show(context, SYNC_ID, syncing(context))

    fun showSending(context: Context, changes: Long, device: String) {
        val text = context.resources.getQuantityString(
            R.plurals.sending_changes,
            changes.coerceAtMost(Int.MAX_VALUE.toLong()).toInt(),
            changes,
            device,
        )
        show(context, SYNC_ID, build(context, SYNC_CHANNEL, text, null))
    }

    fun clearSyncing(context: Context) = NotificationManagerCompat.from(context).cancel(SYNC_ID)

    fun showFallback(context: Context) = show(
        context,
        FALLBACK_ID,
        build(
            context,
            FALLBACK_CHANNEL,
            context.getString(R.string.sync_fallback_title),
            context.getString(R.string.sync_fallback_body),
        ),
    )

    private fun build(context: Context, channel: String, title: String, text: String?) =
        NotificationCompat.Builder(context, channel)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(title)
            .apply { if (text != null) setContentText(text) }
            .setContentIntent(openApp(context))
            .setAutoCancel(true)
            .build()

    private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    @SuppressLint("MissingPermission") // checked by allowed()
    private fun show(context: Context, id: Int, notification: Notification) {
        if (allowed(context)) NotificationManagerCompat.from(context).notify(id, notification)
    }
}
