//! The application-wide error type at the Tauri command boundary.
//!
//! The backend already has mature, domain-scoped error enums
//! ([`DatabaseError`](crate::db::DatabaseError),
//! [`ProviderError`](crate::provider::ProviderError),
//! [`EnrollmentError`](crate::enrollment::EnrollmentError),
//! [`TransportError`](threestrands_sync_transport::TransportError)), each with
//! `thiserror` and `#[from]` plumbing. `DatabaseError` and `EnrollmentError`
//! also carry a `From<X> for String` compatibility shim. What was missing was
//! a single top-level type that unifies them so internal code can
//! `?`-propagate a *typed* error all the way to the command layer instead of
//! eagerly flattening it with `.map_err(|e| e.to_string())` at the first `?`.
//!
//! [`AppError`] is that type. Every command returns [`AppResult<T>`], and the
//! domain enums convert into it with `#[from]`.
//!
//! ## Migration status
//!
//! Only the command layer has moved so far. Today a failure keeps its
//! category to the boundary only when it is a [`DatabaseError`] reached
//! through `?` or `database_result`. Most internal modules (replicated sync,
//! correspondence, auth, AI) still return `Result<_, String>`, so their
//! enrollment, transport, and provider failures are already text by the time
//! a command sees them and arrive as [`AppError::Message`]. The `Provider`,
//! `Enrollment`, and `Transport` variants are in place for those modules to
//! adopt as they move to typed results; nothing produces them yet.
//!
//! ## Wire contract is unchanged
//!
//! The frontend consumes a command error only as a string — `errorMessage(e)`
//! resolves to `reason instanceof Error ? reason.message : String(reason)`,
//! and nothing in the React layer reads a structured error object. So
//! [`AppError`] serializes to the frontend as its [`Display`] text, exactly
//! like the previous `Err(String)` did. Changing it to a JSON object would
//! turn every `String(e)` into `"[object Object]"` and break error display
//! app-wide. The improvement here is entirely backend-internal: typed
//! `?`-propagation and one unifying error, with a byte-identical boundary.
//!
//! [`Display`]: std::fmt::Display

use serde::{Serialize, Serializer};

use crate::db::DatabaseError;
use crate::enrollment::EnrollmentError;
use crate::provider::ProviderError;
use threestrands_sync_transport::TransportError;

/// The unified error returned by Tauri commands and the internal plumbing
/// that feeds them.
///
/// Internal code produces one of the typed variants via `?` (the `#[from]`
/// conversions), and foreign errors with no domain enum (reqwest, serde_json,
/// std::io, utf8) collapse into [`AppError::Message`] — still `?`-able through
/// [`From<String>`]/[`From<&str>`], and still a plain sentence at the
/// boundary. Nothing is lost relative to the old `.map_err(|e| e.to_string())`
/// for those. An internal `Result<_, String>` also lands in `Message`, so a
/// typed variant appears only where the typed error itself reaches `?`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Enrollment(#[from] EnrollmentError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A failure that carries no domain enum: a foreign error already rendered
    /// to text, or an ad-hoc `Err(format!(...))` sentence. The string is
    /// user-facing, matching the previous `Err(String)` behaviour.
    #[error("{0}")]
    Message(String),
}

/// The command/plumbing result alias. Reads as the drop-in replacement for the
/// old `Result<T, String>`.
pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// Build a [`AppError::Message`] from anything printable. Use at the sites
    /// that previously did `.map_err(|e| e.to_string())` for a *foreign*
    /// error (one with no domain enum, e.g. `reqwest::Error`): the message is
    /// preserved verbatim, so the boundary string is unchanged.
    pub fn message(value: impl std::fmt::Display) -> Self {
        Self::Message(value.to_string())
    }
}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for AppError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

/// The last mile of the compatibility story: a bare `From<AppError> for String`
/// keeps any not-yet-migrated `Result<_, String>` caller able to accept an
/// `AppError` through `?`, so the migration can land file by file without a
/// flag day.
impl From<AppError> for String {
    fn from(error: AppError) -> Self {
        error.to_string()
    }
}

/// Serialize to the frontend as the error's `Display` text — the exact wire
/// shape the old `Err(String)` produced. See the module docs for why this is
/// a string and not a structured object.
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_preserves_text() {
        let err = AppError::message("boom");
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn string_round_trips_through_message() {
        let err: AppError = "nope".to_string().into();
        assert!(matches!(err, AppError::Message(ref m) if m == "nope"));
        let back: String = err.into();
        assert_eq!(back, "nope");
    }

    #[test]
    fn domain_errors_convert_via_from() {
        let db: AppError = DatabaseError::NotFound("Thread").into();
        assert_eq!(db.to_string(), "Thread not found");
        assert!(matches!(db, AppError::Database(_)));

        let provider: AppError = ProviderError::InvalidCursor.into();
        assert!(matches!(provider, AppError::Provider(_)));
        assert_eq!(provider.to_string(), "the sync cursor is invalid or expired");

        let transport: AppError = TransportError::NotFound.into();
        assert!(matches!(transport, AppError::Transport(_)));
    }

    #[test]
    fn serializes_as_a_plain_string_not_an_object() {
        // The frontend reads errors only as strings; a JSON object here would
        // render as "[object Object]". Lock the wire shape.
        let err = AppError::Database(DatabaseError::Validation("bad input".into()));
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, "\"bad input\"");
    }

    #[test]
    fn display_matches_the_old_string_shim() {
        // Parity with `From<DatabaseError> for String`: the boundary text is
        // byte-identical to what the pre-AppError command returned.
        let domain = DatabaseError::Validation("message too long".into());
        let via_old_shim: String = domain.to_string();
        let via_app_error: String = AppError::Database(DatabaseError::Validation(
            "message too long".into(),
        ))
        .into();
        assert_eq!(via_old_shim, via_app_error);
    }
}
