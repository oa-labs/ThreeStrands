//! On-device transcription of meeting recordings (stage 3).
//!
//! - [`models`]: the whisper and voice activity models on disk, and
//!   downloading them with pinned sizes and hashes.
//! - [`worker`]: whisper.cpp in a killable child process, one per job, and
//!   [`protocol`], how the app talks to it. Inside the worker, [`stream`]
//!   carries speech across segment files and [`spans`] decides what whisper
//!   sees; [`engine`] is whisper.cpp itself (macOS only).
//! - [`plan`]: which segment files a recording has, from its manifests.
//! - [`merge`]: the post-filter and the You/Others merge with echo drop.
//! - [`scheduler`]: one job at a time, in batches during the recording (on
//!   AC power, with memory to spare) and after Stop, through to the
//!   transcript rows.
//! - [`power`]: AC or battery; [`memory`]: memory pressure and what is
//!   available.

#[cfg(target_os = "macos")]
pub mod engine;
pub mod memory;
pub mod merge;
pub mod models;
pub mod plan;
pub mod power;
pub mod protocol;
pub mod scheduler;
pub mod spans;
pub mod stream;
pub mod worker;

use protocol::{HeldAudio, SegmentRequest, TranscriptLine};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The engine name stored with a transcript.
pub const ENGINE: &str = "whisper.cpp";

/// What a job's worker is started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobConfig {
    pub model_path: PathBuf,
    pub vad_model_path: PathBuf,
    /// A whisper language code, or `None` to detect it.
    pub language: Option<String>,
    pub threads: u32,
    /// The physical footprint the worker may reach before it is killed: the
    /// model's budget in the policy module.
    pub memory_bytes: u64,
}

/// Why a job stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// Its recording was deleted, or the app is quitting.
    Cancelled,
    /// The worker was stopped to give memory back, or because transcribing
    /// during the meeting was turned off. What it finished stands; the job
    /// goes on from there later.
    Interrupted,
    /// A plain sentence for the user.
    Failed(String),
}

/// What a session checks while it waits on its worker, often enough to stop
/// within a fraction of a second.
pub trait StopCheck: Send + Sync {
    /// [`JobError::Cancelled`] or [`JobError::Interrupted`] to stop now;
    /// `None` to go on.
    fn stop_reason(&self) -> Option<JobError>;
}

/// A plain cancel flag.
impl StopCheck for AtomicBool {
    fn stop_reason(&self) -> Option<JobError> {
        self.load(Ordering::SeqCst).then_some(JobError::Cancelled)
    }
}

/// Starts transcription sessions; the worker process in the app, a fake in
/// tests.
pub trait Transcriber: Send + Sync {
    fn open(
        &self,
        config: &JobConfig,
        stop: Arc<dyn StopCheck>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError>;
}

/// One segment file's result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Transcribed {
    pub lines: Vec<TranscriptLine>,
    /// What the worker now holds back for the file's track.
    pub held: Option<HeldAudio>,
}

/// One worker: from its first segment to the end of its batch or job.
pub trait TranscriptionSession: Send {
    /// Transcribes one segment file holding `audio_ms` of audio.
    fn transcribe(
        &mut self,
        request: &SegmentRequest,
        audio_ms: u64,
    ) -> Result<Transcribed, JobError>;
}
