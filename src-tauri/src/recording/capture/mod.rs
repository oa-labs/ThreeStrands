//! What the recording service needs from a platform's audio capture. The
//! service never sees audio: a backend writes samples itself, through
//! [`pipeline::TrackPipeline`] into `segments.rs`, and reports only health
//! through [`CaptureEvent`]s. That keeps the state machine testable against
//! [`fake`] and lets each platform implement one small trait.
//!
//! The macOS backend ([`macos`]: an IOProc on the default input for the
//! microphone, a Core Audio process tap for system audio) implements
//! [`CaptureBackend`] with one [`CaptureSession`] per track. Each session
//! has its own capture thread, which drains the device callback's ring
//! buffer ([`ring`]), downmixes, resamples ([`resample`]), writes segments,
//! and sends events, so sending an event never happens on the real-time
//! audio thread.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(target_os = "macos")]
pub mod macos;
// Only the macOS backend drives the capture pipeline so far; Windows and
// Linux backends (stages 6 and 7) will reuse it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod pipeline;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod resample;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod ring;

/// One of the two audio streams a recording keeps apart. "You" is the
/// microphone and "Others" is the computer's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Mic,
    System,
}

impl Track {
    pub const ALL: [Self; 2] = [Self::Mic, Self::System];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mic" => Some(Self::Mic),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// Whether this device can capture the computer's audio, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SystemAudioSupport {
    Available,
    /// macOS older than [`super::policy::SYSTEM_AUDIO_MIN_MACOS`]; the
    /// version found is reported so the app can say what to update to.
    NeedsMacos {
        major: u32,
        minor: u32,
    },
    /// This platform has no system audio capture yet.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
}

/// An available audio input device, for Settings → Meeting Recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputDevice {
    pub uid: String,
    pub name: String,
    pub is_default: bool,
    pub is_bluetooth: bool,
}

/// Something a capture session reports. Health only, never samples.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    /// The track's first audio arrived, at `host_time_ns` on the backend's
    /// monotonic host clock ([`CaptureBackend::host_time_ns`]). The system
    /// tap delivers nothing until some app plays audio, so the two tracks'
    /// first frames are far apart.
    FirstFrame { track: Track, host_time_ns: u64 },
    /// `count` more frames were captured and written, with peak amplitude
    /// (`0.0..=1.0`) over the interval. Sent often enough (at least once per
    /// flush interval) that silence means a stall.
    Frames { track: Track, count: u64, peak: f32 },
    /// The track can no longer capture. The service decides whether to
    /// restart it.
    Failed { track: Track, error: String },
    /// The device behind the track changed, such as headphones plugged in.
    DeviceChanged {
        track: Track,
        device_name: String,
        is_headphones: Option<bool>,
    },
}

/// Where a backend sends events. The service owns the receiving end and
/// reads it from `tick()`.
pub type EventSink = Sender<CaptureEvent>;

/// Where a track's audio is written, and the clock it is placed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackTarget {
    /// The recording's directory; segments and manifests go here.
    pub audio_dir: PathBuf,
    /// When the recording started, on the backend's host clock
    /// ([`CaptureBackend::host_time_ns`]). Segment start times are measured
    /// from it.
    pub origin_host_ns: u64,
    /// Optional explicit microphone UID; `None` follows the system default.
    pub microphone_uid: Option<String>,
}

/// A platform's capture capability.
pub trait CaptureBackend: Send + Sync {
    /// Whether this platform can record at all.
    fn supported(&self) -> bool {
        true
    }

    /// Whether the system audio track can be started on this device.
    fn system_audio(&self) -> SystemAudioSupport;

    /// Whether Apple `SpeechAnalyzer` is available on this device.
    fn speech_analyzer(&self) -> bool {
        false
    }

    /// Available audio input devices, empty on unsupported platforms.
    fn list_input_devices(&self) -> Vec<AudioInputDevice> {
        Vec::new()
    }

    /// Now, in nanoseconds on the same monotonic clock that
    /// [`CaptureEvent::FirstFrame`] reports, so the service can place a
    /// track's first audio on the recording's timeline.
    fn host_time_ns(&self) -> u64;

    /// Starts capturing `track` into `target`, reporting through `events`.
    /// An error means the track could not start (permission refused, no
    /// device); the message is shown to the user.
    fn start(
        &self,
        track: Track,
        target: &TrackTarget,
        events: EventSink,
    ) -> Result<Box<dyn CaptureSession>, String>;
}

/// Shown when a platform has no capture backend yet.
pub const UNSUPPORTED_MESSAGE: &str = "Recording meetings isn't available on this platform yet.";

/// The backend for platforms without capture: it reports itself unsupported
/// and refuses every start.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct UnsupportedBackend;

impl CaptureBackend for UnsupportedBackend {
    fn supported(&self) -> bool {
        false
    }

    fn system_audio(&self) -> SystemAudioSupport {
        SystemAudioSupport::Unsupported
    }

    fn host_time_ns(&self) -> u64 {
        // Any monotonic clock will do; nothing is ever placed on it.
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_nanos() as u64
    }

    fn start(
        &self,
        _track: Track,
        _target: &TrackTarget,
        _events: EventSink,
    ) -> Result<Box<dyn CaptureSession>, String> {
        Err(UNSUPPORTED_MESSAGE.into())
    }
}

/// This platform's capture backend.
pub fn platform_backend() -> Arc<dyn CaptureBackend> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacBackend::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(UnsupportedBackend)
    }
}

/// One running track. Stopping is idempotent, and dropping a session stops
/// it, so a track never keeps capturing after the service forgets it.
pub trait CaptureSession: Send {
    /// Stop writing audio without ending the session.
    fn pause(&mut self);
    fn resume(&mut self);
    fn stop(&mut self);
}
