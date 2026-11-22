package com.dhrovak.airproject

import android.content.Intent
import android.content.SharedPreferences
import android.os.Bundle
import android.text.InputType
import android.widget.Toast
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.lifecycleScope
import androidx.preference.EditTextPreference
import androidx.preference.Preference
import androidx.preference.PreferenceCategory
import androidx.preference.PreferenceFragmentCompat
import androidx.preference.PreferenceManager
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.frame_core.AlbumSummary
import uniffi.frame_core.FrameException
import uniffi.frame_core.parseAlbumToken

class SettingsActivity : AppCompatActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            supportFragmentManager.beginTransaction()
                .replace(android.R.id.content, SettingsFragment())
                .commit()
        }
    }
}

class SettingsFragment : PreferenceFragmentCompat(), SharedPreferences.OnSharedPreferenceChangeListener {

    private val albumPrefs = mutableListOf<Preference>()

    override fun onCreatePreferences(savedInstanceState: Bundle?, rootKey: String?) {
        setPreferencesFromResource(R.xml.preferences, rootKey)

        findPreference<EditTextPreference>(KEY_ADD_ALBUM_URL)?.apply {
            setOnBindEditTextListener {
                it.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
                it.isSingleLine = true
                it.setText("")
            }
            // Never persisted: the link is moved into the album list instead.
            setOnPreferenceChangeListener { _, newValue ->
                addTypedAlbum(newValue as? String)
                false
            }
        }
        onClick(KEY_SETUP_WITH_PHONE) {
            startActivity(Intent(requireContext(), SetupActivity::class.java))
        }
        onClick(KEY_SYNC_NOW) {
            SyncWorker.syncNow(requireContext())
            Toast.makeText(requireContext(), R.string.sync_started, Toast.LENGTH_SHORT).show()
        }
        onClick(KEY_SET_SCREENSAVER) { Screensaver.showSetupDialog(requireContext()) }
        onClick(KEY_BOOT_PERMISSION) { StartOnBoot.requestPermission(requireContext()) }
        onClick(KEY_START_SLIDESHOW) {
            startActivity(Intent(requireContext(), SlideshowActivity::class.java))
        }
    }

    override fun onResume() {
        super.onResume()
        PreferenceManager.getDefaultSharedPreferences(requireContext())
            .registerOnSharedPreferenceChangeListener(this)
        refreshSyncStatus()
        refreshScreensaverStatus()
        refreshStartOnBoot()
        refreshAlbums()
        refreshStorage()
    }

    override fun onPause() {
        PreferenceManager.getDefaultSharedPreferences(requireContext())
            .unregisterOnSharedPreferenceChangeListener(this)
        super.onPause()
    }

    override fun onSharedPreferenceChanged(prefs: SharedPreferences?, key: String?) {
        when (key) {
            Prefs.KEY_ALBUM_URLS -> {
                SyncWorker.syncNow(requireContext())
                refreshAlbums()
            }
            Prefs.KEY_INCLUDE_VIDEOS -> SyncWorker.syncNow(requireContext())
            Prefs.KEY_MAX_CACHE_MB -> {
                SyncWorker.syncNow(requireContext())
                refreshStorage()
            }
            Prefs.KEY_LAST_SYNC -> {
                refreshSyncStatus()
                refreshAlbums()
                refreshStorage()
            }
            Prefs.KEY_START_ON_BOOT -> {
                refreshStartOnBoot()
                val ctx = requireContext()
                if (Prefs.startOnBoot(ctx) && StartOnBoot.needsPermission(ctx)) StartOnBoot.requestPermission(ctx)
            }
        }
    }

    private fun onClick(key: String, action: () -> Unit) {
        findPreference<Preference>(key)?.setOnPreferenceClickListener {
            action()
            true
        }
    }

    private fun addTypedAlbum(value: String?) {
        val ctx = requireContext()
        val url = value?.trim().orEmpty()
        if (url.isEmpty()) return
        val message = try {
            parseAlbumToken(url)
            if (Prefs.addAlbumUrl(ctx, url)) R.string.album_added else R.string.album_duplicate
        } catch (e: FrameException) {
            Toast.makeText(ctx, UserError.describe(ctx, e), Toast.LENGTH_LONG).show()
            return
        }
        Toast.makeText(ctx, message, Toast.LENGTH_SHORT).show()
    }

    /** Rebuilds the album entries, using names from the cache once an album has synced. */
    private fun refreshAlbums() {
        val appCtx = requireContext().applicationContext
        val urls = Prefs.albumUrls(appCtx)
        viewLifecycleOwnerLiveData.value ?: return
        viewLifecycleOwner.lifecycleScope.launch {
            val summaries = withContext(Dispatchers.IO) {
                CoreFactory.create(appCtx)?.use { it.albumSummaries() }.orEmpty()
            }
            showAlbums(urls, summaries.associateBy { it.url })
        }
    }

    private fun showAlbums(urls: List<String>, summaries: Map<String, AlbumSummary>) {
        val category = findPreference<PreferenceCategory>(KEY_ALBUMS) ?: return
        albumPrefs.forEach { category.removePreference(it) }
        albumPrefs.clear()

        if (urls.isEmpty()) {
            albumPrefs += Preference(requireContext()).apply {
                setTitle(R.string.albums_empty)
                isSelectable = false
            }
        }
        urls.forEach { url ->
            val summary = summaries[url]
            val name = summary?.name?.takeIf { it.isNotBlank() }
            albumPrefs += Preference(requireContext()).apply {
                title = name ?: getString(R.string.album_not_synced)
                this.summary = if (summary != null && summary.itemCount > 0u) {
                    getString(R.string.album_item_count, summary.itemCount.toInt())
                } else {
                    url
                }
                setOnPreferenceClickListener {
                    confirmRemove(url, name ?: url)
                    true
                }
            }
        }
        albumPrefs.forEachIndexed { i, pref ->
            pref.isPersistent = false
            pref.isIconSpaceReserved = false
            pref.order = ALBUM_ORDER_START + i
            category.addPreference(pref)
        }
    }

    private fun confirmRemove(url: String, label: String) {
        AlertDialog.Builder(requireContext())
            .setTitle(getString(R.string.album_remove_title, label))
            .setMessage(R.string.album_remove_message)
            .setPositiveButton(R.string.album_remove) { _, _ -> Prefs.removeAlbumUrl(requireContext(), url) }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    private fun refreshSyncStatus() {
        findPreference<Preference>(KEY_SYNC_STATUS)?.summary =
            Prefs.lastSyncStatus(requireContext()) ?: getString(R.string.status_never)
    }

    private fun refreshScreensaverStatus() {
        findPreference<Preference>(KEY_SET_SCREENSAVER)?.summary = Screensaver.statusText(requireContext())
    }

    private fun refreshStartOnBoot() {
        val ctx = requireContext()
        findPreference<Preference>(KEY_BOOT_PERMISSION)?.isVisible =
            Prefs.startOnBoot(ctx) && StartOnBoot.needsPermission(ctx)
    }

    private fun refreshStorage() {
        val appCtx = requireContext().applicationContext
        viewLifecycleOwnerLiveData.value ?: return
        viewLifecycleOwner.lifecycleScope.launch {
            val text = withContext(Dispatchers.IO) { CacheStorage.statusText(appCtx) }
            findPreference<Preference>(Prefs.KEY_MAX_CACHE_MB)?.summary = text
        }
    }

    private companion object {
        const val KEY_ALBUMS = "albums"
        const val KEY_ADD_ALBUM_URL = "add_album_url"
        const val KEY_SETUP_WITH_PHONE = "setup_with_phone"
        const val KEY_SYNC_NOW = "sync_now"
        const val KEY_SYNC_STATUS = "sync_status"
        const val KEY_SET_SCREENSAVER = "set_screensaver"
        const val KEY_BOOT_PERMISSION = "boot_permission"
        const val KEY_START_SLIDESHOW = "start_slideshow"
        // Albums are listed above the static entries of their category.
        const val ALBUM_ORDER_START = -1000
    }
}
