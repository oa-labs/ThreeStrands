//! Mail parsing shared by every provider: provider payloads into normalized
//! messages ([`mime`]), HTML to plain text ([`html_plain_text`]), sender trust
//! signals from stored headers ([`sender_trust`]), and safe attachment file
//! names ([`filename`]).
//!
//! Everything here is pure: no database, network, file system, or UI. The
//! email rendering trust boundary (sanitizing, CSP, remote content) stays in
//! the app; this crate only decodes and normalizes what arrives.

pub mod filename;
pub mod html_plain_text;
pub mod mime;
pub mod sender_trust;
pub mod types;

/// Maximum aggregate decoded attachment payload accepted locally.
pub const MAX_ATTACHMENT_BYTES: usize = 18 * 1024 * 1024;
