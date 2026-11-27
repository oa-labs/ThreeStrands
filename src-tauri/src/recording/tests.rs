use super::capture::fake::FakeBackend;
use super::*;
use crate::db::recordings::TrackState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

struct TestClock(Mutex<SystemTime>);
impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

struct TestDisk(AtomicU64);
impl DiskProbe for TestDisk {
    fn free_bytes(&self) -> Result<u64, String> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct Harness {
    service: RecordingService,
    backend: FakeBackend,
    clock: Arc<TestClock>,
    disk: Arc<TestDisk>,
    database: Arc<Database>,
}

impl Harness {
    fn new() -> Self {
        let database = Arc::new(Database::open_memory());
        let backend = FakeBackend::new();
        let clock = Arc::new(TestClock(Mutex::new(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        )));
        let disk = Arc::new(TestDisk(AtomicU64::new(100 * 1024 * 1024 * 1024)));
        let service = RecordingService::new(
            database.clone(),
            Arc::new(backend.clone()),
            clock.clone(),
            disk.clone(),
            PathBuf::from("/tmp/recordings"),
        );
        Self {
            service,
            backend,
            clock,
            disk,
            database,
        }
    }

    fn advance(&self, duration: Duration) {
        *self.clock.0.lock().unwrap() += duration;
    }

    fn advance_secs(&self, seconds: u64) {
        self.advance(Duration::from_secs(seconds));
    }

    fn track_state(&self, track: Track) -> TrackRunState {
        self.service
            .status()
            .tracks
            .iter()
            .find(|t| t.track == track)
            .unwrap()
            .state
    }

    fn stored(&self, id: &str) -> crate::db::recordings::MeetingRecording {
        self.database.recording(id).unwrap().unwrap()
    }

    fn started(&mut self) -> String {
        self.service
            .start(None, "Weekly Sync")
            .unwrap()
            .recording_id
            .unwrap()
    }
}

fn stored_track(
    recording: &crate::db::recordings::MeetingRecording,
    track: Track,
) -> &crate::db::recordings::MeetingRecordingTrack {
    recording.tracks.iter().find(|t| t.track == track).unwrap()
}

const GB: u64 = 1024 * 1024 * 1024;

#[test]
fn start_pause_resume_stop_records_elapsed_without_pauses() {
    let mut h = Harness::new();
    let id = h.started();
    let status = h.service.status();
    assert_eq!(status.state, RecordingPhase::Recording);
    assert_eq!(status.title.as_deref(), Some("Weekly Sync"));
    assert_eq!(h.stored(&id).state, RecordingState::Recording);
    h.backend.emit_first_frame(Track::Mic, 0);
    h.service.tick();
    h.advance_secs(10);
    let paused = h.service.pause().unwrap();
    assert_eq!(paused.state, RecordingPhase::Paused);
    assert_eq!(paused.elapsed_ms, 10_000);
    assert!(h.backend.is_paused(Track::Mic));
    h.advance_secs(300);
    assert_eq!(
        h.service.status().elapsed_ms,
        10_000,
        "pauses are not counted"
    );
    h.service.resume().unwrap();
    assert!(!h.backend.is_paused(Track::Mic));
    h.advance_secs(5);
    assert_eq!(h.service.status().elapsed_ms, 15_000);
    let stopped = h.service.stop().unwrap();
    assert_eq!(stopped.state, RecordingPhase::Idle);
    assert_eq!(stopped.last_stop_reason, None);
    let row = h.stored(&id);
    assert_eq!(row.state, RecordingState::Recorded);
    assert_eq!(row.duration_ms, 15_000);
    assert!(row.ended_at.is_some());
    assert!(!h.backend.is_running(Track::Mic));
}

#[test]
fn stopping_while_paused_does_not_count_the_pause() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend.emit_frames(Track::Mic, 10);
    h.service.tick();
    h.advance_secs(20);
    h.service.pause().unwrap();
    h.advance_secs(100);
    h.service.stop().unwrap();
    assert_eq!(h.stored(&id).duration_ms, 20_000);
}

#[test]
fn a_second_start_is_refused_and_leaves_the_first_running() {
    let mut h = Harness::new();
    let id = h.started();
    assert_eq!(
        h.service.start(None, "Other").unwrap_err(),
        "A recording is already in progress."
    );
    assert_eq!(
        h.service.status().recording_id.as_deref(),
        Some(id.as_str())
    );
}

#[test]
fn pause_resume_and_stop_when_not_applicable_say_so() {
    let mut h = Harness::new();
    assert!(h.service.pause().unwrap_err().contains("no recording"));
    assert!(h.service.resume().unwrap_err().contains("no recording"));
    assert!(h.service.stop().unwrap_err().contains("no recording"));
    h.started();
    assert!(h.service.resume().unwrap_err().contains("isn't paused"));
    h.service.pause().unwrap();
    assert!(h.service.pause().unwrap_err().contains("already paused"));
    h.service.resume().unwrap();
    h.backend.emit_frames(Track::Mic, 1);
    h.service.stop().unwrap();
    assert!(h.service.stop().unwrap_err().contains("no recording"));
}

#[test]
fn a_recording_with_no_audio_ends_failed() {
    let mut h = Harness::new();
    let id = h.started();
    h.advance_secs(3);
    h.service.stop().unwrap();
    let row = h.stored(&id);
    assert_eq!(row.state, RecordingState::Failed);
    assert_eq!(row.error.as_deref(), Some("No audio was recorded."));
}

#[test]
fn recording_is_microphone_only_when_system_audio_is_unavailable() {
    for support in [
        SystemAudioSupport::NeedsMacos {
            major: 13,
            minor: 6,
        },
        SystemAudioSupport::Unsupported,
    ] {
        let mut h = Harness::new();
        h.backend.set_system_audio(support);
        let id = h.started();
        let status = h.service.status();
        assert_eq!(status.system_audio, support);
        assert_eq!(h.track_state(Track::System), TrackRunState::Unavailable);
        assert_eq!(h.backend.start_count(Track::System), 0);
        assert_eq!(
            stored_track(&h.stored(&id), Track::System).state,
            TrackState::Unavailable
        );
        // An unavailable track never counts against "all tracks failed".
        h.backend.emit_frames(Track::Mic, 5);
        h.service.tick();
        assert_eq!(h.service.status().state, RecordingPhase::Recording);
    }
}

#[test]
fn a_track_that_fails_to_start_is_failed_and_the_other_records() {
    let mut h = Harness::new();
    h.backend
        .fail_starts(Track::System, "Audio capture was refused");
    let id = h.started();
    assert_eq!(h.track_state(Track::System), TrackRunState::Failed);
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Active);
    let row = h.stored(&id);
    let system = stored_track(&row, Track::System);
    assert_eq!(system.state, TrackState::Failed);
    assert_eq!(system.error.as_deref(), Some("Audio capture was refused"));
}

#[test]
fn start_fails_when_the_microphone_fails_and_system_audio_cannot_help() {
    let mut h = Harness::new();
    h.backend.set_system_audio(SystemAudioSupport::Unsupported);
    h.backend
        .fail_starts(Track::Mic, "Microphone access was denied");
    assert_eq!(
        h.service.start(None, "Sync").unwrap_err(),
        "Microphone access was denied"
    );
    assert_eq!(h.service.status().state, RecordingPhase::Idle);
    let id = failed_row_id(&h);
    let row = h.stored(&id);
    assert_eq!(row.state, RecordingState::Failed);
    assert_eq!(row.error.as_deref(), Some("Microphone access was denied"));
    // And a later start works.
    h.backend.allow_starts(Track::Mic);
    h.service.start(None, "Sync").unwrap();
}

#[test]
fn start_fails_when_both_tracks_fail() {
    let mut h = Harness::new();
    h.backend.fail_starts(Track::Mic, "no mic");
    h.backend.fail_starts(Track::System, "no tap");
    assert_eq!(h.service.start(None, "Sync").unwrap_err(), "no mic");
    let id = failed_row_id(&h);
    let row = h.stored(&id);
    assert_eq!(row.state, RecordingState::Failed);
    assert!(row.tracks.iter().all(|t| t.state == TrackState::Failed));
}

/// The only recording row in the database.
fn failed_row_id(h: &Harness) -> String {
    h.database
        .test_connection()
        .unwrap()
        .query_row("SELECT id FROM meeting_recordings", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn start_is_refused_below_the_disk_floor_and_allowed_at_it() {
    let mut h = Harness::new();
    h.disk.0.store(2 * GB - 1, Ordering::SeqCst);
    assert!(h
        .service
        .start(None, "Sync")
        .unwrap_err()
        .contains("free disk space"));
    assert_eq!(h.service.status().state, RecordingPhase::Idle);
    h.disk.0.store(2 * GB, Ordering::SeqCst);
    h.service.start(None, "Sync").unwrap();
}

#[test]
fn the_first_frame_of_a_late_system_track_is_placed_on_the_recording_clock() {
    let mut h = Harness::new();
    h.backend.set_host_time_ns(5_000_000_000);
    let id = h.started();
    h.backend.emit_first_frame(Track::Mic, 5_120_000_000);
    h.advance_secs(30);
    h.backend.emit_first_frame(Track::System, 35_500_000_000);
    h.service.tick();
    let row = h.stored(&id);
    assert_eq!(
        stored_track(&row, Track::Mic).first_frame_offset_ms,
        Some(120)
    );
    assert_eq!(
        stored_track(&row, Track::System).first_frame_offset_ms,
        Some(30_500)
    );
}

#[test]
fn a_repeated_first_frame_after_a_restart_keeps_the_original_offset() {
    let mut h = Harness::new();
    h.backend.set_host_time_ns(0);
    let id = h.started();
    h.backend.emit_first_frame(Track::Mic, 1_000_000_000);
    h.service.tick();
    h.backend.emit_failed(Track::Mic, "gone");
    h.service.tick();
    h.advance_secs(1);
    h.service.tick();
    h.backend.emit_first_frame(Track::Mic, 9_000_000_000);
    h.service.tick();
    assert_eq!(
        stored_track(&h.stored(&id), Track::Mic).first_frame_offset_ms,
        Some(1000)
    );
}

#[test]
fn device_changes_are_recorded() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend.emit_device_changed(Track::Mic, "AirPods");
    h.service.tick();
    assert_eq!(
        stored_track(&h.stored(&id), Track::Mic)
            .device_name
            .as_deref(),
        Some("AirPods")
    );
}

#[test]
fn a_silent_track_that_never_delivered_audio_does_not_stall() {
    let mut h = Harness::new();
    h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    for _ in 0..3 {
        h.advance_secs(4);
        h.backend.emit_frames(Track::Mic, 100);
        h.service.tick();
    }
    // The system tap has been silent for 12 s: nobody played audio yet.
    assert_eq!(h.track_state(Track::System), TrackRunState::Active);
    assert_eq!(h.backend.start_count(Track::System), 1);
}

#[test]
fn a_stalled_track_restarts_with_backoff_and_recovers() {
    let mut h = Harness::new();
    h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    h.service.tick();
    h.advance_secs(4);
    h.service.tick();
    assert_eq!(
        h.track_state(Track::Mic),
        TrackRunState::Active,
        "not yet stalled"
    );
    h.advance_secs(1);
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Restarting);
    assert!(!h.backend.is_running(Track::Mic));
    h.advance(Duration::from_millis(999));
    h.service.tick();
    assert_eq!(h.backend.start_count(Track::Mic), 1, "backoff is 1 s");
    h.advance(Duration::from_millis(1));
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Active);
    assert_eq!(h.backend.start_count(Track::Mic), 2);
    h.backend.emit_frames(Track::Mic, 50);
    h.service.tick();
    // Recovered: a later stall starts the backoff over at 1 s.
    h.advance_secs(5);
    h.service.tick();
    h.advance_secs(1);
    h.service.tick();
    assert_eq!(h.backend.start_count(Track::Mic), 3);
}

#[test]
fn a_failed_event_restarts_the_track() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    h.backend.emit_failed(Track::Mic, "device unplugged");
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Restarting);
    // The database still says active: failed means given up.
    assert_eq!(
        stored_track(&h.stored(&id), Track::Mic).state,
        TrackState::Active
    );
    h.advance_secs(1);
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Active);
}

#[test]
fn a_track_that_keeps_stalling_fails_after_three_restarts_while_the_other_continues() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    h.backend.emit_first_frame(Track::System, 0);
    h.service.tick();
    let keep_system_alive = |h: &mut Harness| {
        h.backend.emit_frames(Track::System, 10);
        h.service.tick();
    };
    // Stall, then restarts after 1 s, 2 s, and 4 s; each restarted session
    // stays silent.
    for wait in [1, 2, 4] {
        h.advance_secs(5);
        keep_system_alive(&mut h);
        assert_eq!(h.track_state(Track::Mic), TrackRunState::Restarting);
        h.advance_secs(wait);
        keep_system_alive(&mut h);
        assert_eq!(h.track_state(Track::Mic), TrackRunState::Active);
    }
    h.advance_secs(5);
    keep_system_alive(&mut h);
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Failed);
    assert_eq!(h.backend.start_count(Track::Mic), 4);
    assert_eq!(h.track_state(Track::System), TrackRunState::Active);
    assert_eq!(h.service.status().state, RecordingPhase::Recording);
    assert_eq!(
        stored_track(&h.stored(&id), Track::Mic).state,
        TrackState::Failed
    );
}

#[test]
fn a_restart_that_cannot_start_counts_toward_the_limit() {
    let mut h = Harness::new();
    h.backend.set_system_audio(SystemAudioSupport::Unsupported);
    let id = h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    h.service.tick();
    h.backend.fail_starts(Track::Mic, "device busy");
    h.backend.emit_failed(Track::Mic, "gone");
    h.service.tick();
    for wait in [1, 2, 4] {
        h.advance_secs(wait);
        h.service.tick();
    }
    assert_eq!(h.service.status().state, RecordingPhase::Idle);
    assert_eq!(
        h.service.status().last_stop_reason,
        Some(StopReason::AllTracksFailed)
    );
    assert_eq!(h.stored(&id).state, RecordingState::Recorded);
}

#[test]
fn stop_succeeds_when_a_queued_failure_ends_the_recording_first() {
    let mut h = Harness::new();
    h.backend.set_system_audio(SystemAudioSupport::Unsupported);
    let id = h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    h.service.tick();
    // Use up every restart without the microphone delivering frames again.
    for wait in [1, 2, 4] {
        h.backend.emit_failed(Track::Mic, "gone");
        h.service.tick();
        h.advance_secs(wait);
        h.service.tick();
        assert_eq!(h.track_state(Track::Mic), TrackRunState::Active);
    }
    // The last failure is still queued when the user presses Stop.
    h.backend.emit_failed(Track::Mic, "gone");
    let stopped = h.service.stop().unwrap();
    assert_eq!(stopped.state, RecordingPhase::Idle);
    assert_eq!(h.stored(&id).state, RecordingState::Recorded);
    assert!(h.service.stop().is_err(), "a second stop still says so");
}

#[test]
fn when_every_track_has_failed_the_recording_stops() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    h.service.tick();
    h.backend.fail_starts(Track::Mic, "gone");
    h.backend.fail_starts(Track::System, "gone");
    h.backend.emit_failed(Track::Mic, "gone");
    h.backend.emit_failed(Track::System, "gone");
    h.service.tick();
    for wait in [1, 2, 4] {
        h.advance_secs(wait);
        h.service.tick();
    }
    let status = h.service.status();
    assert_eq!(status.state, RecordingPhase::Idle);
    assert_eq!(status.last_stop_reason, Some(StopReason::AllTracksFailed));
    let row = h.stored(&id);
    assert_eq!(
        row.state,
        RecordingState::Recorded,
        "the mic delivered audio first"
    );
    assert!(row.tracks.iter().all(|t| t.state == TrackState::Failed));
    // A later start clears the reason.
    h.backend.allow_starts(Track::Mic);
    h.backend.allow_starts(Track::System);
    assert_eq!(
        h.service.start(None, "Again").unwrap().last_stop_reason,
        None
    );
}

#[test]
fn a_paused_recording_never_stalls() {
    let mut h = Harness::new();
    h.started();
    h.backend.emit_first_frame(Track::Mic, 0);
    h.service.tick();
    h.service.pause().unwrap();
    h.advance_secs(120);
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Active);
    assert_eq!(h.backend.start_count(Track::Mic), 1);
    h.service.resume().unwrap();
    h.advance_secs(4);
    h.service.tick();
    assert_eq!(
        h.track_state(Track::Mic),
        TrackRunState::Active,
        "the silence clock restarted"
    );
    h.advance_secs(1);
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Restarting);
}

#[test]
fn the_length_limit_stops_at_exactly_four_hours() {
    let mut h = Harness::new();
    let id = h.started();
    let keep_alive = |h: &mut Harness| {
        h.backend.emit_frames(Track::Mic, 1);
        h.backend.emit_frames(Track::System, 1);
        h.service.tick();
    };
    h.backend.emit_first_frame(Track::Mic, 0);
    // Step in 4 s strides so nothing stalls.
    let limit = policy::MAX_RECORDING;
    let step = Duration::from_secs(4);
    let mut elapsed = Duration::ZERO;
    while elapsed + step < limit {
        h.advance(step);
        elapsed += step;
        keep_alive(&mut h);
    }
    h.advance(limit - Duration::from_millis(1) - elapsed);
    keep_alive(&mut h);
    assert_eq!(
        h.service.status().state,
        RecordingPhase::Recording,
        "just below"
    );
    h.advance(Duration::from_millis(1));
    keep_alive(&mut h);
    let status = h.service.status();
    assert_eq!(status.state, RecordingPhase::Idle, "at the limit");
    assert_eq!(status.last_stop_reason, Some(StopReason::LengthLimit));
    let row = h.stored(&id);
    assert_eq!(row.state, RecordingState::Recorded);
    assert_eq!(row.duration_ms, limit.as_millis() as i64);
}

#[test]
fn time_spent_paused_does_not_count_toward_the_length_limit() {
    let mut h = Harness::new();
    h.started();
    h.backend.emit_frames(Track::Mic, 1);
    h.service.tick();
    h.service.pause().unwrap();
    h.advance(policy::MAX_RECORDING * 2);
    h.service.tick();
    assert_eq!(h.service.status().state, RecordingPhase::Paused);
}

#[test]
fn recording_stops_when_free_disk_drops_below_the_floor() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend.emit_frames(Track::Mic, 1);
    h.disk.0.store(2 * GB, Ordering::SeqCst);
    h.service.tick();
    assert_eq!(
        h.service.status().state,
        RecordingPhase::Recording,
        "at the floor"
    );
    h.disk.0.store(2 * GB - 1, Ordering::SeqCst);
    h.service.tick();
    let status = h.service.status();
    assert_eq!(status.state, RecordingPhase::Idle);
    assert_eq!(status.last_stop_reason, Some(StopReason::LowDisk));
    assert_eq!(h.stored(&id).state, RecordingState::Recorded);
}

#[test]
fn a_recording_keeps_its_event_reference() {
    let mut h = Harness::new();
    let event = NoteReference {
        version: 1,
        kind: "event".into(),
        id: "evt-1".into(),
        account: Some("you@example.com".into()),
        calendar: Some("primary".into()),
        note: None,
        contact: None,
        series: None,
    };
    let status = h.service.start(Some(event.clone()), "Weekly Sync").unwrap();
    assert_eq!(status.event_ref, Some(event.clone()));
    assert_eq!(h.database.list_recordings(&event).unwrap().len(), 1);
}

#[test]
fn the_volume_probe_reads_free_space() {
    let dir = super::test_support::TempDir::new();
    let free = VolumeProbe(dir.path().to_path_buf()).free_bytes().unwrap();
    assert!(free > 0);
    assert!(VolumeProbe(PathBuf::from("/no/such/folder/here"))
        .free_bytes()
        .is_err());
}

#[test]
fn a_restart_writes_to_the_same_folder_and_a_failed_first_retry_backs_off_again() {
    let mut h = Harness::new();
    let id = h.started();
    let first = h.backend.target(Track::Mic).unwrap();
    assert_eq!(first.audio_dir, PathBuf::from("/tmp/recordings").join(&id));
    h.backend.fail_next_starts(Track::Mic, 1, "busy");
    h.backend.emit_failed(Track::Mic, "gone");
    h.service.tick();
    assert_eq!(
        h.backend.stop_count(Track::Mic),
        1,
        "the failed session stopped"
    );
    h.advance_secs(1);
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Restarting);
    h.advance_secs(2);
    h.service.tick();
    assert_eq!(h.track_state(Track::Mic), TrackRunState::Active);
    assert_eq!(
        h.backend.target(Track::Mic).unwrap(),
        first,
        "a restarted track keeps the recording's folder and clock"
    );
}

#[test]
fn sleep_stops_active_recording_cleanly() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend.emit_frames(Track::Mic, 10);
    h.service.tick();
    h.service.stop_for_sleep();
    let status = h.service.status();
    assert_eq!(status.state, RecordingPhase::Idle);
    assert_eq!(status.last_stop_reason, Some(StopReason::SystemSleep));
    assert_eq!(h.stored(&id).state, RecordingState::Recorded);
}

#[test]
fn digital_zero_system_tap_warns_after_sustained_mic_speech_and_clears_on_signal() {
    let mut h = Harness::new();
    h.started();
    for _ in 0..(policy::SILENT_TAP_SPEECH_SECS - 1) {
        h.backend.emit_frames_with_peak(Track::Mic, 16_000, 0.1);
        h.backend.emit_frames_with_peak(Track::System, 16_000, 0.0);
        h.service.tick();
        assert!(!h.service.status().system_audio_silent_warning);
    }
    h.backend.emit_frames_with_peak(Track::Mic, 16_000, 0.1);
    h.backend.emit_frames_with_peak(Track::System, 16_000, 0.0);
    h.service.tick();
    assert!(h.service.status().system_audio_silent_warning);
    // Real system audio clears the warning.
    h.backend.emit_frames_with_peak(Track::System, 16_000, 0.05);
    h.service.tick();
    assert!(!h.service.status().system_audio_silent_warning);
}

#[test]
fn device_change_records_headphones_state_unless_any_output_used_speakers() {
    let mut h = Harness::new();
    let id = h.started();
    h.backend
        .emit_device_changed_route(Track::System, "AirPods", Some(true));
    h.service.tick();
    assert_eq!(
        stored_track(&h.stored(&id), Track::System).is_headphones,
        Some(true)
    );
    h.backend
        .emit_device_changed_route(Track::System, "MacBook Speakers", Some(false));
    h.service.tick();
    assert_eq!(
        stored_track(&h.stored(&id), Track::System).is_headphones,
        Some(false)
    );
    h.backend
        .emit_device_changed_route(Track::System, "AirPods", Some(true));
    h.service.tick();
    assert_eq!(
        stored_track(&h.stored(&id), Track::System).is_headphones,
        Some(false),
        "stay false once speakers were used"
    );
}
