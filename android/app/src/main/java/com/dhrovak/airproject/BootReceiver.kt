package com.dhrovak.airproject

import android.content.ActivityNotFoundException
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.appcompat.app.AlertDialog

/** Refreshes photos after the TV starts and, if enabled, opens the slideshow. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        if (Prefs.albumUrls(ctx).isEmpty()) return
        SyncWorker.syncNow(ctx)
        if (!Prefs.startOnBoot(ctx)) return
        try {
            ctx.startActivity(
                Intent(ctx, SlideshowActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        } catch (e: RuntimeException) {
            Log.w(TAG, "Cannot open slideshow after boot", e)
        }
    }

    private companion object {
        const val TAG = "BootReceiver"
    }
}

object StartOnBoot {
    /** Android 10+ only lets apps open a screen from the background with "display over other apps". */
    fun needsPermission(ctx: Context): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && !Settings.canDrawOverlays(ctx)

    fun requestPermission(ctx: Context) {
        val intent = Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:${ctx.packageName}"))
        try {
            ctx.startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            // Many TV builds have no screen for this permission.
            AlertDialog.Builder(ctx)
                .setTitle(R.string.boot_permission_title)
                .setMessage(ctx.getString(R.string.boot_permission_instructions, ctx.packageName))
                .setPositiveButton(android.R.string.ok, null)
                .show()
        }
    }
}
