package com.dhrovak.airproject

import android.content.Context
import android.text.format.Formatter
import java.io.File

/** Where photos are cached and how much space they may use. Methods walk the cache, so call them off the main thread. */
object CacheStorage {
    private const val MB = 1024L * 1024L
    private const val MIN_MB = 50L
    private const val AUTO_MAX_MB = 4000L

    fun dir(ctx: Context): File = File(ctx.noBackupFilesDir, "album")

    fun usedBytes(ctx: Context): Long =
        dir(ctx).walkTopDown().filter { it.isFile }.sumOf { it.length() }

    fun limitMb(ctx: Context): Int {
        val setting = Prefs.cacheSettingMb(ctx)
        return if (setting > 0) setting else autoMb(ctx, usedBytes(ctx)).toInt()
    }

    fun statusText(ctx: Context): String {
        val used = usedBytes(ctx)
        val setting = Prefs.cacheSettingMb(ctx)
        val limitMb = if (setting > 0) setting.toLong() else autoMb(ctx, used)
        return ctx.getString(
            if (setting > 0) R.string.storage_status else R.string.storage_status_auto,
            Formatter.formatShortFileSize(ctx, used),
            Formatter.formatShortFileSize(ctx, limitMb * MB),
        )
    }

    /** Half of what the cache could grow into, so the TV keeps room for its own apps and updates. */
    private fun autoMb(ctx: Context, usedBytes: Long): Long {
        val free = ctx.noBackupFilesDir.usableSpace
        return ((usedBytes + free) / 2 / MB).coerceIn(MIN_MB, AUTO_MAX_MB)
    }
}
