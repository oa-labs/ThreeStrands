//! Image formats that may be handed to a webview as image data.
//!
//! Keep this list to formats decoded as raster images. In particular, SVG is
//! intentionally excluded: it is an active document format with external
//! reference and engine-specific behavior, not just encoded pixels.

pub(crate) fn is_supported_raster_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/avif" | "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    )
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
}
