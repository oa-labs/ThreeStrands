/// Maximum aggregate decoded attachment payload accepted locally; owned by the
/// mail parser, which enforces it while decoding.
pub(crate) use threestrands_mail_mime::MAX_ATTACHMENT_BYTES;

/// Longest `mailto:` link accepted from macOS or the webview. Generous for a
/// prefilled body while keeping a hostile link from flooding the composer.
pub(crate) const MAX_MAIL_LINK_BYTES: usize = 128 * 1024;

/// Mail links held for the webview before it drains them (for example, links
/// opened while the app is still launching). Further links are dropped.
pub(crate) const MAX_PENDING_MAIL_LINKS: usize = 16;

/// Opened calendar files held for the webview before it shows them. Each one
/// is already parsed (at most `calendar::MAX_CALENDAR_BYTES` of input), so
/// this bounds memory as well as the dialog queue.
pub(crate) const MAX_PENDING_CALENDAR_FILES: usize = 16;

/// Longest iCalendar UID looked up on connected calendars.
pub(crate) const MAX_CALENDAR_UID_BYTES: usize = 1024;
