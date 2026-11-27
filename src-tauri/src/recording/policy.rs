//! Every limit, threshold, and model pin of meeting recording, in one place.
//! The recording service, the transcription worker, and the recap request all
//! read these constants and check functions; nothing else hard-codes a number.
//!
//! Nothing here is sent anywhere. The model URLs are fetched only when the
//! user chooses to download a model, and each download is checked against its
//! pinned size and SHA-256 before it is used.
// Offers (stage 5), transcription (stage 3), and the recap (stage 4) read
// the limits here that nothing uses yet.
#![allow(dead_code)]

use std::time::Duration;

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
/// spans are split.
pub const VAD_MAX_SPAN_MS: u32 = 30_000;

/// Merging the two tracks' transcripts: a microphone segment whose text
/// overlaps a system segment by at least this fraction (0 to 1) is dropped
/// as speaker echo.
pub const ECHO_DROP_THRESHOLD: f32 = 0.5;
/// Slack in milliseconds, either side, when deciding that two segments
/// overlap in time.
pub const MERGE_TIME_SLACK_MS: i64 = 1000;

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
    fn restart_backoff_doubles() {
        assert_eq!(RESTART_BACKOFF.map(|d| d.as_secs()), [1, 2, 4]);
    }
}
