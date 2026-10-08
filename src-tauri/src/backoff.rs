use chrono::{DateTime, Duration, Utc};
use rand::RngExt;

const RETRY_BACKOFF_BASE_SECS: i64 = 30;
const RETRY_BACKOFF_MAX_SECS: i64 = 60 * 60;
const RETRY_BACKOFF_MAX_EXPONENT: u32 = 16;
const RETRY_BACKOFF_JITTER_WINDOW_MILLIS: i64 = 1_000;

/// Returns the shared exponential retry delay before jitter. `attempts` is
/// the number of failed attempts so far and is expected to be at least one.
pub(crate) fn retry_delay_secs(attempts: u32) -> i64 {
    let exponent = attempts.saturating_sub(1).min(RETRY_BACKOFF_MAX_EXPONENT);
    RETRY_BACKOFF_BASE_SECS
        .saturating_mul(1_i64 << exponent)
        .min(RETRY_BACKOFF_MAX_SECS)
}

/// Returns a durable retry timestamp using the shared exponential policy and
/// a small random jitter window. The final delay, including jitter, never
/// exceeds the configured cap.
pub(crate) fn retry_at(attempts: u32) -> String {
    let jitter_millis = rand::rng().random_range(0..RETRY_BACKOFF_JITTER_WINDOW_MILLIS);
    retry_at_with_jitter(Utc::now(), attempts, jitter_millis).to_rfc3339()
}

fn retry_at_with_jitter(now: DateTime<Utc>, attempts: u32, jitter_millis: i64) -> DateTime<Utc> {
    let delay_millis = retry_delay_secs(attempts)
        .saturating_mul(1_000)
        .saturating_add(jitter_millis)
        .min(RETRY_BACKOFF_MAX_SECS * 1_000);
    now + Duration::milliseconds(delay_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_is_exponential_and_bounded() {
        assert_eq!(retry_delay_secs(1), 30);
        assert_eq!(retry_delay_secs(2), 60);
        assert_eq!(retry_delay_secs(8), 3_600);
        assert_eq!(retry_delay_secs(u32::MAX), 3_600);
    }

    #[test]
    fn retry_jitter_stays_within_the_shared_cap() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(
            retry_at_with_jitter(now, 1, 0).signed_duration_since(now),
            Duration::seconds(30)
        );
        assert_eq!(
            retry_at_with_jitter(now, 1, RETRY_BACKOFF_JITTER_WINDOW_MILLIS - 1)
                .signed_duration_since(now),
            Duration::milliseconds(30_999)
        );
        assert_eq!(
            retry_at_with_jitter(now, u32::MAX, RETRY_BACKOFF_JITTER_WINDOW_MILLIS - 1)
                .signed_duration_since(now),
            Duration::hours(1)
        );
    }
}
