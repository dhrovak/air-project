//! Client for public iCloud shared albums served from CloudKit
//! (links like `https://photos.icloud.com/shared/album/003UOr...`).

use std::collections::HashMap;

use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, ORIGIN};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::album::de_lenient_vec;
use crate::error::{FrameError, Result};
use crate::http::{self, is_safe_https_url, read_limited, MAX_JSON_BYTES};
use crate::model::{assemble, iso8601_from_ms, Derivative, RemoteAsset, Renditions};
use crate::source::Listing;

const RESOLVE_HOST: &str = "https://ckdatabasews.icloud.com";
const DB_PATH: &str = "/database/1/com.apple.photos.cloud/production";
const WEB_ORIGIN: &str = "https://photos.icloud.com";
const QUERY_RECORD_TYPE: &str = "CPLAssetAndMasterByAssetDateWithoutHiddenOrDeleted";
const PAGE_SIZE: u32 = 200;
const MAX_PAGES: usize = 1000;

pub fn is_valid_id(id: &str) -> bool {
    (10..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[derive(Debug, Deserialize)]
struct ResolveResponse {
    #[serde(default)]
    results: Vec<ResolveResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveResult {
    #[serde(default)]
    server_error_code: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    require_apple_login: Option<bool>,
    #[serde(rename = "zoneID", default)]
    zone_id: Option<Value>,
    #[serde(default)]
    share: Option<Record>,
    #[serde(default)]
    anonymous_public_access: Option<AnonymousAccess>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnonymousAccess {
    token: String,
    database_partition: String,
}

#[derive(Debug, Deserialize)]
struct QueryResponse {
    #[serde(default, deserialize_with = "de_lenient_vec")]
    records: Vec<Record>,
    #[serde(rename = "continuationMarker", default)]
    continuation_marker: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    record_name: String,
    #[serde(default)]
    record_type: String,
    #[serde(default)]
    fields: HashMap<String, Field>,
    #[serde(default)]
    deleted: bool,
}

#[derive(Debug, Deserialize)]
struct Field {
    #[serde(default)]
    value: Value,
}

impl Record {
    fn value(&self, key: &str) -> Option<&Value> {
        self.fields.get(key).map(|f| &f.value)
    }

    fn int(&self, key: &str) -> Option<i64> {
        self.value(key)?.as_i64()
    }

    fn string(&self, key: &str) -> Option<&str> {
        self.value(key)?.as_str()
    }

    /// Downloadable renditions whose field names match `res<Name>Res` with an accepted `<Name>`.
    fn renditions(&self, accept: impl Fn(&str) -> bool, file_name: &str) -> Vec<Derivative> {
        self.fields
            .iter()
            .filter_map(|(key, field)| {
                let name = key.strip_prefix("res")?.strip_suffix("Res")?;
                if !accept(name) {
                    return None;
                }
                let v = &field.value;
                let url = v.get("downloadURL")?.as_str()?.replace("${f}", file_name);
                if !is_safe_https_url(&url) {
                    return None;
                }
                let dim = |suffix: &str| {
                    self.int(&format!("res{name}{suffix}"))
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0)
                };
                Some(Derivative {
                    checksum: v.get("fileChecksum")?.as_str()?.to_string(),
                    width: dim("Width"),
                    height: dim("Height"),
                    file_size: v
                        .get("size")
                        .and_then(Value::as_u64)
                        .or_else(|| self.int(&format!("res{name}FileSize")).and_then(|n| u64::try_from(n).ok())),
                    url: Some(url),
                })
            })
            .collect()
    }
}

fn is_jpeg(name: &str) -> bool {
    name.starts_with("JPEG")
}

fn is_video(name: &str) -> bool {
    name.starts_with("Vid") && !name.starts_with("Video") && !name.starts_with("VidCompl")
}

fn is_live_motion(name: &str) -> bool {
    name.starts_with("VidCompl") || name.starts_with("OriginalVidCompl")
}

/// Record names can contain base64 characters; map them to a file-name-safe form.
fn file_safe_id(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c {
            '+' => Some('-'),
            '/' => Some('_'),
            '=' => None,
            c => Some(c),
        })
        .collect()
}

fn to_assets(records: &[Record], screen_w: u32, screen_h: u32, include_videos: bool) -> Vec<RemoteAsset> {
    let masters: HashMap<&str, &Record> = records
        .iter()
        .filter(|r| r.record_type == "CPLMaster" && !r.deleted)
        .map(|r| (r.record_name.as_str(), r))
        .collect();

    records
        .iter()
        .filter(|r| r.record_type == "CPLAsset" && !r.deleted && r.int("isHidden") != Some(1))
        .filter_map(|asset| {
            let master_name = asset.value("masterRef")?.get("recordName")?.as_str()?;
            let master = *masters.get(master_name)?;

            // Edited renditions live on the asset record, originals on the master.
            let pick = |accept: fn(&str) -> bool, file: &str| {
                let edited = asset.renditions(accept, file);
                if edited.is_empty() { master.renditions(accept, file) } else { edited }
            };
            let photos = pick(is_jpeg, "photo.jpg");
            let videos = pick(is_video, "video.mp4");
            let item_type = master.string("itemType").unwrap_or_default();
            let is_video_item = item_type.contains("movie")
                || item_type.contains("video")
                || item_type == "public.mpeg-4"
                || (photos.is_empty() && !videos.is_empty());
            let motion = if is_video_item { videos } else { pick(is_live_motion, "live.mov") };

            let taken_at = asset
                .int("assetDate")
                .or_else(|| master.int("originalCreationDate"))
                .map(iso8601_from_ms)
                .unwrap_or_default();

            assemble(
                file_safe_id(&asset.record_name),
                taken_at,
                is_video_item,
                Renditions { photos, poster: None, motion },
                screen_w,
                screen_h,
                include_videos,
            )
        })
        .collect()
}

pub struct CollectionClient {
    http: Client,
    id: String,
}

impl CollectionClient {
    pub fn new(id: &str) -> Result<Self> {
        if !is_valid_id(id) {
            return Err(FrameError::InvalidUrl("malformed shared album id".into()));
        }
        Ok(Self {
            http: http::client()?,
            id: id.to_string(),
        })
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    pub fn list(&self, screen_w: u32, screen_h: u32, include_videos: bool) -> Result<Listing> {
        let resolved = self.resolve()?;
        let name = resolved
            .share
            .as_ref()
            .and_then(|s| s.string("cloudkit.title"))
            .unwrap_or_default()
            .to_string();
        let records = self.query_all(&resolved)?;
        Ok(Listing {
            name,
            assets: to_assets(&records, screen_w, screen_h, include_videos),
        })
    }

    fn post(&self, url: Url, body: &Value) -> Result<Value> {
        let resp = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "text/plain")
            .header(ORIGIN, WEB_ORIGIN)
            .body(body.to_string())
            .send()?;
        let status = resp.status();
        let bytes = read_limited(resp, MAX_JSON_BYTES)?;
        if status.is_client_error() {
            return Err(FrameError::AlbumUnavailable(format!(
                "HTTP {}; check the link is still shared publicly",
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Err(FrameError::Network(format!("HTTP {}", status.as_u16())));
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn resolve(&self) -> Result<ResolveResult> {
        let url = Url::parse_with_params(
            &format!("{RESOLVE_HOST}{DB_PATH}/public/records/resolve"),
            &[("remapEnums", "true"), ("sharing_url_key", self.id.as_str())],
        )
        .map_err(|e| FrameError::Protocol(e.to_string()))?;
        let body = json!({ "shortGUIDs": [{ "value": self.id }] });
        let response: ResolveResponse = serde_json::from_value(self.post(url, &body)?)?;
        let result = response
            .results
            .into_iter()
            .next()
            .ok_or_else(|| FrameError::Protocol("empty resolve response".into()))?;

        if let Some(code) = &result.server_error_code {
            let reason = result.reason.as_deref().unwrap_or("");
            return Err(FrameError::AlbumUnavailable(format!("{code} {reason}")));
        }
        if result.require_apple_login == Some(true) || result.anonymous_public_access.is_none() {
            return Err(FrameError::AlbumUnavailable(
                "album requires Apple sign-in; share it with a public link".into(),
            ));
        }
        Ok(result)
    }

    fn query_all(&self, resolved: &ResolveResult) -> Result<Vec<Record>> {
        let access = resolved
            .anonymous_public_access
            .as_ref()
            .ok_or_else(|| FrameError::Protocol("missing public access token".into()))?;
        let zone_id = resolved
            .zone_id
            .clone()
            .ok_or_else(|| FrameError::Protocol("missing zone id".into()))?;
        let partition = access.database_partition.trim_end_matches('/');
        if !is_safe_https_url(partition) {
            return Err(FrameError::Protocol(format!("unexpected partition {partition:?}")));
        }
        let url = Url::parse_with_params(
            &format!("{partition}{DB_PATH}/shared/records/query"),
            &[
                ("remapEnums", "true"),
                ("sharing_url_key", self.id.as_str()),
                ("publicAccessAuthToken", access.token.as_str()),
            ],
        )
        .map_err(|e| FrameError::Protocol(e.to_string()))?;

        let mut records = Vec::new();
        let mut marker: Option<Value> = None;
        for _ in 0..MAX_PAGES {
            let mut body = json!({
                "query": {
                    "recordType": QUERY_RECORD_TYPE,
                    "filterBy": [
                        {"fieldName": "direction", "comparator": "EQUALS",
                         "fieldValue": {"value": "ASCENDING", "type": "STRING"}},
                        {"fieldName": "startRank", "comparator": "EQUALS",
                         "fieldValue": {"value": 0, "type": "INT64"}}
                    ]
                },
                "zoneID": zone_id,
                "resultsLimit": PAGE_SIZE,
            });
            if let Some(m) = marker.take() {
                body["continuationMarker"] = m;
            }
            let page: QueryResponse = serde_json::from_value(self.post(url.clone(), &body)?)?;
            let empty = page.records.is_empty();
            records.extend(page.records);
            match page.continuation_marker {
                Some(m) if !m.is_null() && !empty => marker = Some(m),
                _ => return Ok(records),
            }
        }
        Err(FrameError::Protocol("album listing did not terminate".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MediaKind;

    fn res(checksum: &str) -> Value {
        json!({"value": {"fileChecksum": checksum, "size": 1000,
                         "downloadURL": format!("https://cvws.icloud-content.com/B/{checksum}/${{f}}?o=x")},
               "type": "ASSETID"})
    }

    fn records() -> Vec<Record> {
        let v = json!([
            {"recordName": "M1", "recordType": "CPLMaster", "fields": {
                "itemType": {"value": "public.heic"},
                "resJPEGThumbRes": res("t1"), "resJPEGThumbWidth": {"value": 480}, "resJPEGThumbHeight": {"value": 360},
                "resJPEGMedRes": res("m1"), "resJPEGMedWidth": {"value": 2048}, "resJPEGMedHeight": {"value": 1536},
                "resOriginalRes": res("o1"), "resOriginalWidth": {"value": 8064}, "resOriginalHeight": {"value": 6048}}},
            {"recordName": "A/1+x=", "recordType": "CPLAsset", "fields": {
                "masterRef": {"value": {"recordName": "M1"}}, "assetDate": {"value": 1_700_000_000_000_i64}}},
            {"recordName": "M2", "recordType": "CPLMaster", "fields": {
                "itemType": {"value": "com.apple.quicktime-movie"},
                "resJPEGMedRes": res("p2"), "resJPEGMedWidth": {"value": 1280}, "resJPEGMedHeight": {"value": 720},
                "resVidSmallRes": res("v360"), "resVidSmallHeight": {"value": 360},
                "resVidMedRes": res("v720"), "resVidMedHeight": {"value": 720},
                "resVidLargeRes": res("v1080"), "resVidLargeHeight": {"value": 1080},
                "resVideoMediaMetaDataRes": res("meta")}},
            {"recordName": "A2", "recordType": "CPLAsset", "fields": {"masterRef": {"value": {"recordName": "M2"}}}},
            {"recordName": "A3", "recordType": "CPLAsset", "fields": {"masterRef": {"value": {"recordName": "missing"}}}},
            {"recordName": "A4", "recordType": "CPLAsset", "fields": {
                "masterRef": {"value": {"recordName": "M1"}}, "isHidden": {"value": 1}}}
        ]);
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn builds_photo_and_video_assets() {
        let assets = to_assets(&records(), 1920, 1080, true);
        assert_eq!(assets.len(), 2);

        let photo = &assets[0];
        assert_eq!(photo.guid, "A_1-x");
        assert_eq!(photo.kind, MediaKind::Photo);
        assert_eq!(photo.taken_at, "2023-11-14T22:13:20Z");
        let image = photo.image.as_ref().unwrap();
        assert_eq!(image.checksum, "m1");
        assert_eq!(image.url.as_deref(), Some("https://cvws.icloud-content.com/B/m1/photo.jpg?o=x"));

        let video = &assets[1];
        assert_eq!(video.kind, MediaKind::Video);
        assert_eq!(video.image.as_ref().unwrap().checksum, "p2");
        assert_eq!(video.video.as_ref().unwrap().checksum, "v720");
        assert!(video.has_direct_urls());
    }

    #[test]
    fn skips_videos_when_disabled() {
        let assets = to_assets(&records(), 1920, 1080, false);
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].kind, MediaKind::Photo);
    }

    #[test]
    fn validates_ids() {
        assert!(is_valid_id("0ExampleSharedAlbum123"));
        assert!(!is_valid_id("short"));
        assert!(!is_valid_id("0ExampleSh/redAlbum123"));
    }
}
