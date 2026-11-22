package com.dhrovak.airproject

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.net.Uri
import android.util.AttributeSet
import android.util.Log
import android.view.View
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.TextView
import androidx.core.view.isVisible
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import java.io.File
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.frame_core.MediaItem
import uniffi.frame_core.MediaKind
import uniffi.frame_core.PhotoCore
import uniffi.frame_core.syncProgress

/** Full-screen slideshow shared by the screensaver and the launcher activity. */
class SlideshowView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : FrameLayout(context, attrs) {

    private class Slot(val root: View, val background: ImageView, val image: ImageView) {
        fun clear() {
            background.setImageDrawable(null)
            image.setImageDrawable(null)
        }
    }

    private val slotA: Slot
    private val slotB: Slot
    private val videoView: PlayerView
    private val clockBox: View
    private val message: TextView
    private val hint: TextView
    private val pausedBadge: TextView
    private val syncStatus: TextView

    private var front: Slot
    private var back: Slot

    private var scope: CoroutineScope? = null
    private var core: PhotoCore? = null
    private var player: ExoPlayer? = null
    private var initialSync = false

    private enum class Command { NEXT, PREVIOUS, TOGGLE_PAUSE }

    private val commands = Channel<Command>(Channel.CONFLATED)
    private var paused = false
    private val hideHint = Runnable { hint.isVisible = false }

    init {
        inflate(context, R.layout.view_slideshow, this)
        setBackgroundColor(Color.BLACK)
        slotA = Slot(findViewById(R.id.slot_a), findViewById(R.id.background_a), findViewById(R.id.image_a))
        slotB = Slot(findViewById(R.id.slot_b), findViewById(R.id.background_b), findViewById(R.id.image_b))
        videoView = findViewById(R.id.video)
        clockBox = findViewById(R.id.clock_box)
        message = findViewById(R.id.message)
        hint = findViewById(R.id.hint)
        pausedBadge = findViewById(R.id.paused)
        syncStatus = findViewById(R.id.sync_status)
        front = slotA
        back = slotB
    }

    fun next() {
        commands.trySend(Command.NEXT)
    }

    fun previous() {
        commands.trySend(Command.PREVIOUS)
    }

    fun togglePause() {
        commands.trySend(Command.TOGGLE_PAUSE)
    }

    fun showControlsHint() {
        hint.isVisible = true
        removeCallbacks(hideHint)
        postDelayed(hideHint, HINT_MS)
    }

    fun start() {
        if (scope != null) return
        clockBox.isVisible = Prefs.showClock(context)
        while (commands.tryReceive().isSuccess) Unit
        setPaused(false)
        val s = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        scope = s
        s.launch { runSlideshow() }
        s.launch {
            while (isActive) {
                updateSyncStatus()
                delay(PROGRESS_POLL_MS)
            }
        }
    }

    fun stop() {
        scope?.cancel()
        scope = null
        initialSync = false
        syncStatus.isVisible = false
        removeCallbacks(hideHint)
        hint.isVisible = false
        player?.release()
        player = null
        videoView.player = null
        videoView.isVisible = false
        core?.close()
        core = null
        for (slot in listOf(slotA, slotB)) {
            slot.root.animate().cancel()
            slot.clear()
        }
        slotA.root.alpha = 1f
        slotB.root.alpha = 0f
        front = slotA
        back = slotB
    }

    private fun updateSyncStatus() {
        val p = syncProgress()
        if (initialSync) {
            SyncProgressText.describe(context, p)?.let { message.text = it }
            syncStatus.isVisible = false
        } else {
            val text = SyncProgressText.short(context, p)
            syncStatus.text = text
            syncStatus.isVisible = text != null
        }
    }

    private suspend fun runSlideshow() {
        val c = withContext(Dispatchers.IO) { CoreFactory.create(context) }
        if (c == null) {
            showMessage(context.getString(R.string.msg_no_album))
            return
        }
        core = c
        val shuffle = Prefs.shuffle(context)
        var items = withContext(Dispatchers.IO) { c.playlist(shuffle) }

        if (items.isEmpty()) {
            showMessage(context.getString(R.string.msg_syncing))
            initialSync = true
            val error = withContext(Dispatchers.IO) {
                try {
                    SyncStatus.recordSuccess(context, c.sync())
                    null
                } catch (e: Exception) {
                    Log.w(TAG, "Initial sync failed", e)
                    SyncStatus.recordFailure(context, e)
                    UserError.describe(context, e)
                }
            }
            items = withContext(Dispatchers.IO) { c.playlist(shuffle) }
            if (items.isEmpty()) {
                initialSync = false
                showMessage(error ?: context.getString(R.string.msg_empty_album))
                return
            }
        }
        initialSync = false
        message.isVisible = false

        val intervalMs = Prefs.intervalMs(context)
        var index = 0
        while (currentCoroutineContext().isActive) {
            if (index >= items.size) {
                index = 0
                // Picks up items added by background syncs and reshuffles.
                items = withContext(Dispatchers.IO) { c.playlist(shuffle) }.ifEmpty { items }
            } else if (index < 0) {
                index = items.lastIndex
            }
            index += if (show(items[index], intervalMs) == Command.PREVIOUS) -1 else 1
        }
    }

    /** Shows one item; returns the navigation command that interrupted it, if any. */
    private suspend fun show(item: MediaItem, intervalMs: Long): Command? {
        val imageShown = showImage(item.imagePath, item.backgroundPath)
        val video = item.videoPath
        if (video != null && item.kind != MediaKind.PHOTO) {
            val isVideo = item.kind == MediaKind.VIDEO
            val (played, command) = playVideo(video, if (isVideo) MAX_VIDEO_MS else MAX_LIVE_MS)
            if (command != null) return command
            if (isVideo && played) return null
        }
        // A short wait on failure avoids a tight loop over unreadable files.
        return hold(if (imageShown) intervalMs else FAILURE_DELAY_MS)
    }

    /** Waits for the interval (indefinitely while paused), handling pause toggles. */
    private suspend fun hold(ms: Long): Command? {
        while (true) {
            val command = if (paused) {
                commands.receive()
            } else {
                withTimeoutOrNull(ms) { commands.receive() } ?: return null
            }
            if (command != Command.TOGGLE_PAUSE) return command
            setPaused(!paused)
        }
    }

    private fun setPaused(value: Boolean) {
        paused = value
        pausedBadge.isVisible = value
        player?.playWhenReady = !value
    }

    private suspend fun showImage(path: String?, backgroundPath: String?): Boolean {
        if (path == null) return false
        val (bitmap, background) = withContext(Dispatchers.IO) {
            decode(path) to backgroundPath?.let(::decode)
        }
        if (bitmap == null) return false
        val incoming = back
        val outgoing = front
        incoming.image.setImageBitmap(bitmap)
        incoming.background.setImageBitmap(background)
        incoming.root.animate().alpha(1f).setDuration(FADE_MS)
        outgoing.root.animate().alpha(0f).setDuration(FADE_MS)
        front = incoming
        back = outgoing
        delay(FADE_MS)
        outgoing.clear()
        return true
    }

    /** Plays a muted clip over the current image; returns whether it played and any interrupting command. */
    private suspend fun playVideo(path: String, maxMs: Long): Pair<Boolean, Command?> {
        val p = player ?: ExoPlayer.Builder(context).build().also {
            it.volume = 0f
            player = it
            videoView.player = it
        }
        val outcome = CompletableDeferred<Boolean>()
        val listener = object : Player.Listener {
            override fun onPlaybackStateChanged(playbackState: Int) {
                if (playbackState == Player.STATE_ENDED) outcome.complete(true)
            }

            override fun onPlayerError(error: PlaybackException) {
                Log.w(TAG, "Cannot play $path", error)
                outcome.complete(false)
            }
        }
        p.addListener(listener)
        videoView.isVisible = true
        try {
            p.setMediaItem(androidx.media3.common.MediaItem.fromUri(Uri.fromFile(File(path))))
            p.prepare()
            p.playWhenReady = !paused
            while (true) {
                val result = withTimeoutOrNull(maxMs) {
                    select<Any> {
                        outcome.onAwait { it }
                        commands.onReceive { it }
                    }
                } ?: return true to null
                when (result) {
                    Command.TOGGLE_PAUSE -> setPaused(!paused)
                    is Command -> return true to result
                    else -> return (result as Boolean) to null
                }
            }
        } finally {
            // stop() may already have released the player.
            if (player === p) {
                p.removeListener(listener)
                p.stop()
                p.clearMediaItems()
            }
            videoView.isVisible = false
        }
    }

    private fun showMessage(text: String) {
        message.text = text
        message.isVisible = true
    }

    private fun decode(path: String): Bitmap? = try {
        // Hardware bitmaps keep pixel data out of the app heap on low-RAM TVs.
        val hardware = BitmapFactory.Options().apply { inPreferredConfig = Bitmap.Config.HARDWARE }
        BitmapFactory.decodeFile(path, hardware) ?: BitmapFactory.decodeFile(path)
    } catch (e: Exception) {
        Log.w(TAG, "Cannot decode $path", e)
        null
    } catch (e: OutOfMemoryError) {
        Log.w(TAG, "Out of memory decoding $path", e)
        null
    }

    private companion object {
        const val TAG = "SlideshowView"
        const val FADE_MS = 1_000L
        const val HINT_MS = 5_000L
        const val PROGRESS_POLL_MS = 500L
        const val FAILURE_DELAY_MS = 2_000L
        const val MAX_VIDEO_MS = 5 * 60_000L
        const val MAX_LIVE_MS = 10_000L
    }
}
