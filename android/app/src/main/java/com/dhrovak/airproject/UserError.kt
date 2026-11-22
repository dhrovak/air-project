package com.dhrovak.airproject

import android.content.Context
import uniffi.frame_core.FrameException

/** Plain-language explanation of a failure; technical details belong in the log. */
object UserError {
    fun describe(ctx: Context, e: Throwable): String = ctx.getString(
        when (e) {
            is FrameException.InvalidUrl -> R.string.error_invalid_link
            is FrameException.AlbumUnavailable -> R.string.error_album_unavailable
            is FrameException.Network -> R.string.error_network
            is FrameException.Protocol -> R.string.error_protocol
            is FrameException.Storage -> R.string.error_storage
            is FrameException.Image -> R.string.error_image
            else -> R.string.error_unknown
        },
    )
}
