use std::collections::HashMap;

use reqwest::blocking::Client;

use crate::album::{self, AlbumClient};
use crate::collection::{self, CollectionClient};
use crate::error::{FrameError, Result};
use crate::model::RemoteAsset;

const COLLECTION_PATH: &str = "/shared/album/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlbumRef {
    /// Legacy `https://www.icloud.com/sharedalbum/#<token>` link.
    SharedStream(String),
    /// CloudKit `https://photos.icloud.com/shared/album/<id>` link.
    SharedCollection(String),
}

impl AlbumRef {
    pub fn parse(input: &str) -> Result<Self> {
        let s = input.trim();
        let invalid = || {
            FrameError::InvalidUrl(
                "expected an iCloud shared album link (photos.icloud.com/shared/album/... or icloud.com/sharedalbum/#...)"
                    .into(),
            )
        };

        if let Some(i) = s.find(COLLECTION_PATH) {
            let id = s[i + COLLECTION_PATH.len()..]
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("");
            return collection::is_valid_id(id)
                .then(|| Self::SharedCollection(id.to_string()))
                .ok_or_else(invalid);
        }
        if s.contains('#') {
            return album::parse_token(s).map(Self::SharedStream).map_err(|_| invalid());
        }
        // A bare id: CloudKit short GUIDs start with a digit, legacy tokens with a letter.
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return if s.as_bytes()[0].is_ascii_digit() {
                collection::is_valid_id(s)
                    .then(|| Self::SharedCollection(s.to_string()))
                    .ok_or_else(invalid)
            } else {
                album::parse_token(s).map(Self::SharedStream).map_err(|_| invalid())
            };
        }
        Err(invalid())
    }

    /// Identifies the album in the cache manifest.
    pub fn key(&self) -> String {
        match self {
            Self::SharedStream(token) => token.clone(),
            Self::SharedCollection(id) => format!("sc:{id}"),
        }
    }
}

pub struct Listing {
    pub name: String,
    pub assets: Vec<RemoteAsset>,
}

pub enum Source {
    Stream(AlbumClient),
    Collection(CollectionClient),
}

impl Source {
    pub fn open(album: &AlbumRef) -> Result<Self> {
        Ok(match album {
            AlbumRef::SharedStream(token) => Self::Stream(AlbumClient::new(token)?),
            AlbumRef::SharedCollection(id) => Self::Collection(CollectionClient::new(id)?),
        })
    }

    pub fn http(&self) -> &Client {
        match self {
            Self::Stream(c) => c.http(),
            Self::Collection(c) => c.http(),
        }
    }

    pub fn list(&mut self, screen_w: u32, screen_h: u32, include_videos: bool) -> Result<Listing> {
        match self {
            Self::Stream(c) => c.list(screen_w, screen_h, include_videos),
            Self::Collection(c) => c.list(screen_w, screen_h, include_videos),
        }
    }

    /// Download URLs keyed by checksum, for sources that don't include them in the listing.
    pub fn asset_urls(&self, guids: &[&str]) -> Result<HashMap<String, String>> {
        match self {
            Self::Stream(c) => c.asset_urls(guids),
            Self::Collection(_) => Ok(HashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_link_styles() {
        assert_eq!(
            AlbumRef::parse("https://photos.icloud.com/shared/album/0ExampleSharedAlbum123").unwrap(),
            AlbumRef::SharedCollection("0ExampleSharedAlbum123".into())
        );
        assert_eq!(
            AlbumRef::parse(" https://photos.icloud.com/shared/album/0ExampleSharedAlbum123/?x=1 ").unwrap(),
            AlbumRef::SharedCollection("0ExampleSharedAlbum123".into())
        );
        assert_eq!(
            AlbumRef::parse("https://www.icloud.com/sharedalbum/#B0aExampleAlbum1").unwrap(),
            AlbumRef::SharedStream("B0aExampleAlbum1".into())
        );
        assert_eq!(
            AlbumRef::parse("0ExampleSharedAlbum123").unwrap().key(),
            "sc:0ExampleSharedAlbum123"
        );
    }

    #[test]
    fn rejects_other_links() {
        assert!(AlbumRef::parse("").is_err());
        assert!(AlbumRef::parse("https://photos.icloud.com/shared/album/").is_err());
        assert!(AlbumRef::parse("https://photos.icloud.com/shared/album/../../x").is_err());
        assert!(AlbumRef::parse("https://example.com/photos").is_err());
    }
}
