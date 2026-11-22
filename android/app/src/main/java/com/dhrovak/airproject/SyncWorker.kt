package com.dhrovak.airproject

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import java.text.DateFormat
import java.util.Date
import java.util.concurrent.TimeUnit
import uniffi.frame_core.FrameException
import uniffi.frame_core.SyncReport

class SyncWorker(ctx: Context, params: WorkerParameters) : Worker(ctx, params) {

    override fun doWork(): Result {
        val ctx = applicationContext
        val core = CoreFactory.create(ctx) ?: run {
            // The core only cleans up albums it knows about, so removing the last one is handled here.
            if (Prefs.albumUrls(ctx).isEmpty()) CacheStorage.dir(ctx).deleteRecursively()
            return Result.success()
        }
        return core.use {
            try {
                SyncStatus.recordSuccess(ctx, it.sync())
                Result.success()
            } catch (e: FrameException.Network) {
                Log.w(TAG, "Sync failed, attempt ${runAttemptCount + 1}", e)
                SyncStatus.recordFailure(ctx, e)
                if (runAttemptCount < MAX_RETRIES) Result.retry() else Result.failure()
            } catch (e: Exception) {
                Log.w(TAG, "Sync failed", e)
                SyncStatus.recordFailure(ctx, e)
                Result.failure()
            }
        }
    }

    companion object {
        private const val TAG = "SyncWorker"
        private const val MAX_RETRIES = 3
        private const val PERIODIC_NAME = "album-sync-periodic"
        private const val NOW_NAME = "album-sync-now"

        private val networkConstraint = Constraints.Builder()
            .setRequiredNetworkType(NetworkType.CONNECTED)
            .build()

        fun schedulePeriodic(ctx: Context) {
            val request = PeriodicWorkRequestBuilder<SyncWorker>(6, TimeUnit.HOURS)
                .setConstraints(networkConstraint)
                .build()
            WorkManager.getInstance(ctx)
                .enqueueUniquePeriodicWork(PERIODIC_NAME, ExistingPeriodicWorkPolicy.KEEP, request)
        }

        fun syncNow(ctx: Context) {
            val request = OneTimeWorkRequestBuilder<SyncWorker>()
                .setConstraints(networkConstraint)
                .build()
            WorkManager.getInstance(ctx).enqueueUniqueWork(NOW_NAME, ExistingWorkPolicy.REPLACE, request)
        }
    }
}

object SyncStatus {
    fun recordSuccess(ctx: Context, r: SyncReport) {
        val text = buildString {
            append(
                ctx.getString(
                    R.string.status_ok, now(), r.albumName,
                    r.total.toInt(), r.added.toInt(), r.removed.toInt(),
                ),
            )
            if (r.skipped > 0u) append(ctx.getString(R.string.status_skipped, r.skipped.toInt()))
            if (r.failed > 0u) append(ctx.getString(R.string.status_failed, r.failed.toInt()))
            if (r.failedAlbums > 0u) {
                val n = r.failedAlbums.toInt()
                append(ctx.resources.getQuantityString(R.plurals.status_failed_albums, n, n))
            }
        }
        Prefs.setLastSyncStatus(ctx, text)
    }

    fun recordFailure(ctx: Context, e: Exception) {
        Prefs.setLastSyncStatus(ctx, ctx.getString(R.string.status_error, now(), UserError.describe(ctx, e)))
    }

    private fun now(): String = DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date())
}
