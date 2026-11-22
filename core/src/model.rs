use serde::{Deserialize, Serialize};

use crate::album::{RawDerivative, RawPhoto};

// 720p keeps the offline cache small; a 1080p projector upscales it well.
const MAX_VIDEO_HEIGHT: u32 = 720;
const UNKNOWN_IMAGE_BYTES: u64 = 1024 * 1024;
const UNKNOWN_VIDEO_BYTES: u64 = 30 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum MediaKind {
    Photo,
    Video,
    LivePhoto,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Derivative {
    pub checksum: String,
    pub width: u32,
    pub height: u32,
    pub file_size: Option<u64>,
    /// Direct download URL when the source provides one up front.
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RemoteAsset {
    pub guid: String,
    pub kind: MediaKind,
    pub taken_at: String,
    pub image: Option<Derivative>,
    pub video: Option<Derivative>,
}

impl RemoteAsset {
    pub fn estimated_bytes(&self) -> u64 {
        let image = self
            .image
            .as_ref()
            .map_or(0, |d| d.file_size.unwrap_or(UNKNOWN_IMAGE_BYTES));
        let video = self
            .video
            .as_ref()
            .map_or(0, |d| d.file_size.unwrap_or(UNKNOWN_VIDEO_BYTES));
        image + video
    }

    pub fn has_direct_urls(&self) -> bool {
        self.image.iter().chain(self.video.iter()).all(|d| d.url.is_some())
    }
}

/// GUIDs become file names, so only allow a conservative character set.
pub fn is_safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Smallest photo that still fills the screen on at least one axis, else the largest.
pub fn pick_photo(mut candidates: Vec<Derivative>, screen_w: u32, screen_h: u32) -> Option<Derivative> {
    candidates.retain(|d| d.width > 0 && d.height > 0);
    candidates.sort_by_key(|d| u64::from(d.width) * u64::from(d.height));
    match candidates
        .iter()
        .position(|d| d.width >= screen_w || d.height >= screen_h)
    {
        Some(i) => Some(candidates.swap_remove(i)),
        None => candidates.pop(),
    }
}

/// Highest resolution video up to the cap, else the smallest available.
pub fn pick_video(mut candidates: Vec<Derivative>) -> Option<Derivative> {
    candidates.sort_by_key(|d| d.height);
    match candidates.iter().rposition(|d| d.height <= MAX_VIDEO_HEIGHT) {
        Some(i) => Some(candidates.swap_remove(i)),
        None => candidates.into_iter().next(),
    }
}

pub struct Renditions {
    pub photos: Vec<Derivative>,
    pub poster: Option<Derivative>,
    /// Video for video items, or the motion part of a Live Photo.
    pub motion: Vec<Derivative>,
}

pub fn assemble(
    guid: String,
    taken_at: String,
    is_video: bool,
    r: Renditions,
    screen_w: u32,
    screen_h: u32,
    include_videos: bool,
) -> Option<RemoteAsset> {
    if !is_safe_id(&guid) || (is_video && !include_videos) {
        return None;
    }
    let (kind, image, video) = if is_video {
        let image = r.poster.or_else(|| pick_photo(r.photos, screen_w, screen_h));
        match pick_video(r.motion) {
            Some(v) => (MediaKind::Video, image, Some(v)),
            None => (MediaKind::Photo, Some(image?), None),
        }
    } else {
        let image = pick_photo(r.photos, screen_w, screen_h)?;
        match pick_video(r.motion).filter(|_| include_videos) {
            Some(v) => (MediaKind::LivePhoto, Some(image), Some(v)),
            None => (MediaKind::Photo, Some(image), None),
        }
    };
    Some(RemoteAsset {
        guid,
        kind,
        taken_at,
        image,
        video,
    })
}

fn video_height(key: &str) -> Option<u32> {
    key.strip_suffix('p')?.parse().ok()
}

fn is_poster(key: &str) -> bool {
    key.eq_ignore_ascii_case("PosterFrame")
}

fn to_derivative(raw: &RawDerivative, fallback_height: u32) -> Option<Derivative> {
    let checksum = raw.checksum.as_deref().filter(|c| !c.is_empty())?;
    Some(Derivative {
        checksum: checksum.to_string(),
        width: raw.width.unwrap_or(0),
        height: raw.height.unwrap_or(fallback_height),
        file_size: raw.file_size,
        url: None,
    })
}

/// Classifies an item from the legacy shared-streams API.
pub fn classify(
    raw: &RawPhoto,
    screen_w: u32,
    screen_h: u32,
    include_videos: bool,
) -> Option<RemoteAsset> {
    let taken_at = raw
        .date_created
        .clone()
        .or_else(|| raw.batch_date_created.clone())
        .unwrap_or_default();
    let is_video = raw
        .media_asset_type
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("video"));

    let photos = raw
        .derivatives
        .iter()
        .filter(|(k, _)| !is_poster(k) && video_height(k).is_none())
        .filter_map(|(_, d)| to_derivative(d, 0))
        .collect();
    let poster = raw
        .derivatives
        .iter()
        .find(|(k, _)| is_poster(k))
        .and_then(|(_, d)| to_derivative(d, 0));
    // A still with an attached video derivative is treated as a Live Photo.
    let motion = raw
        .derivatives
        .iter()
        .filter_map(|(k, d)| to_derivative(d, video_height(k)?))
        .collect();

    assemble(
        raw.guid.clone(),
        taken_at,
        is_video,
        Renditions { photos, poster, motion },
        screen_w,
        screen_h,
        include_videos,
    )
}

/// Formats a Unix timestamp in milliseconds as ISO-8601 UTC, so dates sort as strings.
pub fn iso8601_from_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::album::WebStream;

    fn stream() -> WebStream {
        serde_json::from_str(
            r#"{"photos": [
                {"photoGuid": "photo-1", "dateCreated": "2024-01-01T00:00:00Z", "derivatives": {
                    "342":  {"checksum": "s", "width": "342",  "height": "256"},
                    "1280": {"checksum": "m", "width": "1280", "height": "960"},
                    "2049": {"checksum": "l", "width": "2049", "height": "1536"}}},
                {"photoGuid": "video-1", "mediaAssetType": "video", "derivatives": {
                    "PosterFrame": {"checksum": "pf", "width": "1280", "height": "720"},
                    "360p": {"checksum": "v360"}, "720p": {"checksum": "v720"}, "2160p": {"checksum": "v4k"}}},
                {"photoGuid": "live-1", "derivatives": {
                    "2049": {"checksum": "lp", "width": "2049", "height": "1536"},
                    "720p": {"checksum": "lv"}}},
                {"photoGuid": "../evil", "derivatives": {"2049": {"checksum": "x", "width": "1", "height": "1"}}}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn picks_smallest_photo_that_covers_screen() {
        let s = stream();
        let a = classify(&s.photos[0], 1920, 1080, true).unwrap();
        assert_eq!(a.kind, MediaKind::Photo);
        assert_eq!(a.image.unwrap().checksum, "l");
        let small = classify(&s.photos[0], 1280, 720, true).unwrap();
        assert_eq!(small.image.unwrap().checksum, "m");
    }

    #[test]
    fn video_uses_poster_and_caps_resolution() {
        let s = stream();
        let a = classify(&s.photos[1], 1920, 1080, true).unwrap();
        assert_eq!(a.kind, MediaKind::Video);
        assert_eq!(a.image.unwrap().checksum, "pf");
        assert_eq!(a.video.unwrap().checksum, "v720");
        assert!(classify(&s.photos[1], 1920, 1080, false).is_none());
    }

    #[test]
    fn live_photo_degrades_to_photo_without_videos() {
        let s = stream();
        assert_eq!(classify(&s.photos[2], 1920, 1080, true).unwrap().kind, MediaKind::LivePhoto);
        let still = classify(&s.photos[2], 1920, 1080, false).unwrap();
        assert_eq!(still.kind, MediaKind::Photo);
        assert!(still.video.is_none());
    }

    #[test]
    fn rejects_unsafe_guid() {
        assert!(classify(&stream().photos[3], 1920, 1080, true).is_none());
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(iso8601_from_ms(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_from_ms(1_700_000_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso8601_from_ms(951_782_400_000), "2000-02-29T00:00:00Z");
    }
}
