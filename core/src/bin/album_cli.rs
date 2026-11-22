//! Host-side tool to test syncing a shared album without the Android app.

use std::path::PathBuf;
use std::process::ExitCode;

use frame_core::{sync_progress, FrameConfig, PhotoCore};

const USAGE: &str = "usage: album-cli <shared-album-url>... [--out DIR] [--max-mb N] [--no-videos] [--size WxH]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut urls: Vec<String> = Vec::new();
    let mut out = PathBuf::from("album-cache");
    let mut max_mb: u32 = 1000;
    let mut include_videos = true;
    let (mut width, mut height) = (1920u32, 1080u32);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => match args.next() {
                Some(v) => out = PathBuf::from(v),
                None => return usage_error(),
            },
            "--max-mb" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) => max_mb = v,
                None => return usage_error(),
            },
            "--size" => match args.next().as_deref().and_then(parse_size) {
                Some((w, h)) => (width, height) = (w, h),
                None => return usage_error(),
            },
            "--no-videos" => include_videos = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if !arg.starts_with("--") => urls.push(arg),
            _ => return usage_error(),
        }
    }
    if urls.is_empty() {
        return usage_error();
    }

    let core = match PhotoCore::new(FrameConfig {
        cache_dir: out.to_string_lossy().into_owned(),
        album_urls: urls,
        max_cache_mb: max_mb,
        screen_width: width,
        screen_height: height,
        include_videos,
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ticker = {
        let done = std::sync::Arc::clone(&done);
        std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                let p = sync_progress();
                if p.running {
                    eprint!(
                        "\r\x1b[KAlbum {}/{} {:?} {:?}: {}/{} {:?} {}/{} KB (total {} MB)",
                        p.album_index, p.album_count, p.stage, p.album_name, p.done, p.total, p.current_kind,
                        p.current_bytes / 1024, p.current_size / 1024, p.bytes_downloaded / (1024 * 1024)
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            eprintln!();
        })
    };
    let result = core.sync();
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = ticker.join();

    match result {
        Ok(r) => {
            println!(
                "albums {:?}: {} cached, +{} -{}, {} skipped (cache full), {} failed, {} albums failed",
                r.album_name, r.total, r.added, r.removed, r.skipped, r.failed, r.failed_albums
            );
            if let Some(e) = r.last_error {
                eprintln!("last error: {e}");
            }
        }
        Err(e) => {
            eprintln!("sync failed: {e}");
            return ExitCode::FAILURE;
        }
    }

    for item in core.playlist(false) {
        println!(
            "{:<10} {:<22} {}",
            format!("{:?}", item.kind),
            item.taken_at,
            item.video_path.or(item.image_path).unwrap_or_default()
        );
    }
    ExitCode::SUCCESS
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
