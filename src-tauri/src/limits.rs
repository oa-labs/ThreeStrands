/// Maximum aggregate decoded attachment payload accepted locally.
pub(crate) const MAX_ATTACHMENT_BYTES: usize = 18 * 1024 * 1024;

/// Longest `mailto:` link accepted from macOS or the webview. Generous for a
/// prefilled body while keeping a hostile link from flooding the composer.
pub(crate) const MAX_MAIL_LINK_BYTES: usize = 128 * 1024;

/// Mail links held for the webview before it drains them (for example, links
/// opened while the app is still launching). Further links are dropped.
pub(crate) const MAX_PENDING_MAIL_LINKS: usize = 16;
