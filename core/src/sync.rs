use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::{Mutex, PoisonError};

use crate::cache::{remove_other_albums, CachedItem, Manifest, Store};
use crate::error::{FrameError, Result};
use crate::http::{download_bytes, download_file};
use crate::imaging::{decode, write_background, write_display_jpeg};
use crate::model::{Derivative, MediaKind, RemoteAsset};
use crate::source::Source;
use crate::{Album, FrameConfig, PhotoCore, SyncProgress, SyncReport, SyncStage};

const ASSET_URL_BATCH: usize = 25;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_VIDEO_BYTES: u64 = 1024 * 1024 * 1024;

// The screensaver and the background worker may sync concurrently in one process.
static SYNC_LOCK: Mutex<()> = Mutex::new(());
static PROGRESS: Mutex<SyncProgress> = Mutex::new(SyncProgress::IDLE);

fn update_progress(f: impl FnOnce(&mut SyncProgress)) {
    f(&mut PROGRESS.lock().unwrap_or_else(PoisonError::into_inner));
}

pub(crate) fn current_progress() -> SyncProgress {
    PROGRESS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn begin_file(kind: MediaKind, size: Option<u64>) {
    update_progress(|p| {
        p.current_kind = Some(kind);
        p.current_bytes = 0;
        p.current_size = size.unwrap_or(0);
    });
}

fn track_bytes(n: u64) {
    update_progress(|p| {
        p.current_bytes += n;
        p.bytes_downloaded += n;
    });
}

struct ProgressGuard;

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        update_progress(|p| {
            p.running = false;
            p.stage = SyncStage::Idle;
            p.current_kind = None;
        });
    }
}

pub(crate) fn run(core: &PhotoCore) -> Result<SyncReport> {
    let _guard = SYNC_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let album_count = count(core.albums.len());
    update_progress(|p| {
        *p = SyncProgress { running: true, stage: SyncStage::Connecting, album_count, ..SyncProgress::IDLE }
    });
    let _progress = ProgressGuard;

    let total_budget = u64::from(core.config.max_cache_mb) * 1024 * 1024;
    let budget = total_budget / u64::from(album_count.max(1));
    let mut report = SyncReport::default();
    let mut names = Vec::new();
    let mut first_album_error = None;

    for (i, album) in core.albums.iter().enumerate() {
        update_progress(|p| {
            p.stage = SyncStage::Connecting;
            p.album_index = count(i + 1);
            p.album_name.clear();
            p.done = 0;
            p.total = 0;
            p.current_kind = None;
        });
        match sync_album(album, &core.config, budget) {
            Ok(r) => {
                if !r.album_name.is_empty() {
                    names.push(r.album_name);
                }
                report.total += r.total;
                report.added += r.added;
                report.removed += r.removed;
                report.skipped += r.skipped;
                report.failed += r.failed;
                if r.last_error.is_some() {
                    report.last_error = r.last_error;
                }
            }
            Err(e) => {
                report.failed_albums += 1;
                report.last_error = Some(format!("{}: {e}", album.url));
                first_album_error.get_or_insert(e);
            }
        }
    }

    let keep: Vec<&std::path::Path> = core.albums.iter().map(|a| a.store.root()).collect();
    remove_other_albums(std::path::Path::new(&core.config.cache_dir), &keep)?;

    if report.failed_albums == album_count {
        if let Some(e) = first_album_error {
            return Err(e);
        }
    }
    report.album_name = names.join(", ");
    Ok(report)
}

fn sync_album(album: &Album, cfg: &FrameConfig, budget: u64) -> Result<SyncReport> {
    let store = &album.store;
    let mut report = SyncReport::default();

    let album_key = album.source.key();
    let mut source = Source::open(&album.source)?;
    let listing = source.list(cfg.screen_width, cfg.screen_height, cfg.include_videos)?;

    let mut remote: Vec<RemoteAsset> = listing.assets;
    remote.sort_by(|a, b| b.taken_at.cmp(&a.taken_at));
    let mut seen = HashSet::new();
    remote.retain(|a| seen.insert(a.guid.clone()));

    let mut manifest = store.load_manifest();
    if manifest.album_token != album_key {
        store.clear()?;
        manifest = Manifest::new(&album_key);
    }
    manifest.album_name = listing.name;
    report.album_name = manifest.album_name.clone();
    update_progress(|p| p.album_name = manifest.album_name.clone());

    // Drop items that left the album, changed, or lost their files.
    let wanted: HashMap<&str, &RemoteAsset> = remote.iter().map(|a| (a.guid.as_str(), a)).collect();
    let before = manifest.items.len();
    manifest.items.retain(|item| {
        let keep = wanted.get(item.guid.as_str()).is_some_and(|a| item.matches(a)) && store.files_exist(item);
        if !keep {
            store.remove_item_files(item);
        }
        keep
    });
    report.removed = count(before - manifest.items.len());

    let missing_backgrounds = manifest
        .items
        .iter()
        .filter(|i| i.image_file.is_some() && !store.background_exists(i))
        .count();
    if missing_backgrounds > 0 {
        update_progress(|p| {
            p.stage = SyncStage::Preparing;
            p.total = count(missing_backgrounds);
        });
        for item in manifest.items.iter_mut() {
            if ensure_background(store, item, cfg) {
                update_progress(|p| p.done += 1);
            }
        }
        update_progress(|p| p.done = 0);
    }

    // Shrink to budget (e.g. after the user lowered the cache size), oldest first.
    manifest.sort_newest_first();
    let mut used = manifest.total_bytes();
    while used > budget {
        let Some(item) = manifest.items.pop() else { break };
        used = used.saturating_sub(item.bytes);
        store.remove_item_files(&item);
        report.removed += 1;
    }

    let cached: HashSet<String> = manifest.items.iter().map(|i| i.guid.clone()).collect();
    let mut pending: Vec<&RemoteAsset> = Vec::new();
    for asset in remote.iter().filter(|a| !cached.contains(&a.guid)) {
        let estimate = asset.estimated_bytes();
        if used + estimate > budget {
            report.skipped += 1;
            continue;
        }
        used += estimate;
        pending.push(asset);
    }
    update_progress(|p| {
        p.stage = SyncStage::Downloading;
        p.total = count(pending.len());
    });

    // Download URLs expire, so resolve them batch by batch right before use.
    let mut last_error: Option<FrameError> = None;
    for batch in pending.chunks(ASSET_URL_BATCH) {
        let guids: Vec<&str> = batch.iter().map(|a| a.guid.as_str()).collect();
        let urls = if batch.iter().all(|a| a.has_direct_urls()) {
            Ok(HashMap::new())
        } else {
            source.asset_urls(&guids)
        };
        let urls = match urls {
            Ok(u) => u,
            Err(e) => {
                report.failed += count(batch.len());
                update_progress(|p| p.done += count(batch.len()));
                last_error = Some(e);
                continue;
            }
        };
        for asset in batch {
            match download_asset(&source, store, asset, &urls, cfg) {
                Ok(item) => {
                    manifest.items.push(item);
                    report.added += 1;
                }
                Err(e) => {
                    report.failed += 1;
                    last_error = Some(e);
                }
            }
            update_progress(|p| p.done += 1);
        }
        // Persist progress so an interrupted sync keeps what it already fetched.
        manifest.sort_newest_first();
        store.save_manifest(&manifest)?;
    }

    update_progress(|p| {
        p.stage = SyncStage::Finishing;
        p.current_kind = None;
    });
    manifest.sort_newest_first();
    store.save_manifest(&manifest)?;
    store.remove_orphans(&manifest)?;

    report.total = count(manifest.items.len());
    if report.total == 0 && report.failed > 0 {
        if let Some(e) = last_error {
            return Err(e);
        }
    }
    report.last_error = last_error.map(|e| e.to_string());
    Ok(report)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn background_name(guid: &str) -> String {
    format!("{guid}_bg.jpg")
}

/// Creates a missing blurred background from the cached image; returns true if one was made.
fn ensure_background(store: &Store, item: &mut CachedItem, cfg: &FrameConfig) -> bool {
    if store.background_exists(item) {
        return false;
    }
    let Some(src) = item.image_file.as_deref().and_then(|f| store.media_path(f)) else {
        return false;
    };
    let name = background_name(&item.guid);
    let Some(dest) = store.media_path(&name) else { return false };
    let made = fs::read(&src)
        .map_err(FrameError::from)
        .and_then(|bytes| decode(&bytes))
        .and_then(|img| write_background(&img, cfg.screen_width, cfg.screen_height, &dest));
    if made.is_err() {
        return false;
    }
    item.bytes += fs::metadata(&dest).map_or(0, |m| m.len());
    item.background_file = Some(name);
    true
}

fn url_for<'a>(d: &'a Derivative, urls: &'a HashMap<String, String>, guid: &str) -> Result<&'a str> {
    d.url
        .as_deref()
        .or_else(|| urls.get(&d.checksum).map(String::as_str))
        .ok_or_else(|| FrameError::Protocol(format!("no download URL for {guid}")))
}

fn download_asset(
    source: &Source,
    store: &Store,
    asset: &RemoteAsset,
    urls: &HashMap<String, String>,
    cfg: &FrameConfig,
) -> Result<CachedItem> {
    let http = source.http();
    let path_for = |name: &str| {
        store
            .media_path(name)
            .ok_or_else(|| FrameError::Storage(format!("unsafe file name {name:?}")))
    };

    let mut item = CachedItem::new(asset);
    let result = (|| -> Result<()> {
        if let Some(d) = &asset.image {
            let name = format!("{}.jpg", asset.guid);
            let path = path_for(&name)?;
            begin_file(if asset.kind == MediaKind::Video { MediaKind::Photo } else { asset.kind }, d.file_size);
            let bytes = download_bytes(http, url_for(d, urls, &asset.guid)?, MAX_IMAGE_BYTES, track_bytes)?;
            let img = decode(&bytes)?;
            drop(bytes);
            let (w, h) = (cfg.screen_width, cfg.screen_height);
            write_display_jpeg(&img, w, h, &path)?;
            item.bytes += fs::metadata(&path)?.len();
            item.image_file = Some(name);
            item.image_checksum = Some(d.checksum.clone());

            let bg_name = background_name(&asset.guid);
            if let Some(bg_path) = store.media_path(&bg_name) {
                if write_background(&img, w, h, &bg_path).is_ok() {
                    item.bytes += fs::metadata(&bg_path).map_or(0, |m| m.len());
                    item.background_file = Some(bg_name);
                }
            }
        }
        if let Some(d) = &asset.video {
            let name = format!("{}.mp4", asset.guid);
            let path = path_for(&name)?;
            begin_file(MediaKind::Video, d.file_size);
            item.bytes +=
                download_file(http, url_for(d, urls, &asset.guid)?, &path, MAX_VIDEO_BYTES, track_bytes)?;
            item.video_file = Some(name);
            item.video_checksum = Some(d.checksum.clone());
        }
        Ok(())
    })();

    match result {
        Ok(()) => Ok(item),
        Err(e) => {
            store.remove_item_files(&item);
            Err(e)
        }
    }
}
