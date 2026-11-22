package com.dhrovak.airproject

import android.content.Intent
import android.os.Bundle
import android.view.KeyEvent
import android.view.WindowManager
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class SlideshowActivity : AppCompatActivity() {
    private var slideshow: SlideshowView? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Prefs.albumUrls(this).isEmpty()) {
            startActivity(Intent(this, SetupActivity::class.java))
            finish()
            return
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        WindowCompat.getInsetsController(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
        slideshow = SlideshowView(this).also { setContentView(it) }
    }

    override fun onStart() {
        super.onStart()
        slideshow?.start()
        slideshow?.showControlsHint()
    }

    override fun onStop() {
        slideshow?.stop()
        super.onStop()
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val view = slideshow ?: return super.onKeyDown(keyCode, event)
        val action: (() -> Unit)? = when (keyCode) {
            KeyEvent.KEYCODE_DPAD_RIGHT, KeyEvent.KEYCODE_MEDIA_NEXT,
            KeyEvent.KEYCODE_MEDIA_FAST_FORWARD -> view::next
            KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_MEDIA_PREVIOUS,
            KeyEvent.KEYCODE_MEDIA_REWIND -> view::previous
            KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER,
            KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, KeyEvent.KEYCODE_SPACE -> view::togglePause
            KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_MENU -> ::openSettings
            KeyEvent.KEYCODE_DPAD_UP -> view::showControlsHint
            else -> null
        }
        if (action == null) return super.onKeyDown(keyCode, event)
        // Holding a button should not skip through dozens of photos.
        if (event.repeatCount == 0) action()
        return true
    }

    private fun openSettings() {
        startActivity(Intent(this, SettingsActivity::class.java))
    }
}
