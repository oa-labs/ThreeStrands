//! Helpers shared by the recording tests.

use std::path::{Path, PathBuf};

/// A fresh directory under the system temp directory, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("threestrands-rec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Memory pressure normal and memory to spare.
pub(crate) struct PlentyOfMemory;

impl crate::recording::transcribe::memory::MemoryProbe for PlentyOfMemory {
    fn pressure(&self) -> crate::recording::transcribe::memory::MemoryPressure {
        crate::recording::transcribe::memory::MemoryPressure::Normal
    }

    fn available_bytes(&self) -> Option<u64> {
        Some(u64::MAX)
    }
}

/// Transcription that never runs: no model is downloaded, and the worker
/// refuses to start. For tests of everything else.
pub(crate) fn idle_transcription(root: &Path) -> crate::recording::runtime::TranscriptionParts {
    use crate::recording::transcribe::models::{ModelStore, Pins, UrlPolicy};
    use crate::recording::transcribe::{
        JobConfig, JobError, StopCheck, Transcriber, TranscriptionSession,
    };
    use std::sync::Arc;

    struct NoWorker;
    impl Transcriber for NoWorker {
        fn open(
            &self,
            _: &JobConfig,
            _: Arc<dyn StopCheck>,
        ) -> Result<Box<dyn TranscriptionSession>, JobError> {
            Err(JobError::Failed("No worker in this test.".into()))
        }
    }
    struct Ac;
    impl crate::recording::transcribe::power::PowerSource for Ac {
        fn on_ac_power(&self) -> bool {
            true
        }
    }
    struct Disk;
    impl crate::recording::DiskProbe for Disk {
        fn free_bytes(&self) -> Result<u64, String> {
            Ok(u64::MAX)
        }
    }
    crate::recording::runtime::TranscriptionParts {
        transcriber: Arc::new(NoWorker),
        power: Arc::new(Ac),
        memory: Arc::new(PlentyOfMemory),
        models: Arc::new(ModelStore::new(
            root.join("recording-models"),
            Pins::pinned(),
            Arc::new(Disk),
            UrlPolicy {
                https_only: true,
                allowed: Arc::new(|_: &reqwest::Url| false),
            },
        )),
    }
}

/// Heap accounting for the memory tests: the bytes this thread has
/// allocated and not freed while counting is on, and their high-water mark.
///
/// The test binary's global allocator wraps the system allocator and counts
/// on the allocating (or freeing) thread only, so tests running in parallel
/// on other threads never disturb a measurement. A test that measures does
/// all the work it measures on its own thread.
pub(crate) mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static LIVE: Cell<i64> = const { Cell::new(0) };
        static PEAK: Cell<i64> = const { Cell::new(0) };
    }

    fn record(delta: i64) {
        let _ = COUNTING.try_with(|counting| {
            if counting.get() {
                let live = LIVE.get() + delta;
                LIVE.set(live);
                if live > PEAK.get() {
                    PEAK.set(live);
                }
            }
        });
    }

    pub(crate) struct Counting;

    // SAFETY: every call is passed to the system allocator unchanged; the
    // bookkeeping only touches const-initialized thread-locals without
    // destructors, which never allocate.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = System.alloc(layout);
            if !pointer.is_null() {
                record(layout.size() as i64);
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let pointer = System.alloc_zeroed(layout);
            if !pointer.is_null() {
                record(layout.size() as i64);
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            System.dealloc(pointer, layout);
            record(-(layout.size() as i64));
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let moved = System.realloc(pointer, layout, new_size);
            if !moved.is_null() {
                record(new_size as i64 - layout.size() as i64);
            }
            moved
        }
    }

    #[global_allocator]
    static COUNTING_ALLOCATOR: Counting = Counting;

    /// Starts counting on this thread from zero.
    pub(crate) fn start() {
        LIVE.set(0);
        PEAK.set(0);
        COUNTING.set(true);
    }

    /// Stops counting on this thread.
    pub(crate) fn stop() {
        COUNTING.set(false);
    }

    /// Bytes allocated on this thread and not freed since [`start`]
    /// (negative when it freed more than it allocated).
    pub(crate) fn live() -> i64 {
        LIVE.get()
    }

    /// The highest [`live`] since [`start`] or the last [`reset_peak`].
    pub(crate) fn peak() -> i64 {
        PEAK.get()
    }

    /// Starts a new high-water mark from what is live now.
    pub(crate) fn reset_peak() {
        PEAK.set(LIVE.get());
    }

    #[test]
    fn counts_this_threads_live_bytes_and_peak() {
        start();
        let big = vec![0u8; 100_000];
        let other = std::thread::spawn(|| vec![0u8; 1_000_000].len());
        assert_eq!(other.join().unwrap(), 1_000_000);
        assert!(live() >= 100_000 && live() < 110_000, "{}", live());
        drop(big);
        assert!(live() < 10_000, "{}", live());
        assert!(
            peak() >= 100_000 && peak() < 110_000,
            "other threads aren't counted"
        );
        reset_peak();
        assert_eq!(peak(), live());
        stop();
        let ignored = vec![0u8; 50_000];
        assert!(live() < 10_000);
        drop(ignored);
    }
}
