use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, RgbImage};

use crate::cache::part_path;
use crate::error::Result;

const JPEG_QUALITY: u8 = 90;
const BACKGROUND_QUALITY: u8 = 80;
// Kept tiny on purpose: the TV upscales it, which softens it further.
const BACKGROUND_WIDTH: u32 = 320;
const BACKGROUND_BLUR_SIGMA: f32 = 10.0;
const BACKGROUND_DARKEN: i32 = 40;

/// Decodes an image and applies its EXIF rotation.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);

    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Scales to fit `max_w`x`max_h` (never upscales) and writes a JPEG atomically.
pub fn write_display_jpeg(img: &DynamicImage, max_w: u32, max_h: u32, out: &Path) -> Result<()> {
    if img.width() > max_w || img.height() > max_h {
        write_jpeg(&img.resize(max_w, max_h, FilterType::CatmullRom).into_rgb8(), out, JPEG_QUALITY)
    } else {
        write_jpeg(&img.to_rgb8(), out, JPEG_QUALITY)
    }
}

/// Writes a small, blurred, darkened screen-shaped crop to fill the bars around photos that don't fit.
pub fn write_background(img: &DynamicImage, screen_w: u32, screen_h: u32, out: &Path) -> Result<()> {
    let height = (u64::from(BACKGROUND_WIDTH) * u64::from(screen_h) / u64::from(screen_w.max(1))).max(1);
    let height = u32::try_from(height).unwrap_or(BACKGROUND_WIDTH);
    let bg = img
        .resize_to_fill(BACKGROUND_WIDTH, height, FilterType::Triangle)
        .blur(BACKGROUND_BLUR_SIGMA)
        .brighten(-BACKGROUND_DARKEN);
    write_jpeg(&bg.into_rgb8(), out, BACKGROUND_QUALITY)
}

fn write_jpeg(rgb: &RgbImage, out: &Path, quality: u8) -> Result<()> {
    let tmp = part_path(out);
    let result = (|| -> Result<()> {
        let mut w = BufWriter::new(File::create(&tmp)?);
        JpegEncoder::new_with_quality(&mut w, quality).encode_image(rgb)?;
        w.flush()?;
        w.get_ref().sync_all()?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(fs::rename(&tmp, out)?),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        RgbImage::new(w, h).write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        buf.into_inner()
    }

    #[test]
    fn downscales_to_fit() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("a.jpg");
        write_display_jpeg(&decode(&jpeg(2048, 1536)).unwrap(), 1920, 1080, &out).unwrap();
        assert_eq!(image::image_dimensions(&out).unwrap(), (1440, 1080));

        write_display_jpeg(&decode(&jpeg(1536, 2048)).unwrap(), 1920, 1080, &out).unwrap();
        assert_eq!(image::image_dimensions(&out).unwrap(), (810, 1080));
    }

    #[test]
    fn keeps_small_images_and_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("b.jpg");
        write_display_jpeg(&decode(&jpeg(640, 480)).unwrap(), 1920, 1080, &out).unwrap();
        assert_eq!(image::image_dimensions(&out).unwrap(), (640, 480));

        assert!(decode(b"not an image").is_err());
    }

    #[test]
    fn background_matches_screen_shape() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("bg.jpg");
        write_background(&decode(&jpeg(1536, 2048)).unwrap(), 1920, 1080, &out).unwrap();
        assert_eq!(image::image_dimensions(&out).unwrap(), (320, 180));
        assert!(!part_path(&out).exists());
    }
}
