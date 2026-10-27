//! Every numeric limit the IMAP body path enforces, in ONE place.
//!
//! `docs/imap-design.md` ("What gets synced" / "Bodies") and `AGENTS.md`
//! ("Email rendering") both require that numeric limits live in a single
//! policy module with below/at/above tests, following the frontend's
//! `src/emailRenderingPolicy.ts`. This is the Rust-side analogue for the IMAP
//! provider: the body cache's size ceiling, and the MIME-tree shape limits the
//! `rfc822` parser enforces before it will build a [`RawMessage`].
//!
//! The rule from `AGENTS.md` is "never silently clamp". Each limit here is
//! enforced by a helper that returns a typed [`ProviderError`] when the input
//! is over the limit, rather than truncating it. An over-limit message is
//! rejected whole; it is never half-ingested.
//!
//! The MIME-shape limits are deliberately independent of `mime.rs`'s own
//! `validate_mime_structure` limits (which guard the shared normalize path for
//! every provider). These are the IMAP ingest gate, applied to raw RFC 5322
//! bytes BEFORE a `RawMessage` is built. They are set no looser than the
//! shared ones so a message this gate accepts always survives `normalize`:
//! `MAX_MIME_PART_COUNT` (4096) is wider than `mime::MAX_MIME_PARTS` (1000),
//! which means an IMAP tree between those two bounds is accepted here and then
//! rejected by `normalize` with its own typed error — rejection either way,
//! never a silent clamp. `MAX_MIME_DEPTH` matches the shared limit exactly.

use crate::provider::ProviderError;

/// Largest raw message, in bytes, accepted into the on-disk body cache.
///
/// A `BODY.PEEK[]` fetch of a message bigger than this is refused rather than
/// cached: 64 MiB comfortably exceeds any ordinary mail (RFC 822 size plus
/// attachments) while bounding how large a single cache row — and the memory
/// held while writing it — can grow. The shared normalize path has its own,
/// smaller decoded-body and attachment ceilings; this is the gate on the raw
/// bytes before either runs.
pub const MAX_RAW_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Ask for one extra byte so an unknown or understated RFC822.SIZE cannot
/// cause us to cache a silently truncated message at the acceptance boundary.
pub const MAX_RAW_FETCH_BYTES: usize = MAX_RAW_MESSAGE_BYTES + 1;

/// Deepest MIME nesting the `rfc822` parser will build a tree for.
///
/// Matches `mime::MAX_MIME_DEPTH` so a tree this gate accepts is never then
/// rejected for depth by `normalize`. A `message/rfc822` part counts as one
/// level, so a mail-bomb of deeply nested forwarded messages is refused here.
pub const MAX_MIME_DEPTH: usize = 32;

/// Most MIME parts the `rfc822` parser will accept in one message.
///
/// A hostile message can declare thousands of empty parts to exhaust memory.
/// 4096 is generous for legitimate mail (even a large multipart/mixed with
/// many attachments and alternatives stays well under it) while bounding the
/// part vector. It is intentionally wider than `mime::MAX_MIME_PARTS` (1000):
/// the shared normalize path applies the tighter bound afterwards, so a tree
/// between the two is still rejected — by `normalize`, with its own error —
/// never silently clamped.
pub const MAX_MIME_PART_COUNT: usize = 4096;

/// Reject a raw message that is too large to cache. Returns the byte count
/// unchanged when it is within the limit (including exactly at it), so callers
/// read "ok, this many bytes" from the success value.
pub fn check_raw_message_bytes(len: usize) -> Result<usize, ProviderError> {
    if len > MAX_RAW_MESSAGE_BYTES {
        return Err(ProviderError::PermanentClientRejection(format!(
            "message is {len} bytes, over the {MAX_RAW_MESSAGE_BYTES}-byte ({} MiB) body-cache limit",
            MAX_RAW_MESSAGE_BYTES / 1024 / 1024
        )));
    }
    Ok(len)
}

/// Reject a MIME tree nested deeper than [`MAX_MIME_DEPTH`]. `depth` is the
/// deepest level reached (root = 1).
pub fn check_mime_depth(depth: usize) -> Result<usize, ProviderError> {
    if depth > MAX_MIME_DEPTH {
        return Err(ProviderError::PermanentClientRejection(format!(
            "MIME nesting is {depth} levels deep, over the {MAX_MIME_DEPTH}-level limit"
        )));
    }
    Ok(depth)
}

/// Reject a MIME tree with more than [`MAX_MIME_PART_COUNT`] parts.
pub fn check_mime_part_count(count: usize) -> Result<usize, ProviderError> {
    if count > MAX_MIME_PART_COUNT {
        return Err(ProviderError::PermanentClientRejection(format!(
            "message declares {count} MIME parts, over the {MAX_MIME_PART_COUNT}-part limit"
        )));
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Sync window (docs/imap-design.md, "What gets synced" > "Initial window")
// ---------------------------------------------------------------------------
//
// Unlike the ingest gates above, a sync window is a *selection*, not a
// rejection: a mailbox with more mail than its window still syncs, but only
// its newest messages are brought in. The "never silently clamp" rule still
// holds, because [`select_window`] reports exactly how many messages fell
// outside the window instead of dropping them without a trace. Callers must
// carry `beyond_window` through to wherever sync progress is recorded.
//
// "Newest" means highest UID. Within one mailbox and `UIDVALIDITY`, UIDs are
// strictly increasing in arrival order (RFC 3501 section 2.3.1.1), so this
// needs no `INTERNALDATE` fetch and gives the same answer on every server.
//
// There is no age cutoff: the design decided against one.
//
// These items have no non-test caller until Slice 5 wires incremental sync,
// so each carries a scoped `dead_code` allow, removed with that first caller.

/// A ceiling on INBOX, not a normal window. The design syncs all of INBOX
/// because it is a to-do list and a cutoff would hide old mail that still
/// needs handling; "a count limit applies only to inboxes too large to sync in
/// full". The owner chose 5,000, the same figure as Gmail's Sent backfill
/// (`sync::MAX_SENT_BACKFILL_THREADS`), and a test below keeps them equal.
/// Note Gmail itself has no INBOX cap, so this is stricter than Gmail's inbox
/// sync. Reaching it is reported through [`WindowSelection::beyond_window`],
/// never hidden.
pub const MAX_INBOX_SYNC_MESSAGES: usize = 5_000;

/// Newest messages kept in sync from the `\Sent` mailbox. Matches the Gmail
/// limit (`sync::MAX_SENT_BACKFILL_THREADS`) so the address book, contact
/// timelines and Keep in Touch see the same depth of history on both
/// providers; a test below fails if the two drift apart.
pub const SENT_SYNC_WINDOW: usize = 5_000;

/// Newest messages whose headers are kept in sync from every other synced
/// mailbox: Archive, Junk, Trash, user folders and label folders.
pub const FOLDER_SYNC_WINDOW: usize = 2_000;

/// Which sync window applies to a mailbox. Deciding which class a given
/// mailbox belongs to needs the account's role mapping and each mailbox's
/// `LIST` attributes, so that lives with the sync code; this module owns only
/// the numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncWindowClass {
    /// INBOX: everything, up to [`MAX_INBOX_SYNC_MESSAGES`].
    Inbox,
    /// The `\Sent` mailbox: [`SENT_SYNC_WINDOW`].
    Sent,
    /// Archive, Junk, Trash, user folders and label folders:
    /// [`FOLDER_SYNC_WINDOW`].
    Folder,
    /// Never synced: Drafts (read as server drafts instead), `\All` and `\Flagged`
    /// aggregates, and `\Noselect` containers. The window is zero, so nothing
    /// is in it.
    NotSynced,
}

impl SyncWindowClass {
    /// The most messages this class keeps in sync.
    pub const fn limit(self) -> usize {
        match self {
            Self::Inbox => MAX_INBOX_SYNC_MESSAGES,
            Self::Sent => SENT_SYNC_WINDOW,
            Self::Folder => FOLDER_SYNC_WINDOW,
            Self::NotSynced => 0,
        }
    }

    /// Whether a UID that ages out of this class's window becomes a DELETION.
    ///
    /// INBOX and Folder (Archive/Junk/Trash/user folders) are sliding eviction
    /// windows: when newer mail pushes an old UID past the window it is
    /// reported as a deletion, keeping the local store bounded to the newest
    /// mail (the owner's c77fa3f behaviour).
    ///
    /// **Sent is false.** Its 5,000 is a one-time ACQUISITION bound mirroring
    /// Gmail's Sent backfill scan, not a sliding window: once a Sent message is
    /// synced it is never evicted when newer sent mail pushes it past 5,000, so
    /// the address book never loses history. Deletions in Sent come only from
    /// UIDs the server itself no longer has. (Run 3 syncs Sent in chunked,
    /// non-evicting background rounds; see `provider/refresh.rs`.)
    ///
    /// `NotSynced` is not applicable — nothing is ever in its window.
    pub const fn evicts_beyond_window(self) -> bool {
        match self {
            Self::Inbox => true,
            Self::Folder => true,
            Self::Sent => false,
            Self::NotSynced => false,
        }
    }
}

/// The outcome of applying a sync window to a mailbox's UIDs.
#[derive(Debug, PartialEq, Eq)]
pub struct WindowSelection {
    /// The newest UIDs, at most the window's limit, in ascending order.
    pub in_window: Vec<u32>,
    /// How many older UIDs fell outside the window. Zero when the whole
    /// mailbox fits. Never discarded silently: callers record it.
    pub beyond_window: usize,
}

/// Keeps the newest `limit` UIDs. Input order does not matter and duplicate
/// UIDs collapse, so a caller can pass a raw `UID SEARCH` result directly.
/// Exactly `limit` messages fit; one more pushes the oldest one beyond the
/// window.
pub fn select_window(mut uids: Vec<u32>, limit: usize) -> WindowSelection {
    uids.sort_unstable();
    uids.dedup();
    let beyond_window = uids.len().saturating_sub(limit);
    let in_window = uids.split_off(beyond_window);
    WindowSelection {
        in_window,
        beyond_window,
    }
}

// ---------------------------------------------------------------------------
// Slice 5a: threading, journal retention, and sync batching limits
// ---------------------------------------------------------------------------
//
// Three more numeric limits the live-sync path enforces, each with below/
// exact/above coverage below. Like the window above, none is a silent clamp:
// a `References` chain over the cap is handled by a documented, typed rule
// (keep the newest ids, drop the oldest with a count), and the journal
// retention bound is a comparison that produces a typed `InvalidCursor`, never
// a truncation. These have no non-test caller until the sync routine and the
// provider wire them, so each carries a scoped `dead_code` allow removed with
// that first caller — the same convention as the window items above.

/// The most reference ids [`clamp_references`] keeps from one message's
/// `References`/`In-Reply-To` chain when building threads.
///
/// A hostile message can declare a `References` header with tens of thousands
/// of ids to force quadratic thread-merge work. Threading only needs a
/// message's nearest ancestors to place it, so the chain is capped. Over the
/// cap is NOT silent truncation: [`clamp_references`] keeps the newest
/// (right-most, nearest-ancestor) ids — the ones that actually determine
/// placement — drops the oldest, and reports how many it dropped, so the
/// caller records that the chain was shortened rather than losing it without
/// a trace. 64 is far more ancestry than any real conversation carries.
pub const MAX_REFERENCES: usize = 64;

/// How many sync generations of the change journal are retained.
///
/// `poll(cursor=g)` must return every thread changed since generation `g`. A
/// cursor older than the oldest retained generation can no longer be answered
/// completely, so it is rejected with `InvalidCursor` and the engine
/// full-syncs (`docs/imap-design.md`, at-least-once cursor semantics). This
/// bounds how far back the journal is kept: a generation more than this many
/// rounds behind the current one is prunable, and a cursor pointing at a
/// pruned generation is stale. Generous enough that an app offline for a long
/// time still resumes incrementally in the common case.
pub const JOURNAL_RETENTION_GENERATIONS: u64 = 1_000;

/// How many UIDs are packed into one `UID FETCH`/`UID SEARCH` set per round
/// trip. Bounds the size of a single command line and the memory held for one
/// batch's responses, without issuing a request per message. Only the batch
/// SIZE lives here; how the sync routine walks the batches is its own concern.
pub const UID_BATCH_SIZE: usize = 500;

// ---------------------------------------------------------------------------
// Slice 5b-1: non-INBOX mailbox cadence
// ---------------------------------------------------------------------------
//
// INBOX is swept on every poll (it is the to-do list). A non-INBOX synced
// mailbox (Trash, Junk) is cheaper: its EXAMINE result (EXISTS + UIDNEXT) is
// stored in `imap_mailbox_sync_state`, and the UID SEARCH/FETCH is SKIPPED when
// both are unchanged AND the periodic full sweep is not yet due. A flag change
// by another client moves neither EXISTS nor UIDNEXT, so the periodic sweep is
// what eventually catches it. Each limit below has below/exact/above coverage.

/// How long (seconds) between periodic full sweeps of a non-INBOX synced
/// mailbox. Between sweeps an unchanged EXAMINE (same EXISTS + UIDNEXT) skips
/// the SEARCH/FETCH entirely; a sweep is forced at least this often so a flag
/// change another client made (which moves neither counter) is still picked
/// up. 15 minutes balances freshness against the per-poll cost of a full
/// windowed flag sweep on a large folder.
pub const FOLDER_SWEEP_INTERVAL_SECS: i64 = 15 * 60;

/// The most non-INBOX synced mailboxes whose rounds one poll will run. INBOX
/// is always run first and does not count against this. Bounds how much work a
/// single poll does when several folders changed at once; the rest are picked
/// up on the next poll. Run 2 syncs only Trash + Junk, well under this, but the
/// bound is in place for 5b-2's larger mailbox set.
pub const FOLDER_ROUNDS_PER_POLL: usize = 8;

/// Whether a mailbox is due for its periodic full sweep given the last sweep
/// time and now (both unix seconds). A mailbox never swept (`last_sweep_at`
/// 0 / absent) is always due. Exactly at the interval boundary is due.
pub fn folder_sweep_due(last_sweep_at: i64, now: i64) -> bool {
    now.saturating_sub(last_sweep_at) >= FOLDER_SWEEP_INTERVAL_SECS
}

// ---------------------------------------------------------------------------
// Slice 5b-2: fair folder scheduling, the synced-mailbox cap, and the
// emptied-hot-thread grace.
// ---------------------------------------------------------------------------
//
// Slice 5b-1 walked `plan.synced().filter(!inbox).take(FOLDER_ROUNDS_PER_POLL)`
// — a fixed PREFIX. With many folders/labels the later ones never sync, and a
// move's destination mailbox may never be visited in the poll that sees the
// INBOX location disappear. 5b-2 replaces that prefix with a fair schedule:
// INBOX first, then ALL synced SYSTEM-ROLE mailboxes every poll (bounded,
// cheap when idle via the cadence gate), then user/label folders chosen
// LEAST-RECENTLY-ATTEMPTED first up to the remaining budget. The selection is a
// pure function so it is table-testable. None of these is a silent clamp: the
// cap logs the count dropped, and the schedule reports exactly which mailboxes
// it chose.

/// The most SYNCED mailboxes one account keeps in its plan. Over this cap, the
/// account syncs system roles first then a deterministic by-name prefix of the
/// rest, and logs how many it dropped (never silently). 500 is far more than
/// any ordinary account's folder/label count while bounding per-poll planning
/// and the `imap_mailbox_sync_state` row count. Below/exact/above tested.
pub const MAX_SYNCED_MAILBOXES: usize = 500;

/// How many completed COVERAGE EPOCHS a hot thread with zero locations must
/// survive before `fetch_thread` reports it `NotFound` (and the engine
/// deletes the local copy). Until then `fetch_thread` returns `Ok(empty)` and
/// the engine leaves the local thread — tasks and labels — untouched. This is
/// the grace that absorbs the common case: another client moved a hot message
/// out of INBOX into a mailbox not swept that poll, so the thread momentarily
/// has no locations but regains one as soon as the destination is visited. A
/// coverage epoch = every currently-synced mailbox visited across one or more
/// polls, so two epochs is ample for the destination to be reached even when
/// the per-poll budget cannot cover every folder at once. Below/exact/above
/// tested.
pub const EMPTIED_THREAD_GRACE_WALKS: u64 = 2;

/// Whether an emptied hot thread's grace has EXPIRED: it was first seen empty
/// at completed-epoch count `emptied_at_walk`, and `complete_walks` completed
/// coverage epochs have now elapsed. Grace expires once at least
/// [`EMPTIED_THREAD_GRACE_WALKS`] completed epochs have passed SINCE it was
/// first observed empty. Exactly at the boundary the grace is spent (the
/// thread is deleted); one epoch short, it still survives.
pub fn emptied_grace_expired(emptied_at_walk: u64, complete_walks: u64) -> bool {
    complete_walks.saturating_sub(emptied_at_walk) >= EMPTIED_THREAD_GRACE_WALKS
}

/// One candidate mailbox, with durable attempt order and successful coverage.
/// INBOX is always visited first, outside this schedule and its budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleCandidate {
    pub mailbox: String,
    /// Durable logical order of the last attempt, successful or failed.
    pub last_attempt: i64,
    pub last_visited_at: i64,
    pub visited_in_epoch: i64,
    pub is_system_role: bool,
}

/// Choose which non-INBOX mailboxes to VISIT this poll, in visit order.
///
/// System roles are always visited first. User/label folders are ordered by
/// their last attempt's logical sequence, including failed attempts. This
/// rotates every folder within ceil(N/budget) polls even with a frozen clock
/// or permanently failing peers. Ties (including pre-upgrade rows at zero)
/// prefer uncovered folders, then older successful visits, then name.
/// Successful coverage remains separate: a failed attempt never advances the
/// epoch or spends deletion grace. System roles are included even at budget 0;
/// the budget limits the user/label-folder visits after them.
pub fn schedule_visits(
    candidates: &[ScheduleCandidate],
    current_epoch: i64,
    budget: usize,
) -> Vec<String> {
    let mut system: Vec<&ScheduleCandidate> =
        candidates.iter().filter(|c| c.is_system_role).collect();
    system.sort_by(|a, b| a.mailbox.cmp(&b.mailbox));

    let mut folders: Vec<&ScheduleCandidate> =
        candidates.iter().filter(|c| !c.is_system_role).collect();
    // Rotate attempts independently of coverage so failed folders cannot
    // monopolize the budget while keeping the epoch open.
    folders.sort_by(|a, b| {
        let a_covered = a.visited_in_epoch >= current_epoch;
        let b_covered = b.visited_in_epoch >= current_epoch;
        a.last_attempt
            .cmp(&b.last_attempt)
            .then_with(|| a_covered.cmp(&b_covered))
            .then_with(|| a.last_visited_at.cmp(&b.last_visited_at))
            .then_with(|| a.mailbox.cmp(&b.mailbox))
    });

    let mut out: Vec<String> = system.into_iter().map(|c| c.mailbox.clone()).collect();
    out.extend(folders.into_iter().take(budget).map(|c| c.mailbox.clone()));
    out
}

/// Apply [`MAX_SYNCED_MAILBOXES`] to a planned synced-mailbox set, keeping
/// SYSTEM ROLES first (never dropped) then a deterministic by-name prefix of
/// the rest, and reporting how many were dropped so the caller can LOG it
/// rather than silently truncating. Below/exact at the cap keeps everything;
/// above the cap drops the by-name tail of the non-system folders. Returns
/// `(kept_in_a_stable_order, dropped_count)`.
pub fn cap_synced_mailboxes(
    candidates: &[ScheduleCandidate],
    cap: usize,
) -> (Vec<String>, usize) {
    if candidates.len() <= cap {
        // Still return a deterministic order (system roles by name, then the
        // rest by name) so the caller's accounting is stable.
        let mut system: Vec<&ScheduleCandidate> =
            candidates.iter().filter(|c| c.is_system_role).collect();
        system.sort_by(|a, b| a.mailbox.cmp(&b.mailbox));
        let mut rest: Vec<&ScheduleCandidate> =
            candidates.iter().filter(|c| !c.is_system_role).collect();
        rest.sort_by(|a, b| a.mailbox.cmp(&b.mailbox));
        let kept = system
            .into_iter()
            .chain(rest)
            .map(|c| c.mailbox.clone())
            .collect();
        return (kept, 0);
    }
    let mut system: Vec<&ScheduleCandidate> =
        candidates.iter().filter(|c| c.is_system_role).collect();
    system.sort_by(|a, b| a.mailbox.cmp(&b.mailbox));
    let mut rest: Vec<&ScheduleCandidate> =
        candidates.iter().filter(|c| !c.is_system_role).collect();
    rest.sort_by(|a, b| a.mailbox.cmp(&b.mailbox));

    // System roles are never dropped (they are a bounded set); the cap applies
    // to the remaining folders. If the cap is smaller than the system set, keep
    // every system role anyway (correctness over the cap for the bounded set).
    let room_for_folders = cap.saturating_sub(system.len());
    let kept_folders = rest.len().min(room_for_folders);
    let dropped = rest.len() - kept_folders;
    let kept: Vec<String> = system
        .into_iter()
        .map(|c| c.mailbox.clone())
        .chain(rest.into_iter().take(kept_folders).map(|c| c.mailbox.clone()))
        .collect();
    (kept, dropped)
}

// ---------------------------------------------------------------------------
// Slice 5b-1 run 3: Sent chunked acquisition
// ---------------------------------------------------------------------------
//
// An initial sync of a 5,000-message Sent folder must NOT be one atomic round
// that fetches 5,000 bodies. After INBOX is swept in the same walk, each poll's
// Sent round ACQUIRES AT MOST [`SENT_ROUND_MESSAGES`] of the still-unacquired
// UIDs inside the Sent window, NEWEST FIRST. Flag changes and deletions for
// already-acquired Sent messages are not budgeted. Each chunk is one atomic
// round (one `commit_sync_round` transaction), so an interrupted round loses
// nothing and re-acquires nothing (progress is derived from local locations).

/// The most still-unacquired Sent UIDs one poll's Sent round pulls bodies for.
///
/// Mirrors `sync.rs`'s Gmail Sent backfill cadence
/// (`SENT_BACKFILL_FETCHES_PER_STEP` × `SENT_BACKFILL_STEPS_PER_POLL` =
/// 25 × 4 = 100), so IMAP and Gmail fill their Sent history at the same pace.
/// Those two constants are private to `sync.rs` (the brief forbids editing it),
/// so this mirrors their PRODUCT as a documented literal; `the_sent_round_
/// budget_mirrors_the_gmail_cadence` below pins the arithmetic (25 × 4 == 100)
/// so a future reader cannot silently drift this away from that intent.
pub const SENT_ROUND_MESSAGES: usize = 100;

/// Whether a mailbox's Sent backfill is still INCOMPLETE, given the stored
/// `backfill_low_uid` marker. The marker holds the lowest UID acquired so far
/// while the backfill is in progress and is NULL once the whole Sent window has
/// been acquired. So `Some(_)` means pending (cadence gating must NOT skip the
/// mailbox even when EXISTS/UIDNEXT are unchanged), and `None` means complete
/// (normal cadence resumes). A mailbox that has never been swept has no marker
/// row at all; the first sweep establishes it.
pub fn sent_backfill_incomplete(backfill_low_uid: Option<i64>) -> bool {
    backfill_low_uid.is_some()
}

/// Whether a non-INBOX mailbox's cheap-cadence check says its UIDs are
/// UNCHANGED since the last round: the server's EXISTS and UIDNEXT both match
/// what was stored. When unchanged AND not sweep-due, the round skips the UID
/// SEARCH/FETCH. New mail bumps UIDNEXT; an expunge changes EXISTS; a flag
/// change by another client moves neither (hence the periodic sweep).
pub fn folder_counters_unchanged(
    stored_exists: i64,
    stored_uidnext: i64,
    server_exists: i64,
    server_uidnext: i64,
) -> bool {
    stored_exists == server_exists && stored_uidnext == server_uidnext
}

/// Threading tokens bound per SQL statement when seeding the threader. Well
/// below SQLite's bound-variable limit (999 on the oldest builds, 32,766 on
/// the bundled one), so an initial sync that threads thousands of messages in
/// one round queries the token set in chunks instead of failing the round.
pub const TOKEN_QUERY_CHUNK: usize = 500;

/// How many token-less messages the one-time upgrade backfill
/// (`ImapProvider::ensure_tokens_backfilled`) processes per bounded batch
/// before reading the next slice of the work queue.
///
/// The backfill parses one cached body per message to reconstruct its token
/// set, so an unbounded sweep of a large mailbox would hold the whole queue
/// and parse every body in one go. Draining in bounded batches keeps per-step
/// memory and the per-transaction cost bounded and makes the sweep
/// crash-resumable: each message's tokens commit in their own transaction, so
/// a crash resumes from the messages still missing tokens. 500 matches the UID
/// batch size — a round number generous for a step without being unbounded.
pub const TOKEN_BACKFILL_BATCH_SIZE: usize = 500;

/// The outcome of applying [`MAX_REFERENCES`] to a reference chain: the ids
/// kept (newest first-to-last preserved in input order) and how many older
/// ids were dropped.
#[derive(Debug, PartialEq, Eq)]
pub struct ReferenceSelection {
    /// The retained reference ids, at most [`MAX_REFERENCES`], in their
    /// original order (oldest ancestor to nearest).
    pub kept: Vec<String>,
    /// How many older ids were dropped. Zero when the chain fits. Never
    /// discarded silently: the caller records it.
    pub dropped: usize,
}

/// Keep the newest [`MAX_REFERENCES`] reference ids. `References` lists
/// ancestors oldest-first, so the nearest ancestors — the ones that decide
/// where a reply threads — are at the end; when the chain is over the cap the
/// OLDEST (left-most) ids are the ones dropped. Order among the kept ids is
/// preserved. Exactly `MAX_REFERENCES` fit; one more drops the single oldest.
pub fn clamp_references(references: Vec<String>) -> ReferenceSelection {
    let dropped = references.len().saturating_sub(MAX_REFERENCES);
    let kept = references.into_iter().skip(dropped).collect();
    ReferenceSelection { kept, dropped }
}

/// Whether a polled generation `cursor` can still be answered from a journal
/// whose current generation is `current`. A cursor newer than `current` (a
/// store that was reset behind the engine), or older than
/// `current - JOURNAL_RETENTION_GENERATIONS` (pruned), cannot, and the caller
/// returns `InvalidCursor`. Exactly at the retention boundary is still
/// answerable.
pub fn journal_cursor_is_answerable(cursor: u64, current: u64) -> bool {
    if cursor > current {
        return false;
    }
    current.saturating_sub(cursor) <= JOURNAL_RETENTION_GENERATIONS
}

/// The window and batch limits ONE sync round runs under, passed in so the
/// delta and sync code hold no inline numbers and tests can inject tiny values
/// instead of 5000-message fixtures (the brief's "pass limits in" rule). The
/// [`Default`] impl is the production configuration: the INBOX ceiling and the
/// standard UID batch size. A test overrides `mailbox_window` with a small
/// number to exercise the window boundary cheaply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncLimits {
    /// The window ceiling applied to a mailbox's newest UIDs before computing the delta.
    pub mailbox_window: usize,
    /// Whether a UID aged out of the window becomes a deletion (true for
    /// INBOX/Folder sliding windows; false for Sent's acquisition bound). See
    /// [`SyncWindowClass::evicts_beyond_window`].
    pub evicts: bool,
}

impl SyncLimits {
    /// INBOX limits: the INBOX ceiling, sliding (evicts).
    pub const fn inbox() -> Self {
        Self {
            mailbox_window: MAX_INBOX_SYNC_MESSAGES,
            evicts: true,
        }
    }

    /// The limits for a [`SyncWindowClass`]. `NotSynced` yields a zero window
    /// that evicts nothing (nothing is ever in it).
    pub const fn for_class(class: SyncWindowClass) -> Self {
        Self {
            mailbox_window: class.limit(),
            evicts: class.evicts_beyond_window(),
        }
    }
}

impl Default for SyncLimits {
    fn default() -> Self {
        Self::inbox()
    }
}

// ---------------------------------------------------------------------------
// Phase 3 run 1: write-command construction limits
// ---------------------------------------------------------------------------
//
// The write layer (`write.rs`) builds every mutating IMAP command. Three
// numeric limits bound what one command may carry, each with below/exact/above
// coverage below and each enforced by a helper that returns a typed
// `ProviderError` rather than truncating — the same "never silently clamp"
// rule the ingest gates above follow. These have their first non-test caller
// in Phase 3 run 2 (flags/moves/labels), so until then `write.rs`'s callers
// are tests; the limits themselves are plain data and need no `dead_code`
// allow.

/// Longest mailbox NAME, in bytes of its decoded (pre-UTF-7) form, that the
/// write layer will build a command for.
///
/// RFC 3501 does not fix a mailbox-name length, but a hostile or buggy caller
/// could hand us a megabyte-long name that bloats the command line and the
/// server's parse buffer. 1024 bytes is far longer than any real mailbox path
/// (even a deep `Labels/Clients/Acme/2026/Invoices` hierarchy) while bounding
/// one command. Measured on the DECODED name's UTF-8 length, before modified
/// UTF-7 expansion, so the limit is about the name the user sees, not its wire
/// spelling. Over the limit is a typed rejection — nothing is sent.
pub const MAX_MAILBOX_NAME_BYTES: usize = 1024;

/// Longest user LABEL name (the part the user types, relative to the label
/// container) accepted by `create_label`, measured in Unicode scalar values
/// (`chars`), not bytes, so a limit the user sees in characters is not spent
/// early by multi-byte letters. A label becomes a child mailbox
/// `<container><delim><name>`, so this is well under
/// [`MAX_MAILBOX_NAME_BYTES`] once the container prefix and modified-UTF-7
/// expansion are added. 128 characters is far longer than any real label
/// while bounding the created mailbox path. Over the limit is a typed,
/// permanent rejection — nothing is sent.
pub const MAX_LABEL_NAME_CHARS: usize = 128;

/// Most flags one `UID STORE` command may set or clear at once.
///
/// A STORE carries a parenthesised flag list; a caller asking to set hundreds
/// of distinct keywords in one command is never legitimate (ThreeStrands sets
/// at most a handful of system flags plus one label keyword). 64 is generous
/// for any real change while bounding the flag-list length. Over the limit is
/// a typed rejection, not a silent drop of the overflow.
pub const MAX_FLAGS_PER_COMMAND: usize = 64;

/// Most UIDs packed into one write command's UID set, after range
/// compression. This is the WRITE-side analogue of [`UID_BATCH_SIZE`] (which
/// bounds read fetches): a relocate or store over a larger set is split into
/// batches of at most this many UIDs. Set equal to [`UID_BATCH_SIZE`] so read
/// and write batch the same way; a test pins the two together.
pub const MAX_UIDS_PER_COMMAND: usize = UID_BATCH_SIZE;

/// Reject a mailbox name whose decoded UTF-8 length exceeds
/// [`MAX_MAILBOX_NAME_BYTES`]. Returns the length unchanged when within the
/// limit (including exactly at it).
pub fn check_mailbox_name_len(len: usize) -> Result<usize, ProviderError> {
    if len > MAX_MAILBOX_NAME_BYTES {
        return Err(ProviderError::InvalidOperation(format!(
            "mailbox name is {len} bytes, over the {MAX_MAILBOX_NAME_BYTES}-byte limit"
        )));
    }
    Ok(len)
}

/// Reject a label name whose character length exceeds
/// [`MAX_LABEL_NAME_CHARS`]. `chars` is the Unicode scalar count of the
/// trimmed label name. Returns the count unchanged when within the limit
/// (including exactly at it).
pub fn check_label_name_len(chars: usize) -> Result<usize, ProviderError> {
    if chars > MAX_LABEL_NAME_CHARS {
        return Err(ProviderError::InvalidOperation(format!(
            "label name is {chars} characters, over the {MAX_LABEL_NAME_CHARS}-character limit"
        )));
    }
    Ok(chars)
}

/// unchanged when within the limit (including exactly at it).
pub fn check_flags_per_command(count: usize) -> Result<usize, ProviderError> {
    if count > MAX_FLAGS_PER_COMMAND {
        return Err(ProviderError::InvalidOperation(format!(
            "{count} flags in one command, over the {MAX_FLAGS_PER_COMMAND}-flag limit"
        )));
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Phase 4 run 4a: the Sent-copy APPEND primitive
// ---------------------------------------------------------------------------
//
// `docs/imap-design.md` ("Sending", steps 3–4): after SMTP accepts a message,
// IMAP saves the sender's own copy in the Sent mailbox with `APPEND` (flag
// `\Seen`). These bound that one write, following the same "never silently
// clamp — a value over the limit is a typed, PERMANENT error and NOTHING is
// sent" rule as every limit above.

/// Largest raw message, in bytes, the Sent-copy `APPEND` will put on the wire.
///
/// Tied to [`MAX_RAW_MESSAGE_BYTES`] (the body-cache ingest ceiling): a Sent
/// copy we save is a message we will also sync back and cache, so refusing to
/// APPEND anything the cache would then refuse keeps the two gates consistent —
/// a message small enough to save is small enough to re-ingest. (It is set
/// EQUAL rather than independently so the two cannot drift; a test below pins
/// them together.) The sender's own raw bytes are built locally, so hitting
/// this is a pathological attachment, not hostile input — but an over-limit
/// APPEND is still refused with a typed PERMANENT error BEFORE any command or
/// literal reaches the wire, never truncated.
pub const APPEND_MAX_BYTES: usize = MAX_RAW_MESSAGE_BYTES;

/// Most `UID SEARCH HEADER Message-ID` hits the Sent-copy locator will VERIFY
/// (fetch-and-compare the Message-ID header of) before giving up.
///
/// `SEARCH HEADER` is a SUBSTRING match (RFC 3501 §6.4.4), so a short or
/// common Message-ID can match many messages; each candidate must then be
/// confirmed by fetching its own `Message-ID` header and comparing the
/// normalized token exactly. 64 is far more than the handful of copies one
/// Message-ID can legitimately produce (at most one real match, plus any
/// substring near-misses in a small mailbox), while bounding how many verify
/// FETCHes a pathological or hostile server can force. Over the limit is a
/// typed error — the locator refuses rather than fetching unboundedly.
pub const MAX_MESSAGE_ID_SEARCH_HITS: usize = 64;

/// How many times `save_sent_copy` re-runs the locate SEARCH after a
/// successful `APPEND` whose immediate search found nothing, before returning
/// `AppendedUnlocated`. A server may make a just-stored message visible to
/// SEARCH only after a short delay; a couple of bounded re-checks catches the
/// common case without hanging the send path. Injectable in tests (so they run
/// instantly with zero delay); this is the production default.
pub const SENT_COPY_RELOCATE_RECHECKS: usize = 2;

/// Reject a Sent-copy `APPEND` whose raw byte length exceeds
/// [`APPEND_MAX_BYTES`]. Returns the length unchanged when within the limit
/// (including exactly at it), so the caller reads "ok, this many bytes".
pub fn check_append_bytes(len: usize) -> Result<usize, ProviderError> {
    if len > APPEND_MAX_BYTES {
        return Err(ProviderError::PermanentClientRejection(format!(
            "message is {len} bytes, over the {APPEND_MAX_BYTES}-byte ({} MiB) APPEND limit",
            APPEND_MAX_BYTES / 1024 / 1024
        )));
    }
    Ok(len)
}

/// Reject a Message-ID search-hit count over [`MAX_MESSAGE_ID_SEARCH_HITS`].
/// Returns the count unchanged when within the limit (including exactly at it).
pub fn check_message_id_search_hits(count: usize) -> Result<usize, ProviderError> {
    if count > MAX_MESSAGE_ID_SEARCH_HITS {
        return Err(ProviderError::InvalidOperation(format!(
            "{count} Message-ID search hits, over the {MAX_MESSAGE_ID_SEARCH_HITS}-hit verify limit"
        )));
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Phase 4 run 1: SMTP submission limits
// ---------------------------------------------------------------------------
//
// Every numeric limit the SMTP submission client (`smtp.rs`) enforces, in this
// one place, following the "never silently clamp" rule above: a value over a
// limit is a typed, PERMANENT error and nothing is sent — never a truncation.
//
// The three TIMEOUTS exist because mail-send's default is 3600s (far too long
// for an interactive send) and because the delivery-safety model turns on the
// difference between "no final reply yet" and "a reply arrived": a short,
// explicit final-reply timeout is what lets a hung server map to `Unknown`
// quickly rather than hanging the UI. They are plain `Duration` data; the
// client makes them injectable in tests by taking an override struct, so the
// "hung final reply => Unknown" and "hung before 354 => NotDelivered" cases
// run under paused tokio time without waiting these out.
//
// The two gating COUNT/LENGTH limits (address length, recipients per message)
// each have below/exact/above tests, exactly like the ingest gates above.

use std::time::Duration;

/// How long the TCP connect + TLS handshake + EHLO + AUTH ("prepare") phase
/// may take overall. A failure anywhere here is a DEFINITE non-delivery, so a
/// generous-but-bounded ceiling is safe: nothing has been sent. Matches the
/// setup dry-run's own 15s probe ceiling so both paths feel the same.
pub const SMTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long any single envelope command (MAIL FROM, each RCPT TO, the DATA
/// request itself) may wait for its reply. Short, because each is one round
/// trip on an already-open connection; a stall here, before the server's
/// `354`, is a DEFINITE non-delivery.
pub const SMTP_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound the whole body upload, including flush. After DATA's 354 a timeout
/// leaves delivery unknown; releasing the submission task lets other accounts
/// continue sending even if this server stops reading.
pub const SMTP_BODY_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait for the server's FINAL reply after the message body and
/// its terminating `.` have been written. Like the body-upload timeout, its
/// expiry is classified as `Unknown`: the server may already hold the whole
/// message, so we must not claim non-delivery. Kept longer than a
/// command timeout because a server may do real queue work (virus/spam scan)
/// before its `250`.
pub const SMTP_FINAL_REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the best-effort `RSET` / `QUIT` after a finished or aborted
/// submission may take. Cleanup never changes the verdict, so it only needs to
/// be bounded.
pub const SMTP_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest server reply TEXT (the human-readable part after the code) we keep
/// to surface to the user, in bytes. A hostile or buggy server could return a
/// multi-megabyte reply line; the reply reader already caps the raw line, and
/// this bounds what we RETAIN after sanitising control characters out, so the
/// preserved "553 ... names the identity" message cannot bloat an error or a
/// log. 512 bytes is far more than any real SMTP reply text.
pub const MAX_SMTP_REPLY_TEXT_BYTES: usize = 512;

/// Longest envelope address (the `local@domain` inside the angle brackets),
/// in bytes. RFC 5321 §4.5.3.1.3 caps the forward/reverse path at 256 octets
/// INCLUDING the angle brackets, so the address itself is at most 254. An
/// address over this is rejected before any network I/O. Over the limit is a
/// typed PERMANENT rejection — zero bytes reach the wire.
pub const MAX_SMTP_ADDRESS_BYTES: usize = 254;

/// Most recipients one message may carry. ThreeStrands composes ordinary mail;
/// a hundred recipients is already far past any interactive send and a
/// thousand would be abuse. 100 bounds the RCPT round trips while never
/// getting in a real user's way. Over the limit is a typed PERMANENT rejection
/// before any network I/O — never a silent drop of the overflow recipients.
pub const MAX_SMTP_RECIPIENTS: usize = 100;

/// Reject an envelope address whose byte length exceeds
/// [`MAX_SMTP_ADDRESS_BYTES`]. Returns the length unchanged when within the
/// limit (including exactly at it), so the caller reads "ok, this long".
pub fn check_smtp_address_len(len: usize) -> Result<usize, ProviderError> {
    if len > MAX_SMTP_ADDRESS_BYTES {
        return Err(ProviderError::PermanentClientRejection(format!(
            "email address is {len} bytes, over the {MAX_SMTP_ADDRESS_BYTES}-byte (RFC 5321) limit"
        )));
    }
    Ok(len)
}

/// Reject a recipient count over [`MAX_SMTP_RECIPIENTS`]. Returns the count
/// unchanged when within the limit (including exactly at it).
pub fn check_smtp_recipient_count(count: usize) -> Result<usize, ProviderError> {
    if count > MAX_SMTP_RECIPIENTS {
        return Err(ProviderError::PermanentClientRejection(format!(
            "{count} recipients, over the {MAX_SMTP_RECIPIENTS}-recipient per-message limit"
        )));
    }
    Ok(count)
}

/// Most sending identities one IMAP account may store (the login address plus
/// its secondary addresses). An account has a handful of real send-as
/// addresses — a combined-addresses Proton Bridge mailbox, a couple of
/// aliases — never dozens; 20 is generous for every real case while bounding
/// the stored list, the From-picker menu, and the per-send identity scan. The
/// lenient read builder ([`super::account_identities`]) silently drops a stale
/// over-count, but an interactive EDIT must tell the user, so
/// [`check_identity_count`] returns a typed rejection rather than clamping.
pub const IMAP_MAX_IDENTITIES: usize = 20;

/// Longest display name on a stored sending identity, in characters (Unicode
/// scalar values, not bytes). A From display name is header data shown to the
/// user; a real name is short. 100 bounds the stored string and the From
/// header line while never getting in a real name's way. Over the limit is a
/// rejection on edit, never a silent truncation.
pub const MAX_IDENTITY_DISPLAY_NAME_CHARS: usize = 100;

/// Reject a sending-identity count over [`IMAP_MAX_IDENTITIES`]. Returns the
/// count unchanged when within the limit (including exactly at it), so the
/// caller reads "ok, this many". Unlike the lenient read-time builder, an edit
/// uses this to refuse an over-limit list outright with a plain message.
pub fn check_identity_count(count: usize) -> Result<usize, ProviderError> {
    if count > IMAP_MAX_IDENTITIES {
        return Err(ProviderError::PermanentClientRejection(format!(
            "{count} sending addresses, over the {IMAP_MAX_IDENTITIES}-address limit for one account"
        )));
    }
    Ok(count)
}

/// Reject a display name longer than [`MAX_IDENTITY_DISPLAY_NAME_CHARS`]
/// characters. Returns the char count unchanged when within the limit
/// (including exactly at it).
pub fn check_identity_display_name_len(chars: usize) -> Result<usize, ProviderError> {
    if chars > MAX_IDENTITY_DISPLAY_NAME_CHARS {
        return Err(ProviderError::PermanentClientRejection(format!(
            "a sender name is {chars} characters, over the {MAX_IDENTITY_DISPLAY_NAME_CHARS}-character limit"
        )));
    }
    Ok(chars)
}

/// Minimum gap, in milliseconds, between two `prepare_delivery` attempts for
/// the SAME queued outbox item after a TRANSIENT prepare failure.
///
/// The outbox tick runs about every 2 seconds (`Correspondence::run`), and a
/// transient prepare failure (an unreachable SMTP server, a TLS/connect
/// timeout) leaves the item QUEUED — it is never claimed, so the next tick
/// would otherwise re-attempt a full TCP+TLS+AUTH against a down server for
/// every queued item on every tick. That is both wasteful and, against a
/// server that rate-limits connections, counter-productive. This interval
/// gates the re-attempt against the item's own `last_attempt_at`, so a down
/// server is probed at most once per interval per item regardless of tick
/// cadence. 60s is long enough to be gentle on a struggling server while a
/// user who is actively waiting still sees a retry within a minute.
///
/// This is a prepare-failure backoff only; it never delays the FIRST attempt
/// (an item with no recorded attempt is always eligible) and it is unrelated
/// to the undo window or the schedule deadline, which gate when an item
/// becomes due in the first place.
pub const SMTP_PREPARE_RETRY_INTERVAL_MS: i64 = 60_000;

/// Whether a queued item whose last prepare attempt was at `last_attempt_at`
/// (`None` = never attempted) may be re-attempted at wall-clock `now`.
///
/// A never-attempted item is ALWAYS eligible. An item attempted at
/// `last_attempt_at` becomes eligible again once at least
/// [`SMTP_PREPARE_RETRY_INTERVAL_MS`] has elapsed; the boundary is inclusive,
/// so exactly at the interval it is eligible. A clock that went backwards
/// (`now < last_attempt_at`) is treated as "not yet eligible" rather than
/// panicking on the subtraction.
pub fn smtp_prepare_retry_due(last_attempt_at: Option<i64>, now: i64) -> bool {
    match last_attempt_at {
        None => true,
        Some(last) => now.saturating_sub(last) >= SMTP_PREPARE_RETRY_INTERVAL_MS,
    }
}

// ---------------------------------------------------------------------------
// Message-ID domain (docs/imap-design.md, "Sending" > "Message-ID domain")
// ---------------------------------------------------------------------------
//
// An IMAP send generates `<uuid@sender-domain>`, where the domain is the part
// after the last `@` of the From address actually used. The domain is a DNS
// name, so its syntax is bounded by the RFC 1035 presentation-format limits.
// An invalid or over-limit domain is a typed error before any send — never a
// silent fallback to `threestrands.local` (which would leak the client name,
// defeating the whole decision), and never a silent clamp.

/// Longest a single DNS label (one dot-separated component) may be, in bytes
/// (RFC 1035 §2.3.4). A label at exactly 63 is valid; 64 is rejected.
pub const MAX_DNS_LABEL_BYTES: usize = 63;

/// Longest a full DNS name may be in its presentation form, in bytes
/// (RFC 1035 §2.3.4 / RFC 1123). 253 is the printable-form ceiling (the
/// 255-octet wire limit minus the length/terminator octets). A name at exactly
/// 253 is valid; 254 is rejected.
pub const MAX_DNS_NAME_BYTES: usize = 253;

/// Validate a Message-ID domain's SYNTAX against the DNS presentation-format
/// limits, returning it ASCII-lowercased when it is well formed. The input is
/// the already-extracted domain (the part after the last `@`), assumed ASCII;
/// a non-ASCII domain is handled (refused) by the caller before this runs.
///
/// Accepts only: one or more dot-separated labels; each label 1..=63 bytes of
/// ASCII letters, digits or hyphen; no label beginning or ending with a
/// hyphen; no empty label (so no leading/trailing dot and no `..`); total
/// length 1..=253. Everything else is a typed [`ProviderError`], never a
/// clamp. A `_`-prefixed service label is NOT accepted: a Message-ID domain is
/// an ordinary host name, not an SRV owner.
pub fn check_message_id_domain(domain: &str) -> Result<String, ProviderError> {
    let reject = |m: &str| {
        Err(ProviderError::InvalidOperation(format!(
            "invalid Message-ID domain: {m}"
        )))
    };
    if domain.is_empty() {
        return reject("empty");
    }
    if domain.len() > MAX_DNS_NAME_BYTES {
        return reject(&format!(
            "{} bytes, over the {MAX_DNS_NAME_BYTES}-byte DNS name limit",
            domain.len()
        ));
    }
    // Must be ASCII to reason about labels byte-wise; the caller refuses a
    // non-ASCII domain before this, but guard here too so the function is
    // safe in isolation.
    if !domain.is_ascii() {
        return reject("contains non-ASCII characters");
    }
    for label in domain.split('.') {
        if label.is_empty() {
            return reject("has an empty label (a leading/trailing dot or '..')");
        }
        if label.len() > MAX_DNS_LABEL_BYTES {
            return reject(&format!(
                "label {label:?} is {} bytes, over the {MAX_DNS_LABEL_BYTES}-byte label limit",
                label.len()
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return reject(&format!("label {label:?} begins or ends with a hyphen"));
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return reject(&format!(
                "label {label:?} has a character other than a letter, digit or hyphen"
            ));
        }
    }
    Ok(domain.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Phase 4 run 4b: the durable Sent-copy job's timings and limits
// ---------------------------------------------------------------------------
//
// After an accepted IMAP send the engine runs a durable job that saves the
// sender's own copy to the Sent mailbox (first-send detection + idempotent
// APPEND, `docs/imap-design.md` "Sending" steps 3–4). Every number that job
// uses lives here, as plain `Duration`/count data, with below/exact/above
// tests. The engine makes them injectable in tests (zero delays, tiny caps) by
// reading them from here; these are the production defaults.
//
// The ONE invariant that binds several of them: a retry's SEARCH must never
// run earlier than SENT_COPY_SETTLE after the previous attempt. A server that
// makes a just-stored message visible to SEARCH only after a delay would
// otherwise let a fast retry APPEND a second copy (run 4a's residual race).
// `sent_copy_retry_at` enforces this: every scheduled retry is at least
// SENT_COPY_SETTLE after the attempt that produced it.

/// How long to wait after a send is ACCEPTED before the first Sent-copy
/// attempt (the "settle window"), and the floor under every retry delay. A
/// server that saves its own Sent copy (Proton Bridge, Fastmail) does so
/// within a couple of seconds; 10s gives it comfortable room so the first-send
/// detection sees the server's copy rather than racing it, without making the
/// user's sent mail take noticeably long to appear. The engine seeds the job's
/// first `next_at` as `sent_at + SENT_COPY_SETTLE`.
pub const SENT_COPY_SETTLE: Duration = Duration::from_secs(10);

/// How long after our own APPEND to wait before the VERIFY phase counts the
/// copies in Sent. The server, if it saves its own copy, may store it a beat
/// after accepting the send; this delay lets a late server-saved duplicate
/// become visible so the verify phase detects "server saves sent" rather than
/// recording a false "we are the only copy". Kept equal to the settle window
/// so a verify is never scheduled earlier than a retry would be.
pub const SENT_COPY_VERIFY_DELAY: Duration = Duration::from_secs(10);

/// The retry backoff schedule (delay before attempt N+1, 0-indexed by prior
/// attempts) for a Sent-copy job that hit a TRANSIENT error. Capped-exponential
/// with a long tail so a Sent mailbox that is briefly unreachable is retried
/// quickly at first and then sparingly, up to [`SENT_COPY_MAX_ATTEMPTS`]. Every
/// entry is >= SENT_COPY_SETTLE, preserving the "never earlier than the settle
/// window" rule. Attempts past the table's end reuse its last (longest) delay.
pub const SENT_COPY_BACKOFF: &[Duration] = &[
    Duration::from_secs(10),   // 10s
    Duration::from_secs(30),   // 30s
    Duration::from_secs(120),  // 2m
    Duration::from_secs(600),  // 10m
    Duration::from_secs(1800), // 30m
    Duration::from_secs(3600), // 1h
];

/// Most TRANSIENT retry attempts a Sent-copy job makes before it is
/// `abandoned` with its last error. With [`SENT_COPY_BACKOFF`] tailing at one
/// hour, ~30 attempts span roughly a day — long enough to ride out a Sent
/// mailbox that is unreachable for most of a day, bounded so a permanently
/// broken account's job does not retry forever. A PERMANENT failure abandons
/// immediately regardless of this.
pub const SENT_COPY_MAX_ATTEMPTS: u32 = 30;

/// Most Sent-copy jobs processed in one `Correspondence::tick`. The tick holds
/// the send gate for its whole run (per-message serialisation), so each job it
/// processes is a bounded piece of per-tick work; this caps how many due jobs
/// one tick drains so a backlog (many sends while offline) does not make a
/// single tick run long. Remaining due jobs are picked up by the next tick.
pub const SENT_COPY_MAX_JOBS_PER_TICK: usize = 10;

/// The delay before Sent-copy attempt number `prior_attempts + 1` (0-indexed),
/// read from [`SENT_COPY_BACKOFF`] and clamped to never fall below
/// [`SENT_COPY_SETTLE`]. Attempts past the table reuse its last entry. This is
/// the one function the engine calls to schedule a transient retry, so the
/// "never earlier than the settle window" rule holds in exactly one place.
pub fn sent_copy_backoff(prior_attempts: u32) -> Duration {
    let index = (prior_attempts as usize).min(SENT_COPY_BACKOFF.len().saturating_sub(1));
    let delay = SENT_COPY_BACKOFF[index];
    if delay < SENT_COPY_SETTLE {
        SENT_COPY_SETTLE
    } else {
        delay
    }
}

// ---------------------------------------------------------------------------
// Phase 5 run 1: server-search rendering limits
// ---------------------------------------------------------------------------
//
// Every numeric limit the IMAP search renderer (`search.rs`) enforces, in this
// one place, following the "never silently clamp" rule above: a query over any
// of these is REFUSED (the renderer returns `Skipped`, keeping local results
// only) and never truncated. Each `check_*` helper returns the value unchanged
// when within the limit (including exactly at it) and a typed `ProviderError`
// over it, with below/exact/above coverage in the test module.
//
// Unlike the ingest/write gates, these guard a search STRING the renderer
// builds from the user's query AST: how many terms it has, how long one user
// value is, how long the whole rendered `UID SEARCH` criteria string is, and
// how many mailboxes one search would touch. The bounds keep a single
// `UID SEARCH` command line and the per-search server work bounded without
// getting in a real query's way.

/// Most user-supplied terms one query may carry. A real search is a handful of
/// terms; a query with dozens is a mistake or abuse. 16 is generous while
/// bounding how long the rendered criteria can grow. Over the limit the
/// renderer skips the server search.
pub const MAX_SEARCH_TERMS: usize = 16;

/// Longest one user search VALUE may be, in UTF-8 bytes (a `from:`, `to:`,
/// `subject:`, free-text, `label:` or `in:` value). 256 bytes is far longer
/// than any real search value while bounding one quoted string in the command.
pub const MAX_SEARCH_TERM_BYTES: usize = 256;

/// Longest the WHOLE rendered criteria string may be, in bytes (the
/// `CHARSET UTF-8 …` input to `UID SEARCH`). Bounds the command line a server
/// must parse even when every individual term is within its own limit. 1024
/// bytes comfortably holds a rich multi-term query.
pub const MAX_SEARCH_QUERY_BYTES: usize = 1024;

/// Most mailboxes one server search may target. The default set is three
/// (INBOX, Archive, Sent); an `in:`/label-folder term narrows to one. 12
/// bounds a pathological plan or a future multi-folder search without
/// restricting ordinary use.
pub const MAX_SEARCH_TARGET_MAILBOXES: usize = 12;

/// Reject a term count over [`MAX_SEARCH_TERMS`]. Returns the count unchanged
/// when within the limit (including exactly at it).
pub fn check_search_terms(count: usize) -> Result<usize, ProviderError> {
    if count > MAX_SEARCH_TERMS {
        return Err(ProviderError::InvalidOperation(format!(
            "{count} search terms, over the {MAX_SEARCH_TERMS}-term limit"
        )));
    }
    Ok(count)
}

/// Reject a per-term byte length over [`MAX_SEARCH_TERM_BYTES`]. Returns the
/// length unchanged when within the limit (including exactly at it).
pub fn check_search_term_bytes(len: usize) -> Result<usize, ProviderError> {
    if len > MAX_SEARCH_TERM_BYTES {
        return Err(ProviderError::InvalidOperation(format!(
            "a search term is {len} bytes, over the {MAX_SEARCH_TERM_BYTES}-byte limit"
        )));
    }
    Ok(len)
}

/// Reject a whole-criteria byte length over [`MAX_SEARCH_QUERY_BYTES`].
/// Returns the length unchanged when within the limit (including exactly at
/// it).
pub fn check_search_query_bytes(len: usize) -> Result<usize, ProviderError> {
    if len > MAX_SEARCH_QUERY_BYTES {
        return Err(ProviderError::InvalidOperation(format!(
            "the search query is {len} bytes, over the {MAX_SEARCH_QUERY_BYTES}-byte limit"
        )));
    }
    Ok(len)
}

/// Reject a target-mailbox count over [`MAX_SEARCH_TARGET_MAILBOXES`]. Returns
/// the count unchanged when within the limit (including exactly at it).
pub fn check_search_target_mailboxes(count: usize) -> Result<usize, ProviderError> {
    if count > MAX_SEARCH_TARGET_MAILBOXES {
        return Err(ProviderError::InvalidOperation(format!(
            "a search would touch {count} mailboxes, over the {MAX_SEARCH_TARGET_MAILBOXES}-mailbox limit"
        )));
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Phase 5 run 4: server-search EXECUTION limits (the budget + deadline the
// `MailSync::search` driver runs under, as opposed to the run-1 RENDER limits
// above which bound the criteria string).
// ---------------------------------------------------------------------------
//
// Run 1 bounds the query STRING. These two bound what the EXECUTION of a
// rendered plan may spend: how many server hits one search acquires (bodies
// fetched + threaded + pinned, so a cost-bearing bound) and how long the whole
// search — SEARCH + acquire across every planned mailbox — may run before it
// returns PARTIAL results. Neither is a silent clamp: over-budget hits are
// simply not acquired this search (the user can page/refine), and an expired
// deadline returns what was acquired and reports partial, never an error.

/// The most server hits ONE search acquires, across all its planned mailboxes.
///
/// A search EXAMINEs each mailbox, `UID SEARCH`es it, and acquires (fetches +
/// threads + PINS) the chosen UIDs — real per-hit work and a pin that counts
/// against [`MAX_SEARCH_PINS`]. This caps that per-search cost. Set equal to
/// `sync::REMOTE_SEARCH_SCAN_LIMIT` (50) so the IMAP server-search backfill
/// acquires the same depth the engine's `search_and_ingest_missing` scans in
/// one pass — the two must not drift, so a test in `provider/tests/search_exec.rs` pins the
/// value. Over the budget the extra hits are left unacquired (a page/refine
/// away), never silently dropped.
pub const MAX_SEARCH_ACQUIRE: usize = 50;

/// The total wall-clock budget for one `MailSync::search`: connection, SEARCH,
/// acquisition, and logout
/// across every planned mailbox. On expiry the driver stops starting new
/// mailboxes and returns what it has acquired as a PARTIAL result (not an
/// error). A body-scanning server without a full-text index can take real
/// time per mailbox (`docs/imap-design.md`, "Server search": "every search has
/// a timeout and partial results are shown"), so this bounds the user's wait
/// while still returning the fast mailboxes' hits. 20s is long enough for the
/// common case on a loopback or indexed server and short enough that a slow
/// server does not hang the UI. The driver reads this through a `tokio::time`
/// deadline so a test can inject a tiny value (a TEST KNOB on the provider)
/// and drive expiry under paused time without waiting it out.
pub const SEARCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// Split a total acquire budget fairly across `mailbox_count` mailboxes:
/// `ceil(budget / count)` per mailbox so no mailbox is starved, with the
/// LAST-visited mailbox absorbing any remainder (the driver visits mailboxes
/// in a deterministic order and takes newest-UID-first within each). Returns
/// the per-mailbox cap. With one mailbox the whole budget goes to it; with
/// zero mailboxes the cap is zero (there is nothing to search). The driver
/// still stops at the GLOBAL `MAX_SEARCH_ACQUIRE` total, so a per-mailbox cap
/// that rounds up never lets the sum exceed the budget — the running total is
/// the hard bound, this split is only the fair share.
pub fn search_acquire_share(budget: usize, mailbox_count: usize) -> usize {
    if mailbox_count == 0 {
        return 0;
    }
    budget.div_ceil(mailbox_count)
}

// ---------------------------------------------------------------------------
// Phase 5 run 3: search-pin limits
// ---------------------------------------------------------------------------
//
// A search (run 4) acquires mail that is usually OLD — outside the newest-N
// sync window of an evicting mailbox — and PINS the acquired `(mailbox,
// uidvalidity, uid)` so the next poll's delta does not age it out and delete
// it (`docs/imap-design.md`, "Server search"). Two numeric limits bound that
// LOCAL pin table, both in this one module with below/exact/above coverage.
//
// Unlike the search-render gates above, a pin cap is NOT a query refusal: it
// bounds how many pins ONE account may hold at once, and the TTL bounds how
// long a single pin survives before the sync releases it. Neither silently
// drops data — the acquire path reports exactly how many over-cap UIDs it
// refused ([`cap_search_pins`] returns the dropped count), and TTL release is
// a comparison ([`search_pin_expired`]) after which the location simply
// follows normal window rules again.

/// Most search pins ONE account may hold at once.
///
/// A search-acquired pin keeps an old, out-of-window message alive against the
/// delta's eviction, so the pin table bounds how much otherwise-evictable mail
/// one account carries beyond its sync windows. 2,000 is generous for a user
/// who searches a lot of old mail while bounding the pin-row count and the
/// extra `imap_locations` the pins retain. An acquisition that would exceed
/// the cap is REFUSED for the over-cap UIDs (reported, never silently dropped)
/// and NEVER evicts an existing pin. See [`cap_search_pins`].
pub const MAX_SEARCH_PINS: usize = 2_000;

/// How long (seconds) a search pin survives before the sync RELEASES it. After
/// release the location follows normal window rules again (an out-of-window
/// evicting location then ages out as usual). Re-acquiring an already-pinned
/// UID refreshes its `pinned_at`, so actively re-found mail stays pinned. 30
/// days is long enough that a search result the user keeps returning to stays
/// put, bounded so a one-off search does not pin old mail forever.
pub const SEARCH_PIN_TTL: i64 = 30 * 24 * 60 * 60;

/// The outcome of applying [`MAX_SEARCH_PINS`] to a pin acquisition: how many
/// of the requested new pins fit under the cap given how many pins already
/// exist, and how many were refused for being over the cap. NEWEST-first
/// acceptance is the caller's responsibility (it orders the UIDs); this owns
/// only the arithmetic. Never evicts an existing pin and never silently drops:
/// `refused` is surfaced to the acquisition result.
#[derive(Debug, PartialEq, Eq)]
pub struct PinCapSelection {
    /// How many of the `requested` new pins may be accepted.
    pub accepted: usize,
    /// How many were refused because the account is at/over the cap.
    pub refused: usize,
}

/// Apply [`MAX_SEARCH_PINS`] to a pin acquisition. `existing` is how many pins
/// the account already holds (that are NOT among the requested set — a
/// re-acquire of an already-pinned UID refreshes in place and does not count
/// against new room, so the caller excludes those first). `requested` is how
/// many NEW UIDs this acquisition would pin. Returns how many fit (newest
/// first, the caller's ordering) and how many are refused. Below/at the cap
/// accepts everything; above it accepts only up to the remaining room and
/// refuses the rest. Never negative, never evicting.
pub fn cap_search_pins(existing: usize, requested: usize) -> PinCapSelection {
    let room = MAX_SEARCH_PINS.saturating_sub(existing);
    let accepted = requested.min(room);
    PinCapSelection {
        accepted,
        refused: requested - accepted,
    }
}

/// Whether a pin acquired at `pinned_at` (unix seconds) has EXPIRED at `now`
/// and should be released: at least [`SEARCH_PIN_TTL`] seconds have elapsed.
/// The boundary is inclusive — exactly at the TTL the pin is released. A clock
/// that went backwards (`now < pinned_at`) is treated as not expired rather
/// than panicking on the subtraction.
#[cfg(test)]
pub fn search_pin_expired(pinned_at: i64, now: i64) -> bool {
    now.saturating_sub(pinned_at) >= SEARCH_PIN_TTL
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each limit is tested just below, exactly at, and just above the
    // boundary, per AGENTS.md "Email rendering" and the emailRenderingPolicy.ts
    // pattern. The boundary itself is accepted; one past it is a typed error,
    // not a clamp.

    #[test]
    fn sent_copy_backoff_is_monotonic_floored_at_settle_and_saturates() {
        use std::time::Duration;
        // The schedule never schedules a retry earlier than the settle window.
        for prior in 0..SENT_COPY_MAX_ATTEMPTS {
            assert!(
                sent_copy_backoff(prior) >= SENT_COPY_SETTLE,
                "attempt {prior} must not retry earlier than the settle window"
            );
        }
        // Below the table's end: reads the indexed entry.
        assert_eq!(sent_copy_backoff(0), SENT_COPY_BACKOFF[0]);
        assert_eq!(sent_copy_backoff(1), SENT_COPY_BACKOFF[1]);
        // Exactly at the last index: the last (longest) entry.
        let last = SENT_COPY_BACKOFF.len() as u32 - 1;
        assert_eq!(sent_copy_backoff(last), SENT_COPY_BACKOFF[last as usize]);
        // Above the table: saturates to the last entry (never panics, never 0).
        assert_eq!(
            sent_copy_backoff(last + 1),
            *SENT_COPY_BACKOFF.last().unwrap()
        );
        assert_eq!(
            sent_copy_backoff(SENT_COPY_MAX_ATTEMPTS + 100),
            *SENT_COPY_BACKOFF.last().unwrap()
        );
        // The schedule is non-decreasing.
        for window in SENT_COPY_BACKOFF.windows(2) {
            assert!(window[0] <= window[1], "backoff must be non-decreasing");
        }
        // The settle floor would actually bite a sub-settle entry: construct
        // the comparison the function makes.
        assert!(SENT_COPY_SETTLE > Duration::ZERO);
        assert!(SENT_COPY_VERIFY_DELAY >= SENT_COPY_SETTLE);
    }

    #[test]
    fn sent_copy_job_limits_are_sane_bounds() {
        // A per-tick cap exists and is positive (an unbounded cap would let one
        // tick run long on a backlog).
        const { assert!(SENT_COPY_MAX_JOBS_PER_TICK >= 1) };
        // Enough transient attempts to ride out a long outage, bounded so a
        // broken account does not retry forever.
        const { assert!(SENT_COPY_MAX_ATTEMPTS >= 2) };
        assert!(!SENT_COPY_BACKOFF.is_empty());
    }

    #[test]
    fn raw_message_bytes_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(check_raw_message_bytes(0).unwrap(), 0);
        assert_eq!(
            check_raw_message_bytes(MAX_RAW_MESSAGE_BYTES - 1).unwrap(),
            MAX_RAW_MESSAGE_BYTES - 1
        );
        assert_eq!(
            check_raw_message_bytes(MAX_RAW_MESSAGE_BYTES).unwrap(),
            MAX_RAW_MESSAGE_BYTES
        );
        let err = check_raw_message_bytes(MAX_RAW_MESSAGE_BYTES + 1).unwrap_err();
        assert!(
            matches!(err, ProviderError::PermanentClientRejection(_)),
            "over-limit is a typed rejection, not a clamp: {err:?}"
        );
    }

    #[test]
    fn mime_depth_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(check_mime_depth(MAX_MIME_DEPTH - 1).unwrap(), MAX_MIME_DEPTH - 1);
        assert_eq!(check_mime_depth(MAX_MIME_DEPTH).unwrap(), MAX_MIME_DEPTH);
        assert!(matches!(
            check_mime_depth(MAX_MIME_DEPTH + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    #[test]
    fn mime_part_count_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(
            check_mime_part_count(MAX_MIME_PART_COUNT - 1).unwrap(),
            MAX_MIME_PART_COUNT - 1
        );
        assert_eq!(
            check_mime_part_count(MAX_MIME_PART_COUNT).unwrap(),
            MAX_MIME_PART_COUNT
        );
        assert!(matches!(
            check_mime_part_count(MAX_MIME_PART_COUNT + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    // The IMAP ingest limits are never looser than the shared normalize-path
    // limits in a way that would let a message pass ingest and then be rejected
    // for DEPTH downstream: depth matches exactly, and the part gate is only
    // ever wider (so normalize, not a silent clamp, is what rejects the gap).
    #[test]
    fn ingest_depth_limit_matches_the_shared_normalize_limit() {
        assert_eq!(MAX_MIME_DEPTH, crate::mime::MAX_MIME_DEPTH);
        const { assert!(MAX_MIME_PART_COUNT >= crate::mime::MAX_MIME_PARTS) };
    }

    // --- sync window ---------------------------------------------------

    /// UIDs 1..=n, deliberately not starting at zero (UIDs never are).
    fn uids(n: usize) -> Vec<u32> {
        (1..=n as u32).collect()
    }

    /// Below, exactly at, and just above `limit`: the boundary message is
    /// kept, one more pushes the OLDEST (lowest UID) beyond the window, and
    /// the count of what fell outside is reported rather than dropped.
    fn assert_window_boundary(limit: usize) {
        let below = select_window(uids(limit - 1), limit);
        assert_eq!(below.in_window.len(), limit - 1);
        assert_eq!(below.beyond_window, 0);

        let exact = select_window(uids(limit), limit);
        assert_eq!(exact.in_window.len(), limit);
        assert_eq!(exact.beyond_window, 0);
        assert_eq!(exact.in_window.first(), Some(&1), "boundary keeps the oldest too");

        let above = select_window(uids(limit + 1), limit);
        assert_eq!(above.in_window.len(), limit);
        assert_eq!(above.beyond_window, 1);
        assert_eq!(above.in_window.first(), Some(&2), "UID 1, the oldest, fell out");
        assert_eq!(above.in_window.last(), Some(&(limit as u32 + 1)));
    }

    #[test]
    fn inbox_window_is_applied_below_at_and_above_the_guard() {
        assert_window_boundary(MAX_INBOX_SYNC_MESSAGES);
    }

    #[test]
    fn sent_window_is_applied_below_at_and_above_the_limit() {
        assert_window_boundary(SENT_SYNC_WINDOW);
    }

    #[test]
    fn folder_window_is_applied_below_at_and_above_the_limit() {
        assert_window_boundary(FOLDER_SYNC_WINDOW);
    }

    #[test]
    fn window_classes_use_the_documented_limits() {
        assert_eq!(SyncWindowClass::Sent.limit(), 5_000);
        assert_eq!(SyncWindowClass::Folder.limit(), 2_000);
        assert_eq!(SyncWindowClass::Inbox.limit(), MAX_INBOX_SYNC_MESSAGES);
        assert_eq!(SyncWindowClass::NotSynced.limit(), 0);
    }

    #[test]
    fn eviction_is_true_for_sliding_windows_and_false_for_sent_acquisition() {
        // INBOX and Folder are sliding eviction windows (c77fa3f behaviour).
        assert!(SyncWindowClass::Inbox.evicts_beyond_window());
        assert!(SyncWindowClass::Folder.evicts_beyond_window());
        // Sent's 5,000 is an acquisition bound: an aged-out message stays.
        assert!(!SyncWindowClass::Sent.evicts_beyond_window());
        // NotSynced never has anything in its window.
        assert!(!SyncWindowClass::NotSynced.evicts_beyond_window());
    }

    #[test]
    fn sync_limits_constructors_match_their_classes() {
        assert_eq!(SyncLimits::inbox(), SyncLimits::for_class(SyncWindowClass::Inbox));
        assert_eq!(SyncLimits::inbox().mailbox_window, MAX_INBOX_SYNC_MESSAGES);
        assert!(SyncLimits::inbox().evicts);
        let folder = SyncLimits::for_class(SyncWindowClass::Folder);
        assert_eq!(folder.mailbox_window, FOLDER_SYNC_WINDOW);
        assert!(folder.evicts);
        let sent = SyncLimits::for_class(SyncWindowClass::Sent);
        assert_eq!(sent.mailbox_window, SENT_SYNC_WINDOW);
        assert!(!sent.evicts, "Sent does not evict");
    }

    #[test]
    fn folder_sweep_due_below_at_and_above_the_interval() {
        let now = 1_000_000i64;
        // Just inside the interval: not yet due.
        assert!(!folder_sweep_due(now - (FOLDER_SWEEP_INTERVAL_SECS - 1), now));
        // Exactly at the interval: due.
        assert!(folder_sweep_due(now - FOLDER_SWEEP_INTERVAL_SECS, now));
        // Past the interval: due.
        assert!(folder_sweep_due(now - (FOLDER_SWEEP_INTERVAL_SECS + 1), now));
        // Never swept (0): always due.
        assert!(folder_sweep_due(0, now));
    }

    #[test]
    fn folder_counters_unchanged_detects_new_mail_and_expunges_but_not_flag_changes() {
        // Both counters equal => unchanged (a flag change by another client
        // moves neither, so it reads as unchanged — the sweep catches it).
        assert!(folder_counters_unchanged(3, 9, 3, 9));
        // New mail bumps UIDNEXT => changed.
        assert!(!folder_counters_unchanged(3, 9, 3, 10));
        // An expunge changes EXISTS => changed.
        assert!(!folder_counters_unchanged(3, 9, 2, 9));
    }

    #[test]
    fn the_folder_cadence_constants_are_positive_bounds() {
        const { assert!(FOLDER_SWEEP_INTERVAL_SECS > 0) };
        assert_eq!(FOLDER_SWEEP_INTERVAL_SECS, 15 * 60);
        const { assert!(FOLDER_ROUNDS_PER_POLL > 0) };
        assert_eq!(FOLDER_ROUNDS_PER_POLL, 8);
    }

    // --- Slice 5b-2: fair scheduling, cap, grace --------------------------

    fn sys(name: &str) -> ScheduleCandidate {
        ScheduleCandidate {
            mailbox: name.into(),
            last_attempt: 0,
            last_visited_at: 0,
            visited_in_epoch: 0,
            is_system_role: true,
        }
    }
    fn folder(name: &str, last_visited_at: i64) -> ScheduleCandidate {
        ScheduleCandidate {
            mailbox: name.into(),
            last_attempt: 0,
            last_visited_at,
            visited_in_epoch: 0,
            is_system_role: false,
        }
    }
    fn folder_epoch(name: &str, last_visited_at: i64, visited_in_epoch: i64) -> ScheduleCandidate {
        ScheduleCandidate {
            mailbox: name.into(),
            last_attempt: 0,
            last_visited_at,
            visited_in_epoch,
            is_system_role: false,
        }
    }

    #[test]
    fn system_roles_are_always_visited_regardless_of_budget() {
        let candidates = vec![sys("Archive"), sys("Sent"), sys("Trash"), folder("A", 0)];
        // Budget 0: every system role still visited, no user folder.
        let visits = schedule_visits(&candidates, 1, 0);
        assert_eq!(visits, vec!["Archive", "Sent", "Trash"], "roles sorted by name, no folders");
        // Budget 1: the roles plus one folder.
        let visits = schedule_visits(&candidates, 1, 1);
        assert_eq!(visits, vec!["Archive", "Sent", "Trash", "A"]);
    }

    #[test]
    fn not_yet_covered_this_epoch_folders_are_scheduled_before_already_covered_ones() {
        // In epoch 5: A and C already visited this epoch, B and D not. The
        // not-covered folders come first (by name), regardless of their
        // last_visited_at timestamps.
        let candidates = vec![
            folder_epoch("A", 100, 5), // covered this epoch
            folder_epoch("B", 999, 4), // NOT covered this epoch (older epoch)
            folder_epoch("C", 50, 5),  // covered this epoch
            folder_epoch("D", 10, 0),  // never covered
        ];
        let visits = schedule_visits(&candidates, 5, 10);
        assert_eq!(
            visits,
            vec!["D", "B", "C", "A"],
            "not-covered-this-epoch (D, B by LRV) precede covered (C, A by LRV)"
        );
    }

    #[test]
    fn never_visited_folders_come_first_then_least_recently_visited() {
        // "A" never visited (0), "B"/"C" visited at increasing times, "D"
        // never visited. Never-visited first (A, D by name), then LRV (B, C).
        let candidates = vec![
            folder("C", 300),
            folder("A", 0),
            folder("B", 100),
            folder("D", 0),
        ];
        let visits = schedule_visits(&candidates, 1, 10);
        assert_eq!(visits, vec!["A", "D", "B", "C"], "never-visited first (by name), then LRV");
    }

    #[test]
    fn ties_in_visit_time_break_deterministically_by_name() {
        let candidates = vec![folder("Z", 50), folder("A", 50), folder("M", 50)];
        let visits = schedule_visits(&candidates, 1, 10);
        assert_eq!(visits, vec!["A", "M", "Z"], "equal visit times tie-break by name");
    }

    #[test]
    fn budget_below_exact_and_above_the_folder_count() {
        let candidates = vec![folder("A", 0), folder("B", 0), folder("C", 0)];
        // Below: only the first `budget` folders (never-covered, by name).
        assert_eq!(schedule_visits(&candidates, 1, 2), vec!["A", "B"]);
        // Exact: all folders.
        assert_eq!(schedule_visits(&candidates, 1, 3), vec!["A", "B", "C"]);
        // Above: still all folders, no padding.
        assert_eq!(schedule_visits(&candidates, 1, 4), vec!["A", "B", "C"]);
    }

    #[test]
    fn fairness_every_folder_is_visited_within_ceil_n_over_budget_polls() {
        // 20 user folders, budget 3 => ceil(20/3) = 7 polls to cover all. This
        // simulates the COVERAGE EPOCH: a folder visited this epoch is stamped
        // with the current epoch, and the scheduler prefers folders not yet
        // covered this epoch, so coverage completes deterministically even with
        // a frozen clock. Assert full coverage within one epoch's worth of polls.
        let budget = 3usize;
        let n = 20usize;
        let epoch = 1i64;
        let mut visited_in_epoch: std::collections::BTreeMap<String, i64> =
            (0..n).map(|i| (format!("f{i:02}"), 0i64)).collect();
        let mut ever_visited: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        let polls_needed = n.div_ceil(budget);
        for _ in 0..polls_needed {
            let candidates: Vec<ScheduleCandidate> = visited_in_epoch
                .iter()
                .map(|(name, &e)| folder_epoch(name, 0, e))
                .collect();
            let visits = schedule_visits(&candidates, epoch, budget);
            for v in &visits {
                ever_visited.insert(v.clone());
                visited_in_epoch.insert(v.clone(), epoch); // stamp as covered this epoch
            }
        }
        assert_eq!(
            ever_visited.len(),
            n,
            "every folder visited within ceil(N/budget) polls: {}/{n}",
            ever_visited.len()
        );
    }

    #[test]
    fn failed_attempts_rotate_without_successful_coverage_or_clock_progress() {
        let mut candidates = vec![
            folder_epoch("A-fails", 0, -1),
            folder_epoch("B-fails", 0, -1),
            folder_epoch("C-healthy", 0, 7),
        ];
        let mut attempts = Vec::new();
        for sequence in 1..=6 {
            let next = schedule_visits(&candidates, 7, 1).pop().unwrap();
            candidates
                .iter_mut()
                .find(|c| c.mailbox == next)
                .unwrap()
                .last_attempt = sequence;
            attempts.push(next);
        }
        assert_eq!(
            attempts,
            [
                "A-fails",
                "B-fails",
                "C-healthy",
                "A-fails",
                "B-fails",
                "C-healthy"
            ]
        );
        assert_eq!(
            candidates[0].visited_in_epoch, -1,
            "attempts do not claim success"
        );
    }

    #[test]
    fn the_cap_is_a_positive_bound() {
        const { assert!(MAX_SYNCED_MAILBOXES > 0) };
        assert_eq!(MAX_SYNCED_MAILBOXES, 500);
    }

    #[test]
    fn cap_keeps_everything_below_and_at_the_limit() {
        let make = |n: usize| -> Vec<ScheduleCandidate> {
            (0..n).map(|i| folder(&format!("f{i:04}"), 0)).collect()
        };
        // Below the cap: nothing dropped.
        let (kept, dropped) = cap_synced_mailboxes(&make(MAX_SYNCED_MAILBOXES - 1), MAX_SYNCED_MAILBOXES);
        assert_eq!(kept.len(), MAX_SYNCED_MAILBOXES - 1);
        assert_eq!(dropped, 0);
        // Exactly at the cap: nothing dropped.
        let (kept, dropped) = cap_synced_mailboxes(&make(MAX_SYNCED_MAILBOXES), MAX_SYNCED_MAILBOXES);
        assert_eq!(kept.len(), MAX_SYNCED_MAILBOXES);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn cap_drops_the_by_name_tail_above_the_limit_keeping_system_roles() {
        // 2 system roles + 5 folders, cap 4 => keep both roles + first 2
        // folders by name, drop 3.
        let mut candidates = vec![sys("Sent"), sys("Trash")];
        for name in ["e", "d", "c", "b", "a"] {
            candidates.push(folder(name, 0));
        }
        let (kept, dropped) = cap_synced_mailboxes(&candidates, 4);
        assert_eq!(dropped, 3, "5 folders, room for 2 => 3 dropped");
        // System roles kept (by name) then the first 2 folders by name.
        assert_eq!(kept, vec!["Sent", "Trash", "a", "b"]);
    }

    #[test]
    fn cap_never_drops_a_system_role_even_below_the_cap_size() {
        // 3 system roles, cap 2 (smaller than the system set): all 3 roles are
        // still kept — the bounded system set is never sacrificed.
        let candidates = vec![sys("Archive"), sys("Sent"), sys("Trash")];
        let (kept, dropped) = cap_synced_mailboxes(&candidates, 2);
        assert_eq!(kept, vec!["Archive", "Sent", "Trash"]);
        assert_eq!(dropped, 0, "no user folders to drop; roles are never dropped");
    }

    #[test]
    fn the_grace_constant_is_a_positive_bound() {
        const { assert!(EMPTIED_THREAD_GRACE_WALKS > 0) };
        assert_eq!(EMPTIED_THREAD_GRACE_WALKS, 2);
    }

    #[test]
    fn emptied_grace_expiry_below_at_and_above_the_bound() {
        let emptied_at = 10u64;
        // Below the bound (1 of 2 completed coverage epochs elapsed): not expired.
        assert!(!emptied_grace_expired(emptied_at, emptied_at + (EMPTIED_THREAD_GRACE_WALKS - 1)));
        // Exactly at the bound: expired (grace spent).
        assert!(emptied_grace_expired(emptied_at, emptied_at + EMPTIED_THREAD_GRACE_WALKS));
        // Past the bound: expired.
        assert!(emptied_grace_expired(emptied_at, emptied_at + EMPTIED_THREAD_GRACE_WALKS + 5));
        // Observed empty on the SAME walk it is checked: zero elapsed, not expired.
        assert!(!emptied_grace_expired(emptied_at, emptied_at));
    }

    // The Sent per-poll acquisition budget mirrors Gmail's Sent backfill
    // cadence in sync.rs (SENT_BACKFILL_FETCHES_PER_STEP ×
    // SENT_BACKFILL_STEPS_PER_POLL = 25 × 4). Those two are private to sync.rs
    // and the brief forbids editing it, so this pins the arithmetic locally so
    // the mirrored literal cannot drift from its documented intent unnoticed.
    #[test]
    fn the_sent_round_budget_mirrors_the_gmail_cadence() {
        const GMAIL_SENT_FETCHES_PER_STEP: usize = 25;
        const GMAIL_SENT_STEPS_PER_POLL: usize = 4;
        assert_eq!(
            SENT_ROUND_MESSAGES,
            GMAIL_SENT_FETCHES_PER_STEP * GMAIL_SENT_STEPS_PER_POLL
        );
        assert_eq!(SENT_ROUND_MESSAGES, 100);
        const { assert!(SENT_ROUND_MESSAGES > 0) };
    }

    #[test]
    fn sent_backfill_pending_only_while_the_marker_is_present() {
        // A present watermark (lowest UID acquired while incomplete) => pending.
        assert!(sent_backfill_incomplete(Some(42)));
        assert!(sent_backfill_incomplete(Some(1)));
        // NULL => complete; normal cadence resumes.
        assert!(!sent_backfill_incomplete(None));
    }

    #[test]
    fn a_not_synced_class_puts_every_message_beyond_the_window() {
        let selection = select_window(uids(3), SyncWindowClass::NotSynced.limit());
        assert!(selection.in_window.is_empty());
        assert_eq!(selection.beyond_window, 3);
    }

    #[test]
    fn window_selection_accepts_raw_search_output() {
        // A UID SEARCH result is unordered and a server may repeat a UID.
        let selection = select_window(vec![40, 7, 7, 900, 13, 40], 3);
        assert_eq!(selection.in_window, vec![13, 40, 900]);
        assert_eq!(selection.beyond_window, 1, "only UID 7 is outside, once");
    }

    #[test]
    fn an_empty_mailbox_has_an_empty_window() {
        let selection = select_window(Vec::new(), FOLDER_SYNC_WINDOW);
        assert!(selection.in_window.is_empty());
        assert_eq!(selection.beyond_window, 0);
    }

    // The design says IMAP Sent depth matches Gmail's so the address book and
    // Keep in Touch see the same history on both providers. Fail loudly if one
    // side is changed without the other.
    #[test]
    fn sent_window_matches_the_gmail_sent_backfill_limit() {
        assert_eq!(SENT_SYNC_WINDOW, crate::sync::MAX_SENT_BACKFILL_THREADS);
    }

    // The owner chose to hold the INBOX ceiling at the same figure.
    #[test]
    fn inbox_ceiling_matches_the_gmail_sent_backfill_limit() {
        assert_eq!(MAX_INBOX_SYNC_MESSAGES, crate::sync::MAX_SENT_BACKFILL_THREADS);
    }

    // --- Slice 5a limits -----------------------------------------------

    fn refs(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("<id{i}@x>")).collect()
    }

    #[test]
    fn references_cap_keeps_the_newest_below_at_and_above_the_limit() {
        // Below the cap: everything kept, nothing dropped.
        let below = clamp_references(refs(MAX_REFERENCES - 1));
        assert_eq!(below.kept.len(), MAX_REFERENCES - 1);
        assert_eq!(below.dropped, 0);

        // Exactly at the cap: everything kept, including the oldest.
        let exact = clamp_references(refs(MAX_REFERENCES));
        assert_eq!(exact.kept.len(), MAX_REFERENCES);
        assert_eq!(exact.dropped, 0);
        assert_eq!(exact.kept.first().unwrap(), "<id0@x>");

        // One over the cap: the single OLDEST (left-most) id is dropped, the
        // nearest ancestors are kept, and the drop is reported, not silent.
        let above = clamp_references(refs(MAX_REFERENCES + 1));
        assert_eq!(above.kept.len(), MAX_REFERENCES);
        assert_eq!(above.dropped, 1);
        assert_eq!(above.kept.first().unwrap(), "<id1@x>", "oldest fell out");
        assert_eq!(
            above.kept.last().unwrap(),
            &format!("<id{}@x>", MAX_REFERENCES)
        );
    }

    #[test]
    fn references_cap_handles_an_empty_chain() {
        let selection = clamp_references(Vec::new());
        assert!(selection.kept.is_empty());
        assert_eq!(selection.dropped, 0);
    }

    #[test]
    fn journal_cursor_answerability_below_at_and_above_the_retention_bound() {
        let current = 10_000u64;
        // Just inside retention: answerable.
        assert!(journal_cursor_is_answerable(
            current - (JOURNAL_RETENTION_GENERATIONS - 1),
            current
        ));
        // Exactly at the retention boundary: still answerable.
        assert!(journal_cursor_is_answerable(
            current - JOURNAL_RETENTION_GENERATIONS,
            current
        ));
        // One past retention: too old, not answerable (engine full-syncs).
        assert!(!journal_cursor_is_answerable(
            current - (JOURNAL_RETENTION_GENERATIONS + 1),
            current
        ));
        // The current generation and generation 0 bootstrap are answerable.
        assert!(journal_cursor_is_answerable(current, current));
        assert!(journal_cursor_is_answerable(0, 0));
        // A cursor NEWER than the store's generation (store reset behind the
        // engine) is never answerable.
        assert!(!journal_cursor_is_answerable(current + 1, current));
    }

    #[test]
    fn the_uid_batch_size_is_a_positive_bound() {
        const { assert!(UID_BATCH_SIZE > 0) };
        assert_eq!(UID_BATCH_SIZE, 500);
    }

    #[test]
    fn the_token_backfill_batch_size_is_a_positive_bound() {
        const { assert!(TOKEN_BACKFILL_BATCH_SIZE > 0) };
        // One bound variable is the account id; the chunk plus it must stay
        // under SQLite's oldest default limit of 999.
        const { assert!(TOKEN_QUERY_CHUNK > 0 && TOKEN_QUERY_CHUNK < 999) };
        assert_eq!(TOKEN_BACKFILL_BATCH_SIZE, 500);
    }

    #[test]
    fn sync_limits_default_to_the_inbox_ceiling() {
        assert_eq!(SyncLimits::default(), SyncLimits::inbox());
        assert_eq!(SyncLimits::default().mailbox_window, MAX_INBOX_SYNC_MESSAGES);
    }

    // --- Phase 3 run 1: write-command limits ---------------------------

    #[test]
    fn mailbox_name_len_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(check_mailbox_name_len(0).unwrap(), 0);
        assert_eq!(
            check_mailbox_name_len(MAX_MAILBOX_NAME_BYTES - 1).unwrap(),
            MAX_MAILBOX_NAME_BYTES - 1
        );
        assert_eq!(
            check_mailbox_name_len(MAX_MAILBOX_NAME_BYTES).unwrap(),
            MAX_MAILBOX_NAME_BYTES
        );
        let err = check_mailbox_name_len(MAX_MAILBOX_NAME_BYTES + 1).unwrap_err();
        assert!(
            matches!(err, ProviderError::InvalidOperation(_)),
            "over-limit is a typed rejection, not a clamp: {err:?}"
        );
    }

    #[test]
    fn flags_per_command_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(check_flags_per_command(0).unwrap(), 0);
        assert_eq!(
            check_flags_per_command(MAX_FLAGS_PER_COMMAND - 1).unwrap(),
            MAX_FLAGS_PER_COMMAND - 1
        );
        assert_eq!(
            check_flags_per_command(MAX_FLAGS_PER_COMMAND).unwrap(),
            MAX_FLAGS_PER_COMMAND
        );
        assert!(matches!(
            check_flags_per_command(MAX_FLAGS_PER_COMMAND + 1).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn the_write_command_limits_are_positive_bounds() {
        const { assert!(MAX_MAILBOX_NAME_BYTES > 0) };
        assert_eq!(MAX_MAILBOX_NAME_BYTES, 1024);
        const { assert!(MAX_FLAGS_PER_COMMAND > 0) };
        assert_eq!(MAX_FLAGS_PER_COMMAND, 64);
        // The write-side UID batch mirrors the read-side one, so a relocate and
        // a fetch split a large UID set identically.
        assert_eq!(MAX_UIDS_PER_COMMAND, UID_BATCH_SIZE);
        const { assert!(MAX_UIDS_PER_COMMAND > 0) };
    }

    // --- Phase 4 run 4a: the Sent-copy APPEND primitive --------------------

    #[test]
    fn identity_count_gates_below_at_and_above_the_limit() {
        assert_eq!(check_identity_count(0).unwrap(), 0);
        assert_eq!(
            check_identity_count(IMAP_MAX_IDENTITIES - 1).unwrap(),
            IMAP_MAX_IDENTITIES - 1
        );
        assert_eq!(check_identity_count(IMAP_MAX_IDENTITIES).unwrap(), IMAP_MAX_IDENTITIES);
        assert!(matches!(
            check_identity_count(IMAP_MAX_IDENTITIES + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    #[test]
    fn identity_display_name_len_gates_below_at_and_above_the_limit() {
        assert_eq!(check_identity_display_name_len(0).unwrap(), 0);
        assert_eq!(
            check_identity_display_name_len(MAX_IDENTITY_DISPLAY_NAME_CHARS - 1).unwrap(),
            MAX_IDENTITY_DISPLAY_NAME_CHARS - 1
        );
        assert_eq!(
            check_identity_display_name_len(MAX_IDENTITY_DISPLAY_NAME_CHARS).unwrap(),
            MAX_IDENTITY_DISPLAY_NAME_CHARS
        );
        assert!(matches!(
            check_identity_display_name_len(MAX_IDENTITY_DISPLAY_NAME_CHARS + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    #[test]
    fn append_bytes_gates_below_at_and_above_the_limit() {
        assert_eq!(check_append_bytes(0).unwrap(), 0);
        assert_eq!(
            check_append_bytes(APPEND_MAX_BYTES - 1).unwrap(),
            APPEND_MAX_BYTES - 1
        );
        assert_eq!(check_append_bytes(APPEND_MAX_BYTES).unwrap(), APPEND_MAX_BYTES);
        // One over is a PERMANENT rejection, never a silent clamp.
        assert!(matches!(
            check_append_bytes(APPEND_MAX_BYTES + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    #[test]
    fn message_id_search_hits_gate_below_at_and_above_the_limit() {
        assert_eq!(check_message_id_search_hits(0).unwrap(), 0);
        assert_eq!(
            check_message_id_search_hits(MAX_MESSAGE_ID_SEARCH_HITS - 1).unwrap(),
            MAX_MESSAGE_ID_SEARCH_HITS - 1
        );
        assert_eq!(
            check_message_id_search_hits(MAX_MESSAGE_ID_SEARCH_HITS).unwrap(),
            MAX_MESSAGE_ID_SEARCH_HITS
        );
        assert!(matches!(
            check_message_id_search_hits(MAX_MESSAGE_ID_SEARCH_HITS + 1).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn the_append_limits_are_tied_to_their_anchors() {
        // The APPEND ceiling is pinned to the body-cache ingest ceiling so a
        // message small enough to save is small enough to re-ingest; the two
        // must never drift.
        assert_eq!(APPEND_MAX_BYTES, MAX_RAW_MESSAGE_BYTES);
        assert_eq!(MAX_MESSAGE_ID_SEARCH_HITS, 64);
        const { assert!(MAX_MESSAGE_ID_SEARCH_HITS > 0) };
        const { assert!(SENT_COPY_RELOCATE_RECHECKS >= 1) };
    }

    // --- Phase 4 run 1: SMTP submission limits -----------------------------

    #[test]
    fn smtp_address_length_gates_below_at_and_above_the_limit() {
        // Below and exactly at the limit are accepted and echo the length.
        assert_eq!(check_smtp_address_len(0).unwrap(), 0);
        assert_eq!(
            check_smtp_address_len(MAX_SMTP_ADDRESS_BYTES - 1).unwrap(),
            MAX_SMTP_ADDRESS_BYTES - 1
        );
        assert_eq!(
            check_smtp_address_len(MAX_SMTP_ADDRESS_BYTES).unwrap(),
            MAX_SMTP_ADDRESS_BYTES
        );
        // One over is a PERMANENT rejection, never a silent clamp.
        assert!(matches!(
            check_smtp_address_len(MAX_SMTP_ADDRESS_BYTES + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    #[test]
    fn smtp_recipient_count_gates_below_at_and_above_the_limit() {
        assert_eq!(check_smtp_recipient_count(1).unwrap(), 1);
        assert_eq!(
            check_smtp_recipient_count(MAX_SMTP_RECIPIENTS - 1).unwrap(),
            MAX_SMTP_RECIPIENTS - 1
        );
        assert_eq!(
            check_smtp_recipient_count(MAX_SMTP_RECIPIENTS).unwrap(),
            MAX_SMTP_RECIPIENTS
        );
        assert!(matches!(
            check_smtp_recipient_count(MAX_SMTP_RECIPIENTS + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    #[test]
    fn smtp_numeric_limits_are_the_documented_bounds() {
        // The RFC 5321 path cap is 254 for the address inside the brackets.
        assert_eq!(MAX_SMTP_ADDRESS_BYTES, 254);
        assert_eq!(MAX_SMTP_RECIPIENTS, 100);
        assert_eq!(MAX_SMTP_REPLY_TEXT_BYTES, 512);
        // Upload and final reply both map to Unknown; allow more time than
        // an envelope round trip. Connect is bounded and short.
        assert!(SMTP_BODY_TIMEOUT >= SMTP_COMMAND_TIMEOUT);
        assert!(SMTP_FINAL_REPLY_TIMEOUT >= SMTP_COMMAND_TIMEOUT);
        assert!(SMTP_CONNECT_TIMEOUT.as_secs() > 0);
        assert!(SMTP_COMMAND_TIMEOUT.as_secs() > 0);
    }

    #[test]
    fn smtp_prepare_retry_interval_is_the_documented_bound() {
        assert_eq!(SMTP_PREPARE_RETRY_INTERVAL_MS, 60_000);
    }

    #[test]
    fn smtp_prepare_retry_gates_below_at_and_above_the_interval() {
        // A never-attempted item is always eligible (the first attempt is
        // never delayed).
        assert!(smtp_prepare_retry_due(None, 0));
        assert!(smtp_prepare_retry_due(None, 1_000_000));

        let last = 1_000_000i64;
        // BELOW the interval: not yet eligible (one ms short).
        assert!(!smtp_prepare_retry_due(
            Some(last),
            last + SMTP_PREPARE_RETRY_INTERVAL_MS - 1
        ));
        // EXACTLY at the interval: eligible (inclusive boundary).
        assert!(smtp_prepare_retry_due(
            Some(last),
            last + SMTP_PREPARE_RETRY_INTERVAL_MS
        ));
        // ABOVE the interval: eligible.
        assert!(smtp_prepare_retry_due(
            Some(last),
            last + SMTP_PREPARE_RETRY_INTERVAL_MS + 1
        ));
        // A clock that went backwards is treated as not-yet-eligible, never a
        // panic on the subtraction.
        assert!(!smtp_prepare_retry_due(Some(last), last - 5));
    }

    // --- Message-ID domain ---------------------------------------------

    #[test]
    fn message_id_domain_numeric_limits_are_the_dns_bounds() {
        assert_eq!(MAX_DNS_LABEL_BYTES, 63);
        assert_eq!(MAX_DNS_NAME_BYTES, 253);
    }

    #[test]
    fn message_id_domain_accepts_a_plain_host_and_lowercases_it() {
        assert_eq!(check_message_id_domain("proton.me").unwrap(), "proton.me");
        assert_eq!(check_message_id_domain("Proton.ME").unwrap(), "proton.me");
        assert_eq!(
            check_message_id_domain("mail.example.co.uk").unwrap(),
            "mail.example.co.uk"
        );
        // Hyphen and digits inside a label are fine.
        assert_eq!(check_message_id_domain("x-1.a0").unwrap(), "x-1.a0");
    }

    #[test]
    fn message_id_domain_label_length_below_at_above_the_limit() {
        let label_at = "a".repeat(MAX_DNS_LABEL_BYTES);
        let label_below = "a".repeat(MAX_DNS_LABEL_BYTES - 1);
        let label_above = "a".repeat(MAX_DNS_LABEL_BYTES + 1);
        assert!(check_message_id_domain(&format!("{label_below}.me")).is_ok());
        assert!(
            check_message_id_domain(&format!("{label_at}.me")).is_ok(),
            "a 63-byte label is exactly at the limit and accepted"
        );
        let err = check_message_id_domain(&format!("{label_above}.me")).unwrap_err();
        assert!(
            matches!(err, ProviderError::InvalidOperation(_)),
            "a 64-byte label is over the limit — a typed error, not a clamp: {err:?}"
        );
    }

    #[test]
    fn message_id_domain_total_length_below_at_above_the_limit() {
        // Build names whose TOTAL length is exactly MAX_DNS_NAME_BYTES, one
        // below, and one above, while keeping every label within 63 bytes.
        fn name_of_len(total: usize) -> String {
            // Labels of 63 "a" joined by dots, trimmed to `total`.
            let mut s = String::new();
            while s.len() < total {
                if !s.is_empty() {
                    s.push('.');
                }
                let remaining = total - s.len();
                let take = remaining.min(MAX_DNS_LABEL_BYTES);
                s.push_str(&"a".repeat(take));
            }
            debug_assert_eq!(s.len(), total);
            s
        }
        let at = name_of_len(MAX_DNS_NAME_BYTES);
        let below = name_of_len(MAX_DNS_NAME_BYTES - 1);
        let above = name_of_len(MAX_DNS_NAME_BYTES + 1);
        assert_eq!(below.len(), MAX_DNS_NAME_BYTES - 1);
        assert!(check_message_id_domain(&below).is_ok());
        assert_eq!(at.len(), MAX_DNS_NAME_BYTES);
        assert!(
            check_message_id_domain(&at).is_ok(),
            "a 253-byte name is exactly at the limit and accepted"
        );
        assert_eq!(above.len(), MAX_DNS_NAME_BYTES + 1);
        assert!(matches!(
            check_message_id_domain(&above).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn message_id_domain_rejects_malformed_syntax() {
        for bad in [
            "",                 // empty
            ".me",              // leading dot => empty label
            "me.",              // trailing dot => empty label
            "a..b",             // empty inner label
            "-lead.me",         // label begins with hyphen
            "trail-.me",        // label ends with hyphen
            "under_score.me",   // underscore is not a letter/digit/hyphen
            "spa ce.me",        // whitespace
            "pröton.me",        // non-ASCII (caller refuses first; guarded here too)
            "a@b.me",           // '@' is not a legal label character
        ] {
            assert!(
                matches!(
                    check_message_id_domain(bad),
                    Err(ProviderError::InvalidOperation(_))
                ),
                "expected {bad:?} to be rejected"
            );
        }
    }

    // --- Phase 5 run 1: server-search limits ---------------------------

    #[test]
    fn the_search_limits_are_the_documented_bounds() {
        assert_eq!(MAX_SEARCH_TERMS, 16);
        assert_eq!(MAX_SEARCH_TERM_BYTES, 256);
        assert_eq!(MAX_SEARCH_QUERY_BYTES, 1024);
        assert_eq!(MAX_SEARCH_TARGET_MAILBOXES, 12);
        const { assert!(MAX_SEARCH_TERMS > 0) };
        const { assert!(MAX_SEARCH_TERM_BYTES > 0) };
        const { assert!(MAX_SEARCH_QUERY_BYTES > 0) };
        const { assert!(MAX_SEARCH_TARGET_MAILBOXES > 0) };
    }

    #[test]
    fn search_terms_gate_below_at_and_above_the_limit() {
        assert_eq!(check_search_terms(0).unwrap(), 0);
        assert_eq!(check_search_terms(MAX_SEARCH_TERMS - 1).unwrap(), MAX_SEARCH_TERMS - 1);
        assert_eq!(check_search_terms(MAX_SEARCH_TERMS).unwrap(), MAX_SEARCH_TERMS);
        assert!(matches!(
            check_search_terms(MAX_SEARCH_TERMS + 1).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn search_term_bytes_gate_below_at_and_above_the_limit() {
        assert_eq!(check_search_term_bytes(0).unwrap(), 0);
        assert_eq!(
            check_search_term_bytes(MAX_SEARCH_TERM_BYTES - 1).unwrap(),
            MAX_SEARCH_TERM_BYTES - 1
        );
        assert_eq!(
            check_search_term_bytes(MAX_SEARCH_TERM_BYTES).unwrap(),
            MAX_SEARCH_TERM_BYTES
        );
        assert!(matches!(
            check_search_term_bytes(MAX_SEARCH_TERM_BYTES + 1).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn search_query_bytes_gate_below_at_and_above_the_limit() {
        assert_eq!(check_search_query_bytes(0).unwrap(), 0);
        assert_eq!(
            check_search_query_bytes(MAX_SEARCH_QUERY_BYTES - 1).unwrap(),
            MAX_SEARCH_QUERY_BYTES - 1
        );
        assert_eq!(
            check_search_query_bytes(MAX_SEARCH_QUERY_BYTES).unwrap(),
            MAX_SEARCH_QUERY_BYTES
        );
        assert!(matches!(
            check_search_query_bytes(MAX_SEARCH_QUERY_BYTES + 1).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn search_target_mailboxes_gate_below_at_and_above_the_limit() {
        assert_eq!(check_search_target_mailboxes(0).unwrap(), 0);
        assert_eq!(
            check_search_target_mailboxes(MAX_SEARCH_TARGET_MAILBOXES - 1).unwrap(),
            MAX_SEARCH_TARGET_MAILBOXES - 1
        );
        assert_eq!(
            check_search_target_mailboxes(MAX_SEARCH_TARGET_MAILBOXES).unwrap(),
            MAX_SEARCH_TARGET_MAILBOXES
        );
        assert!(matches!(
            check_search_target_mailboxes(MAX_SEARCH_TARGET_MAILBOXES + 1).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    // --- Phase 5 run 4: search-execution budget + deadline -------------

    #[test]
    fn the_search_execution_limits_are_the_documented_bounds() {
        assert_eq!(MAX_SEARCH_ACQUIRE, 50);
        // The deadline is a positive, bounded wall-clock budget.
        assert_eq!(SEARCH_DEADLINE, std::time::Duration::from_secs(20));
        assert!(SEARCH_DEADLINE > std::time::Duration::ZERO);
    }

    #[test]
    fn search_acquire_share_splits_fairly_below_exact_and_above_an_even_divide() {
        // Zero mailboxes: nothing to search, zero share (never a divide-by-zero).
        assert_eq!(search_acquire_share(50, 0), 0);
        // One mailbox: the whole budget.
        assert_eq!(search_acquire_share(50, 1), 50);
        // An EXACT divide: each mailbox gets its equal share.
        assert_eq!(search_acquire_share(50, 5), 10);
        // BELOW an even divide (49/5): ceil keeps each share just over the
        // floor so the running total still reaches the budget rather than
        // starving the last mailbox (10,10,10,10,9 under the global cap).
        assert_eq!(search_acquire_share(49, 5), 10);
        // ABOVE an even divide (51/5): ceil again (11), and the GLOBAL
        // MAX_SEARCH_ACQUIRE total — enforced by the driver — is what keeps
        // the sum from exceeding the budget.
        assert_eq!(search_acquire_share(51, 5), 11);
        // More mailboxes than budget: at least one hit each until the budget
        // runs out (the driver's running total stops the rest).
        assert_eq!(search_acquire_share(3, 10), 1);
        // A zero budget yields a zero share regardless of mailbox count.
        assert_eq!(search_acquire_share(0, 4), 0);
    }

    // --- Phase 5 run 3: search-pin limits ------------------------------

    #[test]
    fn the_search_pin_limits_are_the_documented_bounds() {
        assert_eq!(MAX_SEARCH_PINS, 2_000);
        assert_eq!(SEARCH_PIN_TTL, 30 * 24 * 60 * 60);
    }

    #[test]
    fn search_pin_cap_below_at_and_above_the_limit() {
        // Below the cap: every requested pin fits, none refused.
        let below = cap_search_pins(0, MAX_SEARCH_PINS - 1);
        assert_eq!(below.accepted, MAX_SEARCH_PINS - 1);
        assert_eq!(below.refused, 0);

        // Exactly at the cap: every requested pin fits, none refused.
        let exact = cap_search_pins(0, MAX_SEARCH_PINS);
        assert_eq!(exact.accepted, MAX_SEARCH_PINS);
        assert_eq!(exact.refused, 0);

        // One over the cap: all but one fit; the single over-cap UID is
        // refused and reported, never silently dropped.
        let above = cap_search_pins(0, MAX_SEARCH_PINS + 1);
        assert_eq!(above.accepted, MAX_SEARCH_PINS);
        assert_eq!(above.refused, 1);
    }

    #[test]
    fn search_pin_cap_accounts_for_existing_pins_and_never_evicts() {
        // An account already at the cap accepts nothing new and refuses all.
        let full = cap_search_pins(MAX_SEARCH_PINS, 5);
        assert_eq!(full.accepted, 0);
        assert_eq!(full.refused, 5, "existing pins are never evicted to make room");

        // One slot left: exactly one new pin fits, the rest are refused.
        let one_left = cap_search_pins(MAX_SEARCH_PINS - 1, 3);
        assert_eq!(one_left.accepted, 1);
        assert_eq!(one_left.refused, 2);

        // Over-count existing (should not happen, but must not underflow):
        // room is zero, everything refused.
        let over = cap_search_pins(MAX_SEARCH_PINS + 10, 4);
        assert_eq!(over.accepted, 0);
        assert_eq!(over.refused, 4);

        // No new pins requested: nothing accepted, nothing refused.
        let none = cap_search_pins(10, 0);
        assert_eq!(none.accepted, 0);
        assert_eq!(none.refused, 0);
    }

    #[test]
    fn search_pin_ttl_release_below_at_and_above_the_bound() {
        let pinned_at = 1_000_000i64;
        // Just inside the TTL (one second short): not yet expired.
        assert!(!search_pin_expired(pinned_at, pinned_at + SEARCH_PIN_TTL - 1));
        // Exactly at the TTL: released (inclusive boundary).
        assert!(search_pin_expired(pinned_at, pinned_at + SEARCH_PIN_TTL));
        // Past the TTL: released.
        assert!(search_pin_expired(pinned_at, pinned_at + SEARCH_PIN_TTL + 1));
        // Just acquired (zero elapsed): not expired.
        assert!(!search_pin_expired(pinned_at, pinned_at));
        // A clock that went backwards is treated as not expired, never a panic.
        assert!(!search_pin_expired(pinned_at, pinned_at - 5));
    }
}
