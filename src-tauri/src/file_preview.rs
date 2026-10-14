//! Pictures of attachments for the Files details panel. On macOS, Quick Look
//! draws them: it parses the file in its own sandboxed thumbnail extension
//! process, so this app never parses PDFs or Office documents itself. What
//! comes back is a PNG, which goes through the same raster validator as image
//! thumbnails before it reaches the page as a data URI. Files that the opening
//! check considers risky get no preview, and other platforms get none yet.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The square, in points, a preview is drawn to fit. Quick Look renders at
/// `PREVIEW_SCALE` pixels per point, so the PNG is at most 640px on a side.
pub(crate) const PREVIEW_SIZE_POINTS: f64 = 320.0;
pub(crate) const PREVIEW_SCALE: f64 = 2.0;
/// Larger files are not handed to Quick Look; the panel keeps its icon.
pub(crate) const PREVIEW_MAX_SOURCE_BYTES: usize = 50 * 1024 * 1024;
/// A thumbnail extension that takes longer than this is abandoned.
pub(crate) const PREVIEW_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether a file may be handed to Quick Look at all: within the size limit
/// and with nothing the opening check would warn about.
pub(crate) fn previewable(filename: &str, bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.len() <= PREVIEW_MAX_SOURCE_BYTES
        && crate::attachment_security::opening_warnings(filename, bytes).is_empty()
}

/// Scratch space for one preview, removed when dropped so neither the copy
/// of the attachment nor the thumbnail outlives the request.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Where previews are drawn. Anything left from a crash is cleared at startup.
pub(crate) struct PreviewRoot(PathBuf);

impl PreviewRoot {
    pub(crate) fn new(root: PathBuf) -> io::Result<Self> {
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root)?;
        Ok(Self(root))
    }

    fn scratch(&self) -> io::Result<Scratch> {
        let directory = self.0.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&directory)?;
        Ok(Scratch(directory))
    }
}

/// A validated PNG data URI for the attachment, or `None` when it has no
/// preview: a risky or oversized file, an unsupported platform, or a type
/// Quick Look cannot draw.
pub(crate) async fn preview_data_uri(
    root: &PreviewRoot,
    filename: &str,
    bytes: &[u8],
) -> Result<Option<String>, String> {
    use base64::Engine;

    if !previewable(filename, bytes) {
        return Ok(None);
    }
    let scratch = root
        .scratch()
        .map_err(|error| format!("Unable to prepare preview: {error}"))?;
    let name = crate::attachment_security::normalize_filename(filename);
    let source = scratch.0.join(&name);
    fs::write(&source, bytes).map_err(|error| format!("Unable to prepare preview: {error}"))?;
    crate::attachment_security::quarantine(&source)
        .map_err(|error| format!("Unable to prepare preview: {error}"))?;
    let output = scratch.0.join("preview.png");
    if !render(&source, &output).await {
        return Ok(None);
    }
    let png = match fs::read(&output) {
        Ok(png) => png,
        Err(_) => return Ok(None),
    };
    if !png_is_valid(&png) {
        return Ok(None);
    }
    Ok(Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )))
}

fn png_is_valid(png: &[u8]) -> bool {
    png.starts_with(b"\x89PNG\r\n\x1a\n") && crate::image_format::validate_raster(png).is_ok()
}

#[cfg(target_os = "macos")]
async fn render(source: &Path, output: &Path) -> bool {
    let Some(done) = macos::request_thumbnail(source, output) else {
        return false;
    };
    matches!(
        tokio::time::timeout(PREVIEW_TIMEOUT, done).await,
        Ok(Ok(true))
    )
}

#[cfg(not(target_os = "macos"))]
async fn render(_source: &Path, _output: &Path) -> bool {
    false
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{PREVIEW_SCALE, PREVIEW_SIZE_POINTS};
    use block2::RcBlock;
    use objc2::AllocAnyThread;
    use objc2_core_foundation::CGSize;
    use objc2_foundation::{NSError, NSString, NSURL};
    use objc2_quick_look_thumbnailing::{
        QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
        QLThumbnailGenerator,
    };
    use objc2_uniform_type_identifiers::UTType;
    use std::path::Path;
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    /// Asks Quick Look to save a thumbnail of `source` to `output` as PNG.
    /// The receiver yields whether it succeeded. Everything Objective-C stays
    /// in this synchronous call, so the awaiting future remains `Send`.
    pub(super) fn request_thumbnail(
        source: &Path,
        output: &Path,
    ) -> Option<oneshot::Receiver<bool>> {
        let source = NSURL::fileURLWithPath(&NSString::from_str(source.to_str()?));
        let output = NSURL::fileURLWithPath(&NSString::from_str(output.to_str()?));
        let png = UTType::typeWithIdentifier(&NSString::from_str("public.png"))?;
        let (sender, receiver) = oneshot::channel();
        let sender = Mutex::new(Some(sender));
        // Only a real thumbnail: a generic file icon says nothing the panel's
        // own icon doesn't.
        let request = unsafe {
            QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                QLThumbnailGenerationRequest::alloc(),
                &source,
                CGSize {
                    width: PREVIEW_SIZE_POINTS,
                    height: PREVIEW_SIZE_POINTS,
                },
                PREVIEW_SCALE,
                QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
            )
        };
        let completion = RcBlock::new(move |error: *mut NSError| {
            if let Some(sender) = sender.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = sender.send(error.is_null());
            }
        });
        unsafe {
            QLThumbnailGenerator::sharedGenerator()
                .saveBestRepresentationForRequest_toFileAtURL_asContentType_completionHandler(
                    &request,
                    &output,
                    &png,
                    &completion,
                );
        }
        Some(receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn previews_ordinary_documents_within_the_size_limit() {
        assert!(previewable("Slides.pdf", b"%PDF-1.7\n"));
        assert!(previewable("photo.png", PNG_1X1));
        assert!(previewable(
            "notes.txt",
            &vec![b'a'; PREVIEW_MAX_SOURCE_BYTES]
        ));
    }

    #[test]
    fn refuses_empty_oversized_and_risky_files() {
        assert!(!previewable("empty.pdf", b""));
        assert!(!previewable(
            "notes.txt",
            &vec![b'a'; PREVIEW_MAX_SOURCE_BYTES + 1]
        ));
        // By extension, by Office macro type, and by contents that contradict the name.
        assert!(!previewable("setup.exe", b"MZ\x90\x00"));
        assert!(!previewable("Budget.xlsm", b"PK\x03\x04"));
        assert!(!previewable("Secure Message.html", b"<html><form>"));
        assert!(!previewable("invoice.pdf", b"MZ\x90\x00\x03\x00\x00\x00"));
    }

    #[test]
    fn accepts_only_a_valid_png_back() {
        assert!(png_is_valid(PNG_1X1));
        assert!(!png_is_valid(b"GIF89a"));
        assert!(!png_is_valid(&PNG_1X1[..20]));
    }

    #[tokio::test]
    async fn returns_no_preview_for_a_risky_file_without_touching_disk() {
        let directory = std::env::temp_dir().join(format!("preview-test-{}", uuid::Uuid::new_v4()));
        let root = PreviewRoot::new(directory.clone()).unwrap();
        assert_eq!(
            preview_data_uri(&root, "setup.exe", b"MZ\x90\x00")
                .await
                .unwrap(),
            None
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        let _ = fs::remove_dir_all(directory);
    }

    /// A one-page PDF with a filled square, small enough to inline.
    #[cfg(target_os = "macos")]
    fn tiny_pdf() -> Vec<u8> {
        let content = "0 0 1 rg 20 20 160 160 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn draws_a_pdf_page_through_quick_look_and_cleans_up() {
        let directory = std::env::temp_dir().join(format!("preview-test-{}", uuid::Uuid::new_v4()));
        let root = PreviewRoot::new(directory.clone()).unwrap();
        let uri = preview_data_uri(&root, "Slides.pdf", &tiny_pdf())
            .await
            .unwrap()
            .expect("a preview");
        assert!(uri.starts_with("data:image/png;base64,"));
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn clears_leftover_scratch_space_at_startup_and_after_each_preview() {
        let directory = std::env::temp_dir().join(format!("preview-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(directory.join("stale")).unwrap();
        let root = PreviewRoot::new(directory.clone()).unwrap();
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        {
            let scratch = root.scratch().unwrap();
            fs::write(scratch.0.join("copy.pdf"), b"%PDF").unwrap();
            assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        }
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        let _ = fs::remove_dir_all(directory);
    }
}
