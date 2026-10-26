//! Unix timestamps with explicit units. Consumers may use a fixed Clock in tests.
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) trait Clock {
    fn now(&self) -> SystemTime;

    fn unix_seconds(&self) -> u64 {
        self.now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn unix_millis(&self) -> i64 {
        chrono::DateTime::<chrono::Utc>::from(self.now()).timestamp_millis()
    }
}

struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        #[cfg(test)]
        if let Some(time) = TEST_TIME.with(|time| time.get()) {
            return time;
        }
        SystemTime::now()
    }
}

pub(crate) fn unix_seconds() -> u64 {
    SystemClock.unix_seconds()
}
pub(crate) fn unix_millis() -> i64 {
    SystemClock.unix_millis()
}

#[cfg(test)]
thread_local! {
    static TEST_TIME: std::cell::Cell<Option<SystemTime>> = const { std::cell::Cell::new(None) };
}

/// Freeze all shared timestamp calls in a synchronous scope on this thread.
/// Use a current-thread executor for async tests; the override does not cross spawned threads.
#[cfg(test)]
pub(crate) fn with_time<T>(time: SystemTime, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<SystemTime>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_TIME.with(|time| time.set(self.0));
        }
    }
    let _restore = Restore(TEST_TIME.with(|current| current.replace(Some(time))));
    run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct FixedClock(SystemTime);
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    #[test]
    fn shared_calls_can_be_frozen_and_restore_after_nested_scopes_and_panics() {
        with_time(UNIX_EPOCH + Duration::from_millis(1_234_567), || {
            assert_eq!(unix_seconds(), 1_234);
            assert_eq!(unix_millis(), 1_234_567);
            let _ = std::panic::catch_unwind(|| {
                with_time(UNIX_EPOCH, || {
                    assert_eq!(unix_millis(), 0);
                    panic!("exercise guard");
                })
            });
            assert_eq!(unix_millis(), 1_234_567);
        });
        assert!(unix_seconds() > 1_234);
    }

    #[test]
    fn explicit_units_share_a_deterministic_source() {
        let clock = FixedClock(UNIX_EPOCH + Duration::from_millis(1_234_567));
        assert_eq!(clock.unix_seconds(), 1_234);
        assert_eq!(clock.unix_millis(), 1_234_567);
        let before_epoch = FixedClock(UNIX_EPOCH - Duration::from_millis(1));
        assert_eq!(before_epoch.unix_seconds(), 0);
        assert_eq!(before_epoch.unix_millis(), -1);
    }
}
