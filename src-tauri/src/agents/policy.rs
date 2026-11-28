//! Central bounds and defaults for standing Inbox Assistants.

pub(crate) const CONCURRENT_JOBS: i64 = 1;
pub(crate) const LEASE_SECONDS: i64 = 180;
pub(crate) const POLL_MILLIS: u64 = 2_000;

pub(crate) const MIN_AGE_DAYS: i64 = 1;
pub(crate) const MAX_AGE_DAYS: i64 = 365;
pub(crate) const MIN_THREADS_PER_RUN: i64 = 1;
pub(crate) const MAX_THREADS_PER_RUN: i64 = 30;

pub(crate) const STALE_READ_MIN_CHARS: usize = 1_200;
pub(crate) const MAX_CANDIDATE_MESSAGES: usize = 8;
pub(crate) const MAX_CANDIDATE_BODY_CHARS: usize = 4_000;
pub(crate) const MAX_RELATED_THREADS: usize = 3;
pub(crate) const MAX_RELATED_EVENTS: usize = 3;
pub(crate) const MAX_EXCERPT_CHARS: usize = 240;

pub(crate) const MIN_RESOLVED_CONFIDENCE: f64 = 0.75;
pub(crate) const MIN_REPLY_CONFIDENCE: f64 = 0.70;

/// Sender Hygiene Advisor: how far back triage telemetry counts.
pub(crate) const HYGIENE_LOOKBACK_DAYS: i64 = 90;
/// Quick dismissals (opened, archived or trashed within a second) needed
/// before a sender qualifies on dismissal behavior...
pub(crate) const HYGIENE_MIN_QUICK_DISMISSALS: i64 = 3;
/// ...together with this share of the sender's Inbox dispositions.
pub(crate) const HYGIENE_MIN_QUICK_RATE: f64 = 0.6;
/// Unread Inbox conversations older than the assistant's age setting that
/// qualify a sender on their own.
pub(crate) const HYGIENE_MIN_AGING_UNREAD: i64 = 3;
/// A sender whose suggestion was dismissed is left alone for this long.
pub(crate) const HYGIENE_DISMISSED_COOLDOWN_DAYS: i64 = 30;
/// Most of a sender's Inbox conversations a proposal carries.
pub(crate) const HYGIENE_MAX_INBOX_THREADS: usize = 25;
/// Conversations listed as evidence on a proposal.
pub(crate) const HYGIENE_EVIDENCE_THREADS: usize = 3;

/// Stale Reads Digest: most items sent to the model in one batched request.
pub(crate) const DIGEST_ITEMS_PER_REQUEST: usize = 15;
/// Longest gist, in characters, for one digest item.
pub(crate) const DIGEST_GIST_MAX_CHARS: usize = 160;
/// Most body text, in characters, sent to the model for one digest item.
pub(crate) const DIGEST_ITEM_BODY_CHARS: usize = 1_500;
/// Longest deadline label a digest item keeps.
pub(crate) const DIGEST_DEADLINE_MAX_CHARS: usize = 40;

/// How long a pending suggestion waits, from its creation, before it expires.
/// Expired suggestions disappear and are not proposed again for the same
/// conversation revision; new mail in the conversation allows a fresh one.
/// Stale Read deadlines are free text ("Dec 16", no year), so they do not
/// shorten the limit.
pub(crate) const EXPIRY_DRAFT_REPLY_DAYS: i64 = 7;
pub(crate) const EXPIRY_ARCHIVE_WITH_SUMMARY_DAYS: i64 = 14;
pub(crate) const EXPIRY_ARCHIVE_AS_EXPIRED_DAYS: i64 = 21;
pub(crate) const EXPIRY_ARCHIVE_AS_RESOLVED_DAYS: i64 = 21;
pub(crate) const EXPIRY_SENDER_HYGIENE_DAYS: i64 = 30;
pub(crate) const EXPIRY_DIGEST_ITEM_DAYS: i64 = 14;

pub(crate) fn valid_schedule_hours(hours: Option<i64>) -> bool {
    matches!(hours, None | Some(24) | Some(168))
}
