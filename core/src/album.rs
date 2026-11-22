//! Client for the legacy iCloud Shared Album "sharedstreams" web API
//! (links like `https://www.icloud.com/sharedalbum/#B0aG...`).

use std::collections::HashMap;

use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Deserializer};

use crate::error::{FrameError, Result};
use crate::http::{self, is_valid_host, read_limited, MAX_JSON_BYTES};
use crate::model::{classify, RemoteAsset};
use crate::source::Listing;

const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const MAX_REDIRECTS: usize = 3;

/// Extracts the album token from `...#<token>` or `...#<token>;<photo-guid>`.
pub fn parse_token(input: &str) -> Result<String> {
    let s = input.trim();
    let after_hash = match s.rfind('#') {
        Some(i) => &s[i + 1..],
        None => s,
    };
    let token = after_hash.split(';').next().unwrap_or("").trim();
    if token.len() < 3 || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(FrameError::InvalidUrl("malformed shared album token".into()));
    }
    partition(token)?;
    Ok(token.to_string())
}

fn base62(s: &str) -> Option<u32> {
    s.bytes().try_fold(0u32, |acc, b| {
        let v = BASE62.iter().position(|&c| c == b)? as u32;
        acc.checked_mul(62)?.checked_add(v)
    })
}

/// Server partition encoded in the token; only an initial guess, iCloud may redirect.
fn partition(token: &str) -> Result<u32> {
    let digits = if token.starts_with('A') {
        &token[1..2]
    } else {
        &token[1..3]
    };
    base62(digits).ok_or_else(|| FrameError::InvalidUrl("malformed album token".into()))
}

fn base_url(host: &str, token: &str) -> String {
    format!("https://{host}/{token}/sharedstreams/")
}

#[derive(Debug, Deserialize)]
pub struct WebStream {
    #[serde(rename = "streamName", default)]
    pub stream_name: Option<String>,
    #[serde(default, deserialize_with = "de_lenient_vec")]
    pub photos: Vec<RawPhoto>,
}

#[derive(Debug, Deserialize)]
pub struct RawPhoto {
    #[serde(rename = "photoGuid")]
    pub guid: String,
    #[serde(rename = "dateCreated", default)]
    pub date_created: Option<String>,
    #[serde(rename = "batchDateCreated", default)]
    pub batch_date_created: Option<String>,
    #[serde(rename = "mediaAssetType", default)]
    pub media_asset_type: Option<String>,
    #[serde(default)]
    pub derivatives: HashMap<String, RawDerivative>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawDerivative {
    #[serde(default)]
    pub checksum: Option<String>,
    #[serde(rename = "fileSize", default, deserialize_with = "de_opt_num")]
    pub file_size: Option<u64>,
    #[serde(default, deserialize_with = "de_opt_num")]
    pub width: Option<u32>,
    #[serde(default, deserialize_with = "de_opt_num")]
    pub height: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AssetUrls {
    #[serde(default)]
    items: HashMap<String, AssetLocation>,
}

#[derive(Debug, Deserialize)]
struct AssetLocation {
    url_location: String,
    url_path: String,
}

/// iCloud sends numbers as strings; accept both and treat garbage as missing.
fn de_opt_num<'de, D, T>(d: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr + TryFrom<u64>,
{
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Number(n)) => n.as_u64().and_then(|v| T::try_from(v).ok()),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

/// Skips individual malformed entries instead of failing the whole album.
pub(crate) fn de_lenient_vec<'de, D, T>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let values = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(values
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

pub struct AlbumClient {
    http: Client,
    token: String,
    base: String,
}

impl AlbumClient {
    pub fn new(token: &str) -> Result<Self> {
        let host = format!("p{:02}-sharedstreams.icloud.com", partition(token)?);
        Ok(Self {
            http: http::client()?,
            token: token.to_string(),
            base: base_url(&host, token),
        })
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    pub fn list(&mut self, screen_w: u32, screen_h: u32, include_videos: bool) -> Result<Listing> {
        let stream = self.fetch_stream()?;
        let assets: Vec<RemoteAsset> = stream
            .photos
            .iter()
            .filter_map(|p| classify(p, screen_w, screen_h, include_videos))
            .collect();
        Ok(Listing {
            name: stream.stream_name.unwrap_or_default(),
            assets,
        })
    }

    fn fetch_stream(&mut self) -> Result<WebStream> {
        for _ in 0..=MAX_REDIRECTS {
            let resp = self
                .http
                .post(format!("{}webstream", self.base))
                .header(CONTENT_TYPE, "application/json")
                .body(r#"{"streamCtag":null}"#)
                .send()?;
            let status = resp.status().as_u16();
            let header_host = resp
                .headers()
                .get("X-Apple-MMe-Host")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = read_limited(resp, MAX_JSON_BYTES)?;

            match status {
                200 => return Ok(serde_json::from_slice(&body)?),
                // Album lives on another partition.
                330 => {
                    let host = header_host
                        .or_else(|| {
                            serde_json::from_slice::<serde_json::Value>(&body)
                                .ok()?
                                .get("X-Apple-MMe-Host")?
                                .as_str()
                                .map(str::to_owned)
                        })
                        .ok_or_else(|| FrameError::Protocol("redirect without host".into()))?;
                    if !is_valid_host(&host) || !host.ends_with(".icloud.com") {
                        return Err(FrameError::Protocol(format!(
                            "unexpected redirect host {host:?}"
                        )));
                    }
                    self.base = base_url(&host, &self.token);
                }
                400..=499 => {
                    return Err(FrameError::AlbumUnavailable(format!(
                        "HTTP {status}; check the link and that \"Public Website\" is enabled"
                    )))
                }
                _ => return Err(FrameError::Network(format!("HTTP {status}"))),
            }
        }
        Err(FrameError::Protocol("too many redirects".into()))
    }

    /// Resolves download URLs for the given photo GUIDs, keyed by derivative checksum.
    pub fn asset_urls(&self, guids: &[&str]) -> Result<HashMap<String, String>> {
        let body = serde_json::json!({ "photoGuids": guids }).to_string();
        let resp = self
            .http
            .post(format!("{}webasseturls", self.base))
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()?
            .error_for_status()?;
        let parsed: AssetUrls = serde_json::from_slice(&read_limited(resp, MAX_JSON_BYTES)?)?;
        Ok(parsed
            .items
            .into_iter()
            .filter(|(_, loc)| is_valid_host(&loc.url_location) && loc.url_path.starts_with('/'))
            .map(|(checksum, loc)| {
                (checksum, format!("https://{}{}", loc.url_location, loc.url_path))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_link_shapes() {
        let t = "B0aExampleAlbum1";
        assert_eq!(parse_token(&format!("https://www.icloud.com/sharedalbum/#{t}")).unwrap(), t);
        assert_eq!(parse_token(&format!("  https://www.icloud.com/sharedalbum/en-us/#{t}\n")).unwrap(), t);
        assert_eq!(parse_token(&format!("https://www.icloud.com/sharedalbum/#{t};ABC-123")).unwrap(), t);
        assert_eq!(parse_token(t).unwrap(), t);
    }

    #[test]
    fn rejects_bad_links() {
        assert!(parse_token("").is_err());
        assert!(parse_token("https://www.icloud.com/sharedalbum/").is_err());
        assert!(parse_token("https://example.com/#../../etc").is_err());
        assert!(parse_token("#B0").is_err());
    }

    #[test]
    fn partition_from_token() {
        // 'B' tokens encode the partition in chars 1..3, 'A' tokens in char 1.
        assert_eq!(partition("B0aExampleAlbum1").unwrap(), 36);
        assert_eq!(partition("A7xyz").unwrap(), 7);
        assert_eq!(base_url("p36-sharedstreams.icloud.com", "B0a"), "https://p36-sharedstreams.icloud.com/B0a/sharedstreams/");
    }

    #[test]
    fn parses_stream_with_string_numbers_and_bad_entries() {
        let json = r#"{
            "streamName": "Family",
            "photos": [
                {"photoGuid": "G1", "dateCreated": "2024-05-01T10:00:00Z",
                 "derivatives": {"2049": {"checksum": "c1", "fileSize": "12345", "width": "2049", "height": "1536"}}},
                {"noGuid": true},
                {"photoGuid": "G2", "mediaAssetType": "video",
                 "derivatives": {"PosterFrame": {"checksum": "p2", "width": 1280, "height": 720},
                                 "720p": {"checksum": "v2", "fileSize": 999}}}
            ]
        }"#;
        let s: WebStream = serde_json::from_str(json).unwrap();
        assert_eq!(s.stream_name.as_deref(), Some("Family"));
        assert_eq!(s.photos.len(), 2);
        let d = &s.photos[0].derivatives["2049"];
        assert_eq!((d.width, d.height, d.file_size), (Some(2049), Some(1536), Some(12345)));
        assert_eq!(s.photos[1].media_asset_type.as_deref(), Some("video"));
    }
}
