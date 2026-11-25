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

pub(crate) fn valid_schedule_hours(hours: Option<i64>) -> bool {
    matches!(hours, None | Some(24) | Some(168))
}
