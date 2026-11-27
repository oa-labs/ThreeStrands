//! The scheduler through the recording runtime, with a fake transcriber.

use super::*;
use crate::clock::Clock;
use crate::db::recordings::{MeetingRecording, NewRecording, TrackState};
use crate::db::Database;
use crate::recording::capture::fake::FakeBackend;
use crate::recording::offers::TrayOffer;
use crate::recording::runtime::{RecordingNotifier, TranscriptionParts, TrayReview};
use crate::recording::segments::{self, GapReason, SegmentWriter};
use crate::recording::settings::{KeepAudio, RecordingSettings};
use crate::recording::test_support::TempDir;
use crate::recording::transcribe::memory::MemoryPressure;
use crate::recording::transcribe::models::tests::{
    content, local_urls, model_content, test_pins, vad_content, Reply, Server, MODEL_BYTES,
};
use crate::recording::transcribe::models::{ModelState, Pins, TranscriptionModelStatus, UrlPolicy};
use crate::recording::transcribe::protocol::{HeldPiece, SegmentRequest};
use crate::recording::transcribe::Transcribed;
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

/// Memory pressure and available memory, set by each test.
struct Memory {
    pressure: Mutex<MemoryPressure>,
    available: Mutex<Option<u64>>,
}
impl MemoryProbe for Memory {
    fn pressure(&self) -> MemoryPressure {
        *self.pressure.lock().unwrap()
    }
    fn available_bytes(&self) -> Option<u64> {
        *self.available.lock().unwrap()
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
        _: Option<&TrayReview>,
        _: Option<&TrayOffer>,
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
    /// Block requests until stopped (cancelled or interrupted).
    block: bool,
    /// Text for a (track, start ms); a default otherwise.
    texts: HashMap<(Track, u64), String>,
    /// What the worker reports holding after the file starting at (track,
    /// start ms); nothing otherwise.
    held: HashMap<(Track, u64), HeldAudio>,
    /// Detected language reported back by the fake worker.
    detected_language: Option<String>,
    /// The memory budget each worker was started with.
    budgets: Vec<u64>,
}

#[derive(Clone, Default)]
struct FakeTranscriber(Arc<Mutex<Script>>);

struct FakeSession {
    script: Arc<Mutex<Script>>,
    stop: Arc<dyn StopCheck>,
}

impl Transcriber for FakeTranscriber {
    fn open(
        &self,
        config: &JobConfig,
        stop: Arc<dyn StopCheck>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError> {
        let mut script = self.0.lock().unwrap();
        script.opened.push(config.clone());
        script.budgets.push(config.memory_bytes);
        if let Some(error) = script.fail_open.take() {
            return Err(error);
        }
        script.sessions_alive += 1;
        Ok(Box::new(FakeSession {
            script: self.0.clone(),
            stop,
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
    ) -> Result<Transcribed, JobError> {
        let (index, block, text, held, detected_language) = {
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
            let held = script.held.get(&(request.track, request.start_ms)).cloned();
            let detected_language = script.detected_language.clone();
            (
                script.requests.len() - 1,
                script.block,
                text,
                held,
                detected_language,
            )
        };
        if block {
            loop {
                if let Some(reason) = self.stop.stop_reason() {
                    return Err(reason);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if self.script.lock().unwrap().fail_request == Some(index) {
            return Err(JobError::Failed(
                "The transcription engine stopped unexpectedly.".into(),
            ));
        }
        Ok(Transcribed {
            lines: vec![TranscriptLine {
                track: request.track,
                start_ms: request.start_ms,
                end_ms: request.start_ms + audio_ms,
                text,
                rms: None,
                speaker_label: None,
            }],
            held,
            detected_language,
        })
    }
}

struct Harness {
    runtime: Arc<RecordingRuntime>,
    backend: FakeBackend,
    database: Arc<Database>,
    calls: Arc<Recorder>,
    fake: FakeTranscriber,
    power: Arc<Power>,
    memory: Arc<Memory>,
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
        let fake = FakeTranscriber::default();
        Self::with(pins, urls, Arc::new(fake.clone()), fake)
    }

    /// With another transcriber than the scripted fake (whose script then
    /// stays empty).
    fn with_transcriber(transcriber: Arc<dyn Transcriber>) -> Self {
        Self::with(
            test_pins(1),
            local_urls(1),
            transcriber,
            FakeTranscriber::default(),
        )
    }

    fn with_speech_analyzer() -> Self {
        let fake = FakeTranscriber::default();
        Self::build(
            test_pins(1),
            local_urls(1),
            Arc::new(fake.clone()),
            fake,
            true,
        )
    }

    fn with(
        pins: Pins,
        urls: UrlPolicy,
        transcriber: Arc<dyn Transcriber>,
        fake: FakeTranscriber,
    ) -> Self {
        Self::build(pins, urls, transcriber, fake, false)
    }

    fn build(
        pins: Pins,
        urls: UrlPolicy,
        transcriber: Arc<dyn Transcriber>,
        fake: FakeTranscriber,
        speech_analyzer: bool,
    ) -> Self {
        let database = Arc::new(Database::open_memory());
        let calls = Arc::new(Recorder::default());
        let root = TempDir::new();
        let data = TempDir::new();
        let models_dir = data.path().join("recording-models");
        let backend = FakeBackend::new();
        backend.set_speech_analyzer(speech_analyzer);
        let power = Arc::new(Power(AtomicBool::new(true)));
        let memory = Arc::new(Memory {
            pressure: Mutex::new(MemoryPressure::Normal),
            available: Mutex::new(Some(u64::MAX)),
        });
        let models = Arc::new(ModelStore::new(
            models_dir.clone(),
            pins,
            Arc::new(Plenty),
            urls,
        ));
        struct NoCloud;
        impl crate::recording::transcribe::cloud::CloudTranscriptionBackend for NoCloud {
            fn transcribe_chunk(
                &self,
                _: &crate::recording::transcribe::cloud::CloudChunkRequest,
            ) -> Result<crate::recording::transcribe::cloud::CloudChunkResponse, String>
            {
                Err("No cloud backend in this test.".into())
            }
        }
        let runtime = Arc::new(RecordingRuntime::new(
            database.clone(),
            Arc::new(backend.clone()),
            Arc::new(TestClock(Mutex::new(at("2026-11-27T12:00:00Z")))),
            Arc::new(Plenty),
            root.path().to_path_buf(),
            calls.clone(),
            TranscriptionParts {
                transcriber,
                cloud: Arc::new(NoCloud),
                power: power.clone(),
                memory: memory.clone(),
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
            memory,
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
                attendees: &[],
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
    assert_eq!(
        h.script().opened[0].memory_bytes,
        policy::WORKER_MEMORY_ACCURATE_BYTES
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
fn closed_segments_are_transcribed_in_batches_during_a_recording_on_ac_power() {
    let h = Harness::new();
    h.install_models();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    // Four closed minutes: less than a batch.
    mic.write(&seconds(270), 0).unwrap();
    mic.flush().unwrap();
    assert_eq!(h.run(), 0);
    assert!(
        h.script().opened.is_empty(),
        "no worker for less than a batch"
    );

    // The fifth minute closes: a worker transcribes the five, then exits.
    mic.write(&seconds(40), 270_000).unwrap();
    mic.flush().unwrap();
    h.calls.take();
    h.run();
    assert_eq!(h.script().requests.len(), 5);
    assert_eq!(h.script().opened.len(), 1);
    assert_eq!(h.script().sessions_alive, 0, "the batch's worker exited");
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

    // Another minute closes: not enough for the next batch.
    mic.write(&seconds(60), 310_000).unwrap();
    mic.flush().unwrap();
    assert_eq!(h.run(), 0);
    assert_eq!(h.script().requests.len(), 5);

    // Stop: the rest, in one more worker.
    mic.end_run(GapReason::Stop).unwrap();
    drop(mic);
    h.stop();
    assert_eq!(h.state(&id), RecordingState::Recorded);
    h.calls.take();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let script = h.script();
    assert_eq!(script.opened.len(), 2, "one batch, then the rest");
    assert_eq!(script.sessions_alive, 0);
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
            (120_000, true, false),
            (180_000, true, false),
            (240_000, true, false),
            (300_000, true, false),
            (360_000, true, true)
        ]
    );
    drop(script);
    // 300 of 370 s were done before Stop: 81%, then 97% and 100%.
    assert_eq!(h.progress(), [97, 100]);
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
    mic.write(&seconds(320), 0).unwrap();
    mic.flush().unwrap();
    h.run();
    assert_eq!(h.script().requests.len(), 5);
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
        percent: 93
    }))));
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
}

#[test]
fn a_batch_running_at_stop_goes_on_with_the_same_worker_and_shows_progress() {
    let h = Harness::new();
    h.install_models();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    mic.write(&seconds(310), 0).unwrap();
    mic.flush().unwrap();
    assert!(h.runtime.transcription_step(), "the batch's first file");
    assert_eq!(h.script().sessions_alive, 1);
    drop(mic);
    h.stop();
    h.calls.take();
    assert!(h.runtime.transcription_step());
    assert_eq!(h.state(&id), RecordingState::Transcribing);
    assert!(h.calls.take().contains(&Call::Changed(id.clone())));
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    assert_eq!(h.script().opened.len(), 1, "the same worker");
    assert_eq!(h.script().requests.len(), 6);
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
    mic.write(&seconds(330), 0).unwrap();
    mic.flush().unwrap();
    assert_eq!(h.run(), 0);
    assert!(h.script().opened.is_empty(), "no worker on battery");

    // Plugged in, a batch starts; back on battery, its worker exits.
    h.power.0.store(true, Ordering::SeqCst);
    assert!(h.runtime.transcription_step());
    assert_eq!(h.script().requests.len(), 1);
    h.power.0.store(false, Ordering::SeqCst);
    h.run();
    assert_eq!(h.script().requests.len(), 1);
    assert_eq!(h.script().sessions_alive, 0);

    drop(mic);
    h.stop();
    h.run();
    assert_eq!(
        h.state(&id),
        RecordingState::Transcribed,
        "after Stop, battery or not"
    );
    assert_eq!(h.script().requests.len(), 6, "each file once");
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
    mic.write(&seconds(330), 0).unwrap();
    mic.flush().unwrap();
    h.script().fail_request = Some(0);
    h.run();
    assert_eq!(h.state(&id), RecordingState::Recording);
    assert_eq!(h.script().opened.len(), 1);
    mic.write(&seconds(300), 330_000).unwrap();
    mic.flush().unwrap();
    assert_eq!(h.run(), 0, "not retried during the recording");
    drop(mic);
    h.stop();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let script = h.script();
    assert_eq!(script.opened.len(), 2);
    assert_eq!(script.requests.len(), 1 + 11, "from the start");
}

#[test]
fn nothing_is_transcribed_during_a_meeting_with_the_setting_off() {
    let h = Harness::new();
    h.install_models();
    h.runtime
        .set_settings(RecordingSettings {
            transcribe_during_meetings: false,
            ..Default::default()
        })
        .unwrap();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    mic.write(&seconds(610), 0).unwrap();
    mic.flush().unwrap();
    assert_eq!(h.run(), 0);
    assert!(h.script().opened.is_empty(), "no worker at all");

    // Turned on, the waiting audio goes in a batch.
    h.runtime
        .set_settings(RecordingSettings::default())
        .unwrap();
    assert!(h.runtime.transcription_step());
    assert_eq!(h.script().requests.len(), 1);

    // Turned off again mid-file: the worker is stopped at once, and that
    // file is done again after Stop.
    h.script().block = true;
    let runtime = h.runtime.clone();
    let stepping = std::thread::spawn(move || runtime.transcription_step());
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.script().requests.len() < 2 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.runtime
        .set_settings(RecordingSettings {
            transcribe_during_meetings: false,
            ..Default::default()
        })
        .unwrap();
    assert!(stepping.join().unwrap());
    assert_eq!(h.script().sessions_alive, 0);
    h.script().block = false;
    assert_eq!(h.run(), 0);
    assert_eq!(h.script().requests.len(), 2);

    drop(mic);
    h.stop();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let starts: Vec<u64> = h.script().requests.iter().map(|r| r.start_ms).collect();
    assert_eq!(
        starts,
        [
            0, 60_000, 60_000, 120_000, 180_000, 240_000, 300_000, 360_000, 420_000, 480_000,
            540_000, 600_000
        ]
    );
}

fn held(path: &std::path::Path, from_sample: u64, start_ms: u64) -> HeldAudio {
    HeldAudio {
        start_ms,
        pieces: vec![HeldPiece {
            wav_path: path.to_path_buf(),
            from_sample,
            samples: 960_000 - from_sample,
        }],
    }
}

#[test]
fn speech_held_at_the_end_of_a_batch_is_carried_into_the_next_worker() {
    let h = Harness::new();
    h.install_models();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    let dir = h.backend.target(Track::Mic).unwrap().audio_dir;
    let fifth = std::fs::canonicalize(&dir).unwrap().join("mic-0005.wav");
    let tenth = std::fs::canonicalize(&dir).unwrap().join("mic-0010.wav");
    let end_of_fifth = held(&fifth, 944_000, 299_000);
    let end_of_tenth = held(&tenth, 900_000, 597_250);
    {
        let mut script = h.script();
        script
            .held
            .insert((Track::Mic, 240_000), end_of_fifth.clone());
        script
            .held
            .insert((Track::Mic, 540_000), end_of_tenth.clone());
    }
    mic.write(&seconds(310), 0).unwrap();
    mic.flush().unwrap();
    h.run();
    assert_eq!(h.script().opened.len(), 1);
    mic.write(&seconds(300), 310_000).unwrap();
    mic.flush().unwrap();
    h.run();
    assert_eq!(h.script().opened.len(), 2, "a second batch");
    drop(mic);
    h.stop();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let script = h.script();
    assert_eq!(script.opened.len(), 3);
    let carried: Vec<(u64, Option<HeldAudio>)> = script
        .requests
        .iter()
        .map(|r| (r.start_ms, r.carry_in.clone()))
        .filter(|(_, carry)| carry.is_some())
        .collect();
    assert_eq!(
        carried,
        [(300_000, Some(end_of_fifth)), (600_000, Some(end_of_tenth))],
        "each worker's first file takes over what the last one held, and only that file"
    );
}

#[test]
fn held_audio_from_a_file_the_job_did_not_plan_fails_the_job() {
    let h = Harness::new();
    h.install_models();
    let outside = h.root.path().join("elsewhere").join("mic-0001.wav");
    let dir = h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        90,
        0,
        RecordingState::Recorded,
    );
    let first = std::fs::canonicalize(&dir).unwrap().join("mic-0001.wav");
    h.script()
        .held
        .insert((Track::Mic, 0), held(&outside, 900_000, 59_000));
    h.run();
    let failed = h.database.recording("r1").unwrap().unwrap();
    assert_eq!(failed.state, RecordingState::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("The transcription engine stopped unexpectedly.")
    );
    // Its own file is fine.
    h.script()
        .held
        .insert((Track::Mic, 0), held(&first, 900_000, 59_000));
    h.runtime.transcribe_recording("r1").unwrap();
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
}

#[test]
fn no_job_starts_under_memory_pressure_or_without_room_and_the_recording_stays_recorded() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        90,
        0,
        RecordingState::Recorded,
    );
    for pressure in [MemoryPressure::Warning, MemoryPressure::Critical] {
        *h.memory.pressure.lock().unwrap() = pressure;
        assert_eq!(h.run(), 0, "{pressure:?}");
        assert_eq!(h.state("r1"), RecordingState::Recorded);
    }
    *h.memory.pressure.lock().unwrap() = MemoryPressure::Normal;
    let needed = policy::WORKER_MEMORY_FAST_BYTES + policy::WORKER_START_MARGIN_BYTES;
    *h.memory.available.lock().unwrap() = Some(needed - 1);
    assert_eq!(h.run(), 0);
    assert_eq!(h.state("r1"), RecordingState::Recorded);
    assert!(h.script().opened.is_empty());
    assert!(h.runtime.scheduler.current().is_none(), "not even picked");
    assert_eq!(h.listed("r1").transcription_percent, None);

    *h.memory.available.lock().unwrap() = Some(needed);
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
    assert_eq!(
        h.script().budgets,
        [policy::WORKER_MEMORY_FAST_BYTES],
        "the worker is capped at the model's budget"
    );

    // A Mac that can't say how much is available still transcribes.
    *h.memory.available.lock().unwrap() = None;
    h.recorded(
        "r2",
        "2026-11-27T11:00:00Z",
        10,
        0,
        RecordingState::Recorded,
    );
    h.run();
    assert_eq!(h.state("r2"), RecordingState::Transcribed);
}

#[test]
fn memory_pressure_stops_the_batch_and_live_transcription_until_stop() {
    let h = Harness::new();
    h.install_models();
    let id = h
        .runtime
        .start(None, "Standup")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    mic.write(&seconds(330), 0).unwrap();
    mic.flush().unwrap();
    assert!(h.runtime.transcription_step());
    h.script().block = true;
    let runtime = h.runtime.clone();
    let stepping = std::thread::spawn(move || runtime.transcription_step());
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.script().requests.len() < 2 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    *h.memory.pressure.lock().unwrap() = MemoryPressure::Warning;
    assert!(stepping.join().unwrap(), "the worker was stopped mid-file");
    assert_eq!(h.script().sessions_alive, 0);
    h.script().block = false;
    assert_eq!(h.state(&id), RecordingState::Recording);

    // Pressure falls, and more audio waits: still nothing until Stop.
    *h.memory.pressure.lock().unwrap() = MemoryPressure::Normal;
    mic.write(&seconds(300), 330_000).unwrap();
    mic.flush().unwrap();
    assert_eq!(h.run(), 0);
    assert_eq!(h.script().opened.len(), 1);

    drop(mic);
    h.stop();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let script = h.script();
    assert_eq!(script.opened.len(), 2);
    let starts: Vec<u64> = script.requests.iter().map(|r| r.start_ms).collect();
    assert_eq!(
        &starts[..3],
        [0, 60_000, 60_000],
        "the stopped file is redone"
    );
    assert_eq!(starts.len(), 2 + 10);
}

#[test]
fn after_stop_only_critical_pressure_stops_a_job_which_goes_on_once_pressure_is_normal() {
    let h = Harness::new();
    h.install_models();
    h.recorded(
        "r1",
        "2026-11-27T10:00:00Z",
        150,
        0,
        RecordingState::Recorded,
    );
    assert!(h.runtime.transcription_step());
    assert_eq!(h.listed("r1").transcription_percent, Some(40));
    h.script().block = true;
    let runtime = h.runtime.clone();
    let stepping = std::thread::spawn(move || runtime.transcription_step());
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.script().requests.len() < 2 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    *h.memory.pressure.lock().unwrap() = MemoryPressure::Warning;
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !stepping.is_finished(),
        "warning doesn't stop a job after Stop"
    );
    assert_eq!(h.script().sessions_alive, 1);
    h.calls.take();
    *h.memory.pressure.lock().unwrap() = MemoryPressure::Critical;
    assert!(stepping.join().unwrap());
    assert_eq!(h.script().sessions_alive, 0);
    assert_eq!(h.state("r1"), RecordingState::Recorded, "waiting again");
    assert_eq!(h.listed("r1").transcription_percent, None);
    assert!(h.runtime.scheduler.tray().is_none());
    let calls = h.calls.take();
    assert!(calls.contains(&Call::Changed("r1".into())));
    assert_eq!(calls.last(), Some(&Call::Tray(None)));
    h.script().block = false;
    assert_eq!(h.run(), 0);

    *h.memory.pressure.lock().unwrap() = MemoryPressure::Normal;
    h.run();
    assert_eq!(h.state("r1"), RecordingState::Transcribed);
    let script = h.script();
    assert_eq!(script.opened.len(), 2);
    let starts: Vec<u64> = script.requests.iter().map(|r| r.start_ms).collect();
    assert_eq!(
        starts,
        [0, 60_000, 60_000, 120_000],
        "on from the last file done"
    );
    drop(script);
    // Back to 40% when it goes on, then on to the end.
    assert_eq!(h.progress(), [80, 100]);
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

/// The worker's own carry-over (its streams over the recording's real
/// files) with the scripted speech engine, so batches can be compared with
/// one worker doing everything.
struct StreamTranscriber;

struct StreamSession {
    streams: crate::recording::transcribe::stream::Streams,
    engine: crate::recording::transcribe::stream::tests::FakeEngine,
}

impl Transcriber for StreamTranscriber {
    fn open(
        &self,
        _: &JobConfig,
        _: Arc<dyn StopCheck>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError> {
        Ok(Box::new(StreamSession {
            streams: Default::default(),
            engine: crate::recording::transcribe::stream::tests::FakeEngine::new(),
        }))
    }
}

impl TranscriptionSession for StreamSession {
    fn transcribe(
        &mut self,
        request: &SegmentRequest,
        _audio_ms: u64,
    ) -> Result<Transcribed, JobError> {
        use crate::recording::transcribe::stream::read_segment;
        if let Some(held) = &request.carry_in {
            self.streams
                .restore(request.track, held, read_segment)
                .map_err(JobError::Failed)?;
        }
        let samples = read_segment(&request.wav_path).map_err(JobError::Failed)?;
        let lines = self
            .streams
            .feed(&mut self.engine, request, samples)
            .map_err(JobError::Failed)?;
        Ok(Transcribed {
            lines,
            held: self.streams.held(request.track),
            detected_language: None,
        })
    }
}

/// `seconds` of quiet with loud "speech" over each `(from, to)` range, as
/// the scripted engine hears it.
fn speech(seconds: f64, ranges: &[(f64, f64)]) -> Vec<i16> {
    let mut samples = vec![1i16; (seconds * 16_000.0) as usize];
    for &(from, to) in ranges {
        samples[(from * 16_000.0) as usize..(to * 16_000.0) as usize].fill(20_000);
    }
    samples
}

#[test]
fn batches_give_the_transcript_and_percentages_of_one_worker() {
    let h = Harness::with_transcriber(Arc::new(StreamTranscriber));
    h.install_models();
    // Every sentence a different length, so none repeats another. Two
    // cross a batch's end (300 s and 600 s), others a minute's.
    let audio = speech(
        680.0,
        &[
            (10.0, 12.5),
            (58.6, 61.2),
            (119.0, 123.5),
            (297.5, 303.25),
            (400.0, 401.0),
            (597.0, 604.5),
            (639.0, 641.75),
        ],
    );

    // One worker, after the fact.
    let dir = h.root.path().join("whole");
    h.database
        .create_recording(&NewRecording {
            id: "whole",
            event_ref: None,
            title: "Weekly Sync",
            attendees: &[],
            started_at: "2026-11-27T10:00:00Z",
            audio_dir: &dir.to_string_lossy(),
            tracks: &[(Track::Mic, TrackState::Active)],
        })
        .unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let mut writer = SegmentWriter::open(&dir, Track::Mic).unwrap();
    writer.write(&audio, 0).unwrap();
    drop(writer);
    h.database
        .finish_recording(
            "whole",
            "2026-11-27T10:00:00Z",
            680_000,
            RecordingState::Recorded,
            None,
        )
        .unwrap();
    h.run();
    assert_eq!(h.state("whole"), RecordingState::Transcribed);
    let whole = h.database.transcript_rows("whole").unwrap();

    // The same audio, in batches while it is recorded.
    let id = h
        .runtime
        .start(None, "Weekly Sync")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    for (from, to) in [(0, 310), (310, 610), (610, 680)] {
        mic.write(&audio[from * 16_000..to * 16_000], from as u64 * 1000)
            .unwrap();
        mic.flush().unwrap();
        h.run();
    }
    mic.end_run(GapReason::Stop).unwrap();
    drop(mic);
    h.stop();
    h.calls.take();
    h.run();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let batched = h.database.transcript_rows(&id).unwrap();

    let summary = |rows: &[TranscriptRow]| -> Vec<(i64, i64, String)> {
        rows.iter()
            .map(|r| (r.start_ms, r.end_ms, r.text.clone()))
            .collect()
    };
    assert_eq!(summary(&batched), summary(&whole));
    assert_eq!(batched.len(), 7, "{:?}", summary(&batched));
    assert!(
        summary(&batched).contains(&(297_500, 303_250, "speech 0.61035156 for 5750 ms".into())),
        "the sentence across the first batch's end is whole"
    );
    // 600 of 680 s were done before Stop: 88%, then 97% and 100%.
    let calls = h.calls.take();
    assert!(calls.contains(&Call::Tray(Some(TrayTranscription {
        title: "Weekly Sync".into(),
        percent: 88
    }))));
    let progress: Vec<u8> = calls
        .iter()
        .filter_map(|c| match c {
            Call::Progress(_, percent) => Some(*percent),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [97, 100]);
}

/// Answers each minute with about 150 words, as a busy meeting would, and
/// counts the bytes of transcript it hands back.
struct TextTranscriber(Arc<std::sync::atomic::AtomicU64>);

struct TextSession(Arc<std::sync::atomic::AtomicU64>);

impl Transcriber for TextTranscriber {
    fn open(
        &self,
        _: &JobConfig,
        _: Arc<dyn StopCheck>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError> {
        Ok(Box::new(TextSession(self.0.clone())))
    }
}

impl TranscriptionSession for TextSession {
    fn transcribe(
        &mut self,
        request: &SegmentRequest,
        audio_ms: u64,
    ) -> Result<Transcribed, JobError> {
        let lines: Vec<TranscriptLine> = (0..5u64)
            .map(|sentence| TranscriptLine {
                track: request.track,
                start_ms: request.start_ms + sentence * audio_ms / 5,
                end_ms: request.start_ms + (sentence + 1) * audio_ms / 5,
                text: format!(
                    "At minute {} we went over item {sentence} on the {} list, agreed who \
                     follows up with the vendor, and moved the review to next week.",
                    request.start_ms / 60_000,
                    request.track.as_str()
                ),
                rms: None,
                speaker_label: None,
            })
            .collect();
        let bytes: usize = lines
            .iter()
            .map(|line| std::mem::size_of::<TranscriptLine>() + line.text.capacity())
            .sum();
        self.0.fetch_add(bytes as u64, Ordering::SeqCst);
        Ok(Transcribed {
            lines,
            held: None,
            detected_language: None,
        })
    }
}

#[test]
fn a_two_hour_job_holds_its_transcript_and_nothing_else_that_grows() {
    use crate::recording::test_support::heap;
    use std::io::Write;
    let transcript = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let h = Harness::with_transcriber(Arc::new(TextTranscriber(transcript.clone())));
    h.install_models();
    let id = h
        .runtime
        .start(None, "All Hands")
        .unwrap()
        .recording_id
        .unwrap();
    let dir = h.backend.target(Track::Mic).unwrap().audio_dir;
    std::fs::create_dir_all(&dir).unwrap();
    // A minute of audio on each track: its file (sparse; the fake reads
    // nothing) and its manifest line, as the segment writer leaves them.
    let add_minute = |minute: u32| {
        for track in Track::ALL {
            let number = minute + 1;
            let name = segments::segment_file_name(track, number);
            std::fs::File::create(dir.join(&name))
                .unwrap()
                .set_len(44 + 60 * 16_000 * 2)
                .unwrap();
            let mut manifest = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(segments::manifest_path(&dir, track))
                .unwrap();
            writeln!(
                manifest,
                "{}",
                serde_json::json!({ "segment": number, "file": name, "startMs": minute as u64 * 60_000 })
            )
            .unwrap();
        }
    };
    let held_beyond_transcript = |transcript: &std::sync::atomic::AtomicU64| {
        let bytes = transcript.load(Ordering::SeqCst) as i64;
        (heap::live() - bytes, heap::peak() - bytes)
    };

    heap::start();
    let mut early = (i64::MIN, i64::MIN);
    let mut late = (i64::MIN, i64::MIN);
    for minute in 0..121 {
        heap::reset_peak();
        add_minute(minute);
        h.run();
        h.calls.take();
        let (live, peak) = held_beyond_transcript(&transcript);
        if (10..20).contains(&minute) {
            early = (early.0.max(live), early.1.max(peak));
        }
        if minute >= 110 {
            late = (late.0.max(live), late.1.max(peak));
        }
    }
    let transcribed = transcript.load(Ordering::SeqCst) as i64;
    assert!(
        transcribed > 2 * 115 * 5 * 100,
        "about two hours of both tracks: {transcribed}"
    );

    // Stop: the rest, the merge, and the save.
    let before_finish = heap::live();
    heap::reset_peak();
    h.stop();
    h.run();
    h.calls.take();
    let finish_peak = heap::peak() - before_finish;
    let after_finish = heap::live() - before_finish;
    heap::stop();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    assert_eq!(h.database.transcript_rows(&id).unwrap().len(), 2 * 121 * 5);
    // Beyond the transcript, a step near two hours holds and peaks at what
    // one near ten minutes does (the peak is mostly checking the model's
    // hash before each worker).
    const SLACK: i64 = 16 * 1024;
    assert!(
        late.0 <= early.0 + SLACK && late.1 <= early.1 + SLACK,
        "beyond the transcript: {late:?} near two hours, {early:?} near ten minutes"
    );
    // The merge and the save work on the transcript (once, then as rows),
    // and nothing of the job stays behind.
    assert!(
        finish_peak <= 2 * transcribed + early.1,
        "finishing peaked {finish_peak} bytes above a {transcribed}-byte transcript"
    );
    assert!(
        after_finish <= SLACK,
        "{after_finish} bytes left after the job"
    );
}

#[test]
fn speech_analyzer_transcribes_without_whisper_model_when_available() {
    use crate::recording::settings::TranscriptionEngineChoice;
    use crate::recording::transcribe::WorkerEngine;

    let h = Harness::with_speech_analyzer();
    h.runtime
        .set_settings(RecordingSettings {
            transcription_engine: TranscriptionEngineChoice::Auto,
            ..Default::default()
        })
        .unwrap();
    h.recorded(
        "r-sa",
        "2026-11-27T10:00:00Z",
        60,
        60,
        RecordingState::Recorded,
    );
    h.run();
    assert_eq!(h.state("r-sa"), RecordingState::Transcribed);
    let opened = h.script().opened.clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].engine, WorkerEngine::SpeechAnalyzer);
}

#[test]
fn explicit_whisper_engine_requires_whisper_model_even_when_speech_analyzer_available() {
    use crate::recording::settings::TranscriptionEngineChoice;
    use crate::recording::transcribe::WorkerEngine;

    let h = Harness::with_speech_analyzer();
    h.runtime
        .set_settings(RecordingSettings {
            transcription_engine: TranscriptionEngineChoice::Whisper,
            ..Default::default()
        })
        .unwrap();
    h.recorded(
        "r-wh",
        "2026-11-27T10:00:00Z",
        60,
        60,
        RecordingState::Recorded,
    );
    h.run();
    assert_eq!(h.state("r-wh"), RecordingState::Recorded);
    assert!(h.script().opened.is_empty());

    h.install_models();
    h.run();
    assert_eq!(h.state("r-wh"), RecordingState::Transcribed);
    let opened = h.script().opened.clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].engine, WorkerEngine::Whisper);
}

#[test]
fn initial_prompt_and_detected_language_propagate_across_batches() {
    let h = Harness::new();
    h.install_models();
    h.install_accurate();
    h.runtime
        .set_settings(RecordingSettings {
            transcription_model: TranscriptionModelChoice::Accurate,
            ..Default::default()
        })
        .unwrap();
    h.script().detected_language = Some("es".into());
    let id = h
        .runtime
        .start_meeting(None, "Q4 Roadmap Sync", &["Alice".into(), "Bob".into()])
        .unwrap()
        .recording_id
        .unwrap();

    let mut mic = h.live_mic();
    let batch_ms = policy::LIVE_BATCH.as_millis() as u64;
    let chunk_s = (batch_ms / 1000) + 60;
    mic.write(&seconds(chunk_s), 0).unwrap();
    mic.flush().unwrap();
    h.run();

    mic.end_run(GapReason::Stop).unwrap();
    drop(mic);
    h.stop();
    h.run();

    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let opened = h.script().opened.clone();
    assert!(opened.len() >= 2, "expected multiple batches: {opened:?}");
    assert_eq!(
        opened[0].initial_prompt.as_deref(),
        Some("Q4, Roadmap, Sync, Alice, Bob")
    );
    assert_eq!(opened[0].language, None);
    assert_eq!(opened[1].initial_prompt, opened[0].initial_prompt);
    assert_eq!(opened[1].language.as_deref(), Some("es"));
}

#[test]
fn headphones_skips_echo_drop_and_post_transcription_transcodes_to_m4a() {
    use crate::db::recordings::TrackPatch;

    let h = Harness::new();
    h.install_models();
    let dir = h.recorded(
        "r-hp",
        "2026-11-27T10:00:00Z",
        20,
        20,
        RecordingState::Recorded,
    );
    h.database
        .update_recording_track(
            "r-hp",
            Track::System,
            &TrackPatch {
                is_headphones: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    {
        let mut script = h.script();
        script.texts.insert(
            (Track::Mic, 0),
            "Thanks everyone for confirming Thursday tomorrow.".into(),
        );
        script.texts.insert(
            (Track::System, 5_000),
            "Thanks everyone for confirming Thursday tomorrow.".into(),
        );
    }
    h.run();
    assert_eq!(h.state("r-hp"), RecordingState::Transcribed);
    let transcript = h.runtime.recording_transcript("r-hp").unwrap();
    assert_eq!(
        transcript.lines.len(),
        2,
        "headphones should exempt overlapping matching mic line from echo removal"
    );

    #[cfg(target_os = "macos")]
    {
        assert!(dir.join("mic-0001.m4a").is_file());
        assert!(!dir.join("mic-0001.wav").exists());
        h.database
            .finish_recording(
                "r-hp",
                "2026-11-27T10:00:00Z",
                20_000,
                RecordingState::Failed,
                Some("retry test"),
            )
            .unwrap();
        h.runtime.transcribe_recording("r-hp").unwrap();
        h.run();
        assert_eq!(h.state("r-hp"), RecordingState::Transcribed);
    }
    let _ = dir;
}

/// The real worker over two hours of speech-like audio, in batches as
/// during a recording, sampling the worker's physical footprint.
///
/// Needs the pinned models in `THREESTRANDS_SMOKE_MODELS` (as for the
/// worker smoke test) and macOS `say`. The worker is this build's app
/// binary, or `THREESTRANDS_WORKER_BIN` (a release build gives realistic
/// timings). `THREESTRANDS_MEASURE_MODEL=accurate` and
/// `THREESTRANDS_MEASURE_MINUTES=20` measure the Accurate model on a
/// shorter recording. The model's budget is lifted so the peak can be
/// measured rather than cut off.
///
/// ```text
/// THREESTRANDS_SMOKE_MODELS=/path/to/models \
///   cargo test --lib real_worker_memory_stays_flat -- --ignored --nocapture
/// ```
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn real_worker_memory_stays_flat_over_a_two_hour_recording() {
    use crate::recording::transcribe::worker::{
        footprint_bytes, WorkerCommand, WorkerLimits, WorkerTranscriber,
    };

    /// The real transcriber with the memory budget lifted.
    struct Uncapped(WorkerTranscriber);
    impl Transcriber for Uncapped {
        fn open(
            &self,
            config: &JobConfig,
            stop: Arc<dyn StopCheck>,
        ) -> Result<Box<dyn TranscriptionSession>, JobError> {
            let config = JobConfig {
                memory_bytes: u64::MAX,
                ..config.clone()
            };
            self.0.open(&config, stop)
        }
    }

    let models = PathBuf::from(std::env::var("THREESTRANDS_SMOKE_MODELS").unwrap());
    let accurate = std::env::var("THREESTRANDS_MEASURE_MODEL").as_deref() == Ok("accurate");
    let minutes: usize = std::env::var("THREESTRANDS_MEASURE_MINUTES")
        .map_or(120, |minutes| minutes.parse().unwrap());
    let program = std::env::var("THREESTRANDS_WORKER_BIN").map_or_else(
        |_| {
            let exe = std::env::current_exe().unwrap();
            exe.parent().unwrap().parent().unwrap().join("threestrands")
        },
        PathBuf::from,
    );
    assert!(program.exists(), "build the app binary first: {program:?}");
    let transcriber = Uncapped(WorkerTranscriber {
        command: WorkerCommand {
            program,
            args: vec![crate::recording::transcribe::worker::WORKER_ARG.into()],
            env: vec![(
                crate::recording::transcribe::worker::WORKER_ENV.into(),
                "1".into(),
            )],
        },
        limits: WorkerLimits::default(),
    });
    let h = Harness::with(
        Pins::pinned(),
        local_urls(1),
        Arc::new(transcriber),
        FakeTranscriber::default(),
    );
    std::fs::create_dir_all(&h.models_dir).unwrap();
    let model = if accurate {
        TranscriptionModelChoice::Accurate
    } else {
        TranscriptionModelChoice::Fast
    };
    for file in [model.model().file().file_name, policy::VAD_MODEL.file_name] {
        std::fs::copy(models.join(file), h.models_dir.join(file)).unwrap();
    }
    h.runtime
        .set_settings(RecordingSettings {
            transcription_model: model,
            ..Default::default()
        })
        .unwrap();

    // Sentences read by `say`, in 16 kHz mono 16-bit WAV.
    let voices = TempDir::new();
    let sentences = [
        "Good morning everyone, let's get started with the weekly planning meeting.",
        "The launch moves to March because the vendor needs two more weeks for the security review.",
        "I will send the revised budget to finance by Thursday afternoon.",
        "Can someone take the action item to follow up with the design team about the onboarding flow?",
        "Our customer support queue dropped by twenty percent after the last release.",
        "We still have an open question about whether the mobile app ships in the same release.",
        "Priya agreed to own the migration plan and will share a draft on Monday.",
        "The infrastructure costs went up slightly, mostly because of the new search cluster.",
        "Let's revisit the hiring plan next week once we have the updated headcount numbers.",
        "Thanks, that covers everything on the agenda for today.",
        "Before we move on, does anyone have concerns about the testing schedule?",
        "The analytics dashboard should be ready for the leadership review at the end of the month.",
    ];
    let clips: Vec<Vec<i16>> = sentences
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let path = voices.path().join(format!("{index}.wav"));
            let status = std::process::Command::new("/usr/bin/say")
                .args(["--file-format=WAVE", "--data-format=LEI16@16000", "-o"])
                .arg(&path)
                .arg(text)
                .status()
                .unwrap();
            assert!(status.success());
            let bytes = std::fs::read(&path).unwrap();
            let data = bytes
                .windows(4)
                .position(|window| window == b"data")
                .unwrap();
            bytes[data + 8..]
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect()
        })
        .collect();
    // Two hours of them, with pauses and a little noise.
    let mut seed = 0x9e37_79b9_u32;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let total = minutes * 60 * 16_000;
    let mut audio: Vec<i16> = Vec::with_capacity(total + 16_000 * 30);
    while audio.len() < total {
        let clip = &clips[random() as usize % clips.len()];
        audio.extend_from_slice(clip);
        let pause = 16_000 * 3 / 10 + random() as usize % (16_000 * 22 / 10);
        audio.extend(std::iter::repeat_n(0i16, pause));
    }
    audio.truncate(total);
    for sample in &mut audio {
        *sample = sample.saturating_add((random() % 301) as i16 - 150);
    }

    // Sample the worker every second; the curve uses every tenth sample.
    let sampling = Arc::new(AtomicBool::new(true));
    let samples = Arc::new(Mutex::new(Vec::<u64>::new()));
    let sampler = {
        let sampling = sampling.clone();
        let samples = samples.clone();
        std::thread::spawn(move || {
            while sampling.load(Ordering::SeqCst) {
                let mut pids = [0 as libc::pid_t; 64];
                // SAFETY: the buffer holds 64 pids and its size is given.
                let count = unsafe {
                    libc::proc_listchildpids(
                        std::process::id() as libc::pid_t,
                        pids.as_mut_ptr().cast(),
                        std::mem::size_of_val(&pids) as libc::c_int,
                    )
                };
                let bytes: u64 = pids[..count.max(0) as usize]
                    .iter()
                    .filter_map(|&pid| footprint_bytes(pid as u32))
                    .sum();
                samples.lock().unwrap().push(bytes);
                std::thread::sleep(Duration::from_secs(1));
            }
        })
    };

    let started = Instant::now();
    let id = h
        .runtime
        .start(None, "Two Hour Meeting")
        .unwrap()
        .recording_id
        .unwrap();
    let mut mic = h.live_mic();
    let step = 5 * 60 * 16_000;
    for (index, chunk) in audio.chunks(step).enumerate() {
        mic.write(chunk, (index * step / 16) as u64).unwrap();
        mic.flush().unwrap();
        while h.runtime.transcription_step() {}
        eprintln!(
            "{:>4} min recorded, {:>6.0} s",
            (index + 1) * 5,
            started.elapsed().as_secs_f64()
        );
    }
    mic.end_run(GapReason::Stop).unwrap();
    drop(mic);
    h.stop();
    while h.runtime.transcription_step() {}
    let wall = started.elapsed();
    sampling.store(false, Ordering::SeqCst);
    sampler.join().unwrap();
    assert_eq!(h.state(&id), RecordingState::Transcribed);
    let transcript = h.runtime.recording_transcript(&id).unwrap();

    let all = samples.lock().unwrap().clone();
    let curve: Vec<u64> = all.iter().step_by(10).copied().collect();
    let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    let quarter = curve.len().div_ceil(4);
    let mut maxima = Vec::new();
    for (index, part) in curve.chunks(quarter).enumerate() {
        let mut running: Vec<u64> = part.iter().copied().filter(|&b| b > 0).collect();
        running.sort_unstable();
        if running.is_empty() {
            eprintln!("quarter {}: no worker sampled", index + 1);
            maxima.push(0);
            continue;
        }
        eprintln!(
            "quarter {}: {} samples with a worker of {}: min {:.0} MiB, median {:.0} MiB, max {:.0} MiB",
            index + 1,
            running.len(),
            part.len(),
            mib(running[0]),
            mib(running[running.len() / 2]),
            mib(*running.last().unwrap())
        );
        maxima.push(*running.last().unwrap());
    }
    let peak = all.iter().copied().max().unwrap_or(0);
    eprintln!(
        "{} model, {minutes} min of audio: wall {:.0} s, peak {:.0} MiB, {} lines; first: {:?}",
        if accurate { "Accurate" } else { "Fast" },
        wall.as_secs_f64(),
        mib(peak),
        transcript.lines.len(),
        transcript.lines.first().map(|line| &line.text)
    );
    eprintln!(
        "10 s samples (MiB): {:?}",
        curve
            .iter()
            .map(|&b| mib(b).round() as u64)
            .collect::<Vec<_>>()
    );
    assert!(!transcript.lines.is_empty());
    // No upward trend: the last quarter's worst is the first quarter's,
    // within a tolerance for this measurement.
    const TOLERANCE: u64 = 64 * 1024 * 1024;
    assert!(
        maxima[maxima.len() - 1] <= maxima[0] + TOLERANCE,
        "{:?}",
        maxima.iter().map(|&b| mib(b)).collect::<Vec<_>>()
    );
}
