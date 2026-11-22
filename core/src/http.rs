use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;

use crate::cache::part_path;
use crate::error::{FrameError, Result};

pub const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .user_agent(concat!("frame-core/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

pub fn is_valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

/// Accepts only `https://<plain-host>[:port]/...` URLs.
pub fn is_safe_https_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => authority,
    };
    is_valid_host(host)
}

pub fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        return Err(FrameError::Network(format!("response larger than {limit} bytes")));
    }
    Ok(buf)
}

/// Reports the number of bytes read on every chunk.
struct Progress<R, F> {
    inner: R,
    on_read: F,
}

impl<R: Read, F: FnMut(u64)> Read for Progress<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        (self.on_read)(n as u64);
        Ok(n)
    }
}

pub fn download_bytes(http: &Client, url: &str, limit: u64, on_read: impl FnMut(u64)) -> Result<Vec<u8>> {
    let resp = http.get(url).send()?.error_for_status()?;
    read_limited(Progress { inner: resp, on_read }, limit)
}

/// Streams to `<dest>.part` and renames on success so readers never see partial files.
pub fn download_file(
    http: &Client,
    url: &str,
    dest: &Path,
    limit: u64,
    on_read: impl FnMut(u64),
) -> Result<u64> {
    let resp = http.get(url).send()?.error_for_status()?;
    let mut reader = Progress { inner: resp, on_read }.take(limit + 1);
    let tmp = part_path(dest);
    let result = (|| -> Result<u64> {
        let mut out = BufWriter::new(File::create(&tmp)?);
        let written = std::io::copy(&mut reader, &mut out)?;
        if written > limit {
            return Err(FrameError::Network(format!("file larger than {limit} bytes")));
        }
        out.flush()?;
        out.get_ref().sync_all()?;
        Ok(written)
    })();
    match result {
        Ok(n) => {
            fs::rename(&tmp, dest)?;
            Ok(n)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_https_urls() {
        assert!(is_safe_https_url("https://cvws-h2.icloud-content.com/B/abc/file?o=x"));
        assert!(is_safe_https_url("https://p195-ckdatabasews.icloud.com:443"));
        assert!(!is_safe_https_url("http://example.com/"));
        assert!(!is_safe_https_url("https://user@evil.com/"));
        assert!(!is_safe_https_url("https:///path"));
        assert!(!is_safe_https_url("file:///etc/passwd"));
    }
}
