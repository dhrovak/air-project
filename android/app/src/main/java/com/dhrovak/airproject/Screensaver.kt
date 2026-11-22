package com.dhrovak.airproject

import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.provider.Settings
import android.widget.Toast
import androidx.appcompat.app.AlertDialog

object Screensaver {
    private const val SCREENSAVER_COMPONENTS = "screensaver_components"

    /** Null when the platform does not let apps read this setting. */
    fun isActive(ctx: Context): Boolean? {
        return try {
            val value = Settings.Secure.getString(ctx.contentResolver, SCREENSAVER_COMPONENTS)
                ?: return false
            val self = ComponentName(ctx, PhotoDreamService::class.java)
            value.split(',').any { ComponentName.unflattenFromString(it.trim()) == self }
        } catch (e: SecurityException) {
            null
        }
    }

    fun statusText(ctx: Context): String = when (isActive(ctx)) {
        true -> ctx.getString(R.string.screensaver_active)
        false -> ctx.getString(R.string.screensaver_inactive)
        null -> ctx.getString(R.string.screensaver_unknown)
    }

    fun showSetupDialog(ctx: Context) {
        val component = ComponentName(ctx, PhotoDreamService::class.java).flattenToShortString()
        AlertDialog.Builder(ctx)
            .setTitle(R.string.screensaver_dialog_title)
            .setMessage(ctx.getString(R.string.screensaver_instructions, component))
            .setPositiveButton(R.string.open_system_settings) { _, _ -> openSystemSettings(ctx) }
            .setNegativeButton(android.R.string.ok, null)
            .show()
    }

    private fun openSystemSettings(ctx: Context) {
        try {
            ctx.startActivity(Intent(Settings.ACTION_DREAM_SETTINGS))
        } catch (e: ActivityNotFoundException) {
            Toast.makeText(ctx, R.string.no_system_settings, Toast.LENGTH_LONG).show()
        }
    }
}
