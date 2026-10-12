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
    pub fn from_generation(generation: u64) -> Self {
        Self(generation.to_string())
    }

    /// Reads this cursor back as a sync generation number. `None` when the
    /// string was not minted by [`SyncCursor::from_generation`] (for example
    /// a Gmail `historyId` cursor), so a caller can tell a generation cursor
    /// apart from any other provider's encoding rather than guessing.
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

/// What a server search was able to do, carried out of the provider WITHOUT
/// branching on provider kind (Phase 5 run 5). A provider error still returns
/// an `Err` from [`MailSync::search_with_report`]; this type only describes a
/// search that RAN (fully, partially, or not at all because a term could not
/// be expressed). The frontend words it — the status is a typed CODE, never a
/// developer string, a mailbox wire name or the user's query — so wording
/// lives and is tested in TypeScript.
///
/// Serialised camelCase (externally tagged on `status`) for the Tauri command,
/// like the app's other command results.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchReport {
    pub status: SearchReportStatus,
    /// How many PLANNED mailboxes were skipped because they are not synced, so
    /// a pin there would never be maintained. Summed across accounts.
    pub unsynced_mailboxes: usize,
    /// How many hits were refused across all mailboxes because keeping them
    /// would exceed the per-account search-pin cap. Summed across accounts.
    pub skipped_over_cap: usize,
}

impl SearchReport {
    /// A fully-complete search touching nothing worth wording. The default a
    /// provider inherits unless it reports otherwise.
    pub fn complete() -> Self {
        Self {
            status: SearchReportStatus::Complete,
            unsynced_mailboxes: 0,
            skipped_over_cap: 0,
        }
    }

    /// Combine this account's report with another's for a unified search over
    /// several accounts. Counts SUM; the status takes the most-severe of the
    /// two by [`SearchReportStatus::severity`] (a reason-carrying status keeps
    /// ALL reasons, concatenated in account order), so one account's partial
    /// or skipped outcome is never hidden by another's `Complete`. Commutative
    /// in counts; status order follows the fold order (account iteration
    /// order) when two statuses tie on severity.
    pub fn combine(self, other: SearchReport) -> SearchReport {
        let unsynced_mailboxes = self.unsynced_mailboxes + other.unsynced_mailboxes;
        let skipped_over_cap = self.skipped_over_cap + other.skipped_over_cap;
        let status = SearchReportStatus::combine(self.status, other.status);
        SearchReport {
            status,
            unsynced_mailboxes,
            skipped_over_cap,
        }
    }
}

/// The outcome of one server search, as a typed code the UI words.
///
/// Severity order (least → most, for the multi-account combine): `Complete` <
/// `Partial` < `NoServerMatches` < `Skipped`. `Skipped` wins because it means
/// "only locally-synced mail is shown" — the strongest caveat; `NoServerMatches`
/// outranks `Partial` because, when it survives to the UI (only when the local
/// result is empty too), it is a definite "nothing by that name" the user must
/// see over a partial-coverage note.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SearchReportStatus {
    /// Every planned mailbox was searched to completion.
    Complete,
    /// The search ran but did not cover everything (deadline, server budget,
    /// or a single mailbox erroring). Carries one or more typed reasons.
    Partial { reasons: Vec<PartialSearchReason> },
    /// No server search ran: a term the server cannot honestly evaluate. Only
    /// locally-synced mail is shown. Carries one or more typed reasons.
    Skipped { reasons: Vec<SkipSearchReason> },
    /// The server proved the answer is empty (an unknown folder/label by that
    /// name). The UI shows this only when the local result is also empty.
    NoServerMatches { reason: NoMatchSearchReason },
}

impl SearchReportStatus {
    /// Severity rank for the multi-account combine (higher wins).
    fn severity(&self) -> u8 {
        match self {
            SearchReportStatus::Complete => 0,
            SearchReportStatus::Partial { .. } => 1,
            SearchReportStatus::NoServerMatches { .. } => 2,
            SearchReportStatus::Skipped { .. } => 3,
        }
    }

    /// Fold two statuses: keep the more severe; when both are the SAME
    /// reason-carrying variant, concatenate their reasons (so two accounts can
    /// each contribute a partial reason). A `NoServerMatches` tie keeps the
    /// first (`self`) — a single reason suffices for the UI's one-line note.
    fn combine(self, other: SearchReportStatus) -> SearchReportStatus {
        use SearchReportStatus::*;
        match (self, other) {
            (Partial { mut reasons }, Partial { reasons: more }) => {
                reasons.extend(more);
                Partial { reasons }
            }
            (Skipped { mut reasons }, Skipped { reasons: more }) => {
                reasons.extend(more);
                Skipped { reasons }
            }
            (a, b) => {
                if b.severity() > a.severity() {
                    b
                } else {
                    a
                }
            }
        }
    }
}

/// Why a search returned fewer results than a full scan would. A typed code,
/// no developer strings or wire names reach the UI.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum PartialSearchReason {
    /// A wall-clock deadline expired before every planned mailbox ran.
    Deadline,
    /// A server-side result budget was reached; older matches were not fetched.
    Budget,
    /// A single folder errored mid-search; the rest were returned.
    FolderError,
}

/// Why a server search was skipped entirely (a term the server cannot honestly
/// evaluate). A typed code; the user's query is NEVER echoed back.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum SkipSearchReason {
    /// A syntax/term the server cannot search for (negation, `OR`, an unknown
    /// operator, invalid characters, an over-limit value, an empty query, a
    /// cross-mailbox combination, no label storage). All collapse to this one
    /// user-facing code: the UI tells the user only that the search fell back
    /// to local mail, never the internal reason or their query text.
    Unsupported,
}

/// Why the server proved there are no matches. A typed code.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum NoMatchSearchReason {
    /// No folder or label by that name exists on the server.
    UnknownName,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum LabelModel {
    /// Many-to-many labels, as Gmail exposes them. A message can carry any
    /// number of labels and they are toggled directly. The default so that an
    /// account row deserialized without a `labelModel` (an older cache row, a
    /// synced projection, a settings import) keeps the pre-IMAP behaviour.
    #[default]
    GmailLabels,
    /// IMAP folders plus keyword user labels (`PERMANENTFLAGS` has `\*`): user
    /// labels are stored as IMAP keywords.
    ImapKeywords,
    /// IMAP folders plus label-folder user labels: user labels are copies
    /// under a container mailbox the user picked at setup.
    ImapLabelFolders,
    /// IMAP folders with no user-label storage available (no `\*` and no
    /// container chosen): the UI hides label actions for the account.
    ImapNoUserLabels,
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
    /// Submission is implemented; checking this never contacts the server.
    pub send_supported: bool,
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
    /// Whether this provider saves the sender's own copy into the account's
    /// Sent mailbox as a SEPARATE step the engine must drive (`true` for IMAP:
    /// after an accepted SMTP send the engine runs a durable Sent-copy job via
    /// [`MailSend::save_sent_copy`]). Gmail stores the sent message itself as
    /// part of submission, so it is `false` and no job is created. The engine
    /// reads this at the moment it records a send accepted to decide — ATOMICALLY
    /// with the `sent` transition — whether to seed the Sent-copy job row. No
    /// code path branches on a provider name or id; this capability is the only
    /// signal. See `docs/imap-design.md` ("Sending" steps 3–4).
    pub client_saves_sent_copy: bool,
}

/// What a provider reports back after accepting a message for delivery.
#[derive(Debug, Clone)]
pub struct DeliveryReceipt {
    /// The provider's id for the stored copy of the sent message.
    pub provider_message_id: String,
    /// The conversation the sent copy landed in, when the provider says.
    pub thread_id: Option<String>,
}

/// Which phase of the Sent-copy job the engine is driving when it calls
/// [`MailSend::save_sent_copy`]. The provider holds NO per-send state (an
/// [`ImapProvider`](crate::provider::imap) is built per call), so the engine's
/// durable job state tells the provider whether this is the first attempt or a
/// post-APPEND verify. See `docs/imap-design.md` ("Sending" steps 3–4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SentCopyPhase {
    /// First (and retried-before-APPEND) attempt: the provider decides whether
    /// the server already saved a copy, else APPENDs and asks to be re-checked.
    Pending,
    /// Post-APPEND verify: the provider counts the copies now present to tell a
    /// server that saved its own copy (duplicate) from the single copy we
    /// appended, recording the per-device detection result.
    Verify,
}

/// Everything a provider needs to save (or decide not to save) the sender's
/// own copy of one sent message, provider-neutrally. The engine owns the
/// durable job and passes its current state in; the provider owns the decision
/// and performs any network I/O. See [`MailSend::save_sent_copy`].
#[derive(Clone, Debug)]
pub struct SentCopyRequest {
    /// The RAW bytes of the sent message (the outbox `raw`, UNMODIFIED — it
    /// keeps its `Bcc:` header, which is correct for the sender's own copy).
    pub raw: Vec<u8>,
    /// The outbox id of the message, for logging/correlation only.
    pub outbox_id: String,
    /// Whether an APPEND of OUR copy has already been attempted in a prior
    /// cycle of this job (the engine's write-ahead flag). Only a copy found
    /// BEFORE any APPEND of ours is evidence the server saves its own copies;
    /// a copy found once this is `true` may be our own appended copy, so it is
    /// never counted as such. See `docs/imap-design.md` ("Sending").
    pub append_attempted: bool,
    /// Which phase the engine's durable job is in. The provider cannot keep
    /// this itself (it is rebuilt per call), so it is carried here.
    pub phase: SentCopyPhase,
}

/// The provider's answer from one [`MailSend::save_sent_copy`] call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SentCopyStep {
    /// Nothing more to do: the copy exists (ours or the server's) or the
    /// provider saves nothing. The engine marks the job `done`.
    Done,
    /// The provider needs to be called again after `after` (it appended our
    /// copy and wants to verify the server did not also save one). The engine
    /// moves the job to the `verify` phase with `next_at = now + after`.
    Recheck { after: std::time::Duration },
}

/// One address an account may send as, with an optional display name.
///
/// Provider-neutral by design: Gmail and any single-identity account report
/// exactly one (their account address, no display name); an IMAP account
/// reports its ordered, login-anchored identity list. The list's FIRST entry
/// is always the account's primary (login) address — callers that need "the
/// account's own address" read `identities()[0]`, and every "sent by me"
/// decision treats a `From` matching ANY entry as the user's own. See
/// `docs/imap-design.md` ("Identities") and
/// [`crate::provider::imap::account_identities`], the one builder that applies
/// the normalization/validation rules.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendingIdentity {
    /// The address as displayed (original case preserved for the header).
    pub address: String,
    /// An optional display name for the From header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
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

    /// Merged thread ids and their current canonical ids. Providers with
    /// immutable thread ids need no alias handling. Ingestion retires aliased
    /// cached threads atomically with a successfully fetched survivor.
    fn thread_aliases(&self) -> ProviderResult<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    /// Server-side search, for finding mail the local index has never seen.
    /// Optional: providers without it report
    /// `server_search: false` and inherit this rejection.
    async fn search(&self, _query: &str, _page: Option<&str>) -> ProviderResult<ThreadPage> {
        Err(ProviderError::InvalidOperation(
            "server-side search is not supported by this provider".into(),
        ))
    }

    /// Server-side search that also reports WHAT the search could do (Phase 5
    /// run 5), so the caller can word the UI without branching on provider
    /// kind. The default runs [`MailSync::search`] and reports
    /// [`SearchReport::complete`]: a provider that cannot describe partial /
    /// skipped outcomes (Gmail) stays byte-identical in behaviour and always
    /// reports `Complete`. IMAP overrides this to map its rich outcome. A
    /// provider error is still an `Err`, exactly as [`MailSync::search`].
    async fn search_with_report(
        &self,
        query: &str,
        page: Option<&str>,
    ) -> ProviderResult<(ThreadPage, SearchReport)> {
        let page = self.search(query, page).await?;
        Ok((page, SearchReport::complete()))
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

    /// The account's ordered sending identities. `account` is the account key
    /// (its own address). The default is the single identity `[account]` with
    /// no display name — correct for Gmail and any account that sends as
    /// exactly one address — and deliberately makes NO provider call:
    /// `sender_identity()` is a network request for Gmail, and queueing,
    /// scheduling and reply-draft creation must not start needing one. The IMAP
    /// provider overrides this from its stored identity list
    /// (`docs/imap-design.md`, "Identities"): login first, then the addresses
    /// the user added, normalized and validated. The FIRST entry is always
    /// the account's primary (login) address, so callers may rely on
    /// `identities(account)[0]` being that address.
    async fn identities(&self, account: &str) -> ProviderResult<Vec<SendingIdentity>> {
        Ok(vec![SendingIdentity {
            address: account.trim().to_string(),
            display_name: None,
        }])
    }

    /// The domain to put in a generated `Message-ID` for a send from
    /// `from_address`, or `None` to keep the app's reserved
    /// `threestrands.local` domain.
    ///
    /// The default is `None`: Gmail (and every account that has not opted in)
    /// keeps `<{id}@threestrands.local>`, so `find_sent_copy`'s exact
    /// Message-ID match is unchanged. The IMAP provider overrides this to
    /// return the domain of the From address actually used
    /// (`docs/imap-design.md`, "Sending" > "Message-ID domain"), so an IMAP
    /// send is `<{id}@{sender-domain}>`. An invalid domain is an `Err` BEFORE
    /// any send — never a silent fallback for IMAP, which would leak the
    /// client name. This is the ONLY provider difference in the Message-ID; no
    /// code path branches on a provider name or id.
    fn message_id_domain(&self, _from_address: &str) -> ProviderResult<Option<String>> {
        Ok(None)
    }

    /// Authorizes one delivery. See [`Delivery`] for why this is separate.
    async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>>;

    /// Looks for an already-sent copy of `operation`, to resolve a delivery
    /// whose outcome was left unknown by a crash or a dropped connection.
    async fn find_sent_copy(
        &self,
        operation: &str,
        expected_sender: &str,
    ) -> ProviderResult<Option<DeliveryReceipt>>;

    /// Saves the sender's own copy of a just-sent message into the account's
    /// Sent mailbox, or decides nothing is needed, driven by the engine's
    /// durable Sent-copy job (`docs/imap-design.md`, "Sending" steps 3–4).
    ///
    /// The default does NOTHING and returns [`SentCopyStep::Done`]: Gmail (and
    /// any provider whose [`ProviderCapabilities::client_saves_sent_copy`] is
    /// `false`) stores the sent message itself during submission, so the engine
    /// never creates a job for it and never calls this. The IMAP provider
    /// overrides it to run the first-send detection + idempotent APPEND. A
    /// provider that returns `Recheck` is called again after the delay in the
    /// `verify` phase. No code path branches on a provider name or id — the
    /// capability and the request's `phase`/`append_attempted` carry everything.
    async fn save_sent_copy(&self, _request: &SentCopyRequest) -> ProviderResult<SentCopyStep> {
        Ok(SentCopyStep::Done)
    }
}

/// A complete mail account backend.
pub trait MailProvider: MailSync + MailFetch + MailMutate + MailSend {
    fn capabilities(&self) -> ProviderCapabilities;
}

#[cfg(test)]
mod sending_identity_default_tests {
    use super::*;

    /// A provider whose `sender_identity` is a network round trip that must
    /// never be needed just to learn the account's own address.
    struct OfflineProvider;

    #[async_trait]
    impl MailSend for OfflineProvider {
        async fn sender_identity(&self) -> ProviderResult<String> {
            panic!("identities() must not call sender_identity(): that is a network request for Gmail");
        }
        async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
            unreachable!()
        }
        async fn find_sent_copy(&self, _: &str, _: &str) -> ProviderResult<Option<DeliveryReceipt>> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn default_identities_is_the_account_address_without_any_provider_call() {
        let got = OfflineProvider.identities("me@example.com").await.unwrap();
        assert_eq!(
            got,
            vec![SendingIdentity { address: "me@example.com".into(), display_name: None }]
        );
    }
}


/// Phase 5 run 5: the provider-neutral `SearchReport` combining rule for a
/// unified search over several accounts, and the `search_with_report` DEFAULT
/// (a provider that cannot describe partial/skip outcomes always reports
/// `Complete` and stays byte-identical to its `search`).
#[cfg(test)]
mod search_report_tests {
    use super::*;

    fn partial(reason: PartialSearchReason) -> SearchReportStatus {
        SearchReportStatus::Partial {
            reasons: vec![reason],
        }
    }

    #[test]
    fn complete_combined_with_complete_is_complete() {
        let got = SearchReport::complete().combine(SearchReport::complete());
        assert_eq!(got.status, SearchReportStatus::Complete);
        assert_eq!(got.unsynced_mailboxes, 0);
        assert_eq!(got.skipped_over_cap, 0);
    }

    #[test]
    fn counts_sum_across_accounts() {
        let a = SearchReport {
            status: SearchReportStatus::Complete,
            unsynced_mailboxes: 2,
            skipped_over_cap: 3,
        };
        let b = SearchReport {
            status: SearchReportStatus::Complete,
            unsynced_mailboxes: 1,
            skipped_over_cap: 4,
        };
        let got = a.combine(b);
        assert_eq!(got.unsynced_mailboxes, 3);
        assert_eq!(got.skipped_over_cap, 7);
    }

    #[test]
    fn a_partial_account_is_not_masked_by_a_complete_one() {
        let complete = SearchReport::complete();
        let partial_report = SearchReport {
            status: partial(PartialSearchReason::Deadline),
            unsynced_mailboxes: 0,
            skipped_over_cap: 0,
        };
        // Either fold order yields the partial status.
        assert_eq!(
            complete.clone().combine(partial_report.clone()).status,
            partial(PartialSearchReason::Deadline)
        );
        assert_eq!(
            partial_report.combine(complete).status,
            partial(PartialSearchReason::Deadline)
        );
    }

    #[test]
    fn skipped_outranks_everything() {
        let skipped = SearchReport {
            status: SearchReportStatus::Skipped {
                reasons: vec![SkipSearchReason::Unsupported],
            },
            unsynced_mailboxes: 0,
            skipped_over_cap: 0,
        };
        for other in [
            SearchReportStatus::Complete,
            partial(PartialSearchReason::Budget),
            SearchReportStatus::NoServerMatches {
                reason: NoMatchSearchReason::UnknownName,
            },
        ] {
            let combined = skipped.clone().combine(SearchReport {
                status: other,
                unsynced_mailboxes: 0,
                skipped_over_cap: 0,
            });
            assert_eq!(
                combined.status,
                SearchReportStatus::Skipped {
                    reasons: vec![SkipSearchReason::Unsupported]
                }
            );
        }
    }

    #[test]
    fn no_server_matches_outranks_partial_but_not_skipped() {
        let no_match = SearchReportStatus::NoServerMatches {
            reason: NoMatchSearchReason::UnknownName,
        };
        let a = SearchReport {
            status: no_match.clone(),
            unsynced_mailboxes: 0,
            skipped_over_cap: 0,
        };
        let b = SearchReport {
            status: partial(PartialSearchReason::Deadline),
            unsynced_mailboxes: 0,
            skipped_over_cap: 0,
        };
        assert_eq!(a.combine(b).status, no_match);
    }

    #[test]
    fn two_partial_accounts_concatenate_their_reasons() {
        let a = SearchReport {
            status: partial(PartialSearchReason::Deadline),
            unsynced_mailboxes: 0,
            skipped_over_cap: 0,
        };
        let b = SearchReport {
            status: partial(PartialSearchReason::Budget),
            unsynced_mailboxes: 0,
            skipped_over_cap: 0,
        };
        assert_eq!(
            a.combine(b).status,
            SearchReportStatus::Partial {
                reasons: vec![PartialSearchReason::Deadline, PartialSearchReason::Budget],
            }
        );
    }

    /// A provider that only implements `search` (never overriding
    /// `search_with_report`) inherits the default: the page is byte-identical
    /// to its `search`, and the report is always `Complete`. This is the Gmail
    /// contract (its 50-thread scan limit stays unreported).
    struct DefaultSearchProvider {
        ids: Vec<String>,
    }

    #[async_trait]
    impl MailSync for DefaultSearchProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            unreachable!()
        }
        async fn poll(&self, _cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
            unreachable!()
        }
        async fn list_inbox(&self, _page: Option<&str>) -> ProviderResult<ThreadPage> {
            unreachable!()
        }
        async fn fetch_thread(&self, _id: &str) -> ProviderResult<Vec<RawMessage>> {
            unreachable!()
        }
        async fn search(&self, _query: &str, _page: Option<&str>) -> ProviderResult<ThreadPage> {
            Ok(ThreadPage {
                thread_ids: self.ids.clone(),
                next: None,
            })
        }
    }

    #[tokio::test]
    async fn the_default_report_is_complete_and_the_page_is_unchanged() {
        let provider = DefaultSearchProvider {
            ids: vec!["t1".into(), "t2".into()],
        };
        let (page, report) = provider.search_with_report("anything", None).await.unwrap();
        let plain = provider.search("anything", None).await.unwrap();
        assert_eq!(page.thread_ids, plain.thread_ids);
        assert_eq!(page.thread_ids, vec!["t1".to_string(), "t2".to_string()]);
        assert_eq!(report, SearchReport::complete());
    }

    /// A provider error from the default path still propagates as `Err`,
    /// exactly as `search` — the UI's "unavailable" state is unchanged.
    struct ErroringProvider;

    #[async_trait]
    impl MailSync for ErroringProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            unreachable!()
        }
        async fn poll(&self, _cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
            unreachable!()
        }
        async fn list_inbox(&self, _page: Option<&str>) -> ProviderResult<ThreadPage> {
            unreachable!()
        }
        async fn fetch_thread(&self, _id: &str) -> ProviderResult<Vec<RawMessage>> {
            unreachable!()
        }
        async fn search(&self, _query: &str, _page: Option<&str>) -> ProviderResult<ThreadPage> {
            Err(ProviderError::TransientTransport("offline".into()))
        }
    }

    #[tokio::test]
    async fn a_provider_error_still_propagates_from_the_report_path() {
        let got = ErroringProvider.search_with_report("q", None).await;
        assert!(matches!(got, Err(ProviderError::TransientTransport(_))));
    }
}
