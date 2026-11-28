//! Meeting recording: the service that owns at most one active recording.
//!
//! `RecordingService` is a state machine driven by method calls, capture
//! events, and `tick()`. It holds no threads or timers: the caller (stage 2's
//! wiring) calls `tick()` about once a second, and tests call it after moving
//! a fake clock. Audio never passes through here; a [`capture::CaptureBackend`]
//! writes samples itself and reports health, which is all the service uses.
//!
//! The app reaches it through [`runtime::RecordingRuntime`], which adds the
//! tick thread's work, crash recovery, retention, and change events.

pub mod capture;
pub mod files;
pub mod meeting_links;
pub mod offers;
pub mod permissions;
pub mod policy;
pub mod retention;
pub mod runtime;
pub mod segments;
pub mod settings;
pub mod sleep;
#[cfg(test)]
pub(crate) mod test_support;
pub mod transcribe;
pub mod vocabulary;

use crate::clock::Clock;
use crate::db::recordings::{NewRecording, RecordingState, TrackPatch, TrackState};
use crate::db::Database;
use crate::note_entities::NoteReference;
use capture::{
    CaptureBackend, CaptureEvent, CaptureSession, EventSink, SystemAudioSupport, Track, TrackTarget,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Free space on the volume recordings are written to.
pub trait DiskProbe: Send + Sync {
    fn free_bytes(&self) -> Result<u64, String>;
}

/// The real [`DiskProbe`]: free space on the volume holding `path`.
pub struct VolumeProbe(pub PathBuf);

impl DiskProbe for VolumeProbe {
    #[cfg(unix)]
    fn free_bytes(&self) -> Result<u64, String> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(self.0.as_os_str().as_bytes())
            .map_err(|_| "The recordings folder has an invalid name.".to_string())?;
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated and `stats` a valid out-pointer.
        if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // Space available to this (unprivileged) user.
        Ok(stats.f_bavail as u64 * stats.f_frsize as u64)
    }

    #[cfg(not(unix))]
    fn free_bytes(&self) -> Result<u64, String> {
        // Recording isn't available on these platforms yet; nothing writes.
        Ok(u64::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingPhase {
    Idle,
    Recording,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackRunState {
    Active,
    /// Waiting out a backoff before the track is started again.
    Restarting,
    Failed,
    Unavailable,
}

/// Why the service stopped a recording by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    LengthLimit,
    LowDisk,
    AllTracksFailed,
    SystemSleep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackStatus {
    pub track: Track,
    pub state: TrackRunState,
    pub signal_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    pub state: RecordingPhase,
    pub recording_id: Option<String>,
    pub title: Option<String>,
    pub event_ref: Option<NoteReference>,
    pub elapsed_ms: u64,
    pub tracks: Vec<TrackStatus>,
    pub system_audio: SystemAudioSupport,
    /// True when the system-audio tap has only streamed digital zeros while
    /// the microphone hears speech, suggesting missing macOS permission.
    pub system_audio_silent_warning: bool,
    /// Why the last recording was stopped automatically; cleared by the
    /// next start and by a stop the user asked for.
    pub last_stop_reason: Option<StopReason>,
    /// True when a call released the microphone (or event end passed) and
    /// both tracks have been silent long enough to suggest stopping.
    pub still_recording_nudge: bool,
}

enum RunState {
    Active,
    Restarting { retry_at: SystemTime },
    Failed,
    Unavailable,
}

struct TrackRun {
    state: RunState,
    session: Option<Box<dyn CaptureSession>>,
    /// The track has delivered audio at least once. A track that has not,
    /// such as a system tap nobody is playing through, cannot stall.
    first_frame_seen: bool,
    last_activity: SystemTime,
    /// Restarts already scheduled since the track last delivered frames.
    restarts_used: usize,
    signal_active: bool,
    ever_nonzero_audio: bool,
}

struct Active {
    id: String,
    title: String,
    event_ref: Option<NoteReference>,
    started_at: SystemTime,
    start_host_ns: u64,
    target: TrackTarget,
    paused_total: Duration,
    paused_since: Option<SystemTime>,
    tracks: HashMap<Track, TrackRun>,
    any_audio: bool,
    all_outputs_headphones: Option<bool>,
    mic_speech_secs_while_tap_zero: u64,
    system_audio_silent_warning: bool,
    both_tracks_silent_since: Option<SystemTime>,
    still_recording_nudge: bool,
}

pub struct RecordingService {
    database: Arc<Database>,
    backend: Arc<dyn CaptureBackend>,
    clock: Arc<dyn Clock + Send + Sync>,
    disk: Arc<dyn DiskProbe>,
    audio_root: PathBuf,
    events: EventSink,
    inbox: Receiver<CaptureEvent>,
    active: Option<Active>,
    last_stop_reason: Option<StopReason>,
    /// A recording row the last start created and then failed, so its
    /// caller can announce it.
    failed_start: Option<String>,
}

fn rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl RecordingService {
    pub fn new(
        database: Arc<Database>,
        backend: Arc<dyn CaptureBackend>,
        clock: Arc<dyn Clock + Send + Sync>,
        disk: Arc<dyn DiskProbe>,
        audio_root: PathBuf,
    ) -> Self {
        let (events, inbox) = channel();
        Self {
            database,
            backend,
            clock,
            disk,
            audio_root,
            events,
            inbox,
            active: None,
            last_stop_reason: None,
            failed_start: None,
        }
    }

    /// Starts a recording with default input device and no attendee hints.
    #[cfg(test)]
    pub fn start(
        &mut self,
        event_ref: Option<NoteReference>,
        title: &str,
    ) -> Result<RecordingStatus, String> {
        self.start_with_options(event_ref, title, None, &[])
    }

    /// Starts a recording. The microphone is the one track that must start
    /// unless system audio is running; a track that cannot start is marked
    /// failed and the other continues.
    pub fn start_with_options(
        &mut self,
        event_ref: Option<NoteReference>,
        title: &str,
        microphone_uid: Option<String>,
        attendees: &[String],
    ) -> Result<RecordingStatus, String> {
        if self.active.is_some() {
            return Err("A recording is already in progress.".into());
        }
        match self.disk.free_bytes() {
            Ok(free) if policy::disk_is_low(free) => {
                return Err(
                    "There isn't enough free disk space to record. Free up at least 2 GB and try again."
                        .into(),
                )
            }
            Ok(_) => {}
            Err(error) => return Err(format!("Couldn't check free disk space: {error}")),
        }
        // Drop events from a recording that has already ended.
        while self.inbox.try_recv().is_ok() {}
        self.last_stop_reason = None;

        let id = uuid::Uuid::new_v4().to_string();
        let now = self.clock.now();
        let system = self.backend.system_audio();
        let system_state = if system == SystemAudioSupport::Available {
            TrackState::Active
        } else {
            TrackState::Unavailable
        };
        let audio_dir = self.audio_root.join(&id);
        // Taken before any track starts, so every segment and first frame
        // is placed after it.
        let target = TrackTarget {
            audio_dir: audio_dir.clone(),
            origin_host_ns: self.backend.host_time_ns(),
            microphone_uid,
        };
        self.database
            .create_recording(&NewRecording {
                id: &id,
                event_ref: event_ref.as_ref(),
                title,
                attendees,
                started_at: &rfc3339(now),
                audio_dir: &audio_dir.to_string_lossy(),
                tracks: &[
                    (Track::Mic, TrackState::Active),
                    (Track::System, system_state),
                ],
            })
            .map_err(String::from)?;

        let mut tracks = HashMap::new();
        let mut errors = Vec::new();
        for (track, wanted) in [
            (Track::Mic, TrackState::Active),
            (Track::System, system_state),
        ] {
            if wanted == TrackState::Unavailable {
                tracks.insert(track, TrackRun::unavailable(now));
                continue;
            }
            match self.backend.start(track, &target, self.events.clone()) {
                Ok(session) => {
                    tracks.insert(track, TrackRun::active(session, now));
                }
                Err(error) => {
                    self.persist_track(&id, track, TrackState::Failed, Some(&error));
                    errors.push(error);
                    tracks.insert(track, TrackRun::failed(now));
                }
            }
        }
        let any_running = tracks.values().any(|t| matches!(t.state, RunState::Active));
        if !any_running {
            let error = errors
                .into_iter()
                .next()
                .unwrap_or_else(|| "Couldn't start recording.".into());
            self.database
                .finish_recording(&id, &rfc3339(now), 0, RecordingState::Failed, Some(&error))
                .map_err(String::from)?;
            self.failed_start = Some(id);
            return Err(error);
        }
        self.active = Some(Active {
            id,
            title: title.to_string(),
            event_ref,
            started_at: now,
            start_host_ns: target.origin_host_ns,
            target,
            paused_total: Duration::ZERO,
            paused_since: None,
            tracks,
            any_audio: false,
            all_outputs_headphones: None,
            mic_speech_secs_while_tap_zero: 0,
            system_audio_silent_warning: false,
            both_tracks_silent_since: None,
            still_recording_nudge: false,
        });
        Ok(self.status())
    }

    /// The recording the last start created and then ended as failed, once.
    pub fn take_failed_start(&mut self) -> Option<String> {
        self.failed_start.take()
    }

    pub fn pause(&mut self) -> Result<RecordingStatus, String> {
        let now = self.clock.now();
        let active = self
            .active
            .as_mut()
            .ok_or("There is no recording to pause.")?;
        if active.paused_since.is_some() {
            return Err("The recording is already paused.".into());
        }
        active.paused_since = Some(now);
        active.both_tracks_silent_since = None;
        active.still_recording_nudge = false;
        for run in active.tracks.values_mut() {
            run.signal_active = false;
            if let Some(session) = run.session.as_mut() {
                session.pause();
            }
        }
        Ok(self.status())
    }

    pub fn resume(&mut self) -> Result<RecordingStatus, String> {
        let now = self.clock.now();
        let active = self
            .active
            .as_mut()
            .ok_or("There is no recording to resume.")?;
        let since = active
            .paused_since
            .take()
            .ok_or("The recording isn't paused.")?;
        active.paused_total += now.duration_since(since).unwrap_or_default();
        active.both_tracks_silent_since = None;
        active.still_recording_nudge = false;
        for run in active.tracks.values_mut() {
            // Silence while paused is not a stall.
            run.last_activity = now;
            if let Some(session) = run.session.as_mut() {
                session.resume();
            }
        }
        Ok(self.status())
    }

    /// Stops the recording the user is looking at.
    pub fn stop(&mut self) -> Result<RecordingStatus, String> {
        self.finish(None)
    }

    /// Stops any active recording when the Mac goes to sleep, so waking never
    /// silently resumes microphone capture.
    pub fn stop_for_sleep(&mut self) {
        if self.active.is_some() {
            let _ = self.finish(Some(StopReason::SystemSleep));
        }
    }

    fn finish(&mut self, reason: Option<StopReason>) -> Result<RecordingStatus, String> {
        if self.active.is_none() {
            return Err("There is no recording to stop.".into());
        }
        self.pump();
        // A queued failure can end the recording while being applied (every
        // track has now failed). It is stopped either way.
        let Some(mut active) = self.active.take() else {
            return Ok(self.status());
        };
        let now = self.clock.now();
        for run in active.tracks.values_mut() {
            if let Some(mut session) = run.session.take() {
                session.stop();
            }
        }
        let elapsed = active.elapsed(now);
        let (state, error) = if active.any_audio {
            (RecordingState::Recorded, None)
        } else {
            (RecordingState::Failed, Some("No audio was recorded."))
        };
        self.last_stop_reason = reason;
        let written = self.database.finish_recording(
            &active.id,
            &rfc3339(now),
            elapsed.as_millis() as i64,
            state,
            error,
        );
        let status = self.status();
        written.map_err(String::from)?;
        Ok(status)
    }

    /// Applies one capture event. Events for a track that is not running
    /// (a stale session's last words) are ignored.
    pub fn handle(&mut self, event: CaptureEvent) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let now = self.clock.now();
        let track = match &event {
            CaptureEvent::FirstFrame { track, .. }
            | CaptureEvent::Frames { track, .. }
            | CaptureEvent::Failed { track, .. }
            | CaptureEvent::DeviceChanged { track, .. } => *track,
        };
        let Some(run) = active.tracks.get_mut(&track) else {
            return;
        };
        if !matches!(run.state, RunState::Active) {
            return;
        }
        match event {
            CaptureEvent::FirstFrame { host_time_ns, .. } => {
                active.any_audio = true;
                run.last_activity = now;
                run.restarts_used = 0;
                if !run.first_frame_seen {
                    run.first_frame_seen = true;
                    let offset_ms = host_time_ns.saturating_sub(active.start_host_ns) / 1_000_000;
                    let id = active.id.clone();
                    self.patch_track(
                        &id,
                        track,
                        TrackPatch {
                            first_frame_offset_ms: Some(offset_ms as i64),
                            ..Default::default()
                        },
                    );
                }
            }
            CaptureEvent::Frames { count, peak, .. } => {
                if count > 0 {
                    active.any_audio = true;
                    run.first_frame_seen = true;
                    run.last_activity = now;
                    run.restarts_used = 0;
                    run.signal_active = peak >= policy::SIGNAL_ACTIVE_PEAK;
                    if run.signal_active {
                        active.both_tracks_silent_since = None;
                        active.still_recording_nudge = false;
                    }
                    if peak > 0.0 {
                        run.ever_nonzero_audio = true;
                    }
                    if track == Track::System && peak > 0.0 {
                        active.mic_speech_secs_while_tap_zero = 0;
                        active.system_audio_silent_warning = false;
                    } else if track == Track::Mic && peak >= policy::MIC_SPEECH_PEAK {
                        let system_stuck_zero =
                            active.tracks.get(&Track::System).is_some_and(|s| {
                                matches!(s.state, RunState::Active) && !s.ever_nonzero_audio
                            });
                        if system_stuck_zero {
                            active.mic_speech_secs_while_tap_zero += 1;
                            if active.mic_speech_secs_while_tap_zero
                                >= policy::SILENT_TAP_SPEECH_SECS
                            {
                                active.system_audio_silent_warning = true;
                            }
                        }
                    }
                }
            }
            CaptureEvent::DeviceChanged {
                device_name,
                is_headphones,
                ..
            } => {
                let id = active.id.clone();
                let effective_headphones = if track == Track::System {
                    if let Some(headphones) = is_headphones {
                        active.all_outputs_headphones = Some(match active.all_outputs_headphones {
                            Some(false) => false,
                            _ => headphones,
                        });
                    }
                    active.all_outputs_headphones
                } else {
                    is_headphones
                };
                self.patch_track(
                    &id,
                    track,
                    TrackPatch {
                        device_name: Some(device_name),
                        is_headphones: effective_headphones,
                        ..Default::default()
                    },
                );
            }
            CaptureEvent::Failed { .. } => self.begin_restart(track, now),
        }
    }

    /// Periodic work: limits, restarts, and stall detection.
    pub fn tick(&mut self) {
        self.pump();
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let now = self.clock.now();
        if policy::length_limit_reached(active.elapsed(now)) {
            let _ = self.finish(Some(StopReason::LengthLimit));
            return;
        }
        if let Ok(free) = self.disk.free_bytes() {
            if policy::disk_is_low(free) {
                let _ = self.finish(Some(StopReason::LowDisk));
                return;
            }
        }
        let paused = active.paused_since.is_some();
        if !paused {
            let stalled: Vec<Track> = active
                .tracks
                .iter()
                .filter(|(_, run)| {
                    matches!(run.state, RunState::Active)
                        && run.first_frame_seen
                        && now.duration_since(run.last_activity).unwrap_or_default()
                            >= policy::STALL_AFTER
                })
                .map(|(track, _)| *track)
                .collect();
            for track in stalled {
                self.begin_restart(track, now);
            }
            let due: Vec<Track> = self
                .active
                .as_ref()
                .map(|a| {
                    a.tracks
                        .iter()
                        .filter(|(_, run)| {
                            matches!(run.state, RunState::Restarting { retry_at } if retry_at <= now)
                        })
                        .map(|(track, _)| *track)
                        .collect()
                })
                .unwrap_or_default();
            for track in due {
                self.restart(track, now);
            }
        }
        self.stop_if_all_failed();
        if let Some(active) = self.active.as_mut() {
            if active.paused_since.is_some() {
                active.both_tracks_silent_since = None;
                active.still_recording_nudge = false;
            } else {
                let any_signal = active
                    .tracks
                    .values()
                    .any(|run| matches!(run.state, RunState::Active) && run.signal_active);
                if any_signal {
                    active.both_tracks_silent_since = None;
                    active.still_recording_nudge = false;
                } else {
                    active.both_tracks_silent_since.get_or_insert(now);
                }
            }
        }
    }

    pub fn both_tracks_silent_for(&self) -> Duration {
        self.active
            .as_ref()
            .and_then(|a| a.both_tracks_silent_since)
            .map_or(Duration::ZERO, |since| {
                self.clock.now().duration_since(since).unwrap_or_default()
            })
    }

    pub fn set_still_recording_nudge(&mut self, nudge: bool) {
        if let Some(active) = self.active.as_mut() {
            active.still_recording_nudge = if active.paused_since.is_some() {
                false
            } else {
                nudge
            };
        }
    }

    pub fn status(&self) -> RecordingStatus {
        let system_audio = self.backend.system_audio();
        let Some(active) = self.active.as_ref() else {
            return RecordingStatus {
                state: RecordingPhase::Idle,
                recording_id: None,
                title: None,
                event_ref: None,
                elapsed_ms: 0,
                tracks: Vec::new(),
                system_audio,
                system_audio_silent_warning: false,
                last_stop_reason: self.last_stop_reason,
                still_recording_nudge: false,
            };
        };
        let mut tracks: Vec<TrackStatus> = active
            .tracks
            .iter()
            .map(|(track, run)| TrackStatus {
                track: *track,
                state: match run.state {
                    RunState::Active => TrackRunState::Active,
                    RunState::Restarting { .. } => TrackRunState::Restarting,
                    RunState::Failed => TrackRunState::Failed,
                    RunState::Unavailable => TrackRunState::Unavailable,
                },
                signal_active: run.signal_active,
            })
            .collect();
        tracks.sort_by_key(|t| t.track);
        RecordingStatus {
            state: if active.paused_since.is_some() {
                RecordingPhase::Paused
            } else {
                RecordingPhase::Recording
            },
            recording_id: Some(active.id.clone()),
            title: Some(active.title.clone()),
            event_ref: active.event_ref.clone(),
            elapsed_ms: active.elapsed(self.clock.now()).as_millis() as u64,
            tracks,
            system_audio,
            system_audio_silent_warning: active.system_audio_silent_warning,
            last_stop_reason: self.last_stop_reason,
            still_recording_nudge: active.still_recording_nudge,
        }
    }

    /// Applies the events backends have queued.
    fn pump(&mut self) {
        while let Ok(event) = self.inbox.try_recv() {
            self.handle(event);
        }
    }

    /// Schedules a restart of a failed or stalled track, or fails it for
    /// good once the backoff list is used up.
    fn begin_restart(&mut self, track: Track, now: SystemTime) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let id = active.id.clone();
        let Some(run) = active.tracks.get_mut(&track) else {
            return;
        };
        if let Some(mut session) = run.session.take() {
            session.stop();
        }
        match policy::RESTART_BACKOFF.get(run.restarts_used) {
            Some(wait) => {
                run.restarts_used += 1;
                run.state = RunState::Restarting {
                    retry_at: now + *wait,
                };
            }
            None => {
                run.state = RunState::Failed;
                self.persist_track(
                    &id,
                    track,
                    TrackState::Failed,
                    Some("The audio device stopped delivering audio."),
                );
            }
        }
        self.stop_if_all_failed();
    }

    fn restart(&mut self, track: Track, now: SystemTime) {
        let Some(target) = self.active.as_ref().map(|a| a.target.clone()) else {
            return;
        };
        match self.backend.start(track, &target, self.events.clone()) {
            Ok(session) => {
                if let Some(run) = self.active.as_mut().and_then(|a| a.tracks.get_mut(&track)) {
                    run.state = RunState::Active;
                    run.session = Some(session);
                    run.last_activity = now;
                }
            }
            Err(_) => self.begin_restart(track, now),
        }
    }

    fn stop_if_all_failed(&mut self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let any_left = active
            .tracks
            .values()
            .any(|run| matches!(run.state, RunState::Active | RunState::Restarting { .. }));
        if !any_left {
            let _ = self.finish(Some(StopReason::AllTracksFailed));
        }
    }

    fn persist_track(&self, id: &str, track: Track, state: TrackState, error: Option<&str>) {
        self.patch_track(
            id,
            track,
            TrackPatch {
                state: Some(state),
                error: error.map(str::to_string),
                ..Default::default()
            },
        );
    }

    /// Track bookkeeping must not stop a recording, so a failed write is
    /// dropped; the recording's own start and finish writes are checked.
    fn patch_track(&self, id: &str, track: Track, patch: TrackPatch) {
        let _ = self.database.update_recording_track(id, track, &patch);
    }
}

impl Active {
    /// Time recorded so far, pauses excluded.
    fn elapsed(&self, now: SystemTime) -> Duration {
        let end = self.paused_since.unwrap_or(now);
        end.duration_since(self.started_at)
            .unwrap_or_default()
            .saturating_sub(self.paused_total)
    }
}

impl TrackRun {
    fn active(session: Box<dyn CaptureSession>, now: SystemTime) -> Self {
        Self {
            state: RunState::Active,
            session: Some(session),
            first_frame_seen: false,
            last_activity: now,
            restarts_used: 0,
            signal_active: false,
            ever_nonzero_audio: false,
        }
    }

    fn failed(now: SystemTime) -> Self {
        Self {
            state: RunState::Failed,
            session: None,
            first_frame_seen: false,
            last_activity: now,
            restarts_used: 0,
            signal_active: false,
            ever_nonzero_audio: false,
        }
    }

    fn unavailable(now: SystemTime) -> Self {
        Self {
            state: RunState::Unavailable,
            ..Self::failed(now)
        }
    }
}

#[cfg(test)]
mod memory_tests;
#[cfg(test)]
mod tests;
