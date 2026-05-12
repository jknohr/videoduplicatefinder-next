//! Thumbnail JPEG composition — horizontal strip from multiple frames.
//!
//! Faithful port of VDF.Core/Utils/JpegCompositor.cs.
//! Uses the `image` crate instead of SixLabors.ImageSharp.

use image::{DynamicImage, ImageBuffer, RgbaImage};
use std::io::Write;
use tracing::warn;

/// Maximum width for the displayable composite (Dioxus/WebGPU texture limit).
pub const MAX_DISPLAYABLE_COMPOSITE_WIDTH: u32 = 4096;

/// Hard upper limit to avoid GPU texture overflow.
pub const ABSOLUTE_MAX_WIDTH: u32 = 32767;

/// JPEG encode quality (0–100). Mirrors C# `JpegEncoder { Quality = 90 }`.
pub const JPEG_QUALITY: u8 = 90;

/// Concatenate `images` horizontally and write a JPEG to `out`.
/// Returns `false` (writes nothing) if `images` is empty.
///
/// Mirrors `JpegCompositor.TryWriteJoinedJpeg`.
pub fn try_write_joined_jpeg(images: &[DynamicImage], out: &mut dyn Write) -> bool {
    if images.is_empty() {
        return false;
    }
    let composite = build_composite(images);
    let encoded = match jpeg_encode(&composite) {
        Some(v) => v,
        None => return false,
    };
    out.write_all(&encoded).is_ok()
}

/// Encode a single `DynamicImage` as JPEG bytes.
pub fn jpeg_encode(image: &DynamicImage) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    match image.write_with_encoder(encoder) {
        Ok(_) => Some(buf),
        Err(e) => {
            warn!("jpeg_encode failed: {}", e);
            None
        }
    }
}

/// Build the in-memory composite image without encoding.
/// Applies `MAX_DISPLAYABLE_COMPOSITE_WIDTH` and `ABSOLUTE_MAX_WIDTH` limits.
///
/// Mirrors `JpegCompositor.BuildComposite`.
pub fn build_composite(images: &[DynamicImage]) -> DynamicImage {
    assert!(!images.is_empty(), "images must contain at least one element");

    let height = images[0].height();
    let total_width: u32 = images.iter().map(|img| img.width()).sum();

    let mut canvas: RgbaImage = ImageBuffer::new(total_width, height);

    let mut x_offset = 0u32;
    for src in images {
        let src_rgba = src.to_rgba8();
        let w = src_rgba.width();
        let h = src_rgba.height().min(height);
        for y in 0..h {
            for x in 0..w {
                let px = src_rgba.get_pixel(x, y);
                canvas.put_pixel(x_offset + x, y, *px);
            }
        }
        x_offset += w;
    }

    let mut result = DynamicImage::ImageRgba8(canvas);

    // Apply width limits via Lanczos3 resize.
    if result.width() > ABSOLUTE_MAX_WIDTH {
        let h = (result.height() as f64 * ABSOLUTE_MAX_WIDTH as f64 / result.width() as f64) as u32;
        result = result.resize_exact(ABSOLUTE_MAX_WIDTH, h, image::imageops::FilterType::Lanczos3);
    }
    if result.width() > MAX_DISPLAYABLE_COMPOSITE_WIDTH {
        let h = (result.height() as f64 * MAX_DISPLAYABLE_COMPOSITE_WIDTH as f64 / result.width() as f64) as u32;
        result = result.resize_exact(
            MAX_DISPLAYABLE_COMPOSITE_WIDTH,
            h,
            image::imageops::FilterType::Lanczos3,
        );
    }

    result
}

/// Decode a JPEG byte slice into a `DynamicImage`.
/// Returns `None` if decoding fails.
pub fn decode_jpeg(bytes: &[u8]) -> Option<DynamicImage> {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    fn make_image(w: u32, h: u32, r: u8) -> DynamicImage {
        let buf: RgbaImage = ImageBuffer::from_pixel(w, h, image::Rgba([r, 0, 0, 255]));
        DynamicImage::ImageRgba8(buf)
    }

    #[test]
    fn composite_width_is_sum() {
        let images = vec![make_image(100, 50, 255), make_image(200, 50, 128)];
        let composite = build_composite(&images);
        assert_eq!(composite.width(), 300);
        assert_eq!(composite.height(), 50);
    }

    #[test]
    fn empty_images_returns_false() {
        let mut buf = Vec::new();
        assert!(!try_write_joined_jpeg(&[], &mut buf));
    }

    #[test]
    fn single_image_roundtrip() {
        let img = make_image(32, 32, 200);
        let mut buf = Vec::new();
        assert!(try_write_joined_jpeg(&[img], &mut buf));
        assert!(!buf.is_empty());
        // Verify it decodes back as JPEG
        let decoded = decode_jpeg(&buf);
        assert!(decoded.is_some());
    }
}
