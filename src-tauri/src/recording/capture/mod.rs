//! What the recording service needs from a platform's audio capture. The
//! service never sees audio: a backend writes samples itself (stage 2) and
//! reports only health, through [`CaptureEvent`]s. That keeps the state
//! machine testable against [`fake`] and lets each platform implement one
//! small trait.
//!
//! The macOS backend (a Core Audio process tap for system audio, `cpal` for
//! the microphone; see `spikes/meeting-recording/src/tap.rs`) implements
//! [`CaptureBackend`] with one [`CaptureSession`] per track. Its audio
//! callbacks call the [`EventSink`] from their own threads, so sending an
//! event must never block.

use serde::{Deserialize, Serialize};
use std::sync::mpsc::Sender;

#[cfg(test)]
pub(crate) mod fake;

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
    Unsupported,
}

/// Something a capture session reports. Health only, never samples.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    /// The track's first audio arrived, at `host_time_ns` on the backend's
    /// monotonic host clock ([`CaptureBackend::host_time_ns`]). The system
    /// tap delivers nothing until some app plays audio, so the two tracks'
    /// first frames are far apart.
    FirstFrame { track: Track, host_time_ns: u64 },
    /// `count` more frames were captured and written. Sent often enough
    /// (at least once per flush interval) that silence means a stall.
    Frames { track: Track, count: u64 },
    /// The track can no longer capture. The service decides whether to
    /// restart it.
    Failed { track: Track, error: String },
    /// The device behind the track changed, such as headphones plugged in.
    DeviceChanged { track: Track, device_name: String },
}

/// Where a backend sends events. The service owns the receiving end and
/// reads it from `tick()`.
pub type EventSink = Sender<CaptureEvent>;

/// A platform's capture capability.
pub trait CaptureBackend: Send + Sync {
    /// Whether the system audio track can be started on this device.
    fn system_audio(&self) -> SystemAudioSupport;

    /// Now, in nanoseconds on the same monotonic clock that
    /// [`CaptureEvent::FirstFrame`] reports, so the service can place a
    /// track's first audio on the recording's timeline.
    fn host_time_ns(&self) -> u64;

    /// Starts capturing `track`, reporting through `events`. An error means
    /// the track could not start (permission refused, no device); the
    /// message is shown to the user.
    fn start(&self, track: Track, events: EventSink) -> Result<Box<dyn CaptureSession>, String>;
}

/// One running track. Stopping is idempotent, and dropping a session stops
/// it, so a track never keeps capturing after the service forgets it.
pub trait CaptureSession: Send {
    /// Stop writing audio without ending the session.
    fn pause(&mut self);
    fn resume(&mut self);
    fn stop(&mut self);
}
