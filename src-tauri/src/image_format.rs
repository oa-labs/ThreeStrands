//! Image formats that may be handed to a webview as image data.
//!
//! Keep this list to formats decoded as raster images. In particular, SVG is
//! intentionally excluded: it is an active document format with external
//! reference and engine-specific behavior, not just encoded pixels.

pub(crate) const MAX_RASTER_BYTES: usize = 5 * 1024 * 1024;
pub(crate) const MAX_CONTACT_PHOTO_BYTES: usize = 64 * 1024;
const MAX_RASTER_WIDTH: usize = 8_192;
const MAX_RASTER_HEIGHT: usize = 8_192;
const MAX_RASTER_PIXELS: usize = 16_000_000;

pub(crate) fn is_supported_raster_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/avif" | "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    )
}

/// Types a server sends when it doesn't know (or doesn't say) what the bytes
/// are. Only these may be identified by sniffing; any other declared type,
/// including `image/svg+xml` and `text/html`, is taken at its word.
pub(crate) fn is_generic_binary_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "" | "application/octet-stream" | "binary/octet-stream"
    )
}

/// Identifies a supported raster format from its file signature. Only the
/// formats `is_supported_raster_mime` accepts are recognized, so text formats
/// such as SVG or HTML can never be promoted to an image type.
pub(crate) fn sniff_raster_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && matches!(&bytes[8..12], b"avif" | b"avis")
    {
        Some("image/avif")
    } else {
        None
    }
}

pub(crate) fn validate_raster(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_RASTER_BYTES {
        return Err("Image exceeds the maximum allowed size".to_string());
    }
    let dimensions =
        imagesize::blob_size(bytes).map_err(|_| "Unable to verify image dimensions".to_string())?;
    validate_raster_dimensions(dimensions.width, dimensions.height)
}

pub(crate) fn validate_contact_photo(bytes:&[u8])->Result<(),String>{
    if bytes.len()>MAX_CONTACT_PHOTO_BYTES{return Err("Contact photo must be 64 KiB or smaller".into())}
    if !bytes.starts_with(&[0xff,0xd8,0xff]){return Err("Contact photos must be JPEG images".into())}
    let dimensions=imagesize::blob_size(bytes).map_err(|_|"Unable to verify contact photo dimensions")?;
    if dimensions.width>160||dimensions.height>160{return Err("Contact photos must be 160 × 160 pixels or smaller".into())}
    validate_raster(bytes)
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
    fn only_unlabelled_binary_types_are_eligible_for_sniffing() {
        for mime_type in ["", "application/octet-stream", "binary/octet-stream"] {
            assert!(is_generic_binary_mime(mime_type), "{mime_type}");
        }
        for mime_type in ["image/svg+xml", "text/html", "text/plain", "application/xml"] {
            assert!(!is_generic_binary_mime(mime_type), "{mime_type}");
        }
    }

    #[test]
    fn sniffs_each_supported_raster_signature() {
        let cases: [(&[u8], &str); 7] = [
            (b"\x89PNG\r\n\x1a\nrest", "image/png"),
            (&[0xff, 0xd8, 0xff, 0xe0, 0, 0x10], "image/jpeg"),
            (b"GIF87a....", "image/gif"),
            (b"GIF89a....", "image/gif"),
            (b"RIFF\x00\x00\x00\x00WEBPVP8 ", "image/webp"),
            (b"\x00\x00\x00\x1cftypavif\x00\x00", "image/avif"),
            (b"\x00\x00\x00\x1cftypavis\x00\x00", "image/avif"),
        ];
        for (bytes, expected) in cases {
            let sniffed = sniff_raster_mime(bytes);
            assert_eq!(sniffed, Some(expected), "{bytes:?}");
            assert!(is_supported_raster_mime(sniffed.unwrap()));
        }
    }

    #[test]
    fn never_sniffs_active_or_truncated_content_as_an_image() {
        for bytes in [
            &b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"[..],
            b"<?xml version=\"1.0\"?><svg/>",
            b"<!DOCTYPE html><img src=x onerror=alert(1)>",
            b"\x89PNG",
            b"RIFF\x00\x00\x00\x00WAVE",
            b"\x00\x00\x00\x1cftypheic",
            b"",
        ] {
            assert_eq!(sniff_raster_mime(bytes), None, "{bytes:?}");
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

    #[test]
    fn contact_photos_require_small_jpeg_rasters() {
        assert_eq!(validate_contact_photo(&[0x89,0x50,0x4e,0x47]).unwrap_err(),"Contact photos must be JPEG images");
        assert_eq!(validate_contact_photo(&vec![0xff;MAX_CONTACT_PHOTO_BYTES+1]).unwrap_err(),"Contact photo must be 64 KiB or smaller");
    }
}
