package com.dhrovak.airproject

import android.app.Application
import androidx.preference.PreferenceManager

class FrameApp : Application() {
    override fun onCreate() {
        super.onCreate()
        PreferenceManager.setDefaultValues(this, R.xml.preferences, false)
        SyncWorker.schedulePeriodic(this)
    }
}
