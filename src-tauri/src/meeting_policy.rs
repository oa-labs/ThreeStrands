//! Every limit on meeting preparation's local retrieval, in one place. The
//! client receives them with each result (see [`MeetingLimits`]) so it can
//! say what was left out instead of implying the lists are complete.
//!
//! These bound local reads only. Requests to an AI provider get their own
//! budgets when that stage is built; nothing here is sent anywhere.
use serde::Serialize;

/// Most events one readiness request may name, such as a day's meetings.
pub const MAX_READINESS_EVENTS: usize = 50;
/// Most notes listed for this occurrence, and for the series as a whole.
pub const MAX_MEETING_NOTE_SOURCES: usize = 20;
/// Most earlier occurrences of the series listed, newest first.
pub const MAX_PRIOR_OCCURRENCES: usize = 8;
/// Longest text kept from one meeting block, in UTF-8 bytes.
pub const MAX_MEETING_EXCERPT_BYTES: usize = 4096;
/// Longest preview kept of a reusable meeting note linked from a block.
pub const MAX_LINKED_NOTE_PREVIEW_BYTES: usize = 1024;
/// Most attendee addresses used to look for related mail.
pub const MAX_MEETING_ATTENDEES: usize = 50;
/// Most conversations listed, the originating one included.
pub const MAX_MEETING_CONVERSATIONS: usize = 10;
/// Most recent conversations with attendees examined to rank those that
/// involve several attendees first.
pub const MAX_ATTENDEE_CANDIDATE_THREADS: usize = 100;
/// How far back attendee correspondence is looked for.
pub const MEETING_MAIL_LOOKBACK_DAYS: i64 = 90;
/// Most commitments listed, definite ones before candidates.
pub const MAX_MEETING_COMMITMENTS: usize = 25;

/// The limits a result was produced under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingLimits {
    pub note_sources: usize,
    pub prior_occurrences: usize,
    pub excerpt_bytes: usize,
    pub linked_note_preview_bytes: usize,
    pub attendees: usize,
    pub conversations: usize,
    pub mail_lookback_days: i64,
    pub commitments: usize,
}

pub const LIMITS: MeetingLimits = MeetingLimits {
    note_sources: MAX_MEETING_NOTE_SOURCES,
    prior_occurrences: MAX_PRIOR_OCCURRENCES,
    excerpt_bytes: MAX_MEETING_EXCERPT_BYTES,
    linked_note_preview_bytes: MAX_LINKED_NOTE_PREVIEW_BYTES,
    attendees: MAX_MEETING_ATTENDEES,
    conversations: MAX_MEETING_CONVERSATIONS,
    mail_lookback_days: MEETING_MAIL_LOOKBACK_DAYS,
    commitments: MAX_MEETING_COMMITMENTS,
};

/// Most decisions and actions one follow-through section saves.
pub const MAX_FOLLOW_THROUGH_ITEMS: usize = 50;
/// Longest decision or action, in characters, after spaces are collapsed.
pub const MAX_FOLLOW_THROUGH_ITEM_CHARS: usize = 500;
/// How long an operation receipt is kept, so a retry within it returns the
/// original result instead of writing again.
pub const OPERATION_RECEIPT_RETENTION_DAYS: i64 = 30;
/// Most operation receipts kept; the oldest go first.
pub const MAX_OPERATION_RECEIPTS: usize = 500;
/// Most follow-up draft handoffs remembered; the oldest go first. A
/// forgotten handoff can't be resumed, so reopening it starts a new draft.
pub const MAX_DRAFT_HANDOFFS: usize = 500;
/// How long a follow-up draft handoff is remembered.
pub const DRAFT_HANDOFF_RETENTION_DAYS: i64 = 90;
/// Longest subject a follow-up draft can start with, in characters.
pub const MAX_FOLLOW_UP_SUBJECT_CHARS: usize = 998;
/// Longest body a follow-up draft can start with, in UTF-8 bytes.
pub const MAX_FOLLOW_UP_BODY_BYTES: usize = 65_536;
/// Most recipients a follow-up draft can start with.
pub const MAX_FOLLOW_UP_RECIPIENTS: usize = 100;
