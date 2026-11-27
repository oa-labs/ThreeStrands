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

/// Transcription that never runs: no model is downloaded, and the worker
/// refuses to start. For tests of everything else.
pub(crate) fn idle_transcription(root: &Path) -> crate::recording::runtime::TranscriptionParts {
    use crate::recording::transcribe::models::{ModelStore, Pins, UrlPolicy};
    use crate::recording::transcribe::{JobConfig, JobError, Transcriber, TranscriptionSession};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    struct NoWorker;
    impl Transcriber for NoWorker {
        fn open(
            &self,
            _: &JobConfig,
            _: Arc<AtomicBool>,
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
