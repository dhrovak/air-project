package com.dhrovak.airproject

import android.content.Context
import android.text.format.Formatter
import uniffi.frame_core.MediaKind
import uniffi.frame_core.SyncProgress
import uniffi.frame_core.SyncStage

object SyncProgressText {
    /** Detailed description for full-screen status, or null when no sync is running. */
    fun describe(ctx: Context, p: SyncProgress): String? {
        if (!p.running) return null
        val detail = describeStage(ctx, p) ?: return null
        return if (p.albumCount > 1u && p.stage != SyncStage.FINISHING) {
            ctx.getString(R.string.progress_album_of, p.albumIndex.toInt(), p.albumCount.toInt()) + SEPARATOR + detail
        } else {
            detail
        }
    }

    private fun describeStage(ctx: Context, p: SyncProgress): String? {
        return when (p.stage) {
            SyncStage.IDLE -> null
            SyncStage.CONNECTING -> ctx.getString(R.string.progress_connecting)
            SyncStage.PREPARING ->
                ctx.getString(R.string.progress_preparing, p.done.toInt(), p.total.toInt())
            SyncStage.DOWNLOADING -> if (p.total == 0u) {
                ctx.getString(R.string.progress_up_to_date)
            } else buildString {
                val album = p.albumName.ifBlank { ctx.getString(R.string.progress_your_album) }
                append(ctx.getString(R.string.progress_downloading, album, current(p), p.total.toInt()))
                p.currentKind?.let { kind ->
                    append(SEPARATOR)
                    append(fileProgress(ctx, kind, p))
                }
                append(SEPARATOR)
                append(ctx.getString(R.string.progress_total, size(ctx, p.bytesDownloaded)))
            }
            SyncStage.FINISHING -> ctx.getString(R.string.progress_finishing)
        }
    }

    /** Compact one-liner for a corner badge, or null when no sync is running. */
    fun short(ctx: Context, p: SyncProgress): String? {
        if (!p.running) return null
        return when (p.stage) {
            SyncStage.IDLE -> null
            SyncStage.CONNECTING -> ctx.getString(R.string.progress_short_checking)
            SyncStage.PREPARING -> ctx.getString(R.string.progress_short_preparing)
            SyncStage.DOWNLOADING -> if (p.total == 0u) {
                null
            } else if (p.albumCount > 1u) {
                ctx.getString(
                    R.string.progress_short_downloading_albums,
                    p.albumIndex.toInt(), p.albumCount.toInt(), current(p), p.total.toInt(),
                )
            } else {
                ctx.getString(R.string.progress_short_downloading, current(p), p.total.toInt())
            }
            SyncStage.FINISHING -> ctx.getString(R.string.progress_short_finishing)
        }
    }

    /** Overall fraction in [0, 1] including progress within the current file, or null if unknown. */
    fun fraction(p: SyncProgress): Float? {
        if (!p.running || p.total == 0u) return null
        if (p.stage != SyncStage.DOWNLOADING && p.stage != SyncStage.PREPARING) return null
        val within = if (p.currentSize > 0uL) {
            (p.currentBytes.toDouble() / p.currentSize.toDouble()).coerceIn(0.0, 1.0)
        } else {
            0.0
        }
        return ((p.done.toDouble() + within) / p.total.toDouble()).toFloat().coerceIn(0f, 1f)
    }

    private fun current(p: SyncProgress): Int = minOf(p.done + 1u, p.total).toInt()

    private fun fileProgress(ctx: Context, kind: MediaKind, p: SyncProgress): String {
        val what = ctx.getString(
            when (kind) {
                MediaKind.VIDEO -> R.string.kind_video
                MediaKind.LIVE_PHOTO -> R.string.kind_live_photo
                MediaKind.PHOTO -> R.string.kind_photo
            },
        )
        return if (p.currentSize > 0uL) {
            ctx.getString(R.string.progress_file_of, what, size(ctx, p.currentBytes), size(ctx, p.currentSize))
        } else {
            ctx.getString(R.string.progress_file, what, size(ctx, p.currentBytes))
        }
    }

    private fun size(ctx: Context, bytes: ULong): String = Formatter.formatShortFileSize(ctx, bytes.toLong())

    private const val SEPARATOR = "  ·  "
}
