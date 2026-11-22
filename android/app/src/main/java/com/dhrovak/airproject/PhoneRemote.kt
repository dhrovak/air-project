package com.dhrovak.airproject

import android.content.Context
import androidx.annotation.ArrayRes
import uniffi.frame_core.PhoneAlbum
import uniffi.frame_core.PhoneCommand
import uniffi.frame_core.PhoneOption
import uniffi.frame_core.PhoneSettings
import uniffi.frame_core.PhoneState

/** Bridges the phone web page served by [uniffi.frame_core.SetupServer] and the app's settings. */
object PhoneRemote {
    /** Blocking: reads the cache. */
    fun state(ctx: Context): PhoneState {
        val summaries = CoreFactory.create(ctx)?.use { it.albumSummaries() }.orEmpty().associateBy { it.url }
        val albums = Prefs.albumUrls(ctx).map { url ->
            val summary = summaries[url]
            val count = summary?.itemCount?.toInt() ?: 0
            PhoneAlbum(
                url = url,
                name = summary?.name.orEmpty(),
                detail = if (count > 0) {
                    ctx.resources.getQuantityString(R.plurals.phone_album_items, count, count)
                } else {
                    ctx.getString(R.string.album_waiting_sync)
                },
            )
        }
        return PhoneState(
            albums = albums,
            settings = PhoneSettings(
                intervalSeconds = Prefs.intervalSeconds(ctx).toUInt(),
                shuffle = Prefs.shuffle(ctx),
                showClock = Prefs.showClock(ctx),
                includeVideos = Prefs.includeVideos(ctx),
                cacheMb = Prefs.cacheSettingMb(ctx).toUInt(),
            ),
            intervalOptions = options(ctx, R.array.interval_entries, R.array.interval_values),
            cacheOptions = options(ctx, R.array.cache_entries, R.array.cache_values),
            syncStatus = Prefs.lastSyncStatus(ctx) ?: ctx.getString(R.string.status_never),
            storageStatus = CacheStorage.statusText(ctx),
        )
    }

    /** Applies a change made on the phone; returns a message to show on the TV. */
    fun apply(ctx: Context, command: PhoneCommand): Int = when (command) {
        is PhoneCommand.AddAlbum -> if (Prefs.addAlbumUrl(ctx, command.url)) {
            SyncWorker.syncNow(ctx)
            R.string.album_added
        } else {
            R.string.album_duplicate
        }
        is PhoneCommand.RemoveAlbum -> {
            Prefs.removeAlbumUrl(ctx, command.url)
            SyncWorker.syncNow(ctx)
            R.string.album_removed
        }
        is PhoneCommand.UpdateSettings -> {
            if (Prefs.applySettings(ctx, command.settings)) SyncWorker.syncNow(ctx)
            R.string.settings_saved
        }
    }

    private fun options(ctx: Context, @ArrayRes labels: Int, @ArrayRes values: Int): List<PhoneOption> =
        ctx.resources.getStringArray(labels).zip(ctx.resources.getStringArray(values))
            .mapNotNull { (label, value) -> value.toUIntOrNull()?.let { PhoneOption(it, label) } }
}
