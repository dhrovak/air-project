package com.dhrovak.airproject

import android.content.Context
import android.content.SharedPreferences
import androidx.preference.PreferenceManager
import uniffi.frame_core.FrameException
import uniffi.frame_core.PhoneSettings
import uniffi.frame_core.parseAlbumToken

object Prefs {
    const val KEY_ALBUM_URLS = "album_urls"
    private const val KEY_LEGACY_ALBUM_URL = "album_url"
    const val KEY_INTERVAL = "interval_seconds"
    const val KEY_SHUFFLE = "shuffle"
    const val KEY_SHOW_CLOCK = "show_clock"
    const val KEY_INCLUDE_VIDEOS = "include_videos"
    const val KEY_MAX_CACHE_MB = "max_cache_mb"
    const val KEY_LAST_SYNC = "last_sync_status"
    const val KEY_START_ON_BOOT = "start_on_boot"

    private fun of(ctx: Context): SharedPreferences = PreferenceManager.getDefaultSharedPreferences(ctx)

    fun albumUrls(ctx: Context): List<String> {
        val prefs = of(ctx)
        val stored = prefs.getString(KEY_ALBUM_URLS, null)
        if (stored == null) {
            val legacy = prefs.getString(KEY_LEGACY_ALBUM_URL, null)?.trim().orEmpty()
            if (legacy.isEmpty()) return emptyList()
            prefs.edit().putString(KEY_ALBUM_URLS, legacy).remove(KEY_LEGACY_ALBUM_URL).apply()
            return listOf(legacy)
        }
        return stored.lines().map { it.trim() }.filter { it.isNotEmpty() }
    }

    /** Adds a link unless the same album is already present; returns whether it was added. */
    fun addAlbumUrl(ctx: Context, url: String): Boolean {
        val key = albumKey(url) ?: return false
        val current = albumUrls(ctx)
        if (current.any { albumKey(it) == key }) return false
        setAlbumUrls(ctx, current + url.trim())
        return true
    }

    fun removeAlbumUrl(ctx: Context, url: String) {
        setAlbumUrls(ctx, albumUrls(ctx).filterNot { it == url })
    }

    private fun setAlbumUrls(ctx: Context, urls: List<String>) {
        of(ctx).edit().putString(KEY_ALBUM_URLS, urls.joinToString("\n")).apply()
    }

    private fun albumKey(url: String): String? = try {
        parseAlbumToken(url)
    } catch (e: FrameException) {
        null
    }

    fun intervalSeconds(ctx: Context): Int =
        (of(ctx).getString(KEY_INTERVAL, null)?.toIntOrNull() ?: 15).coerceIn(5, 3600)

    fun intervalMs(ctx: Context): Long = intervalSeconds(ctx) * 1000L

    fun shuffle(ctx: Context): Boolean = of(ctx).getBoolean(KEY_SHUFFLE, true)

    fun showClock(ctx: Context): Boolean = of(ctx).getBoolean(KEY_SHOW_CLOCK, true)

    fun includeVideos(ctx: Context): Boolean = of(ctx).getBoolean(KEY_INCLUDE_VIDEOS, true)

    /** Cache size chosen by the user; 0 means automatic (see [CacheStorage.limitMb]). */
    fun cacheSettingMb(ctx: Context): Int =
        (of(ctx).getString(KEY_MAX_CACHE_MB, null)?.toIntOrNull() ?: 0).coerceAtLeast(0)

    fun startOnBoot(ctx: Context): Boolean = of(ctx).getBoolean(KEY_START_ON_BOOT, false)

    /** Returns whether a setting that affects downloads changed. */
    fun applySettings(ctx: Context, s: PhoneSettings): Boolean {
        val resync = s.includeVideos != includeVideos(ctx) || s.cacheMb.toInt() != cacheSettingMb(ctx)
        of(ctx).edit()
            .putString(KEY_INTERVAL, s.intervalSeconds.toString())
            .putBoolean(KEY_SHUFFLE, s.shuffle)
            .putBoolean(KEY_SHOW_CLOCK, s.showClock)
            .putBoolean(KEY_INCLUDE_VIDEOS, s.includeVideos)
            .putString(KEY_MAX_CACHE_MB, s.cacheMb.toString())
            .apply()
        return resync
    }

    fun lastSyncStatus(ctx: Context): String? = of(ctx).getString(KEY_LAST_SYNC, null)

    fun setLastSyncStatus(ctx: Context, status: String) {
        of(ctx).edit().putString(KEY_LAST_SYNC, status).apply()
    }
}
