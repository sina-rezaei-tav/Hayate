//! Bounded image probe. Decoding happens off the UI thread; the preview
//! pane only receives width and height.
//!
//! A full pixel buffer is intentionally not kept. A hostile image can
//! advertise a huge allocation, so dimensions past the cap are rejected
//! before the pixels are decoded.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Reject anything larger than this on either axis, even if the file is small.
pub const MAX_DIMENSION: u32 = 16_384;

/// Hard cap on decoded allocation, in bytes.
pub const MAX_ALLOC: u64 = 32 * 1024 * 1024;

pub fn is_image_name(name: &str) -> bool {
    matches!(
        extension(name).to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    )
}

fn extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => "",
        Some(index) => &name[index + 1..],
    }
}

pub fn dimensions_allowed(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && width <= MAX_DIMENSION && height <= MAX_DIMENSION
}

/// Reads the header and rejects images that would decode past [`MAX_ALLOC`]
/// or [`MAX_DIMENSION`]. Does not return pixels.
pub fn probe(path: &Path) -> Result<(u32, u32), String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let reader = BufReader::new(file);
    let reader = image::ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    let mut reader = reader;
    reader.limits(limits);
    let (width, height) = reader.into_dimensions().map_err(|err| err.to_string())?;
    if !dimensions_allowed(width, height) {
        return Err(format!("image is too large ({width}x{height})"));
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_image_extensions_are_probed() {
        assert!(is_image_name("photo.PNG"));
        assert!(is_image_name("a.jpeg"));
        assert!(!is_image_name("notes.txt"));
        assert!(!is_image_name("Makefile"));
        assert!(!is_image_name(".png"));
    }

    #[test]
    fn huge_dimensions_are_rejected_before_anyone_allocates_pixels() {
        assert!(!dimensions_allowed(0, 1));
        assert!(!dimensions_allowed(MAX_DIMENSION + 1, 1));
        assert!(dimensions_allowed(1, 1));
        assert!(dimensions_allowed(MAX_DIMENSION, 8));
    }

    #[test]
    fn a_real_png_reports_its_size_and_garbage_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("one.png");
        let image = image::RgbImage::from_pixel(2, 3, image::Rgb([1, 2, 3]));
        image.save(&png).unwrap();
        assert_eq!(probe(&png).unwrap(), (2, 3));

        let junk = dir.path().join("nope.png");
        std::fs::write(&junk, b"this is not a png").unwrap();
        assert!(probe(&junk).is_err());
    }
}
