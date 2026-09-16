//! Image formats that may be handed to a webview as image data.
//!
//! Keep this list to formats decoded as raster images. In particular, SVG is
//! intentionally excluded: it is an active document format with external
//! reference and engine-specific behavior, not just encoded pixels.

pub(crate) const MAX_RASTER_BYTES: usize = 5 * 1024 * 1024;
const MAX_RASTER_WIDTH: usize = 8_192;
const MAX_RASTER_HEIGHT: usize = 8_192;
const MAX_RASTER_PIXELS: usize = 16_000_000;

pub(crate) fn is_supported_raster_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/avif" | "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    )
}

pub(crate) fn validate_raster(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_RASTER_BYTES {
        return Err("Image exceeds the maximum allowed size".to_string());
    }
    let dimensions =
        imagesize::blob_size(bytes).map_err(|_| "Unable to verify image dimensions".to_string())?;
    validate_raster_dimensions(dimensions.width, dimensions.height)
}

fn validate_raster_dimensions(width: usize, height: usize) -> Result<(), String> {
    if width > MAX_RASTER_WIDTH
        || height > MAX_RASTER_HEIGHT
        || width.saturating_mul(height) > MAX_RASTER_PIXELS
    {
        return Err("Image dimensions exceed the maximum allowed size".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_only_the_supported_raster_formats() {
        for mime_type in [
            "image/avif",
            "image/gif",
            "image/jpeg",
            "image/png",
            "image/webp",
        ] {
            assert!(is_supported_raster_mime(mime_type), "{mime_type}");
        }

        for mime_type in [
            "image/svg+xml",
            "image/x-icon",
            "image/",
            "image/png; charset=binary",
            "text/html",
            "",
        ] {
            assert!(!is_supported_raster_mime(mime_type), "{mime_type}");
        }
    }

    #[test]
    fn enforces_raster_dimension_and_pixel_boundaries() {
        assert!(validate_raster_dimensions(3_999, 4_000).is_ok());
        assert!(validate_raster_dimensions(4_000, 4_000).is_ok());
        assert!(validate_raster_dimensions(4_001, 4_000).is_err());
        assert!(validate_raster_dimensions(MAX_RASTER_WIDTH + 1, 1).is_err());
        assert!(validate_raster_dimensions(1, MAX_RASTER_HEIGHT + 1).is_err());
    }

    #[test]
    fn enforces_raster_byte_boundaries_before_parsing() {
        assert_eq!(
            validate_raster(&vec![0; MAX_RASTER_BYTES + 1]).unwrap_err(),
            "Image exceeds the maximum allowed size"
        );
    }
}
