//! The scheduler through the recording runtime, with a fake transcriber.

use super::*;
use crate::clock::Clock;
use crate::db::recordings::{MeetingRecording, NewRecording, TrackState};
use crate::db::Database;
use crate::recording::capture::fake::FakeBackend;
use crate::recording::runtime::{RecordingNotifier, TranscriptionParts};
use crate::recording::segments::{GapReason, SegmentWriter};
use crate::recording::settings::{KeepAudio, RecordingSettings};
use crate::recording::test_support::TempDir;
use crate::recording::transcribe::models::tests::{
    content, local_urls, model_content, test_pins, vad_content, Reply, Server, MODEL_BYTES,
};
use crate::recording::transcribe::models::{ModelState, Pins, TranscriptionModelStatus, UrlPolicy};
use crate::recording::transcribe::protocol::SegmentRequest;
use crate::recording::{DiskProbe, RecordingStatus};
use std::collections::HashMap;
use std::time::SystemTime;

struct TestClock(Mutex<SystemTime>);
impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

struct Plenty;
impl DiskProbe for Plenty {
    fn free_bytes(&self) -> Result<u64, String> {
        Ok(u64::MAX)
    }
}

struct Power(AtomicBool);
impl PowerSource for Power {
    fn on_ac_power(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Changed(String),
    Progress(String, u8),
    Tray(Option<TrayTranscription>),
    Models(Vec<ModelState>),
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Call>>);
impl RecordingNotifier for Recorder {
    fn state_changed(&self, _: &RecordingStatus) {}
    fn recordings_changed(&self, id: &str) {
        self.0.lock().unwrap().push(Call::Changed(id.into()));
    }
    fn refresh_tray(
        &self,
        _: &RecordingStatus,
        _: &RecordingSettings,
        transcribing: Option<&TrayTranscription>,
        _: bool,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(Call::Tray(transcribing.cloned()));
    }
    fn transcription_progress(&self, id: &str, percent: u8) {
        self.0
            .lock()
            .unwrap()
            .push(Call::Progress(id.into(), percent));
    }
    fn models_changed(&self, statuses: &[TranscriptionModelStatus]) {
        self.0
            .lock()
            .unwrap()
            .push(Call::Models(statuses.iter().map(|s| s.state).collect()));
    }
}

impl Recorder {
    fn take(&self) -> Vec<Call> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

/// What the fake transcriber was asked, and how it should answer.
#[derive(Default)]
struct Script {
    opened: Vec<JobConfig>,
    requests: Vec<SegmentRequest>,
    sessions_alive: usize,
    /// Fail the request with this index (0-based, across sessions).
    fail_request: Option<usize>,
    fail_open: Option<JobError>,
    /// Block requests until cancelled.
    block: bool,
    /// Text for a (track, start ms); a default otherwise.
    texts: HashMap<(Track, u64), String>,
}

#[derive(Clone, Default)]
struct FakeTranscriber(Arc<Mutex<Script>>);

struct FakeSession {
    script: Arc<Mutex<Script>>,
    cancel: Arc<AtomicBool>,
}

impl Transcriber for FakeTranscriber {
    fn open(
        &self,
        config: &JobConfig,
        cancel: Arc<AtomicBool>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError> {
        let mut script = self.0.lock().unwrap();
        script.opened.push(config.clone());
        if let Some(error) = script.fail_open.take() {
            return Err(error);
        }
        script.sessions_alive += 1;
        Ok(Box::new(FakeSession {
            script: self.0.clone(),
            cancel,
        }))
    }
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.script.lock().unwrap().sessions_alive -= 1;
    }
}

impl TranscriptionSession for FakeSession {
    fn transcribe(
        &mut self,
        request: &SegmentRequest,
        audio_ms: u64,
    ) -> Result<Vec<TranscriptLine>, JobError> {
        let (index, block, text) = {
            let mut script = self.script.lock().unwrap();
            script.requests.push(request.clone());
            let text = script
                .texts
                .get(&(request.track, request.start_ms))
                .cloned()
                .unwrap_or_else(|| match request.track {
                    // No words in common, so nothing is echo.
                    Track::Mic => format!("I will send the notes, {}.", request.start_ms / 1000),
                    Track::System => format!(
                        "Thanks everyone for joining at {}.",
                        request.start_ms / 1000
                    ),
                });
            (script.requests.len() - 1, script.block, text)
        };
        if block {
            while !self.cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            return Err(JobError::Cancelled);
        }
        if self.script.lock().unwrap().fail_request == Some(index) {
            return Err(JobError::Failed(
                "The transcription engine stopped unexpectedly.".into(),
            ));
        }
        Ok(vec![TranscriptLine {
            track: request.track,
            start_ms: request.start_ms,
            end_ms: request.start_ms + audio_ms,
            text,
        }])
    }
}

struct Harness {
    runtime: Arc<RecordingRuntime>,
    backend: FakeBackend,
    database: Arc<Database>,
    calls: Arc<Recorder>,
    fake: FakeTranscriber,
    power: Arc<Power>,
    root: TempDir,
    models_dir: PathBuf,
    _data: TempDir,
}

fn at(text: &str) -> SystemTime {
    SystemTime::from(chrono::DateTime::parse_from_rfc3339(text).unwrap())
}

fn seconds(count: u64) -> Vec<i16> {
    vec![1; count as usize * 16_000]
}

impl Harness {
    fn new() -> Self {
        Self::with_pins(test_pins(1), local_urls(1))
    }

    fn with_pins(pins: Pins, urls: UrlPolicy) -> Self {
        let database = Arc::new(Database::open_memory());
        let calls = Arc::new(Recorder::default());
        let root = TempDir::new();
        let data = TempDir::new();
        let models_dir = data.path().join("recording-models");
        let backend = FakeBackend::new();
        let fake = FakeTranscriber::default();
        let power = Arc::new(Power(AtomicBool::new(true)));
        let models = Arc::new(ModelStore::new(
            models_dir.clone(),
            pins,
            Arc::new(Plenty),
            urls,
        ));
        let runtime = Arc::new(RecordingRuntime::new(
            database.clone(),
            Arc::new(backend.clone()),
            Arc::new(TestClock(Mutex::new(at("2026-11-27T12:00:00Z")))),
            Arc::new(Plenty),
            root.path().to_path_buf(),
            calls.clone(),
            TranscriptionParts {
                transcriber: Arc::new(fake.clone()),
                power: power.clone(),
                models,
            },
        ));
        Self {
            runtime,
            backend,
            database,
            calls,
            fake,
            power,
            root,
            models_dir,
            _data: data,
        }
    }

    /// Puts the Fast model and the voice model in place.
    fn install_models(&self) {
        std::fs::create_dir_all(&self.models_dir).unwrap();
        std::fs::write(self.models_dir.join("fast.bin"), model_content()).unwrap();
        std::fs::write(self.models_dir.join("vad.bin"), vad_content()).unwrap();
    }

    fn install_accurate(&self) {
        std::fs::write(
            self.models_dir.join("accurate.bin"),
            content(MODEL_BYTES, 5),
        )
        .unwrap();
    }

    /// A stopped recording with `mic` and `system` seconds of audio (system
    /// starting 5 s in), in state `state`.
    fn recorded(
        &self,
        id: &str,
        started: &str,
        mic: u64,
        system: u64,
        state: RecordingState,
    ) -> PathBuf {
        let dir = self.root.path().join(id);
        self.database
            .create_recording(&NewRecording {
                id,
                event_ref: None,
                title: "Weekly Sync",
                started_at: started,
                audio_dir: &dir.to_string_lossy(),
                tracks: &[
                    (Track::Mic, TrackState::Active),
                    (Track::System, TrackState::Active),
                ],
            })
            .unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        if mic > 0 {
            let mut writer = SegmentWriter::open(&dir, Track::Mic).unwrap();
            writer.write(&seconds(mic), 0).unwrap();
        }
        if system > 0 {
            let mut writer = SegmentWriter::open(&dir, Track::System).unwrap();
            writer.write(&seconds(system), 5_000).unwrap();
        }
        self.database
            .finish_recording(id, started, (mic.max(system) * 1000) as i64, state, None)
            .unwrap();
        dir
    }

    /// Steps until there is nothing left to do now.
    fn run(&self) -> usize {
        let mut steps = 0;
        while self.runtime.transcription_step() {
            steps += 1;
            assert!(steps < 200, "the scheduler never settled");
        }
        steps
    }

    fn state(&self, id: &str) -> RecordingState {
        self.database.recording(id).unwrap().unwrap().state
    }

    fn script(&self) -> MutexGuard<'_, Script> {
        self.fake.0.lock().unwrap()
    }

    /// One recording as `list_recordings` shows it.
    fn listed(&self, id: &str) -> MeetingRecording {
        let mut recording = self.database.recording(id).unwrap().unwrap();
        self.runtime.annotate(&mut recording);
        recording
    }

    fn progress(&self) -> Vec<u8> {
        self.calls
            .take()
            .into_iter()
            .filter_map(|call| match call {
                Call::Progress(_, percent) => Some(percent),
                _ => None,
            })
            .collect()
    }

    /// Writes `count` seconds of microphone audio into the recording in
    /// progress.
    fn live_mic(&self) -> SegmentWriter {
        let dir = self.backend.target(Track::Mic).unwrap().audio_dir;
        SegmentWriter::open(&dir, Track::Mic).unwrap()
    }

    fn stop(&self) {
        self.backend.emit_frames(Track::Mic, 10);
        self.runtime.stop().unwrap();
    }
}

#[test]
fn a_recorded_recording_is_transcribed_with_progress_and_events() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        90,
        60,
        RecordingState::Recorded,
    );
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
    let calls = h.calls.take();
    let changed: Vec<&Call> = calls
        .iter()
        .filter(|c| matches!(c, Call::Changed(_)))
        .collect();
    assert_eq!(
        changed,
        [&Call::Changed("r1".into()), &Call::Changed("r1".into())],
        "transcribing, transcribed"
    );
    let progress: Vec<u8> = calls
        .iter()
        .filter_map(|c| match c {
            Call::Progress(id, percent) if id == "r1" => Some(*percent),
            _ => None,
        })
        .collect();
    // 60, then 60, then 30 s of 150.
    assert_eq!(progress, [40, 80, 100]);
    assert!(calls.contains(&Call::Tray(Some(TrayTranscription {
        title: "Weekly Sync".into(),
        percent: 40
    }))));
    assert_eq!(
        calls.last(),
        Some(&Call::Tray(None)),
        "the tray line goes at the end"
    );

    let script = h.script();
    assert_eq!(script.opened.len(), 1, "one worker for the whole job");
    assert_eq!(script.opened[0].language.as_deref(), Some("en"));
    assert_eq!(script.opened[0].model_path, h.models_dir.join("fast.bin"));
    assert_eq!(
        script.opened[0].vad_model_path,
        h.models_dir.join("vad.bin")
    );
    let order: Vec<(Track, u64, bool, bool)> = script
        .requests
        .iter()
        .map(|r| (r.track, r.start_ms, r.continues, r.ends_run))
        .collect();
    assert_eq!(
        order,
        [
            (Track::Mic, 0, false, false),
            (Track::System, 5_000, false, true),
            (Track::Mic, 60_000, true, true),
        ]
    );
    assert_eq!(script.sessions_alive, 0, "the worker is gone");
    drop(script);
    assert_eq!(
        h.database.recording_engine("r1").unwrap(),
        Some((Some("whisper.cpp".into()), Some("fast.bin".into())))
    );
    let transcript = h.runtime.recording_transcript("r1").unwrap();
    let texts: Vec<&str> = transcript.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "I will send the notes, 0.",
            "Thanks everyone for joining at 5.",
            "I will send the notes, 60."
        ]
    );
    assert_eq!(h.run(), 0, "nothing left");
}

#[test]
fn recordings_wait_for_the_chosen_model_and_go_when_it_is_ready() {
    let server = Server::start(|path, range| match path {
        "/vad" => Reply::file(&vad_content(), range),
        _ => Reply::file(&model_content(), range),
    });
    let h = Harness::with_pins(test_pins(server.port), local_urls(server.port));
    h.recorded(
        "old",
        "2026-11-27T09:00:00Z",
        10,
        0,
        RecordingState::Interrupted,
    );
    h.recorded(
        "new",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    assert_eq!(h.run(), 0);
    assert_eq!(h.state("old"), RecordingState::Interrupted, "no model yet");
    assert!(h.script().opened.is_empty());

    // The download's end wakes the transcription thread.
    let started = Instant::now();
    h.runtime
        .download_transcription_model(TranscriptionModelChoice::Fast)
        .unwrap()
        .join()
        .unwrap();
    h.runtime
        .wait_for_transcription_work(Duration::from_secs(10));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "woken, not timed out"
    );
    assert!(h
        .calls
        .take()
        .contains(&Call::Models(vec![ModelState::Ready, ModelState::Absent])));

    // Oldest first: one job, then the next.
    assert!(h.runtime.transcription_step());
    assert_eq!(h.state("old"), RecordingState::Transcribing);
    assert_eq!(h.state("new"), RecordingState::Recorded);
    h.run();
    assert_eq!(h.state("old"), RecordingState::Transcribed);
    assert_eq!(h.state("new"), RecordingState::Transcribed);
    assert_eq!(h.script().opened.len(), 2);
}

#[test]
fn the_other_model_being_ready_does_not_start_a_job() {
    let h = Harness::new();
    h.install_models();
    h.runtime
        .set_settings(RecordingSettings {
            transcription_model: TranscriptionModelChoice::Accurate,
            ..Default::default()
        })
        .unwrap();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    assert_eq!(h.run(), 0);
    assert_eq!(h.state("r1"), RecordingState::Recorded);
    h.install_accurate();
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
    assert_eq!(
        h.script().opened[0].language,
        None,
        "Accurate detects the language"
    );
}

#[test]
fn a_running_job_keeps_the_model_it_started_with() {
    let h = Harness::new();
    h.install_models();
    h.install_accurate();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        90,
        0,
        RecordingState::Recorded,
    );
    assert!(h.runtime.transcription_step(), "opened, first segment done");
    h.runtime
        .set_settings(RecordingSettings {
            transcription_model: TranscriptionModelChoice::Accurate,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        h.runtime
            .delete_transcription_model(TranscriptionModelChoice::Fast)
            .unwrap_err(),
        "A transcription is using this model. Try again when it finishes."
    );
    h.runtime
        .delete_transcription_model(TranscriptionModelChoice::Accurate)
        .unwrap();
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
    assert_eq!(h.script().opened.len(), 1);
    assert_eq!(
        h.database
            .recording_engine("r1")
            .unwrap()
            .unwrap()
            .1
            .as_deref(),
        Some("fast.bin")
    );
    h.runtime
        .delete_transcription_model(TranscriptionModelChoice::Fast)
        .unwrap();
}

#[test]
fn a_failed_job_fails_the_recording_and_retry_transcribes_it_again() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        90,
        0,
        RecordingState::Recorded,
    );
    h.script().fail_request = Some(1);
    h.run();
    let failed = h.database.recording("r1").unwrap().unwrap();
    assert_eq!(failed.state, RecordingState::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("The transcription engine stopped unexpectedly.")
    );
    assert_eq!(h.script().sessions_alive, 0);
    assert_eq!(h.run(), 0, "a failed recording isn't picked up by itself");
    assert!(!h.listed("r1").audio_deleted, "so Retry is offered");

    h.calls.take();
    h.runtime.transcribe_recording("r1").unwrap();
    assert_eq!(h.state("r1"), RecordingState::Recorded);
    assert_eq!(h.calls.take()[0], Call::Changed("r1".into()));
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
    assert_eq!(h.database.recording("r1").unwrap().unwrap().error, None);
    assert_eq!(h.script().opened.len(), 2, "a fresh worker");
}

#[test]
fn a_worker_that_will_not_start_fails_the_recording() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    h.script().fail_open = Some(JobError::Failed(
        "The transcription model couldn't be loaded.".into(),
    ));
    h.run();
    let failed = h.database.recording("r1").unwrap().unwrap();
    assert_eq!(failed.state, RecordingState::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("The transcription model couldn't be loaded.")
    );
    assert!(h.runtime.scheduler.current().is_none());
}

#[test]
fn a_damaged_model_is_failed_and_the_recording_waits() {
    let h = Harness::new();
    h.install_models();
    let mut bytes = model_content();
    bytes[0] ^= 1;
    std::fs::write(h.models_dir.join("fast.bin"), bytes).unwrap();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    assert_eq!(h.run(), 0);
    assert_eq!(h.state("r1"), RecordingState::Recorded);
    assert!(h.script().opened.is_empty(), "never loaded");
    let fast = h.runtime.transcription_models().unwrap().remove(0);
    assert_eq!(fast.state, ModelState::Failed);
    assert_eq!(
        fast.error.as_deref(),
        Some("The model file was damaged. Download it again.")
    );
}

#[test]
fn retry_is_refused_unless_there_is_audio_to_transcribe() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "done",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Transcribed,
    );
    assert_eq!(
        h.runtime.transcribe_recording("done").unwrap_err(),
        "This recording is already transcribed."
    );
    h.recorded(
        "gone",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Failed,
    );
    h.runtime.delete_recording_audio("gone").unwrap();
    assert_eq!(
        h.runtime.transcribe_recording("gone").unwrap_err(),
        "This recording has no audio to transcribe."
    );
    h.recorded(
        "silent",
        "2026-11-27T10:00:00Z",
        0,
        0,
        RecordingState::Failed,
    );
    assert_eq!(
        h.runtime.transcribe_recording("silent").unwrap_err(),
        "This recording has no audio to transcribe."
    );
    assert_eq!(
        h.runtime.transcribe_recording("missing").unwrap_err(),
        "This recording no longer exists."
    );
    let live = h.runtime.start(None, "Sync").unwrap().recording_id.unwrap();
    assert_eq!(
        h.runtime.transcribe_recording(&live).unwrap_err(),
        "This recording is still in progress. Stop it first."
    );
}

#[test]
fn audio_deleted_covers_deleted_and_never_saved_audio() {
    let h = Harness::new();
    h.recorded(
        "ok",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    h.recorded(
        "deleted",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    h.runtime.delete_recording_audio("deleted").unwrap();
    h.recorded(
        "capture-failed",
        "2026-11-27T10:00:00Z",
        0,
        0,
        RecordingState::Failed,
    );
    assert!(!h.listed("ok").audio_deleted);
    assert!(h.listed("deleted").audio_deleted);
    assert!(h.listed("capture-failed").audio_deleted);
    // A recording in progress hasn't saved anything yet, but it will.
    let live = h.runtime.start(None, "Sync").unwrap().recording_id.unwrap();
    assert!(!h.listed(&live).audio_deleted);
}

#[test]
fn an_interrupted_recording_without_audio_fails_without_a_worker() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "empty",
        "2026-11-27T10:00:00Z",
        0,
        0,
        RecordingState::Interrupted,
    );
    h.recorded(
        "next",
        "2026-11-27T11:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    assert!(
        h.runtime.transcription_step(),
        "goes on to the next at once"
    );
    let failed = h.database.recording("empty").unwrap().unwrap();
    assert_eq!(failed.state, RecordingState::Failed);
    assert_eq!(failed.error.as_deref(), Some("No audio was recorded."));
    h.run();
    assert_eq!(h.state("next"), RecordingState::Transcribed);
    assert_eq!(h.script().opened.len(), 1, "only for the one with audio");
    assert!(h.listed("empty").audio_deleted, "no Retry");
}

#[test]
fn deleting_a_recording_cancels_its_job_first() {
    let h = Harness::new();
    h.install_models();
    let dir = h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        30,
        0,
        RecordingState::Recorded,
    );
    h.script().block = true;
    let runtime = h.runtime.clone();
    let stepping = std::thread::spawn(move || runtime.transcription_step());
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.script().requests.is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        h.runtime.delete_recording_audio("r1").unwrap_err(),
        "Wait for the transcription to finish, or delete the whole recording."
    );
    h.runtime.delete_recording("r1").unwrap();
    assert!(stepping.join().unwrap(), "the step ended");
    assert!(h.database.recording("r1").unwrap().is_none());
    assert!(!dir.exists());
    assert_eq!(h.script().sessions_alive, 0, "the worker was stopped");
    assert!(h.runtime.scheduler.current().is_none());
    assert_eq!(h.run(), 0);
}

#[test]
fn deleting_a_recording_between_segments_drops_its_job() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        90,
        0,
        RecordingState::Recorded,
    );
    assert!(h.runtime.transcription_step());
    assert_eq!(h.script().sessions_alive, 1);
    h.runtime.delete_recording("r1").unwrap();
    assert_eq!(h.script().sessions_alive, 0);
    assert_eq!(h.run(), 0);
}

#[test]
fn closed_segments_are_transcribed_during_a_recording_on_ac_power() {
    let h = Harness::new();
    h.install_models();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    mic.write(&seconds(61), 0).unwrap();
    mic.flush().unwrap();
    h.calls.take();
    h.run();
    assert_eq!(h.script().requests.len(), 1, "only the closed minute");
    assert_eq!(
        h.state(&id),
        RecordingState::Recording,
        "nothing visible yet"
    );
    assert!(h.runtime.scheduler.tray().is_none());
    let calls = h.calls.take();
    assert!(
        calls
            .iter()
            .all(|c| !matches!(c, Call::Changed(_) | Call::Progress(..))),
        "{calls:?}"
    );
    assert_eq!(h.listed(&id).transcription_percent, None);

    // Another minute closes.
    mic.write(&seconds(60), 61_000).unwrap();
    mic.flush().unwrap();
    h.run();
    assert_eq!(h.script().requests.len(), 2);

    // Stop: the rest, in the same worker.
    mic.end_run(GapReason::Stop).unwrap();
    drop(mic);
    h.stop();
    assert_eq!(h.state(&id), RecordingState::Recorded);
    h.calls.take();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let script = h.script();
    assert_eq!(script.opened.len(), 1, "the model loaded once");
    let flags: Vec<(u64, bool, bool)> = script
        .requests
        .iter()
        .map(|r| (r.start_ms, r.continues, r.ends_run))
        .collect();
    assert_eq!(
        flags,
        [
            (0, false, false),
            (60_000, true, false),
            (120_000, true, true)
        ]
    );
    drop(script);
    // 120 of 121 s were done before Stop: 99%, then 100%.
    assert_eq!(h.progress(), [100]);
}

#[test]
fn the_progress_after_stop_counts_what_was_done_during_the_recording() {
    let h = Harness::new();
    h.install_models();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    mic.write(&seconds(200), 0).unwrap();
    mic.flush().unwrap();
    h.run();
    assert_eq!(h.script().requests.len(), 3);
    drop(mic);
    h.stop();
    // The step that announces the job also transcribes the last 20 s.
    h.calls.take();
    assert!(h.runtime.transcription_step());
    assert_eq!(h.state(&id), RecordingState::Transcribing);
    assert_eq!(h.listed(&id).transcription_percent, Some(100));
    let calls = h.calls.take();
    assert!(calls.contains(&Call::Changed(id.clone())));
    assert!(calls.contains(&Call::Tray(Some(TrayTranscription {
        title: "Standup".into(),
        percent: 90
    }))));
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
}

#[test]
fn on_battery_transcription_waits_for_stop() {
    let h = Harness::new();
    h.install_models();
    h.power.0.store(false, Ordering::SeqCst);
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    mic.write(&seconds(130), 0).unwrap();
    mic.flush().unwrap();
    assert_eq!(h.run(), 0);
    assert!(h.script().opened.is_empty(), "no worker on battery");

    // Plugged in, it starts; back on battery, it pauses.
    h.power.0.store(true, Ordering::SeqCst);
    assert!(h.runtime.transcription_step());
    assert_eq!(h.script().requests.len(), 1);
    h.power.0.store(false, Ordering::SeqCst);
    assert_eq!(h.run(), 0);
    assert_eq!(h.script().requests.len(), 1);

    drop(mic);
    h.stop();
    h.run();
    assert_eq!(
        h.state(&id),
        RecordingState::Transcribed,
        "after Stop, battery or not"
    );
}

#[test]
fn a_job_that_fails_during_the_recording_starts_over_after_stop() {
    let h = Harness::new();
    h.install_models();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    mic.write(&seconds(130), 0).unwrap();
    mic.flush().unwrap();
    h.script().fail_request = Some(0);
    h.run();
    assert_eq!(h.state(&id), RecordingState::Recording);
    assert_eq!(h.script().opened.len(), 1);
    assert_eq!(h.run(), 0, "not retried during the recording");
    drop(mic);
    h.stop();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let script = h.script();
    assert_eq!(script.opened.len(), 2);
    assert_eq!(script.requests.len(), 1 + 3, "from the start");
}

#[test]
fn launch_recovery_requeues_a_transcription_that_was_running() {
    let h = Harness::new();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        10,
        0,
        RecordingState::Transcribing,
    );
    h.runtime.recover();
    assert_eq!(h.state("r1"), RecordingState::Recorded);
    h.install_models();
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
}

#[test]
fn retention_skips_the_recording_being_transcribed_and_runs_after_it() {
    let h = Harness::new();
    h.install_models();
    // Ended long ago, so 7 days would delete it.
    let dir = h.recorded(
        "r1",
        "2020-01-01T00:00:00Z",
        90,
        0,
        RecordingState::Recorded,
    );
    assert!(h.runtime.transcription_step());
    assert_eq!(h.runtime.sweep(), 0);
    assert!(dir.exists());
    h.runtime
        .set_settings(RecordingSettings {
            keep_audio: KeepAudio::UntilTranscribed,
            ..Default::default()
        })
        .unwrap();
    assert!(dir.exists(), "still transcribing");
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
    assert!(!dir.exists(), "Until Transcribed applies as soon as it is");
    assert!(h.listed("r1").audio_deleted);
}

#[test]
fn the_transcript_leaves_out_echo_and_labels_speakers_only_with_system_audio() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "both",
        "2026-11-27T10:00:00Z",
        10,
        10,
        RecordingState::Recorded,
    );
    {
        let mut script = h.script();
        script
            .texts
            .insert((Track::System, 5_000), "The launch moves to March.".into());
        // The microphone hears the speakers, overlapping in time.
        script
            .texts
            .insert((Track::Mic, 0), "the launch moves to march".into());
    }
    h.run();
    let transcript = h.runtime.recording_transcript("both").unwrap();
    assert_eq!(
        serde_json::to_value(&transcript).unwrap(),
        serde_json::json!({
            "recordingId": "both",
            "speakerLabels": true,
            "lines": [
                { "startMs": 5000, "endMs": 15000, "speaker": "others", "text": "The launch moves to March." }
            ]
        })
    );
    let rows = h.database.transcript_rows("both").unwrap();
    assert_eq!(rows.len(), 2, "the echo is stored, marked");
    assert!(rows.iter().any(|row| row.dropped_as_echo));

    h.recorded(
        "mic",
        "2026-11-27T11:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    h.run();
    let transcript = h.runtime.recording_transcript("mic").unwrap();
    assert!(!transcript.speaker_labels);
    assert_eq!(transcript.lines.len(), 1);
    assert_eq!(transcript.lines[0].speaker, None);
    assert_eq!(
        serde_json::to_value(&transcript.lines[0]).unwrap()["speaker"],
        serde_json::Value::Null
    );

    h.recorded(
        "waiting",
        "2026-11-27T12:00:00Z",
        10,
        0,
        RecordingState::Failed,
    );
    assert_eq!(
        h.runtime.recording_transcript("waiting").unwrap_err(),
        "This recording hasn't been transcribed yet."
    );
}

#[test]
fn the_scheduler_waits_until_woken() {
    let h = Harness::new();
    let started = Instant::now();
    h.runtime
        .wait_for_transcription_work(Duration::from_millis(50));
    assert!(started.elapsed() >= Duration::from_millis(50));
    h.runtime.scheduler.wake();
    let started = Instant::now();
    h.runtime
        .wait_for_transcription_work(Duration::from_secs(10));
    assert!(started.elapsed() < Duration::from_secs(5));
}
