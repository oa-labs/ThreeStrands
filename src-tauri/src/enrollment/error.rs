//! Typed failures for enrollment, rotation, recovery, and join codes.
//!
//! The `Display` text of each variant is the sentence Settings shows, so it
//! stays user-facing. Code that needs to react to a failure matches on the
//! variant instead of comparing that text: a malformed key, a grant that
//! isn't sealed to this device, an epoch that moved underneath an
//! operation, and unreachable storage are distinct cases here.
//!
//! A signature from a device that isn't trusted is deliberately *not* an
//! error. The sweep ignores such objects so a hostile or stale object on a
//! shared connector can never stall it.

use threestrands_sync_envelope::limits::MAX_EARLIER_EPOCH_KEYS;
use threestrands_sync_envelope::{EnvelopeError, JoinCodeError};
use threestrands_sync_transport::TransportError;

use crate::db::DatabaseError;

use super::join_codes::{CREDENTIALS_REJECTED, INVITATION_NOT_FOUND, STORAGE_UNREACHABLE};
use super::{EXISTING_SPACE_REFUSAL, LEGACY_SPACE_REFUSAL, RECOVERY_INCOMPLETE};

pub(crate) type EnrollmentResult<T> = Result<T, EnrollmentError>;

#[derive(Debug, thiserror::Error)]
pub enum EnrollmentError {
    // ----------------------------- Local storage -----------------------------
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// The OS keychain refused to store or load an epoch key.
    #[error("{0}")]
    Keychain(String),
    /// A connector could not be loaded, validated, opened, or saved.
    #[error("{0}")]
    Connector(String),

    // ------------------------------- Transport -------------------------------
    /// A transport failed while scanning or fetching.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// No transport accepted an object that had to be published.
    #[error("{0}")]
    PublishFailed(&'static str),

    // ------------------------- Envelopes and key material ---------------------
    /// Signing, encoding, or decoding a control object failed.
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
    /// A public key in a roster, request, grant, or rotation has the wrong
    /// length or isn't a valid curve point.
    #[error("{0}")]
    MalformedKey(&'static str),
    /// A device or request id isn't a valid identifier.
    #[error("{0}")]
    MalformedId(String),
    /// A grant opens with this device's secret for its current key but not
    /// for everything it carries, or not at all.
    #[error("{0}")]
    NotSealedToThisDevice(&'static str),
    #[error("{0}")]
    InvalidRecoveryPhrase(String),

    // ----------------------------- Group state -------------------------------
    /// The active epoch changed after this operation loaded its keys.
    #[error("This device's sync keys changed while rotating. Try again.")]
    EpochChanged,
    #[error("No recovery keys on record for this sync group")]
    MissingRecoveryKeys,
    #[error(
        "This sync group has changed its keys more than {MAX_EARLIER_EPOCH_KEYS} times, so it can't hand its history to a new device. Create a new sync group instead."
    )]
    HistoryLimitReached,
    #[error("This device already belongs to a sync group or is joining one. Leave it first before starting or joining another sync group.")]
    AlreadyEnrolled,
    #[error("{}", LEGACY_SPACE_REFUSAL)]
    LegacySpace,
    #[error("{}", EXISTING_SPACE_REFUSAL)]
    ExistingSpace,
    #[error("{}", RECOVERY_INCOMPLETE)]
    RecoveryIncomplete,
    #[error("No rotation object on any configured transport opened with this recovery phrase yet")]
    NoRecoverableRotation,
    #[error("This request is no longer pending.")]
    RequestNotPending,

    // ------------------------------ Join codes -------------------------------
    #[error(transparent)]
    JoinCode(#[from] JoinCodeError),
    #[error("This join code expired. Ask for a new one.")]
    JoinCodeExpired,
    #[error("That join code isn't open anymore.")]
    JoinCodeNotOpen,
    #[error("This join code's invitation is damaged or doesn't match the code. Ask for a new one.")]
    InvitationDamaged,
    #[error("{}", INVITATION_NOT_FOUND)]
    InvitationNotFound,
    #[error("{}", CREDENTIALS_REJECTED)]
    CredentialsRejected,
    #[error("{}", STORAGE_UNREACHABLE)]
    StorageUnreachable,

    /// A request the person must correct, such as a label that is too long
    /// or a join-code lifetime out of range.
    #[error("{0}")]
    Invalid(String),
}

impl From<rusqlite::Error> for EnrollmentError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(DatabaseError::Sqlite(error))
    }
}

/// Compatibility conversion for the Tauri commands, which still report
/// errors to the frontend as strings.
impl From<EnrollmentError> for String {
    fn from(error: EnrollmentError) -> Self {
        error.to_string()
    }
}
