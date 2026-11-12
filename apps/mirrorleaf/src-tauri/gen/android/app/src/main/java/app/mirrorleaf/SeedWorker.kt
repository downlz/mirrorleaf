package app.mirrorleaf

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequest
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/**
 * "Seed while charging" (M12, E8), off by default: while the phone charges on unmetered Wi-Fi, it
 * keeps live sync running for a while, so the other devices can reach it and catch up from it.
 *
 * An ordinary background job, not a foreground service: Android 12 and later don't let an app
 * start a foreground service from the background, which is where it would have to start when the
 * phone is plugged in. Android runs this job only while both conditions hold, stops it when either
 * ends, and runs it again every 15 minutes while they hold; each run lasts at most 9 minutes,
 * within the 10 minutes Android gives background work. No notification is needed.
 */
class SeedWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        Log.i(MirrorleafApp.TAG, "seeding while charging")
        return try {
            MirrorleafApp.engine.setSeeding(true)
            // Cancelled early when the phone is unplugged or leaves Wi-Fi.
            delay(RUN_FOR_MS)
            Result.success()
        } catch (e: app.mirrorleaf.engine.EngineException) {
            Log.w(MirrorleafApp.TAG, "seeding: ${e.message}")
            Result.success()
        } finally {
            withContext(NonCancellable) {
                runCatching { MirrorleafApp.engine.setSeeding(false) }
            }
            Log.i(MirrorleafApp.TAG, "seeding stopped")
        }
    }

    companion object {
        private const val WORK_NAME = "seed"
        private const val PREFS = "mirrorleaf"
        private const val KEY_ON = "seed_while_charging"
        private const val RUN_FOR_MS = 9 * 60 * 1000L

        fun isOn(context: Context): Boolean =
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_ON, false)

        /** Turns the job on or off; WorkManager keeps it across restarts. */
        fun schedule(context: Context, on: Boolean) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ON, on).apply()
            val work = WorkManager.getInstance(context)
            if (!on) {
                work.cancelUniqueWork(WORK_NAME)
                return
            }
            // The plain builders: the Kotlin helpers are compiled for Java 11 (ADR-005).
            val request = PeriodicWorkRequest.Builder(SeedWorker::class.java, 15, TimeUnit.MINUTES)
                .setConstraints(
                    Constraints.Builder()
                        .setRequiresCharging(true)
                        .setRequiredNetworkType(NetworkType.UNMETERED)
                        .build(),
                )
                .build()
            work.enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.UPDATE, request)
        }
    }
}
