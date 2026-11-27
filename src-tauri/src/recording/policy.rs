//! Every limit, threshold, and model pin of meeting recording, in one place.
//! The recording service, the transcription worker, and the recap request all
//! read these constants and check functions; nothing else hard-codes a number.
//!
//! Nothing here is sent anywhere. The model URLs are fetched only when the
//! user chooses to download a model, and each download is checked against its
//! pinned size and SHA-256 before it is used.
// Offers (stage 5) and the recap (stage 4) read the limits here that
// nothing uses yet.
#![allow(dead_code)]

use std::time::Duration;

const MIB: u64 = 1024 * 1024;

/// The first macOS release (major, minor) with the Core Audio process tap
/// that captures other apps' audio. Below it, recordings are microphone-only.
pub const SYSTEM_AUDIO_MIN_MACOS: (u32, u32) = (14, 4);

/// Whether this macOS version can capture system audio.
pub fn macos_has_system_audio(major: u32, minor: u32) -> bool {
    (major, minor) >= SYSTEM_AUDIO_MIN_MACOS
}

/// Longest single recording. The service stops at this length.
pub const MAX_RECORDING: Duration = Duration::from_secs(4 * 60 * 60);
/// Free disk space the recording volume must keep, in bytes. Recording stops
/// (and will not start) below it, so a long meeting cannot fill the disk.
pub const MIN_FREE_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Length of one audio segment on disk, so a crash loses at most this much.
pub const SEGMENT_LENGTH: Duration = Duration::from_secs(60);
/// Sample rate of the written audio and of what whisper reads, in hertz.
pub const OUTPUT_SAMPLE_RATE: u32 = 16_000;
/// How often buffered audio is flushed to disk.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// A track that delivers no frames for this long (while recording and not
/// paused) is treated as stalled and restarted.
pub const STALL_AFTER: Duration = Duration::from_secs(5);
/// Waits before each restart of a stalled or failed track. When the last
/// restart also fails, the track is failed for the rest of the recording.
pub const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
/// A track's audio is treated as contiguous while each callback's host time
/// lands within this much of where the previous callback's audio ended.
/// Beyond it (sleep, a dropout, audio lost to an overrun) the segment ends
/// and a gap is recorded, so timestamps stay true to the wall clock.
pub const CAPTURE_GAP_TOLERANCE: Duration = Duration::from_millis(100);
/// Audio a capture ring buffer holds between the device callback and the
/// capture thread. Beyond it, new audio is dropped and counted.
pub const CAPTURE_RING: Duration = Duration::from_secs(2);
/// How often the capture thread drains its ring buffer.
pub const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Quiet time after the last device notification (default input or output
/// changed, a device added or removed) before capture follows the change,
/// so a burst of notifications moves the track once.
pub const DEVICE_CHANGE_DEBOUNCE: Duration = Duration::from_millis(500);

/// How often the retention sweep runs while the app is open (it also runs
/// at launch).
pub const RETENTION_SWEEP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Audio age, counted from when the recording ended, at which **Keep Audio**
/// set to 7 days deletes it.
pub const KEEP_AUDIO_SHORT: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The same for **Keep Audio** set to 30 days.
pub const KEEP_AUDIO_LONG: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long before a meeting starts the app may offer to record it.
pub const OFFER_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Voice activity detection (transcription stage): probability at which
/// speech starts.
pub const VAD_START_THRESHOLD: f32 = 0.50;
/// Probability below which ongoing speech ends.
pub const VAD_CONTINUE_THRESHOLD: f32 = 0.35;
/// Audio kept before a detected speech start, in milliseconds.
pub const VAD_PAD_BEFORE_MS: u32 = 300;
/// Audio kept after a detected speech end, in milliseconds.
pub const VAD_PAD_AFTER_MS: u32 = 400;
/// Shortest span treated as speech, in milliseconds.
pub const VAD_MIN_SPEECH_MS: u32 = 250;
/// Longest single speech span handed to whisper, in milliseconds; longer
/// spans are split. Padding included, so whisper never sees more than this.
pub const VAD_MAX_SPAN_MS: u32 = 30_000;
/// Silence that ends a speech span, in milliseconds. Speech that reaches
/// within this much of the end of the audio may still be going on, so at a
/// segment file's end it is carried into the next segment of the run.
pub const VAD_MIN_SILENCE_MS: u32 = 500;
/// whisper.cpp derives the continue threshold from the start threshold as
/// `threshold - 0.15`; [`VAD_CONTINUE_THRESHOLD`] is that value, checked by a
/// test.
pub const VAD_CONTINUE_OFFSET: f32 = 0.15;

/// Merging the two tracks' transcripts: a microphone segment whose text
/// overlaps a system segment by at least this fraction (0 to 1) is dropped
/// as speaker echo.
pub const ECHO_DROP_THRESHOLD: f32 = 0.5;
/// Slack in milliseconds, either side, when deciding that two segments
/// overlap in time.
pub const MERGE_TIME_SLACK_MS: i64 = 1000;

/// Lines dropped from a transcript as whisper hallucinations, compared by
/// their words (case and punctuation ignored). Whisper produces these from
/// silence, music, and noise; voice activity detection removes most of the
/// audio that causes them, and this catches the rest. Text wholly inside
/// brackets or parentheses ("[BLANK_AUDIO]", "(music)") is dropped too.
pub const HALLUCINATIONS: &[&str] = &[
    "Thank you.",
    "Thanks for watching!",
    "Thank you for watching.",
    "Thank you so much for watching.",
    "Please subscribe.",
    "Subtitles by the Amara.org community",
    "you",
    "[BLANK_AUDIO]",
];

/// Whisper decoding: beam search width (the spike's accuracy figures used 5).
pub const WHISPER_BEAM_SIZE: i32 = 5;
/// Whisper's no-speech probability above which a window is treated as
/// silence.
pub const WHISPER_NO_SPEECH_THRESHOLD: f32 = 0.55;

/// The transcription worker: wall-clock time allowed to load the models.
pub const WORKER_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// The fixed part of the time allowed for one segment request.
pub const WORKER_REQUEST_BASE_TIMEOUT: Duration = Duration::from_secs(60);
/// The part that scales with the request's audio: this many seconds of
/// wall-clock time per second of audio. Measured real-time factors are
/// 0.04 to 0.32, so 4 leaves room for slow Macs and a busy machine.
pub const WORKER_REQUEST_TIME_PER_AUDIO: u32 = 4;
/// Physical footprint the Fast model's worker may reach before it is
/// killed. A two-hour recording in the VM peaked at 700 MiB on Metal (its
/// paravirtual GPU; the developer's Mac must confirm), so 1.5 times that
/// would be 1,050 MiB; the budget keeps the floor of 1.5 times the spike's
/// 796 MB on the CPU instead. See `docs/meeting-recording-plan.md`
/// (Memory during long meetings).
pub const WORKER_MEMORY_FAST_BYTES: u64 = 1_150 * MIB;
/// The same for the Accurate model: it peaked at 1,152 MiB in the VM, and
/// 1.5 times the spike's 1,317 MB on the CPU (1,884 MiB) is the floor.
pub const WORKER_MEMORY_ACCURATE_BYTES: u64 = 1_900 * MIB;
/// Memory the Mac must have available, on top of the model's budget, before
/// a worker starts, so starting one never pushes the Mac into swap.
pub const WORKER_START_MARGIN_BYTES: u64 = 512 * MIB;
/// Transcribing during a recording: finished audio that must be waiting
/// before a worker starts. The worker transcribes what is waiting and then
/// exits, so the model is in memory for a minute or two at a time rather
/// than for the whole meeting.
pub const LIVE_BATCH: Duration = Duration::from_secs(5 * 60);
/// Largest frame either side of the worker protocol may send. One segment's
/// lines are a few kilobytes.
pub const WORKER_MAX_FRAME_BYTES: usize = 1024 * 1024;
/// Largest segment file the worker reads: twice a full segment, so a
/// damaged or foreign file can't make it read without bound.
pub const WORKER_MAX_SEGMENT_FILE_BYTES: u64 =
    44 + 2 * SEGMENT_LENGTH.as_secs() * OUTPUT_SAMPLE_RATE as u64 * 2;
/// Scheduling priority (nice value) of the worker, so a transcription never
/// competes with the call it records.
pub const WORKER_NICE: i32 = 10;
/// How often the transcription thread looks for work without being woken
/// (new closed segments during a recording, a change of power source).
pub const TRANSCRIPTION_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Most whisper threads, whatever the core count.
pub const WORKER_MAX_THREADS: usize = 8;

/// Wall-clock time allowed for one segment request with this much audio.
/// The audio carried over from the previous segment is at most one span, so
/// it is added in.
pub fn worker_request_timeout(audio_ms: u64) -> Duration {
    let audio = Duration::from_millis(audio_ms + VAD_MAX_SPAN_MS as u64);
    WORKER_REQUEST_BASE_TIMEOUT + audio * WORKER_REQUEST_TIME_PER_AUDIO
}

/// The physical footprint a worker running `model` may reach.
pub const fn worker_memory_budget(model: TranscriptionModel) -> u64 {
    match model {
        TranscriptionModel::Fast => WORKER_MEMORY_FAST_BYTES,
        TranscriptionModel::Accurate => WORKER_MEMORY_ACCURATE_BYTES,
    }
}

/// Whether a worker running `model` with this physical footprint is over
/// its memory budget.
pub fn worker_memory_exceeded(model: TranscriptionModel, footprint_bytes: u64) -> bool {
    footprint_bytes > worker_memory_budget(model)
}

/// Whether a worker for `model` may start with this much memory available:
/// its budget plus [`WORKER_START_MARGIN_BYTES`].
pub fn memory_allows_start(model: TranscriptionModel, available_bytes: u64) -> bool {
    available_bytes >= worker_memory_budget(model).saturating_add(WORKER_START_MARGIN_BYTES)
}

/// Whether this much finished, untranscribed audio of a recording in
/// progress is enough to start a batch.
pub fn live_batch_due(waiting_ms: u64) -> bool {
    waiting_ms >= LIVE_BATCH.as_millis() as u64
}

/// Whisper threads for a machine with this many cores: half of them, so the
/// call and the rest of the Mac stay responsive, at least one and at most
/// [`WORKER_MAX_THREADS`].
pub fn transcription_threads(cores: usize) -> usize {
    (cores / 2).clamp(1, WORKER_MAX_THREADS)
}

/// Model downloads: redirects followed at most, all over https to
/// [`model_host_allowed`] hosts.
pub const MODEL_DOWNLOAD_MAX_REDIRECTS: usize = 5;
/// Time allowed to connect to the download host.
pub const MODEL_DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest wait for the next bytes of a download before it fails (and can
/// be resumed).
pub const MODEL_DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Shortest time between two download progress events (at most 4 a second).
pub const MODEL_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Whether a download may follow its `count`th redirect.
pub fn redirect_allowed(count: usize) -> bool {
    count <= MODEL_DOWNLOAD_MAX_REDIRECTS
}

/// Whether a model may be downloaded from (or redirected to) this host:
/// Hugging Face, which serves the pinned files and redirects to its own
/// content hosts. `huggingface.co` and its subdomains, and subdomains of
/// `hf.co`.
pub fn model_host_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "huggingface.co" || host.ends_with(".huggingface.co") || host.ends_with(".hf.co")
}

/// Whether a download progress event is due this long after the last one.
pub fn progress_due(since_last: Duration) -> bool {
    since_last >= MODEL_PROGRESS_INTERVAL
}

/// Whether a download of `remaining` more bytes leaves the floor recording
/// needs, [`MIN_FREE_DISK_BYTES`], free.
pub fn download_fits(free_bytes: u64, remaining: u64) -> bool {
    free_bytes >= remaining.saturating_add(MIN_FREE_DISK_BYTES)
}

/// Most transcript text sent in one recap request, in UTF-8 bytes. A starting
/// point: about a three-hour meeting, well inside current provider context
/// windows.
pub const MAX_RECAP_TRANSCRIPT_BYTES: usize = 400_000;
/// Longest recap summary, in words.
pub const MAX_RECAP_SUMMARY_WORDS: usize = 120;

/// A pinned model file the app may download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    pub file_name: &'static str,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: &'static str,
    pub url: &'static str,
}

/// The whisper model the user chose in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionModel {
    Fast,
    Accurate,
}

impl TranscriptionModel {
    pub const ALL: [Self; 2] = [Self::Fast, Self::Accurate];

    pub const fn file(self) -> ModelFile {
        match self {
            Self::Fast => ModelFile {
                file_name: "ggml-small.en-q5_1.bin",
                bytes: 190_098_681,
                sha256: "bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q5_1.bin",
            },
            Self::Accurate => ModelFile {
                file_name: "ggml-large-v3-turbo-q5_0.bin",
                bytes: 574_041_195,
                sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            },
        }
    }
}

/// The voice activity model, downloaded with the first transcription model.
pub const VAD_MODEL: ModelFile = ModelFile {
    file_name: "ggml-silero-v5.1.2.bin",
    bytes: 885_098,
    sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin",
};

/// Whether a recording that has run this long has reached the length limit.
pub fn length_limit_reached(elapsed: Duration) -> bool {
    elapsed >= MAX_RECORDING
}

/// Whether this much free space is below the floor recording needs.
pub fn disk_is_low(free_bytes: u64) -> bool {
    free_bytes < MIN_FREE_DISK_BYTES
}

/// Whether a transcript of this many bytes fits in one recap request.
pub fn recap_transcript_fits(bytes: usize) -> bool {
    bytes <= MAX_RECAP_TRANSCRIPT_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_audio_needs_macos_14_4() {
        assert!(!macos_has_system_audio(13, 6));
        assert!(!macos_has_system_audio(14, 3));
        assert!(macos_has_system_audio(14, 4));
        assert!(macos_has_system_audio(15, 0));
        assert!(macos_has_system_audio(26, 0));
    }

    #[test]
    fn length_limit_is_checked_below_at_and_above() {
        assert!(!length_limit_reached(
            MAX_RECORDING - Duration::from_millis(1)
        ));
        assert!(length_limit_reached(MAX_RECORDING));
        assert!(length_limit_reached(
            MAX_RECORDING + Duration::from_millis(1)
        ));
    }

    #[test]
    fn disk_floor_is_checked_below_at_and_above() {
        assert!(disk_is_low(MIN_FREE_DISK_BYTES - 1));
        assert!(!disk_is_low(MIN_FREE_DISK_BYTES));
        assert!(!disk_is_low(MIN_FREE_DISK_BYTES + 1));
    }

    #[test]
    fn recap_transcript_size_is_checked_below_at_and_above() {
        assert!(recap_transcript_fits(MAX_RECAP_TRANSCRIPT_BYTES - 1));
        assert!(recap_transcript_fits(MAX_RECAP_TRANSCRIPT_BYTES));
        assert!(!recap_transcript_fits(MAX_RECAP_TRANSCRIPT_BYTES + 1));
    }

    #[test]
    fn model_pins_are_well_formed() {
        let mut files: Vec<ModelFile> = TranscriptionModel::ALL.iter().map(|m| m.file()).collect();
        files.push(VAD_MODEL);
        for file in files {
            assert_eq!(file.sha256.len(), 64, "{}", file.file_name);
            assert!(
                file.sha256
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                "{}",
                file.file_name
            );
            assert!(
                file.url.starts_with("https://huggingface.co/"),
                "{}",
                file.file_name
            );
            assert!(file.url.ends_with(file.file_name));
            assert!(file.bytes > 0);
        }
    }

    #[test]
    fn the_continue_threshold_is_what_whisper_cpp_derives() {
        assert!((VAD_START_THRESHOLD - VAD_CONTINUE_OFFSET - VAD_CONTINUE_THRESHOLD).abs() < 1e-6);
        assert!(VAD_PAD_BEFORE_MS + VAD_PAD_AFTER_MS < VAD_MAX_SPAN_MS);
    }

    #[test]
    fn request_timeouts_scale_with_audio() {
        assert_eq!(worker_request_timeout(0), Duration::from_secs(60 + 4 * 30));
        assert_eq!(
            worker_request_timeout(60_000),
            Duration::from_secs(60 + 4 * 90)
        );
        assert!(worker_request_timeout(60_001) > worker_request_timeout(60_000));
    }

    #[test]
    fn worker_memory_is_checked_per_model_below_at_and_above() {
        for (model, budget) in [
            (TranscriptionModel::Fast, WORKER_MEMORY_FAST_BYTES),
            (TranscriptionModel::Accurate, WORKER_MEMORY_ACCURATE_BYTES),
        ] {
            assert_eq!(worker_memory_budget(model), budget);
            assert!(!worker_memory_exceeded(model, budget - 1));
            assert!(!worker_memory_exceeded(model, budget));
            assert!(worker_memory_exceeded(model, budget + 1));
        }
        // No lower than 1.5 times the spike's peak on the CPU.
        assert!(WORKER_MEMORY_FAST_BYTES * 2 >= 3 * 796_000_000);
        assert!(WORKER_MEMORY_ACCURATE_BYTES * 2 >= 3 * 1_317_000_000);
    }

    #[test]
    fn a_worker_starts_only_with_its_budget_and_margin_available() {
        for model in TranscriptionModel::ALL {
            let needed = worker_memory_budget(model) + WORKER_START_MARGIN_BYTES;
            assert!(!memory_allows_start(model, needed - 1));
            assert!(memory_allows_start(model, needed));
            assert!(memory_allows_start(model, needed + 1));
        }
        assert!(!memory_allows_start(TranscriptionModel::Accurate, 0));
    }

    #[test]
    fn a_live_batch_is_due_below_at_and_above() {
        let batch = LIVE_BATCH.as_millis() as u64;
        assert_eq!(batch, 300_000);
        assert!(!live_batch_due(batch - 1));
        assert!(live_batch_due(batch));
        assert!(live_batch_due(batch + 1));
    }

    #[test]
    fn threads_are_half_the_cores_within_bounds() {
        assert_eq!(transcription_threads(0), 1);
        assert_eq!(transcription_threads(1), 1);
        assert_eq!(transcription_threads(6), 3);
        assert_eq!(
            transcription_threads(2 * WORKER_MAX_THREADS - 1),
            WORKER_MAX_THREADS - 1
        );
        assert_eq!(
            transcription_threads(2 * WORKER_MAX_THREADS),
            WORKER_MAX_THREADS
        );
        assert_eq!(
            transcription_threads(2 * WORKER_MAX_THREADS + 2),
            WORKER_MAX_THREADS
        );
    }

    #[test]
    fn redirects_are_capped_below_at_and_above() {
        assert!(redirect_allowed(MODEL_DOWNLOAD_MAX_REDIRECTS - 1));
        assert!(redirect_allowed(MODEL_DOWNLOAD_MAX_REDIRECTS));
        assert!(!redirect_allowed(MODEL_DOWNLOAD_MAX_REDIRECTS + 1));
    }

    #[test]
    fn progress_is_throttled_below_at_and_above() {
        let ms = Duration::from_millis(1);
        assert!(!progress_due(MODEL_PROGRESS_INTERVAL - ms));
        assert!(progress_due(MODEL_PROGRESS_INTERVAL));
        assert!(progress_due(MODEL_PROGRESS_INTERVAL + ms));
        assert_eq!(MODEL_PROGRESS_INTERVAL * 4, Duration::from_secs(1));
    }

    #[test]
    fn downloads_keep_the_disk_floor_below_at_and_above() {
        let remaining = 190_000_000;
        assert!(!download_fits(
            MIN_FREE_DISK_BYTES + remaining - 1,
            remaining
        ));
        assert!(download_fits(MIN_FREE_DISK_BYTES + remaining, remaining));
        assert!(download_fits(
            MIN_FREE_DISK_BYTES + remaining + 1,
            remaining
        ));
        assert!(!download_fits(u64::MAX - 1, u64::MAX));
    }

    #[test]
    fn only_hugging_face_hosts_serve_models() {
        for host in [
            "huggingface.co",
            "HuggingFace.co",
            "cdn-lfs.huggingface.co",
            "cdn-lfs-us-1.hf.co",
            "cas-bridge.xethub.hf.co",
            "huggingface.co.",
        ] {
            assert!(model_host_allowed(host), "{host}");
        }
        for host in [
            "hf.co",
            "evilhuggingface.co",
            "huggingface.co.evil.com",
            "huggingface.com",
            "xhf.co",
            "127.0.0.1",
            "",
        ] {
            assert!(!model_host_allowed(host), "{host}");
        }
    }

    #[test]
    fn segment_files_are_capped_at_twice_a_segment() {
        assert_eq!(WORKER_MAX_SEGMENT_FILE_BYTES, 44 + 2 * 1_920_000);
    }

    #[test]
    fn restart_backoff_doubles() {
        assert_eq!(RESTART_BACKOFF.map(|d| d.as_secs()), [1, 2, 4]);
    }
}
