//! Local drafts and a send queue deliberately separate from retryable label mutations.

mod mime;
mod model;
mod service;
#[cfg(test)]
mod tests;

pub use mime::{addresses, stored_addresses};
pub(crate) use mime::{build_draft_mime, build_mime};
pub use model::{Attachment, Draft, OutboxItem, Request, SentCopyStatus};
pub use service::Correspondence;

use serde::Serialize;

const UNDO_MS: i64 = 10_000;
/// How long a discarded draft's server copy stays, so Undo within it leaves
/// the server untouched. The same window as undoing a send.
pub(crate) const DISCARD_SERVER_DELAY_MS: i64 = UNDO_MS;
/// How long a discarded draft, and its attachment files, can be restored.
pub(crate) const DISCARDED_DRAFT_RETENTION_MS: i64 = 60 * 60 * 1000;

pub(crate) fn validate_retention_days(days: Option<i64>) -> Result<(), String> {
    if matches!(days, None | Some(30 | 90 | 365)) {
        Ok(())
    } else {
        Err("Retention must be unlimited or 30, 90, or 365 days".to_string())
    }
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(error)
}
