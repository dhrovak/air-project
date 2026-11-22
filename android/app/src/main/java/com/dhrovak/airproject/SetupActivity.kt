package com.dhrovak.airproject

import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Color
import android.net.ConnectivityManager
import android.os.Bundle
import android.util.Log
import android.widget.Button
import android.widget.ImageView
import android.widget.ProgressBar
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.isVisible
import androidx.lifecycle.lifecycleScope
import java.net.Inet4Address
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.frame_core.SetupServer
import uniffi.frame_core.qrMatrix
import uniffi.frame_core.syncProgress

/** TV-friendly first-run screen: manage albums and settings from a phone, watch the download, then start. */
class SetupActivity : AppCompatActivity() {
    private lateinit var step1Status: TextView
    private lateinit var step2Status: TextView
    private lateinit var step3Status: TextView
    private lateinit var progress: ProgressBar
    private lateinit var qr: ImageView
    private lateinit var qrCaption: TextView
    private lateinit var qrUrl: TextView
    private lateinit var btnStart: Button

    private var server: SetupServer? = null
    private var pollJob: Job? = null
    private var wasLinked = false
    private var qrInitialized = false
    private var shownUrl: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_setup)
        step1Status = findViewById(R.id.step1_status)
        step2Status = findViewById(R.id.step2_status)
        step3Status = findViewById(R.id.step3_status)
        progress = findViewById(R.id.progress)
        qr = findViewById(R.id.qr)
        qrCaption = findViewById(R.id.qr_caption)
        qrUrl = findViewById(R.id.qr_url)
        btnStart = findViewById(R.id.btn_start)

        btnStart.setOnClickListener {
            startActivity(Intent(this, SlideshowActivity::class.java))
        }
        findViewById<Button>(R.id.btn_screensaver).setOnClickListener { Screensaver.showSetupDialog(this) }
        findViewById<Button>(R.id.btn_settings).setOnClickListener {
            startActivity(Intent(this, SettingsActivity::class.java))
        }
    }

    override fun onStart() {
        super.onStart()
        wasLinked = Prefs.albumUrls(this).isNotEmpty()
        qrInitialized = false
        updateQr()
        refresh()
        (if (wasLinked) btnStart else findViewById<Button>(R.id.btn_settings)).requestFocus()
        pollJob = lifecycleScope.launch {
            var tick = 0
            while (isActive) {
                // Network checks and state snapshots are costlier, so they run less often than the UI refresh.
                if (tick++ % SLOW_TICKS == 0) {
                    updateQr()
                    pushPhoneState()
                }
                delay(POLL_MS)
                if (refresh()) pushPhoneState()
            }
        }
    }

    override fun onStop() {
        pollJob?.cancel()
        pollJob = null
        server?.let {
            it.stop()
            it.close()
        }
        server = null
        super.onStop()
    }

    /** Starts the server once the TV has a LAN address and keeps the QR code in sync with that address. */
    private fun updateQr() {
        val ip = lanIpv4()
        if (ip != null && server == null) {
            server = try {
                SetupServer.start()
            } catch (e: Exception) {
                Log.w(TAG, "Cannot start setup server", e)
                null
            }
        }
        val s = server
        val url = if (ip != null && s != null) s.url(ip) else null
        if (qrInitialized && url == shownUrl) return
        qrInitialized = true
        shownUrl = url
        qr.isVisible = url != null
        qrUrl.isVisible = url != null
        if (url == null) {
            qrCaption.setText(R.string.setup_no_network)
            return
        }
        qr.setImageBitmap(qrBitmap(url))
        qrUrl.text = url
        qrCaption.setText(R.string.setup_qr_caption)
    }

    private suspend fun pushPhoneState() {
        val s = server ?: return
        val appCtx = applicationContext
        val state = withContext(Dispatchers.IO) { PhoneRemote.state(appCtx) }
        if (server === s) s.setState(state)
    }

    /** Applies changes made on the phone and updates the screen; returns whether anything changed. */
    private fun refresh(): Boolean {
        var changed = false
        while (true) {
            val command = server?.takeCommand() ?: break
            changed = true
            Toast.makeText(this, PhoneRemote.apply(this, command), Toast.LENGTH_SHORT).show()
        }

        val albumCount = Prefs.albumUrls(this).size
        val linked = albumCount > 0
        step1Status.text = if (linked) {
            resources.getQuantityString(R.plurals.setup_linked, albumCount, albumCount)
        } else {
            getString(R.string.setup_not_linked)
        }
        btnStart.isEnabled = linked
        if (linked && !wasLinked) btnStart.requestFocus()
        wasLinked = linked

        val p = syncProgress()
        val fraction = SyncProgressText.fraction(p)
        progress.isVisible = p.running
        progress.isIndeterminate = fraction == null
        if (fraction != null) {
            progress.max = PROGRESS_STEPS
            progress.progress = (fraction * PROGRESS_STEPS).toInt()
        }
        step2Status.text = SyncProgressText.describe(this, p)
            ?: if (linked) {
                Prefs.lastSyncStatus(this) ?: getString(R.string.setup_waiting)
            } else {
                getString(R.string.setup_after_link)
            }

        step3Status.text = Screensaver.statusText(this)
        return changed
    }

    private fun lanIpv4(): String? {
        val cm = getSystemService(ConnectivityManager::class.java) ?: return null
        val props = cm.getLinkProperties(cm.activeNetwork) ?: return null
        return props.linkAddresses
            .map { it.address }
            .firstOrNull { it is Inet4Address && !it.isLoopbackAddress }
            ?.hostAddress
    }

    private fun qrBitmap(text: String): Bitmap? {
        val matrix = try {
            qrMatrix(text)
        } catch (e: Exception) {
            Log.w(TAG, "Cannot encode QR code", e)
            return null
        }
        val size = matrix.size.toInt()
        val side = (size + 2 * QR_QUIET_ZONE) * QR_SCALE
        val pixels = IntArray(side * side) { Color.WHITE }
        for (y in 0 until size) {
            for (x in 0 until size) {
                if (!matrix.modules[y * size + x]) continue
                val left = (x + QR_QUIET_ZONE) * QR_SCALE
                val top = (y + QR_QUIET_ZONE) * QR_SCALE
                for (dy in 0 until QR_SCALE) {
                    pixels.fill(Color.BLACK, (top + dy) * side + left, (top + dy) * side + left + QR_SCALE)
                }
            }
        }
        return Bitmap.createBitmap(pixels, side, side, Bitmap.Config.ARGB_8888)
    }

    private companion object {
        const val TAG = "SetupActivity"
        const val POLL_MS = 500L
        const val SLOW_TICKS = 4
        const val PROGRESS_STEPS = 1000
        const val QR_SCALE = 8
        const val QR_QUIET_ZONE = 4
    }
}
