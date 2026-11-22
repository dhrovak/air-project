package com.dhrovak.airproject

import android.service.dreams.DreamService
import android.view.ContextThemeWrapper

class PhotoDreamService : DreamService() {
    private var slideshow: SlideshowView? = null

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        isInteractive = false
        isFullscreen = true
        isScreenBright = true
        val view = SlideshowView(ContextThemeWrapper(this, R.style.Theme_AirProject_Slideshow))
        slideshow = view
        setContentView(view)
    }

    override fun onDreamingStarted() {
        super.onDreamingStarted()
        slideshow?.start()
    }

    override fun onDreamingStopped() {
        slideshow?.stop()
        super.onDreamingStopped()
    }

    override fun onDetachedFromWindow() {
        slideshow?.stop()
        slideshow = null
        super.onDetachedFromWindow()
    }
}
