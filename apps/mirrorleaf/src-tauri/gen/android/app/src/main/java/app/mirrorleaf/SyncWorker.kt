package app.mirrorleaf

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequest
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.mirrorleaf.engine.EngineException
import app.mirrorleaf.engine.SyncListener

/**
 * The push-woken sync job (plan §5.9): a short job that syncs with the devices the ledger points
 * to, then exits. No screen is loaded, only the Rust engine, to stay within Android 17's memory
 * limits for background work.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val high = inputData.getBoolean(KEY_HIGH, false)
        val context = applicationContext
        Log.i(MirrorleafApp.TAG, "sync job started (${if (high) "high" else "normal"})")
        // A high-priority push must lead to something visible, or FCM downgrades later ones.
        if (high) Notifications.showSyncing(context)
        return try {
            val summary = MirrorleafApp.engine.syncNow(
                object : SyncListener {
                    override fun sending(deviceName: String, changes: ULong) {
                        if (high) Notifications.showSending(context, changes.toLong(), deviceName)
                    }
                },
            )
            Log.i(MirrorleafApp.TAG, "sync job (${if (high) "high" else "normal"}): $summary")
            Result.success()
        } catch (e: EngineException) {
            Log.w(MirrorleafApp.TAG, "sync job failed: ${e.message}")
            Result.failure()
        } finally {
            if (high) Notifications.clearSyncing(context)
        }
    }

    /** Expedited work runs as a foreground service on Android 11 and older, which needs this. */
    override suspend fun getForegroundInfo() =
        ForegroundInfo(Notifications.SYNC_ID, Notifications.syncing(applicationContext))

    companion object {
        private const val KEY_HIGH = "high"
        private const val WORK_NAME = "sync"

        /**
         * A normal-priority push queues a regular job; a high-priority one an expedited job, which
         * replaces any regular job still waiting (for example, held back by Doze).
         */
        fun enqueue(context: Context, high: Boolean) {
            // The plain builders, not the Kotlin helpers: those are compiled for Java 11, and
            // Tauri's template builds for Java 8 (ADR-005).
            val request = OneTimeWorkRequest.Builder(SyncWorker::class.java)
                .setInputData(Data.Builder().putBoolean(KEY_HIGH, high).build())
                .setConstraints(
                    Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build(),
                )
                .apply {
                    if (high) setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                }
                .build()
            val policy = if (high) ExistingWorkPolicy.REPLACE else ExistingWorkPolicy.KEEP
            WorkManager.getInstance(context).enqueueUniqueWork(WORK_NAME, policy, request)
        }
    }
}
