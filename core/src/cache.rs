use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::{is_safe_id, MediaKind, RemoteAsset};

const MANIFEST_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const MEDIA_DIR: &str = "media";
const ALBUMS_DIR: &str = "albums";

static LAYOUT_LOCK: Mutex<()> = Mutex::new(());

/// Cache directory for one album, derived from its key (e.g. `sc:003UOr...` -> `sc_003UOr...`).
pub fn album_dir(root: &Path, key: &str) -> Option<PathBuf> {
    let name = key.replace(':', "_");
    is_safe_id(&name).then(|| root.join(ALBUMS_DIR).join(name))
}

/// Moves the old single-album layout (`<root>/manifest.json` + `<root>/media`) under `<root>/albums/`.
pub fn migrate_single_album_layout(root: &Path) -> Result<()> {
    let _lock = LAYOUT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let manifest_path = root.join(MANIFEST_FILE);
    if !manifest_path.exists() {
        return Ok(());
    }
    let media = root.join(MEDIA_DIR);
    let key = fs::read(&manifest_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
        .map(|m| m.album_token);
    if let Some(target) = key.and_then(|k| album_dir(root, &k)) {
        if !target.exists() {
            fs::create_dir_all(&target)?;
            if media.is_dir() {
                fs::rename(&media, target.join(MEDIA_DIR))?;
            }
            fs::rename(&manifest_path, target.join(MANIFEST_FILE))?;
        }
    }
    let _ = fs::remove_dir_all(&media);
    let _ = fs::remove_file(&manifest_path);
    Ok(())
}

/// Deletes cached albums that are no longer configured.
pub fn remove_other_albums(root: &Path, keep: &[&Path]) -> Result<()> {
    let _lock = LAYOUT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let dir = root.join(ALBUMS_DIR);
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() && !keep.iter().any(|k| *k == path) {
            let _ = fs::remove_dir_all(&path);
        }
    }
    Ok(())
}

pub fn part_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub album_token: String,
    pub album_name: String,
    pub items: Vec<CachedItem>,
}

impl Manifest {
    pub fn new(token: &str) -> Self {
        Self {
            version: MANIFEST_VERSION,
            album_token: token.to_string(),
            ..Default::default()
        }
    }

    pub fn sort_newest_first(&mut self) {
        self.items.sort_by(|a, b| b.taken_at.cmp(&a.taken_at));
    }

    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedItem {
    pub guid: String,
    pub kind: MediaKind,
    pub taken_at: String,
    pub image_checksum: Option<String>,
    pub video_checksum: Option<String>,
    pub image_file: Option<String>,
    pub video_file: Option<String>,
    /// Optional; missing backgrounds are regenerated from the cached image.
    #[serde(default)]
    pub background_file: Option<String>,
    pub bytes: u64,
}

impl CachedItem {
    pub fn new(asset: &RemoteAsset) -> Self {
        Self {
            guid: asset.guid.clone(),
            kind: asset.kind,
            taken_at: asset.taken_at.clone(),
            image_checksum: None,
            video_checksum: None,
            image_file: None,
            video_file: None,
            background_file: None,
            bytes: 0,
        }
    }

    /// True when the cached copy is exactly what the album currently offers.
    pub fn matches(&self, asset: &RemoteAsset) -> bool {
        self.kind == asset.kind
            && self.image_checksum.as_deref() == asset.image.as_ref().map(|d| d.checksum.as_str())
            && self.video_checksum.as_deref() == asset.video.as_ref().map(|d| d.checksum.as_str())
    }

    fn content_files(&self) -> impl Iterator<Item = &str> {
        self.image_file.iter().chain(self.video_file.iter()).map(String::as_str)
    }

    fn files(&self) -> impl Iterator<Item = &str> {
        self.content_files().chain(self.background_file.iter().map(String::as_str))
    }
}

pub struct Store {
    root: PathBuf,
    media: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        let media = root.join(MEDIA_DIR);
        fs::create_dir_all(&media)?;
        Ok(Self {
            root: root.to_path_buf(),
            media,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A missing or unreadable manifest is treated as an empty cache.
    pub fn load_manifest(&self) -> Manifest {
        fs::read(self.root.join(MANIFEST_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
            .filter(|m| m.version == MANIFEST_VERSION)
            .unwrap_or_default()
    }

    pub fn save_manifest(&self, m: &Manifest) -> Result<()> {
        let path = self.root.join(MANIFEST_FILE);
        let tmp = part_path(&path);
        {
            let mut w = BufWriter::new(File::create(&tmp)?);
            serde_json::to_writer(&mut w, m)?;
            w.flush()?;
            w.get_ref().sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Returns None for names that could escape the media directory.
    pub fn media_path(&self, file: &str) -> Option<PathBuf> {
        let (stem, ext) = file.rsplit_once('.')?;
        (is_safe_id(stem) && matches!(ext, "jpg" | "mp4")).then(|| self.media.join(file))
    }

    pub fn files_exist(&self, item: &CachedItem) -> bool {
        (item.image_file.is_some() || item.video_file.is_some())
            && item
                .content_files()
                .all(|f| self.media_path(f).is_some_and(|p| p.is_file()))
    }

    pub fn background_exists(&self, item: &CachedItem) -> bool {
        item.background_file
            .as_deref()
            .and_then(|f| self.media_path(f))
            .is_some_and(|p| p.is_file())
    }

    pub fn remove_item_files(&self, item: &CachedItem) {
        for f in item.files() {
            if let Some(p) = self.media_path(f) {
                let _ = fs::remove_file(p);
            }
        }
    }

    /// Deletes files not referenced by the manifest, including stale `.part` downloads.
    pub fn remove_orphans(&self, m: &Manifest) -> Result<()> {
        let keep: std::collections::HashSet<&str> = m.items.iter().flat_map(|i| i.files()).collect();
        for entry in fs::read_dir(&self.media)? {
            let entry = entry?;
            let name = entry.file_name();
            let referenced = name.to_str().is_some_and(|n| keep.contains(n));
            if !referenced && entry.file_type()?.is_file() {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        if self.media.exists() {
            fs::remove_dir_all(&self.media)?;
        }
        fs::create_dir_all(&self.media)?;
        let _ = fs::remove_file(self.root.join(MANIFEST_FILE));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(guid: &str, image: Option<&str>) -> CachedItem {
        CachedItem {
            guid: guid.into(),
            kind: MediaKind::Photo,
            taken_at: String::new(),
            image_checksum: None,
            video_checksum: None,
            image_file: image.map(str::to_owned),
            video_file: None,
            background_file: None,
            bytes: 1,
        }
    }

    #[test]
    fn manifest_roundtrip_and_orphan_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(store.load_manifest().items.is_empty());

        let mut m = Manifest::new("tok");
        m.items.push(item("a", Some("a.jpg")));
        store.save_manifest(&m).unwrap();
        fs::write(store.media_path("a.jpg").unwrap(), b"x").unwrap();
        fs::write(store.media_path("b.jpg").unwrap(), b"x").unwrap();
        fs::write(dir.path().join("media/c.mp4.part"), b"x").unwrap();

        let loaded = store.load_manifest();
        assert_eq!(loaded.album_token, "tok");
        assert!(store.files_exist(&loaded.items[0]));

        store.remove_orphans(&loaded).unwrap();
        let left: Vec<_> = fs::read_dir(dir.path().join("media")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec!["a.jpg"]);
    }

    #[test]
    fn media_path_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(store.media_path("../x.jpg").is_none());
        assert!(store.media_path("a/b.jpg").is_none());
        assert!(store.media_path("a.exe").is_none());
        assert!(!store.files_exist(&item("a", Some("../../etc/passwd.jpg"))));
    }

    #[test]
    fn corrupt_manifest_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(dir.path().join(MANIFEST_FILE), b"{not json").unwrap();
        assert!(store.load_manifest().items.is_empty());
    }

    #[test]
    fn migrates_single_album_layout() {
        let dir = tempfile::tempdir().unwrap();
        let old = Store::open(dir.path()).unwrap();
        let mut m = Manifest::new("sc:0ExampleSharedAlbum123");
        m.items.push(item("a", Some("a.jpg")));
        old.save_manifest(&m).unwrap();
        fs::write(old.media_path("a.jpg").unwrap(), b"x").unwrap();

        migrate_single_album_layout(dir.path()).unwrap();

        let target = album_dir(dir.path(), "sc:0ExampleSharedAlbum123").unwrap();
        assert!(target.ends_with("albums/sc_0ExampleSharedAlbum123"));
        let moved = Store::open(&target).unwrap();
        let manifest = moved.load_manifest();
        assert_eq!(manifest.items.len(), 1);
        assert!(moved.files_exist(&manifest.items[0]));
        assert!(!dir.path().join(MANIFEST_FILE).exists());
        assert!(!dir.path().join(MEDIA_DIR).exists());
    }

    #[test]
    fn removes_unconfigured_albums() {
        let dir = tempfile::tempdir().unwrap();
        let keep = album_dir(dir.path(), "sc:keep000000").unwrap();
        let other = album_dir(dir.path(), "sc:drop000000").unwrap();
        Store::open(&keep).unwrap();
        Store::open(&other).unwrap();
        remove_other_albums(dir.path(), &[keep.as_path()]).unwrap();
        assert!(keep.exists());
        assert!(!other.exists());
    }
}
