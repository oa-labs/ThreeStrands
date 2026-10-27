//! Phase 5: the key hierarchy and device-to-device enrollment protocol.
//!
//! Enrollment/rotation objects are signed-but-public control-plane objects
//! (`threestrands_sync_envelope::enrollment`), published and discovered
//! through the exact same content-addressed object store and `scan()`
//! enumeration every other transport object uses — no new `SyncTransport`
//! capability is needed. The only real secret that ever crosses this layer
//! is an epoch key, and it only ever does so as an anonymous
//! [`threestrands_sync_envelope::seal_to_x25519`] stanza that solely the
//! intended recipient's static X25519 secret can open.
//!
//! Trust bootstraps two ways:
//! - **Peer enrollment**: a new device's request and an existing device's
//!   grant are only self-consistently verified by software. The actual
//!   trust decision is a human comparing [`enrollment_fingerprint`](threestrands_sync_envelope::enrollment_fingerprint) between
//!   two screens before the new device imports the grant — see
//!   [`confirm_and_import_grant`].
//! - **Recovery-phrase import**: the recovery X25519 public key is never
//!   published anywhere in cleartext (it rides in the roster's `RosterEntry`
//!   list is a *device* list; recovery is a distinct, fixed, always-eligible
//!   recipient carried only inside grant/rotation payloads a device must
//!   already be able to decrypt). Successfully opening a sealed stanza with
//!   a phrase-derived recovery secret is itself the trust proof — see
//!   [`join_with_recovery_phrase`].

mod crypto;
mod error;
mod join_codes;
mod service;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

pub(crate) use crypto::{roster_entry_x25519, KeychainEpochKeyStore};
pub(crate) use error::{EnrollmentError, EnrollmentResult};
pub(crate) use join_codes::{
    cancel_join_code, create_join_code, join_with_code, preview_join_code, process_join_codes, JoinCodeConnectorChoice,
    JoinCodeNotice, JoinCodePreview, JoinCredentialsChoice, JoinFolderChoice, OutstandingJoinCode,
};
pub(crate) use service::share_keys_with_lagging_peers;
pub use service::{
    approve_enrollment_request, begin_genesis, confirm_and_import_grant, inspect_sync_space, join_with_recovery_phrase,
    publish_enrollment_request, reject_enrollment_request, rotate_epoch, run_enrollment_sweep, SyncSpacePresence,
};

use serde::Serialize;

// ============================ Status and listing ============================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "state")]
pub enum EnrollmentStatus {
    NotStarted,
    AwaitingGrant { request_id: String, fingerprint: String, created_at: String },
    AwaitingConfirmation { request_id: String, fingerprint: String, approver_fingerprint: String },
    Rejected { request_id: String, fingerprint: String },
    Enrolled {
        device_count: usize,
        /// The inviter's name while this device, having joined with a join
        /// code, waits for the inviter to finish admitting it.
        awaiting_admission_from: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        history_handoff_warning: Option<HistoryHandoffWarning>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryHandoffWarning {
    pub used: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEnrollmentRequest {
    pub request_id: String,
    pub device_id: Option<String>,
    pub fingerprint: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRosterEntry {
    pub device_id: String,
    pub status: String,
    pub is_self: bool,
    /// The resolved shared device name, cached in `sync_device_labels`.
    pub label: Option<String>,
    /// When this device last recorded (for itself) or received (for a
    /// peer) a change from that device. Not a liveness signal: an idle but
    /// online device keeps its last value.
    pub last_change_at: Option<String>,
    /// Whether this device joined the group with a join code.
    pub joined_with_join_code: bool,
}

/// Longest local device label accepted, in characters.
pub const MAX_DEVICE_LABEL_CHARS: usize = 60;
