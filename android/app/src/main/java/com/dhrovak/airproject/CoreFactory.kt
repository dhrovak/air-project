package com.dhrovak.airproject

import android.content.Context
import android.util.Log
import kotlin.math.max
import kotlin.math.min
import uniffi.frame_core.FrameConfig
import uniffi.frame_core.FrameException
import uniffi.frame_core.PhotoCore

object CoreFactory {
    private const val TAG = "CoreFactory"

    /** Returns null when no valid album is configured. Blocking; caller must close it. */
    fun create(ctx: Context): PhotoCore? {
        val urls = Prefs.albumUrls(ctx)
        if (urls.isEmpty()) return null
        val metrics = ctx.resources.displayMetrics
        val config = FrameConfig(
            cacheDir = CacheStorage.dir(ctx).absolutePath,
            albumUrls = urls,
            maxCacheMb = CacheStorage.limitMb(ctx).toUInt(),
            screenWidth = max(metrics.widthPixels, metrics.heightPixels).toUInt(),
            screenHeight = min(metrics.widthPixels, metrics.heightPixels).toUInt(),
            includeVideos = Prefs.includeVideos(ctx),
        )
        return try {
            PhotoCore(config)
        } catch (e: FrameException) {
            Log.w(TAG, "Cannot open photo core", e)
            null
        }
    }
}
