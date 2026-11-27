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
//! - [`scheduler`]: one job at a time, during the recording on AC power and
//!   after Stop, through to the transcript rows.
//! - [`power`]: AC or battery.

#[cfg(target_os = "macos")]
pub mod engine;
pub mod merge;
pub mod models;
pub mod plan;
pub mod power;
pub mod protocol;
pub mod scheduler;
pub mod spans;
pub mod stream;
pub mod worker;

use protocol::{SegmentRequest, TranscriptLine};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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
}

/// Why a job stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// Its recording was deleted, or the app is quitting.
    Cancelled,
    /// A plain sentence for the user.
    Failed(String),
}

/// Starts transcription sessions; the worker process in the app, a fake in
/// tests.
pub trait Transcriber: Send + Sync {
    fn open(
        &self,
        config: &JobConfig,
        cancel: Arc<AtomicBool>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError>;
}

/// One job's engine, kept from its first segment to its last.
pub trait TranscriptionSession: Send {
    /// Transcribes one segment file holding `audio_ms` of audio.
    fn transcribe(
        &mut self,
        request: &SegmentRequest,
        audio_ms: u64,
    ) -> Result<Vec<TranscriptLine>, JobError>;
}
