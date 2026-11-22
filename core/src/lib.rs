//! Core of the iCloud Shared Album screensaver: sync, cache and playlist, exposed to Kotlin via UniFFI.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

mod album;
mod cache;
mod collection;
mod error;
mod http;
mod imaging;
mod model;
mod setup;
mod source;
mod sync;

pub use error::FrameError;
pub use model::MediaKind;

uniffi::setup_scaffolding!();

const MIN_CACHE_MB: u32 = 50;

#[derive(Debug, Clone, uniffi::Record)]
pub struct FrameConfig {
    pub cache_dir: String,
    pub album_urls: Vec<String>,
    /// Total for all albums; split evenly between them.
    pub max_cache_mb: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub include_videos: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MediaItem {
    pub guid: String,
    pub kind: MediaKind,
    /// Still image, or the poster frame for videos.
    pub image_path: Option<String>,
    pub video_path: Option<String>,
    /// Blurred, screen-shaped backdrop shown behind content that doesn't fill the screen.
    pub background_path: Option<String>,
    pub taken_at: String,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SyncReport {
    /// Names of all synced albums, comma separated.
    pub album_name: String,
    pub total: u32,
    pub added: u32,
    pub removed: u32,
    /// Not downloaded because the cache size limit was reached.
    pub skipped: u32,
    pub failed: u32,
    /// Albums that could not be synced at all.
    pub failed_albums: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AlbumSummary {
    pub url: String,
    pub key: String,
    /// Empty until the album has been synced once.
    pub name: String,
    pub item_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SyncStage {
    Idle,
    Connecting,
    /// Generating missing blurred backgrounds for already cached photos.
    Preparing,
    Downloading,
    Finishing,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncProgress {
    pub running: bool,
    pub stage: SyncStage,
    /// 1-based position of the album being synced, out of `album_count`.
    pub album_index: u32,
    pub album_count: u32,
    pub album_name: String,
    /// New items processed so far out of `total`.
    pub done: u32,
    pub total: u32,
    pub current_kind: Option<MediaKind>,
    pub current_bytes: u64,
    /// Expected size of the current file, 0 when unknown.
    pub current_size: u64,
    pub bytes_downloaded: u64,
}

impl SyncProgress {
    pub(crate) const IDLE: Self = Self {
        running: false,
        stage: SyncStage::Idle,
        album_index: 0,
        album_count: 0,
        album_name: String::new(),
        done: 0,
        total: 0,
        current_kind: None,
        current_bytes: 0,
        current_size: 0,
        bytes_downloaded: 0,
    };
}

/// Progress of the sync currently running in this process, if any.
#[uniffi::export]
pub fn sync_progress() -> SyncProgress {
    sync::current_progress()
}

pub(crate) struct Album {
    url: String,
    source: source::AlbumRef,
    store: cache::Store,
}

#[derive(uniffi::Object)]
pub struct PhotoCore {
    config: FrameConfig,
    albums: Vec<Album>,
}

#[uniffi::export]
impl PhotoCore {
    /// Invalid links are skipped; fails only if no valid album remains.
    #[uniffi::constructor]
    pub fn new(mut config: FrameConfig) -> Result<Arc<Self>, FrameError> {
        if config.cache_dir.trim().is_empty() {
            return Err(FrameError::Storage("cache directory not set".into()));
        }
        config.max_cache_mb = config.max_cache_mb.max(MIN_CACHE_MB);
        config.screen_width = config.screen_width.clamp(640, 3840);
        config.screen_height = config.screen_height.clamp(360, 2160);

        let root = PathBuf::from(&config.cache_dir);
        cache::migrate_single_album_layout(&root)?;

        let mut seen = HashSet::new();
        let mut albums = Vec::new();
        let mut first_error = None;
        for url in &config.album_urls {
            match source::AlbumRef::parse(url) {
                Ok(source) if seen.insert(source.key()) => {
                    let dir = cache::album_dir(&root, &source.key())
                        .ok_or_else(|| FrameError::Storage("unsupported album id".into()))?;
                    albums.push(Album {
                        url: url.trim().to_string(),
                        store: cache::Store::open(&dir)?,
                        source,
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        if albums.is_empty() {
            return Err(first_error
                .unwrap_or_else(|| FrameError::InvalidUrl("no album configured".into())));
        }
        Ok(Arc::new(Self { config, albums }))
    }

    /// Blocking; call from a background thread. One failing album doesn't stop the others.
    pub fn sync(&self) -> Result<SyncReport, FrameError> {
        sync::run(self)
    }

    /// Cached items from all albums ready to display, newest first unless shuffled. Works offline.
    pub fn playlist(&self, shuffle: bool) -> Vec<MediaItem> {
        let mut items: Vec<MediaItem> = self.albums.iter().flat_map(Album::items).collect();
        if shuffle {
            fastrand::shuffle(&mut items);
        } else {
            items.sort_by(|a, b| b.taken_at.cmp(&a.taken_at));
        }
        items
    }

    pub fn album_name(&self) -> String {
        self.album_summaries()
            .into_iter()
            .map(|s| s.name)
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn album_summaries(&self) -> Vec<AlbumSummary> {
        self.albums
            .iter()
            .map(|a| {
                let manifest = a.store.load_manifest();
                let current = manifest.album_token == a.source.key();
                AlbumSummary {
                    url: a.url.clone(),
                    key: a.source.key(),
                    name: if current { manifest.album_name } else { String::new() },
                    item_count: if current {
                        u32::try_from(manifest.items.len()).unwrap_or(u32::MAX)
                    } else {
                        0
                    },
                }
            })
            .collect()
    }

    pub fn clear_cache(&self) -> Result<(), FrameError> {
        self.albums.iter().try_for_each(|a| a.store.clear())
    }
}

impl Album {
    fn items(&self) -> Vec<MediaItem> {
        let manifest = self.store.load_manifest();
        if manifest.album_token != self.source.key() {
            return Vec::new();
        }
        let path = |f: &Option<String>| {
            f.as_deref()
                .and_then(|name| self.store.media_path(name))
                .map(|p| p.to_string_lossy().into_owned())
        };
        manifest
            .items
            .iter()
            .filter(|i| self.store.files_exist(i))
            .map(|i| MediaItem {
                guid: i.guid.clone(),
                kind: i.kind,
                image_path: path(&i.image_file),
                video_path: path(&i.video_file),
                background_path: self
                    .store
                    .background_exists(i)
                    .then(|| path(&i.background_file))
                    .flatten(),
                taken_at: i.taken_at.clone(),
            })
            .collect()
    }
}

/// Validates a shared album link and returns a stable key for it.
#[uniffi::export]
pub fn parse_album_token(url: String) -> Result<String, FrameError> {
    source::AlbumRef::parse(&url).map(|a| a.key())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: &std::path::Path, urls: &[&str]) -> FrameConfig {
        FrameConfig {
            cache_dir: dir.to_string_lossy().into_owned(),
            album_urls: urls.iter().map(|s| s.to_string()).collect(),
            max_cache_mb: 100,
            screen_width: 1920,
            screen_height: 1080,
            include_videos: true,
        }
    }

    #[test]
    fn dedupes_and_skips_invalid_albums() {
        let dir = tempfile::tempdir().unwrap();
        let urls = [
            "https://photos.icloud.com/shared/album/0ExampleSharedAlbum123",
            "0ExampleSharedAlbum123",
            "not a link",
            "https://www.icloud.com/sharedalbum/#B0aExampleAlbum1",
        ];
        let core = PhotoCore::new(config(dir.path(), &urls)).unwrap();
        let summaries = core.album_summaries();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].key, "sc:0ExampleSharedAlbum123");
        assert_eq!(summaries[1].key, "B0aExampleAlbum1");
        assert!(core.playlist(false).is_empty());
    }

    #[test]
    fn requires_a_valid_album() {
        let dir = tempfile::tempdir().unwrap();
        assert!(PhotoCore::new(config(dir.path(), &[])).is_err());
        assert!(PhotoCore::new(config(dir.path(), &["https://example.com/photos"])).is_err());
    }
}
