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
    /// Never synced: Drafts (drafts are local), `\All` and `\Flagged`
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
}

impl SyncLimits {
    /// INBOX limits — the only mailbox class this slice syncs. The window is
    /// the INBOX ceiling. Slice 5b adds `Sent`/`Folder` constructors.
    pub const fn inbox() -> Self {
        Self {
            mailbox_window: MAX_INBOX_SYNC_MESSAGES,
        }
    }
}

impl Default for SyncLimits {
    fn default() -> Self {
        Self::inbox()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each limit is tested just below, exactly at, and just above the
    // boundary, per AGENTS.md "Email rendering" and the emailRenderingPolicy.ts
    // pattern. The boundary itself is accepted; one past it is a typed error,
    // not a clamp.

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
        assert!(MAX_MIME_PART_COUNT >= crate::mime::MAX_MIME_PARTS);
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
        assert!(UID_BATCH_SIZE > 0);
        assert_eq!(UID_BATCH_SIZE, 500);
    }

    #[test]
    fn the_token_backfill_batch_size_is_a_positive_bound() {
        assert!(TOKEN_BACKFILL_BATCH_SIZE > 0);
        // One bound variable is the account id; the chunk plus it must stay
        // under SQLite's oldest default limit of 999.
        assert!(TOKEN_QUERY_CHUNK > 0 && TOKEN_QUERY_CHUNK + 1 <= 999);
        assert_eq!(TOKEN_BACKFILL_BATCH_SIZE, 500);
    }

    #[test]
    fn sync_limits_default_to_the_inbox_ceiling() {
        assert_eq!(SyncLimits::default(), SyncLimits::inbox());
        assert_eq!(SyncLimits::default().mailbox_window, MAX_INBOX_SYNC_MESSAGES);
    }
}
