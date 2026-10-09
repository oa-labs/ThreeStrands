//! The provider seam.
//!
//! Everything above this module — the sync engine, the mutation outbox, the
//! compose pipeline — is written against these traits rather than against any
//! one mail service. The vocabulary here is deliberately service-neutral: a
//! sync position is an opaque [`SyncCursor`] rather than a Gmail `historyId`,
//! paging state lives inside that cursor rather than in a separate page
//! token, and a raw message is a [`RawMessage`](crate::mime::RawMessage) MIME
//! tree whichever wire format it arrived in.

pub mod gmail;
pub mod imap;

use async_trait::async_trait;

use crate::{mime::RawMessage, models::Label};

/// Why a provider request failed, in the terms callers actually branch on.
///
/// The sync engine uses [`ProviderError::retry_mutation`] to decide whether a
/// queued mutation stays claimable and
/// [`ProviderError::requires_reauthentication`] to decide whether the account
/// is paused, so every provider must map its own failures onto these.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The stored sync cursor is expired or unusable; the caller falls back
    /// to a full resynchronization.
    #[error("the sync cursor is invalid or expired")]
    InvalidCursor,
    #[error("the requested object was not found")]
    NotFound,
    #[error("temporary transport failure: {0}")]
    TransientTransport(String),
    #[error("authentication failed: {0}")]
    Authentication(String),
    #[error("credentials require reconnection: {0}")]
    ReauthenticationRequired(String),
    #[error("the server rejected a retryable request: {0}")]
    RetryableServer(String),
    #[error("invalid operation: {0}")]
    InvalidOperation(String),
    #[error("the server permanently rejected the request: {0}")]
    PermanentClientRejection(String),
    #[error("{0}")]
    Other(String),
}

impl ProviderError {
    pub fn retry_mutation(&self) -> bool {
        matches!(
            self,
            Self::TransientTransport(_) | Self::Authentication(_) | Self::RetryableServer(_)
        )
    }

    pub fn requires_reauthentication(&self) -> bool {
        matches!(self, Self::ReauthenticationRequired(_))
    }
}

pub type ProviderResult<T> = Result<T, ProviderError>;

/// An opaque, provider-owned record of how far synchronization has got.
///
/// Callers persist it in `sync_state.cursor` and hand it back untouched; only
/// the provider that minted it may interpret it. Gmail stores a `historyId`
/// here. A provider whose change detection is per-folder, or that needs to
/// resume mid-page, encodes that structure in the same string instead of
/// widening this type.
///
/// The IMAP provider's position is deliberately *not* its UID state: per
/// `docs/imap-design.md`, the UID-to-id map and each mailbox's counters live
/// in the [`ImapStateStore`](crate::provider::imap::ImapStateStore), and the
/// cursor "stays small, holding only a sync generation number". That
/// generation is a provider-neutral monotonic epoch — bumped whenever a sync
/// round reconciles against the store — carried through this opaque string by
/// [`SyncCursor::from_generation`] / [`SyncCursor::generation`], so slimming
/// the IMAP cursor needs no change to this shared type or to Gmail's own
/// `historyId` encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncCursor(String);

impl SyncCursor {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A cursor carrying only a provider-neutral sync generation number, as
    /// the IMAP provider uses it. The decimal encoding keeps the persisted
    /// `sync_state.cursor` a plain string like every other provider's.
    ///
    /// The IMAP provider that mints and reads these lands in phase 2, so the
    /// pair has no non-test caller yet; the `dead_code` allow is scoped to
    /// them and removed with that first caller, matching how slice 1 scoped
    /// the unused `ProviderCapabilities` fields.
    #[allow(dead_code)]
    pub fn from_generation(generation: u64) -> Self {
        Self(generation.to_string())
    }

    /// Reads this cursor back as a sync generation number. `None` when the
    /// string was not minted by [`SyncCursor::from_generation`] (for example
    /// a Gmail `historyId` cursor), so a caller can tell a generation cursor
    /// apart from any other provider's encoding rather than guessing.
    #[allow(dead_code)]
    pub fn generation(&self) -> Option<u64> {
        self.0.parse().ok()
    }
}

/// A page of thread ids, with an opaque token for the next page.
#[derive(Debug, Default)]
pub struct ThreadPage {
    pub thread_ids: Vec<String>,
    pub next: Option<String>,
}

/// One round of incremental change detection.
///
/// `more` drives the caller's paging loop; the paging position itself is
/// carried inside `cursor`, so an interrupted loop resumes from whatever was
/// last persisted rather than restarting.
#[derive(Debug)]
pub struct SyncBatch {
    /// Threads whose content or labels changed since the polled cursor.
    pub changed_threads: Vec<String>,
    pub cursor: SyncCursor,
    pub more: bool,
}

/// How a provider models the labels a user can see and toggle.
///
/// The frontend chooses which label actions to offer from this: Gmail-style
/// many-to-many labels behave differently from IMAP folders with keyword or
/// label-folder user labels, and some IMAP accounts have no user-label
/// storage at all. Only the Gmail variant exists in this slice; the IMAP
/// variants land with the IMAP provider (phases 2–3). Matches on this enum
/// should list every variant rather than use a `_` arm, so adding the IMAP
/// variants fails to compile until each caller decides how to handle them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelModel {
    /// Many-to-many labels, as Gmail exposes them. A message can carry any
    /// number of labels and they are toggled directly.
    GmailLabels,
    // IMAP: folders plus keyword user labels (`PERMANENTFLAGS` has `\*`).
    // IMAP: folders plus label-folder user labels (copies under a container).
    // IMAP: folders with no user-label storage available.
    // These land with the IMAP provider in phases 2–3.
}

/// The provider differences callers genuinely have to branch on.
///
/// Deliberately minimal: every flag here is one that a code path branches on.
/// `provided_threads` and `label_model` are the exception while the IMAP
/// provider is being built: `docs/imap-design.md` (phase 1) adds them ahead
/// of their consumers, local threading (phase 2) and the frontend's label
/// actions (phases 3 and 6). Until those land they have no reader, so their
/// dead-code allows are scoped to the two fields and removed with the first
/// caller.
#[derive(Clone, Copy, Debug)]
pub struct ProviderCapabilities {
    /// Whether the provider can find mail the local cache has not ingested.
    /// When false the remote search backfill is skipped entirely rather than
    /// attempted and rejected.
    pub server_search: bool,
    /// Whether the provider supplies thread ids itself (`true`) or threading
    /// is computed locally from message references (`false`). Gmail returns
    /// conversation ids, so it is `true`; the IMAP provider sets it `false`
    /// when the server does not return `THREADID`.
    #[allow(dead_code)]
    pub provided_threads: bool,
    /// How this provider models user-visible labels. Drives which label
    /// actions the frontend offers. See [`LabelModel`].
    #[allow(dead_code)]
    pub label_model: LabelModel,
    /// Whether a message's delivery can be *verified* against a server-side
    /// sent copy. Gmail stores the sent message itself, so an uncertain send
    /// is resolved by looking it up with [`MailSend::find_sent_copy`]
    /// (`true`). A provider that keeps no server-side sent copy of its own —
    /// an IMAP account where only we would `APPEND` to `\Sent`, and the
    /// connection dropped before we could — has nothing to look the send up
    /// against, so `find_sent_copy` can never confirm it (`false`). The
    /// delivery path reads this to decide whether an unconfirmable send is an
    /// error to re-check or the terminal `Unverifiable` outcome from
    /// `docs/imap-design.md` ("Sending"). See [`crate::correspondence`].
    pub verifiable_delivery: bool,
}

/// What a provider reports back after accepting a message for delivery.
#[derive(Debug, Clone)]
pub struct DeliveryReceipt {
    /// The provider's id for the stored copy of the sent message.
    pub provider_message_id: String,
    /// The conversation the sent copy landed in, when the provider says.
    pub thread_id: Option<String>,
}

/// Everything the local cache is populated from.
#[async_trait]
pub trait MailSync: Send + Sync {
    /// A cursor positioned at the present moment, captured *before* a full
    /// snapshot is listed so that mail arriving during the listing is still
    /// picked up by the incremental pass that follows.
    async fn baseline_cursor(&self) -> ProviderResult<SyncCursor>;

    /// One round of incremental change detection. Returns
    /// [`ProviderError::InvalidCursor`] when `cursor` can no longer be used,
    /// which the caller answers with a full resynchronization.
    async fn poll(&self, cursor: &SyncCursor) -> ProviderResult<SyncBatch>;

    /// The inbox's current thread ids, one page at a time.
    async fn list_inbox(&self, page: Option<&str>) -> ProviderResult<ThreadPage>;

    /// Every message in a thread.
    async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>>;

    /// Server-side search, for finding mail the local index has never seen.
    /// Optional: providers without it report
    /// `server_search: false` and inherit this rejection.
    async fn search(&self, _query: &str, _page: Option<&str>) -> ProviderResult<ThreadPage> {
        Err(ProviderError::InvalidOperation(
            "server-side search is not supported by this provider".into(),
        ))
    }
}

/// On-demand reads of a single item, for the parts of the UI that reach past
/// the local cache: downloading an attachment the cache stores out of line,
/// and quoting a message the cache never ingested.
#[async_trait]
pub trait MailFetch: Send + Sync {
    async fn fetch_message(&self, id: &str) -> ProviderResult<RawMessage>;

    /// The bytes of an attachment the provider stored out of line, named by
    /// whatever opaque handle the message payload carried.
    async fn attachment_bytes(&self, message: &str, handle: &str) -> ProviderResult<Vec<u8>>;
}

/// Changes to mail that already exists on the server.
///
/// `add`/`remove` are canonical label ids as produced by the sync engine's
/// single translation point; a folder-model provider turns them into moves.
#[async_trait]
pub trait MailMutate: Send + Sync {
    async fn modify_thread(&self, id: &str, add: &[String], remove: &[String])
        -> ProviderResult<()>;
    async fn modify_messages(
        &self,
        ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()>;
    async fn list_labels(&self) -> ProviderResult<Vec<Label>>;
    async fn create_label(&self, name: &str) -> ProviderResult<Label>;
    async fn update_label(&self, id: &str, name: &str) -> ProviderResult<Label>;
    async fn delete_label(&self, id: &str) -> ProviderResult<()>;
}

/// An authorized, single-use permit to deliver one message.
///
/// Delivery is deliberately two steps. Acquiring authorization can fail for
/// ordinary reasons — an expired token, an unreachable submission server —
/// and that must happen *before* the outbox row is claimed, because a failure
/// after the claim is indistinguishable from "possibly delivered" and strands
/// the message in `uncertain`. Holding the permit in a value makes that
/// ordering a property of the type rather than a convention.
#[async_trait]
pub trait Delivery: Send {
    /// Sends `raw` exactly once. Consumes the permit, because delivery is not
    /// idempotent and must never be retried behind the caller's back.
    ///
    /// On failure the boolean reports whether the rejection was *definite*:
    /// `true` means the message certainly was not delivered and the draft can
    /// be restored, `false` means the outcome is unknown.
    async fn send_once(
        self: Box<Self>,
        raw: &[u8],
        thread: Option<&str>,
    ) -> Result<DeliveryReceipt, (bool, String)>;
}

/// Outbound mail.
#[async_trait]
pub trait MailSend: Send + Sync {
    /// The address this account actually sends as, according to the server.
    async fn sender_identity(&self) -> ProviderResult<String>;

    /// Authorizes one delivery. See [`Delivery`] for why this is separate.
    async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>>;

    /// Looks for an already-sent copy of `operation`, to resolve a delivery
    /// whose outcome was left unknown by a crash or a dropped connection.
    async fn find_sent_copy(
        &self,
        operation: &str,
        expected_sender: &str,
    ) -> ProviderResult<Option<DeliveryReceipt>>;
}

/// A complete mail account backend.
pub trait MailProvider: MailSync + MailFetch + MailMutate + MailSend {
    fn capabilities(&self) -> ProviderCapabilities;
}
