//! Timing policy for replicated sync, and the wall clock it reads. Every
//! sync time interval lives here, so one file answers "how often" and "how
//! long" for the whole engine; wire-format limits live in
//! `threestrands_sync_envelope::limits` instead.

/// A device republishes its signed head at least this often even when
/// nothing in it changed, so other devices can tell how recently it synced.
/// Between heartbeats a head is republished only when its content changes.
pub const HEAD_HEARTBEAT_MS: i64 = 6 * 60 * 60 * 1000;

/// The current wall-clock time in milliseconds since the Unix epoch, as the
/// sync engine sees it. Used only for display and for measuring how
/// recently a device synced, never for ordering. Tests can pin it per
/// thread with [`set_test_clock`].
pub(crate) fn now_ms() -> i64 {
    #[cfg(test)]
    if let Some(now) = TEST_CLOCK.with(std::cell::Cell::get) {
        return now;
    }
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
thread_local! {
    static TEST_CLOCK: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Pins [`now_ms`] for the current thread (`None` restores the real clock).
/// `#[tokio::test]` runs on one thread by default, so a pinned clock covers
/// the whole test.
#[cfg(test)]
pub(crate) fn set_test_clock(now: Option<i64>) {
    TEST_CLOCK.with(|clock| clock.set(now));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_clock_is_used_until_it_is_released() {
        set_test_clock(Some(42));
        assert_eq!(now_ms(), 42);
        set_test_clock(None);
        assert!(now_ms() > 1_700_000_000_000);
    }
}
