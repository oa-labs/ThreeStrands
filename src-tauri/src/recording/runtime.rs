//! The app's handle on meeting recording: the [`RecordingService`] behind a
//! lock, plus everything around it that needs the file system or tells the
//! rest of the app what changed. Commands, the tray, and the tick thread all
//! go through it.
//!
//! Every method that can change the recording ends by publishing: the
//! `recording-state` event goes out when the status changed (anything but
//! the elapsed time), `recordings-changed` when a recording was created,
//! finished, interrupted, or deleted, and the tray is refreshed. Publishing
//! happens after the service lock is released, so a listener can call back
//! in.

use super::capture::{CaptureBackend, SystemAudioSupport, Track};
use super::retention;
use super::segments;
use super::settings::RecordingSettings;
use super::{policy, DiskProbe, RecordingPhase, RecordingService, RecordingStatus};
use crate::clock::Clock;
use crate::db::recordings::RecordingState;
use crate::db::Database;
use crate::note_entities::NoteReference;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

/// Where the runtime reports changes. The app sends Tauri events and
/// refreshes the tray; tests record the calls.
pub trait RecordingNotifier: Send + Sync {
    fn state_changed(&self, status: &RecordingStatus);
    fn recordings_changed(&self, recording_id: &str);
    /// Called after every published change and every tick while a
    /// recording is active, so the tray's elapsed time stays current.
    fn refresh_tray(&self, status: &RecordingStatus, settings: &RecordingSettings, supported: bool);
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingCapabilities {
    pub supported: bool,
    pub system_audio: SystemAudioSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingsUsage {
    pub recordings: u64,
    pub audio_bytes: u64,
}

const ACTIVE_REFUSAL: &str = "This recording is still in progress. Stop it first.";

fn rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub struct RecordingRuntime {
    service: Mutex<RecordingService>,
    database: Arc<Database>,
    supported: bool,
    audio_root: PathBuf,
    clock: Arc<dyn Clock + Send + Sync>,
    notifier: Arc<dyn RecordingNotifier>,
    settings: Mutex<RecordingSettings>,
    /// The last status published, with its elapsed time zeroed.
    published: Mutex<Option<RecordingStatus>>,
    last_sweep: Mutex<Option<SystemTime>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RecordingRuntime {
    pub fn new(
        database: Arc<Database>,
        backend: Arc<dyn CaptureBackend>,
        clock: Arc<dyn Clock + Send + Sync>,
        disk: Arc<dyn DiskProbe>,
        audio_root: PathBuf,
        notifier: Arc<dyn RecordingNotifier>,
    ) -> Self {
        let settings = database.recording_settings().unwrap_or_else(|error| {
            log::warn!(target: "recording", "reading recording settings: {error}");
            RecordingSettings::default()
        });
        let supported = backend.supported();
        Self {
            service: Mutex::new(RecordingService::new(
                database.clone(),
                backend,
                clock.clone(),
                disk,
                audio_root.clone(),
            )),
            database,
            supported,
            audio_root,
            clock,
            notifier,
            settings: Mutex::new(settings),
            published: Mutex::new(None),
            last_sweep: Mutex::new(None),
        }
    }

    pub fn capabilities(&self) -> RecordingCapabilities {
        RecordingCapabilities {
            supported: self.supported,
            system_audio: lock(&self.service).status().system_audio,
        }
    }

    pub fn status(&self) -> RecordingStatus {
        lock(&self.service).status()
    }

    pub fn settings(&self) -> RecordingSettings {
        lock(&self.settings).clone()
    }

    /// Whether closing the window hides it (Record Meetings is on and this
    /// platform has the tray to bring it back).
    pub fn hide_on_close(&self) -> bool {
        self.supported && lock(&self.settings).record_meetings
    }

    pub fn is_active(&self) -> bool {
        lock(&self.service).status().state != RecordingPhase::Idle
    }

    fn active_id(&self) -> Option<String> {
        lock(&self.service).status().recording_id
    }

    /// Sends what changed since the last publish.
    fn publish(&self) {
        let status = self.status();
        let mut comparable = status.clone();
        comparable.elapsed_ms = 0;
        let previous = lock(&self.published).replace(comparable.clone());
        let changed = previous.as_ref() != Some(&comparable);
        if changed {
            self.notifier.state_changed(&status);
            let before = previous.and_then(|p| p.recording_id);
            if let Some(ended) = before.filter(|id| Some(id) != status.recording_id.as_ref()) {
                self.notifier.recordings_changed(&ended);
            }
        }
        if changed || status.state != RecordingPhase::Idle {
            self.notifier
                .refresh_tray(&status, &self.settings(), self.supported);
        }
    }

    pub fn start(
        &self,
        event_ref: Option<NoteReference>,
        title: &str,
    ) -> Result<RecordingStatus, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("A recording needs a title.".into());
        }
        if !self.supported {
            return Err(super::capture::UNSUPPORTED_MESSAGE.into());
        }
        let (result, failed) = {
            let mut service = lock(&self.service);
            let result = service.start(event_ref, title);
            (result, service.take_failed_start())
        };
        let created = result.as_ref().ok().and_then(|s| s.recording_id.clone());
        if let Some(id) = created.or(failed) {
            self.notifier.recordings_changed(&id);
        }
        self.publish();
        result
    }

    pub fn pause(&self) -> Result<RecordingStatus, String> {
        let result = lock(&self.service).pause();
        self.publish();
        result
    }

    pub fn resume(&self) -> Result<RecordingStatus, String> {
        let result = lock(&self.service).resume();
        self.publish();
        result
    }

    pub fn stop(&self) -> Result<RecordingStatus, String> {
        let result = lock(&self.service).stop();
        self.publish();
        result
    }

    /// Stops a recording in progress, if any. Quitting uses it.
    pub fn stop_if_active(&self) {
        if self.is_active() {
            if let Err(error) = self.stop() {
                log::warn!(target: "recording", "stopping before quit: {error}");
            }
        }
    }

    /// The once-a-second work: capture health and limits, then the daily
    /// retention sweep.
    pub fn tick(&self) {
        lock(&self.service).tick();
        self.publish();
        let now = self.clock.now();
        let due = lock(&self.last_sweep).is_none_or(|last| {
            now.duration_since(last).unwrap_or_default() >= policy::RETENTION_SWEEP_INTERVAL
        });
        if due {
            self.sweep();
        }
    }

    pub fn set_settings(&self, settings: RecordingSettings) -> Result<(), String> {
        self.database
            .set_recording_settings(&settings)
            .map_err(String::from)?;
        let keep_changed = lock(&self.settings).keep_audio != settings.keep_audio;
        *lock(&self.settings) = settings.clone();
        self.notifier
            .refresh_tray(&self.status(), &settings, self.supported);
        if keep_changed {
            self.sweep();
        }
        Ok(())
    }

    /// Startup crash recovery. A recording still marked in progress belongs
    /// to a process that is gone: its last segment's header is repaired,
    /// and it is marked interrupted with the length and end of the audio it
    /// saved. Runs before anything can start a new recording.
    pub fn recover(&self) -> usize {
        let rows = match self.database.recording_audio() {
            Ok(rows) => rows,
            Err(error) => {
                log::warn!(target: "recording", "listing recordings to recover: {error}");
                return 0;
            }
        };
        let mut recovered = 0;
        for row in rows
            .into_iter()
            .filter(|row| row.state == RecordingState::Recording)
        {
            let dir = Path::new(&row.audio_dir);
            if let Err(error) = segments::repair_dir(dir) {
                log::warn!(target: "recording", "repairing interrupted audio: {error}");
            }
            let saved = Track::ALL.map(|track| segments::saved_track(dir, track));
            let duration_ms = saved.iter().map(|t| t.audio_ms).max().unwrap_or(0);
            let end_ms = saved.iter().map(|t| t.end_ms).max().unwrap_or(0);
            let ended_at = chrono::DateTime::parse_from_rfc3339(&row.started_at)
                .map(|start| rfc3339(SystemTime::from(start) + Duration::from_millis(end_ms)))
                .unwrap_or_else(|_| row.started_at.clone());
            match self.database.finish_recording(
                &row.id,
                &ended_at,
                duration_ms as i64,
                RecordingState::Interrupted,
                None,
            ) {
                Ok(()) => recovered += 1,
                Err(error) => log::warn!(target: "recording", "marking interrupted: {error}"),
            }
        }
        // Anything the loop could not finish still must not look live.
        if let Err(error) = self.database.mark_interrupted_recordings() {
            log::warn!(target: "recording", "marking interrupted: {error}");
        }
        recovered
    }

    /// Deletes audio past **Keep Audio**. Returns how many recordings lost
    /// their audio.
    pub fn sweep(&self) -> usize {
        let now = self.clock.now();
        *lock(&self.last_sweep) = Some(now);
        let keep = lock(&self.settings).keep_audio;
        let rows = match self.database.recording_audio() {
            Ok(rows) => rows,
            Err(error) => {
                log::warn!(target: "recording", "listing recordings for retention: {error}");
                return 0;
            }
        };
        let active = self.active_id();
        let mut deleted = 0;
        for row in rows {
            if Some(&row.id) == active.as_ref() || !retention::audio_expired(keep, &row, now) {
                continue;
            }
            if self.remove_audio(&row).is_ok() {
                deleted += 1;
                self.notifier.recordings_changed(&row.id);
            }
        }
        deleted
    }

    fn remove_audio(&self, row: &crate::db::recordings::RecordingAudio) -> Result<(), String> {
        retention::delete_audio_dir(&self.audio_root, row)
            .map_err(|error| format!("Couldn't delete the recording's audio: {error}"))?;
        self.database
            .mark_recording_audio_deleted(&row.id, &rfc3339(self.clock.now()))
            .map_err(String::from)
    }

    fn refuse_active(&self, id: &str) -> Result<(), String> {
        if self.active_id().as_deref() == Some(id) {
            return Err(ACTIVE_REFUSAL.into());
        }
        Ok(())
    }

    pub fn delete_recording(&self, id: &str) -> Result<(), String> {
        self.refuse_active(id)?;
        let row = self
            .database
            .recording_audio_of(id)
            .map_err(String::from)?
            .ok_or("This recording no longer exists.")?;
        retention::delete_audio_dir(&self.audio_root, &row)
            .map_err(|error| format!("Couldn't delete the recording's audio: {error}"))?;
        self.database.delete_recording(id).map_err(String::from)?;
        self.notifier.recordings_changed(id);
        Ok(())
    }

    pub fn delete_recording_audio(&self, id: &str) -> Result<(), String> {
        self.refuse_active(id)?;
        let row = self
            .database
            .recording_audio_of(id)
            .map_err(String::from)?
            .ok_or("This recording no longer exists.")?;
        if row.audio_deleted_at.is_none() {
            self.remove_audio(&row)?;
        }
        self.notifier.recordings_changed(id);
        Ok(())
    }

    pub fn usage(&self) -> Result<RecordingsUsage, String> {
        Ok(RecordingsUsage {
            recordings: self
                .database
                .recording_count()
                .map_err(String::from)?
                .max(0) as u64,
            audio_bytes: segments::dir_bytes(&self.audio_root),
        })
    }

    /// Deletes every recording's audio except the one in progress.
    pub fn delete_all_audio(&self) -> Result<(), String> {
        let active = self.active_id();
        let rows = self.database.recording_audio().map_err(String::from)?;
        let mut first_error = None;
        for row in rows {
            if Some(&row.id) == active.as_ref() || row.audio_deleted_at.is_some() {
                continue;
            }
            match self.remove_audio(&row) {
                Ok(()) => self.notifier.recordings_changed(&row.id),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Deletes every recording except the one in progress.
    pub fn delete_all_recordings(&self) -> Result<(), String> {
        let active = self.active_id();
        let rows = self.database.recording_audio().map_err(String::from)?;
        let mut first_error = None;
        for row in rows {
            if Some(&row.id) == active.as_ref() {
                continue;
            }
            let removed = retention::delete_audio_dir(&self.audio_root, &row)
                .map_err(|error| format!("Couldn't delete the recording's audio: {error}"))
                .and_then(|()| {
                    self.database
                        .delete_recording(&row.id)
                        .map(|_| ())
                        .map_err(String::from)
                });
            match removed {
                Ok(()) => self.notifier.recordings_changed(&row.id),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::capture::fake::FakeBackend;
    use crate::recording::capture::Track;
    use crate::recording::segments::{SegmentWriter, WavWriter};
    use crate::recording::settings::KeepAudio;
    use crate::recording::test_support::TempDir;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestClock(Mutex<SystemTime>);
    impl Clock for TestClock {
        fn now(&self) -> SystemTime {
            *self.0.lock().unwrap()
        }
    }

    struct PlentyOfDisk(AtomicU64);
    impl DiskProbe for PlentyOfDisk {
        fn free_bytes(&self) -> Result<u64, String> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        State(RecordingPhase),
        Changed(String),
        Tray(RecordingPhase, bool),
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Call>>);
    impl RecordingNotifier for Recorder {
        fn state_changed(&self, status: &RecordingStatus) {
            self.0.lock().unwrap().push(Call::State(status.state));
        }
        fn recordings_changed(&self, recording_id: &str) {
            self.0
                .lock()
                .unwrap()
                .push(Call::Changed(recording_id.into()));
        }
        fn refresh_tray(&self, status: &RecordingStatus, settings: &RecordingSettings, _: bool) {
            self.0
                .lock()
                .unwrap()
                .push(Call::Tray(status.state, settings.record_meetings));
        }
    }

    impl Recorder {
        fn take(&self) -> Vec<Call> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    struct Harness {
        runtime: RecordingRuntime,
        backend: FakeBackend,
        clock: Arc<TestClock>,
        database: Arc<Database>,
        calls: Arc<Recorder>,
        root: TempDir,
    }

    fn at(text: &str) -> SystemTime {
        SystemTime::from(chrono::DateTime::parse_from_rfc3339(text).unwrap())
    }

    impl Harness {
        fn new() -> Self {
            Self::with_backend(FakeBackend::new())
        }

        fn with_backend(backend: FakeBackend) -> Self {
            let database = Arc::new(Database::open_memory());
            let clock = Arc::new(TestClock(Mutex::new(at("2026-11-27T10:00:00Z"))));
            let calls = Arc::new(Recorder::default());
            let root = TempDir::new();
            let runtime = RecordingRuntime::new(
                database.clone(),
                Arc::new(backend.clone()),
                clock.clone(),
                Arc::new(PlentyOfDisk(AtomicU64::new(u64::MAX))),
                root.path().to_path_buf(),
                calls.clone(),
            );
            Self {
                runtime,
                backend,
                clock,
                database,
                calls,
                root,
            }
        }

        fn advance(&self, duration: Duration) {
            *self.clock.0.lock().unwrap() += duration;
        }

        /// A finished recording with one audio file, ended `ended`.
        fn finished(&self, id: &str, ended: &str, state: RecordingState) -> PathBuf {
            let dir = self.root.path().join(id);
            self.database
                .create_recording(&crate::db::recordings::NewRecording {
                    id,
                    event_ref: None,
                    title: "Sync",
                    started_at: ended,
                    audio_dir: &dir.to_string_lossy(),
                    tracks: &[(Track::Mic, crate::db::recordings::TrackState::Active)],
                })
                .unwrap();
            self.database
                .finish_recording(id, ended, 1000, state, None)
                .unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            let mut wav = WavWriter::create(&dir.join("mic-0001.wav")).unwrap();
            wav.write(&[1; 16_000]).unwrap();
            wav.finish().unwrap();
            dir
        }

        fn audio_deleted(&self, id: &str) -> bool {
            self.database
                .recording_audio_of(id)
                .unwrap()
                .unwrap()
                .audio_deleted_at
                .is_some()
        }
    }

    #[test]
    fn start_needs_a_title_and_announces_the_new_recording() {
        let h = Harness::new();
        assert_eq!(
            h.runtime.start(None, "   ").unwrap_err(),
            "A recording needs a title."
        );
        assert!(h.calls.take().is_empty());
        let status = h.runtime.start(None, " Weekly Sync ").unwrap();
        assert_eq!(status.title.as_deref(), Some("Weekly Sync"));
        let id = status.recording_id.unwrap();
        assert_eq!(
            h.calls.take(),
            vec![
                Call::Changed(id.clone()),
                Call::State(RecordingPhase::Recording),
                Call::Tray(RecordingPhase::Recording, false),
            ]
        );
        assert_eq!(
            h.backend.target(Track::Mic).unwrap().audio_dir,
            h.root.path().join(&id),
            "audio goes in the recording's own folder"
        );
    }

    #[test]
    fn a_start_that_fails_announces_its_failed_recording() {
        let h = Harness::new();
        h.backend
            .fail_starts(Track::Mic, "No microphone is connected.");
        h.backend.fail_starts(Track::System, "No output.");
        assert_eq!(
            h.runtime.start(None, "Sync").unwrap_err(),
            "No microphone is connected."
        );
        let calls = h.calls.take();
        let Some(Call::Changed(id)) = calls.first() else {
            panic!("{calls:?}");
        };
        assert_eq!(
            h.database.recording(id).unwrap().unwrap().state,
            RecordingState::Failed
        );
        h.runtime.start(None, "Sync").unwrap_err();
        h.backend.allow_starts(Track::Mic);
        h.calls.take();
        let id = h.runtime.start(None, "Sync").unwrap().recording_id.unwrap();
        assert_eq!(h.calls.take()[0], Call::Changed(id), "only the new one");
    }

    #[test]
    fn state_is_published_on_change_and_the_tray_ticks_while_recording() {
        let h = Harness::new();
        let id = h.runtime.start(None, "Sync").unwrap().recording_id.unwrap();
        h.calls.take();
        h.advance(Duration::from_secs(1));
        h.runtime.tick();
        assert_eq!(
            h.calls.take(),
            vec![Call::Tray(RecordingPhase::Recording, false)],
            "elapsed time alone refreshes the tray but sends no state event"
        );
        h.runtime.pause().unwrap();
        assert_eq!(
            h.calls.take(),
            vec![
                Call::State(RecordingPhase::Paused),
                Call::Tray(RecordingPhase::Paused, false)
            ]
        );
        h.runtime.resume().unwrap();
        h.calls.take();
        h.backend.emit_frames(Track::Mic, 10);
        h.runtime.stop().unwrap();
        assert_eq!(
            h.calls.take(),
            vec![
                Call::State(RecordingPhase::Idle),
                Call::Changed(id),
                Call::Tray(RecordingPhase::Idle, false)
            ]
        );
        h.runtime.tick();
        assert!(h.calls.take().is_empty(), "idle ticks are quiet");
    }

    #[test]
    fn a_recording_the_service_stops_by_itself_is_announced() {
        let h = Harness::new();
        let id = h.runtime.start(None, "Sync").unwrap().recording_id.unwrap();
        h.calls.take();
        h.advance(policy::MAX_RECORDING);
        h.runtime.tick();
        let calls = h.calls.take();
        assert!(calls.contains(&Call::State(RecordingPhase::Idle)));
        assert!(calls.contains(&Call::Changed(id)));
        assert_eq!(
            h.runtime.status().last_stop_reason,
            Some(super::super::StopReason::LengthLimit)
        );
    }

    #[test]
    fn settings_are_saved_and_refresh_the_tray() {
        let h = Harness::new();
        assert!(!h.runtime.hide_on_close());
        h.runtime
            .set_settings(RecordingSettings {
                record_meetings: true,
                ..Default::default()
            })
            .unwrap();
        assert!(h.runtime.hide_on_close());
        assert!(h.database.recording_settings().unwrap().record_meetings);
        assert_eq!(h.calls.take(), vec![Call::Tray(RecordingPhase::Idle, true)]);
    }

    #[test]
    fn an_unsupported_platform_refuses_to_record_and_never_hides_the_window() {
        let database = Arc::new(Database::open_memory());
        database
            .set_recording_settings(&RecordingSettings {
                record_meetings: true,
                ..Default::default()
            })
            .unwrap();
        let root = TempDir::new();
        let runtime = RecordingRuntime::new(
            database,
            Arc::new(super::super::capture::UnsupportedBackend),
            Arc::new(TestClock(Mutex::new(at("2026-11-27T10:00:00Z")))),
            Arc::new(PlentyOfDisk(AtomicU64::new(u64::MAX))),
            root.path().to_path_buf(),
            Arc::new(Recorder::default()),
        );
        assert_eq!(
            runtime.capabilities(),
            RecordingCapabilities {
                supported: false,
                system_audio: SystemAudioSupport::Unsupported
            }
        );
        assert_eq!(
            runtime.start(None, "Sync").unwrap_err(),
            "Recording meetings isn't available on this platform yet."
        );
        assert!(!runtime.hide_on_close());
    }

    #[test]
    fn capabilities_report_system_audio_support() {
        let backend = FakeBackend::new();
        backend.set_system_audio(SystemAudioSupport::NeedsMacos {
            major: 14,
            minor: 2,
        });
        let h = Harness::with_backend(backend);
        let json = serde_json::to_value(h.runtime.capabilities()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "supported": true,
                "systemAudio": { "kind": "needsMacos", "major": 14, "minor": 2 }
            })
        );
    }

    #[test]
    fn deleting_refuses_the_recording_in_progress() {
        let h = Harness::new();
        let id = h.runtime.start(None, "Sync").unwrap().recording_id.unwrap();
        assert_eq!(
            h.runtime.delete_recording(&id).unwrap_err(),
            "This recording is still in progress. Stop it first."
        );
        assert_eq!(
            h.runtime.delete_recording_audio(&id).unwrap_err(),
            "This recording is still in progress. Stop it first."
        );
        assert!(h.database.recording(&id).unwrap().is_some());
    }

    #[test]
    fn deleting_a_recording_removes_its_row_and_its_audio_folder() {
        let h = Harness::new();
        let dir = h.finished("r1", "2026-11-27T09:00:00Z", RecordingState::Recorded);
        h.runtime.delete_recording("r1").unwrap();
        assert!(!dir.exists());
        assert!(h.database.recording("r1").unwrap().is_none());
        assert_eq!(h.calls.take(), vec![Call::Changed("r1".into())]);
        assert!(h.runtime.delete_recording("r1").is_err());
    }

    #[test]
    fn deleting_audio_keeps_the_recording_and_notes_when() {
        let h = Harness::new();
        let dir = h.finished("r1", "2026-11-27T09:00:00Z", RecordingState::Recorded);
        assert!(!h.audio_deleted("r1"));
        h.runtime.delete_recording_audio("r1").unwrap();
        assert!(!dir.exists());
        assert!(h.audio_deleted("r1"));
        assert!(h.database.recording("r1").unwrap().is_some());
        h.runtime.delete_recording_audio("r1").unwrap(); // again: fine
    }

    #[test]
    fn audio_outside_the_recordings_folder_is_never_deleted() {
        let h = Harness::new();
        let outside = TempDir::new();
        let file = outside.path().join("keep.txt");
        std::fs::write(&file, b"keep").unwrap();
        h.database
            .create_recording(&crate::db::recordings::NewRecording {
                id: "r1",
                event_ref: None,
                title: "Sync",
                started_at: "2026-11-27T09:00:00Z",
                audio_dir: &outside.path().to_string_lossy(),
                tracks: &[],
            })
            .unwrap();
        h.database
            .finish_recording(
                "r1",
                "2026-11-27T09:00:00Z",
                0,
                RecordingState::Recorded,
                None,
            )
            .unwrap();
        h.runtime.delete_recording("r1").unwrap();
        assert!(file.exists());
    }

    #[test]
    fn usage_counts_recordings_and_audio_bytes() {
        let h = Harness::new();
        h.finished("r1", "2026-11-27T09:00:00Z", RecordingState::Recorded);
        h.finished("r2", "2026-11-27T09:30:00Z", RecordingState::Recorded);
        let usage = h.runtime.usage().unwrap();
        assert_eq!(usage.recordings, 2);
        assert_eq!(usage.audio_bytes, 2 * (44 + 32_000));
        assert_eq!(
            serde_json::to_value(&usage).unwrap(),
            serde_json::json!({ "recordings": 2, "audioBytes": 64_088 })
        );
    }

    #[test]
    fn delete_all_skips_the_recording_in_progress() {
        let h = Harness::new();
        let old = h.finished("r1", "2026-11-27T09:00:00Z", RecordingState::Recorded);
        let live = h.runtime.start(None, "Sync").unwrap().recording_id.unwrap();
        let live_dir = h.root.path().join(&live);
        std::fs::create_dir_all(&live_dir).unwrap();
        h.runtime.delete_all_audio().unwrap();
        assert!(!old.exists() && h.audio_deleted("r1"));
        assert!(live_dir.exists() && !h.audio_deleted(&live));
        h.runtime.delete_all_recordings().unwrap();
        assert!(h.database.recording("r1").unwrap().is_none());
        assert!(h.database.recording(&live).unwrap().is_some());
        assert!(live_dir.exists());
    }

    #[test]
    fn the_sweep_follows_keep_audio_and_runs_daily() {
        let h = Harness::new();
        let old = h.finished("old", "2026-11-20T10:00:00Z", RecordingState::Recorded);
        let new = h.finished("new", "2026-11-20T10:00:01Z", RecordingState::Recorded);
        h.runtime.tick(); // the first tick sweeps
        assert!(
            !old.exists() && h.audio_deleted("old"),
            "exactly 7 days old"
        );
        assert!(
            new.exists() && !h.audio_deleted("new"),
            "a second short of 7 days"
        );
        assert!(h.calls.take().contains(&Call::Changed("old".into())));
        h.advance(policy::RETENTION_SWEEP_INTERVAL - Duration::from_secs(1));
        h.runtime.tick();
        assert!(
            !h.audio_deleted("new"),
            "the next sweep is a day after the last"
        );
        h.advance(Duration::from_secs(1));
        h.runtime.tick();
        assert!(!new.exists() && h.audio_deleted("new"));
    }

    #[test]
    fn changing_keep_audio_sweeps_at_once() {
        let h = Harness::new();
        h.runtime
            .set_settings(RecordingSettings {
                keep_audio: KeepAudio::UntilDeleted,
                ..Default::default()
            })
            .unwrap();
        let ancient = h.finished("a", "2020-01-01T00:00:00Z", RecordingState::Recorded);
        let transcribed = h.finished("t", "2026-11-27T09:00:00Z", RecordingState::Transcribed);
        assert_eq!(h.runtime.sweep(), 0);
        assert!(ancient.exists());
        h.runtime
            .set_settings(RecordingSettings {
                keep_audio: KeepAudio::UntilTranscribed,
                ..Default::default()
            })
            .unwrap();
        assert!(ancient.exists(), "not transcribed");
        assert!(!transcribed.exists() && h.audio_deleted("t"));
    }

    #[test]
    fn recovery_repairs_the_audio_and_records_what_was_saved() {
        let h = Harness::new();
        let dir = h.root.path().join("crashed");
        h.database
            .create_recording(&crate::db::recordings::NewRecording {
                id: "crashed",
                event_ref: None,
                title: "Sync",
                started_at: "2026-11-27T09:00:00Z",
                audio_dir: &dir.to_string_lossy(),
                tracks: &[
                    (Track::Mic, crate::db::recordings::TrackState::Active),
                    (Track::System, crate::db::recordings::TrackState::Active),
                ],
            })
            .unwrap();
        // 90 s of microphone, the last 30 s unfinished when the app died;
        // the system track started 10 s in and saved 20 s.
        let mut mic = SegmentWriter::open(&dir, Track::Mic).unwrap();
        mic.write(&vec![1; 90 * 16_000], 0).unwrap();
        mic.flush().unwrap();
        std::mem::forget(mic);
        let mut system = SegmentWriter::open(&dir, Track::System).unwrap();
        system.write(&vec![1; 20 * 16_000], 10_000).unwrap();
        drop(system);
        // An empty recording that never wrote audio.
        h.database
            .create_recording(&crate::db::recordings::NewRecording {
                id: "silent",
                event_ref: None,
                title: "Sync",
                started_at: "2026-11-27T09:30:00Z",
                audio_dir: &h.root.path().join("silent").to_string_lossy(),
                tracks: &[],
            })
            .unwrap();

        assert_eq!(h.runtime.recover(), 2);
        let crashed = h.database.recording("crashed").unwrap().unwrap();
        assert_eq!(crashed.state, RecordingState::Interrupted);
        assert_eq!(crashed.duration_ms, 90_000);
        assert_eq!(
            crashed.ended_at.as_deref(),
            Some("2026-11-27T09:01:30.000Z")
        );
        let header = std::fs::read(dir.join("mic-0002.wav")).unwrap();
        assert_eq!(
            u32::from_le_bytes(header[40..44].try_into().unwrap()),
            30 * 16_000 * 2,
            "the unfinished segment's header was repaired"
        );
        let silent = h.database.recording("silent").unwrap().unwrap();
        assert_eq!(silent.state, RecordingState::Interrupted);
        assert_eq!(silent.duration_ms, 0);
        assert_eq!(silent.ended_at.as_deref(), Some("2026-11-27T09:30:00.000Z"));
        assert_eq!(h.runtime.recover(), 0, "nothing left to recover");
    }

    #[test]
    fn stop_if_active_stops_only_a_recording_in_progress() {
        let h = Harness::new();
        h.runtime.stop_if_active();
        assert!(h.calls.take().is_empty());
        h.runtime.start(None, "Sync").unwrap();
        h.runtime.stop_if_active();
        assert!(!h.runtime.is_active());
        assert!(!h.backend.is_running(Track::Mic));
    }
}
