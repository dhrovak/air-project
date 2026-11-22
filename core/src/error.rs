#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum FrameError {
    #[error("invalid album link: {0}")]
    InvalidUrl(String),
    /// The link is well-formed but iCloud won't serve it publicly (deleted, unshared, needs sign-in).
    #[error("album not available: {0}")]
    AlbumUnavailable(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response from iCloud: {0}")]
    Protocol(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("image error: {0}")]
    Image(String),
}

pub type Result<T> = std::result::Result<T, FrameError>;

impl From<std::io::Error> for FrameError {
    fn from(e: std::io::Error) -> Self {
        FrameError::Storage(e.to_string())
    }
}

impl From<reqwest::Error> for FrameError {
    fn from(e: reqwest::Error) -> Self {
        FrameError::Network(e.to_string())
    }
}

impl From<image::ImageError> for FrameError {
    fn from(e: image::ImageError) -> Self {
        FrameError::Image(e.to_string())
    }
}

impl From<serde_json::Error> for FrameError {
    fn from(e: serde_json::Error) -> Self {
        FrameError::Protocol(e.to_string())
    }
}
