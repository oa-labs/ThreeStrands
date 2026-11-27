//! Which recording is transcribed when, one job at a time, and the job
//! itself from its first segment file to the saved transcript.
//!
//! A **job** is one recording and one model, kept from start to finish: a
//! model chosen or deleted later doesn't affect it. Its **workers**
//! ([`super::Transcriber`]) come and go: each loads the model, transcribes
//! some segment files, and exits, so the model is in memory only while
//! there is work for it. Jobs are picked, whenever the chosen model is
//! ready:
//!
//! 1. the oldest `recorded` or `interrupted` recording whose audio exists;
//!    one worker does all of it;
//! 2. otherwise the recording in progress, in batches, while **Transcribe
//!    During Meetings** is on and the Mac is on AC power: once
//!    [`policy::LIVE_BATCH`] of finished audio is waiting, a worker starts,
//!    transcribes what is waiting, and exits. Until Stop the row stays
//!    `recording`; nothing of this is visible.
//!
//! **Memory.** A worker starts only while memory pressure is normal and the
//! Mac has the model's budget plus a margin available
//! ([`policy::memory_allows_start`]); until then the job waits, and its
//! recording stays `recorded`. During the recording, pressure at warning or
//! above stops the batch's worker, and nothing more is transcribed until
//! Stop. After Stop, critical pressure stops the worker and the recording
//! goes back to `recorded` until pressure is normal again. Each time, what
//! the worker finished stands and the next worker goes on from there.
//!
//! **No sentence is cut between workers.** Speech at the end of a segment
//! file is held back for the next file of its run ([`super::stream`]).
//! Every answer says where the audio a worker holds starts
//! ([`super::protocol::HeldAudio`]); the next worker's first file of that
//! track carries it in, so the transcript and the percentages are the same
//! as one worker doing the whole job.
//!
//! After Stop the job's row goes `recorded` → `transcribing` (with a
//! percentage of audio done, what ran during the recording included) →
//! `transcribed`, or `failed` with a plain reason. A job that fails during
//! the recording is dropped and the recording starts over after Stop.
//!
//! [`RecordingRuntime::transcription_step`] does one unit of work (opens a
//! job or a worker, transcribes one segment file, or finishes) without
//! holding a lock during the slow part, so deletes, listings, and the tray
//! read the job's state at any time. The app runs it on its own thread;
//! tests call it directly.

use super::memory::{MemoryPressure, MemoryProbe};
use super::merge;
use super::models::ModelStore;
use super::plan::{self, PlannedSegment};
use super::power::PowerSource;
use super::protocol::{HeldAudio, TranscriptLine};
use super::{JobConfig, JobError, StopCheck, Transcriber, TranscriptionSession, ENGINE};
use crate::db::recordings::{RecordingState, TranscriptRow};
use crate::recording::capture::Track;
use crate::recording::policy;
use crate::recording::runtime::RecordingRuntime;
use crate::recording::settings::TranscriptionModelChoice;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The job in progress, as the rest of the app sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentJob {
    pub recording_id: String,
    pub title: String,
    pub model: TranscriptionModelChoice,
    /// Whole percent of audio done, once the recording has stopped and the
    /// job is running; `None` while it is still being recorded or waits.
    pub percent: Option<u8>,
}

/// What the tray shows while a job runs after Stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayTranscription {
    pub title: String,
    pub percent: u8,
}

/// What a job's worker checks while the parent waits on it.
struct JobControl {
    cancel: AtomicBool,
    /// The job's recording is still being recorded.
    live: AtomicBool,
    /// Transcribe During Meetings, shared with the scheduler.
    live_allowed: Arc<AtomicBool>,
    memory: Arc<dyn MemoryProbe>,
}

impl StopCheck for JobControl {
    fn stop_reason(&self) -> Option<JobError> {
        if self.cancel.load(Ordering::SeqCst) {
            return Some(JobError::Cancelled);
        }
        let pressure = self.memory.pressure();
        let live = self.live.load(Ordering::SeqCst);
        let stop = pressure == MemoryPressure::Critical
            || (live
                && (pressure != MemoryPressure::Normal
                    || !self.live_allowed.load(Ordering::SeqCst)));
        stop.then_some(JobError::Interrupted)
    }
}

struct Job {
    recording_id: String,
    audio_dir: PathBuf,
    model: TranscriptionModelChoice,
    model_name: String,
    /// The worker settings with the model just checked, for the next
    /// worker; checked again before each later one.
    config: Option<JobConfig>,
    control: Arc<JobControl>,
    /// The worker in use; none between batches.
    session: Option<Box<dyn TranscriptionSession>>,
    /// Per track, the number of the last segment file transcribed. A
    /// track's files are transcribed in order, so this is all the job
    /// needs to remember of them, however long the recording.
    done_through: HashMap<Track, u32>,
    lines: Vec<TranscriptLine>,
    done_ms: u64,
    /// The row has been moved to `transcribing`.
    announced: bool,
    /// The row's state while the job waits: `recorded`, or `interrupted`
    /// for a recording a crash ended.
    waiting_state: RecordingState,
    /// Per track, the audio the last worker held back at the end of the
    /// track's last file.
    held: HashMap<Track, HeldAudio>,
    /// Tracks whose held audio the current worker hasn't been given yet.
    carry_pending: HashSet<Track>,
}

impl Job {
    fn is_done(&self, segment: &PlannedSegment) -> bool {
        self.done_through
            .get(&segment.request.track)
            .is_some_and(|&number| segment.number <= number)
    }

    /// Where the job stands: the next closed file to transcribe, and how
    /// much audio waits.
    fn standing(&self, finished: bool) -> plan::Standing {
        plan::standing(&self.audio_dir, finished, |segment| self.is_done(segment))
    }
}

#[derive(Default)]
struct State {
    wake: bool,
    current: Option<(CurrentJob, Arc<JobControl>)>,
    /// The job between steps; `None` while a step has it.
    parked: Option<Job>,
    /// Recordings whose job failed while they were being recorded; they
    /// start over after Stop rather than during the recording again.
    live_failed: HashSet<String>,
    /// Recordings whose transcription during the recording was stopped by
    /// memory pressure; the rest waits for Stop.
    live_stopped: HashSet<String>,
}

pub struct Scheduler {
    transcriber: Arc<dyn Transcriber>,
    power: Arc<dyn PowerSource>,
    memory: Arc<dyn MemoryProbe>,
    pub(crate) models: Arc<ModelStore>,
    /// Transcribe During Meetings.
    live_allowed: Arc<AtomicBool>,
    state: Mutex<State>,
    changed: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const IN_USE_MESSAGE: &str = "A transcription is using this model. Try again when it finishes.";
const NO_AUDIO_MESSAGE: &str = "No audio was recorded.";

impl Scheduler {
    pub fn new(
        transcriber: Arc<dyn Transcriber>,
        power: Arc<dyn PowerSource>,
        memory: Arc<dyn MemoryProbe>,
        models: Arc<ModelStore>,
    ) -> Self {
        Self {
            transcriber,
            power,
            memory,
            models,
            live_allowed: Arc::new(AtomicBool::new(true)),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }
    }

    /// Asks the transcription thread to look for work now.
    pub fn wake(&self) {
        lock(&self.state).wake = true;
        self.changed.notify_all();
    }

    /// Waits until [`Self::wake`] or `timeout`.
    pub fn wait(&self, timeout: Duration) {
        let mut state = lock(&self.state);
        if !state.wake {
            state = self
                .changed
                .wait_timeout_while(state, timeout, |s| !s.wake)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        state.wake = false;
    }

    /// Follows the **Transcribe During Meetings** setting. Turned off, a
    /// batch in progress stops at once.
    pub fn set_transcribe_during_meetings(&self, on: bool) {
        self.live_allowed.store(on, Ordering::SeqCst);
    }

    /// The recording `recording_id` has stopped: its job, if running, is no
    /// longer live work, so only critical memory pressure stops it now.
    pub fn recording_stopped(&self, recording_id: &str) {
        if let Some((job, control)) = lock(&self.state).current.as_ref() {
            if job.recording_id == recording_id {
                control.live.store(false, Ordering::SeqCst);
            }
        }
    }

    pub fn current(&self) -> Option<CurrentJob> {
        lock(&self.state)
            .current
            .as_ref()
            .map(|(job, _)| job.clone())
    }

    pub fn tray(&self) -> Option<TrayTranscription> {
        self.current().and_then(|job| {
            job.percent.map(|percent| TrayTranscription {
                title: job.title,
                percent,
            })
        })
    }

    /// Refuses while a job uses `model`.
    pub fn check_model_unused(&self, model: TranscriptionModelChoice) -> Result<(), String> {
        match self.current() {
            Some(job) if job.model == model => Err(IN_USE_MESSAGE.into()),
            _ => Ok(()),
        }
    }

    /// Stops the job for `recording_id`, if there is one, and waits until it
    /// has let go of the recording.
    pub fn cancel(&self, recording_id: &str) {
        let parked = {
            let mut state = lock(&self.state);
            let Some((job, control)) = state.current.as_ref() else {
                return;
            };
            if job.recording_id != recording_id {
                return;
            }
            control.cancel.store(true, Ordering::SeqCst);
            match state.parked.take() {
                Some(parked) => {
                    state.current = None;
                    Some(parked)
                }
                None => {
                    // A step has the job; it notices the flag and ends it.
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while state
                        .current
                        .as_ref()
                        .is_some_and(|(job, _)| job.recording_id == recording_id)
                    {
                        let left = deadline.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break;
                        }
                        state = self
                            .changed
                            .wait_timeout(state, left)
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .0;
                    }
                    None
                }
            }
        };
        // Dropping the job kills its worker; not under the lock.
        drop(parked);
    }

    /// Stops whatever job is running (the app is quitting).
    pub fn cancel_all(&self) {
        if let Some(job) = self.current() {
            self.cancel(&job.recording_id);
        }
    }

    fn end_job(&self) {
        let mut state = lock(&self.state);
        state.current = None;
        state.parked = None;
        drop(state);
        self.changed.notify_all();
    }

    fn park(&self, job: Job) {
        let mut state = lock(&self.state);
        let cancelled = state.current.as_ref().is_none_or(|(current, control)| {
            current.recording_id != job.recording_id || control.cancel.load(Ordering::SeqCst)
        });
        if cancelled {
            state.current = None;
            drop(state);
            self.changed.notify_all();
            drop(job);
            return;
        }
        state.parked = Some(job);
    }

    fn is_cancelled(&self, recording_id: &str) -> bool {
        lock(&self.state)
            .current
            .as_ref()
            .is_none_or(|(job, control)| {
                job.recording_id != recording_id || control.cancel.load(Ordering::SeqCst)
            })
    }

    fn set_percent(&self, percent: Option<u8>) {
        if let Some((job, _)) = lock(&self.state).current.as_mut() {
            job.percent = percent;
        }
    }

    /// Whether memory pressure is normal and the Mac has room for a worker
    /// running `model`. A Mac that can't say how much is available is
    /// trusted; the model's budget still stops a runaway worker.
    fn memory_allows(&self, model: TranscriptionModelChoice) -> bool {
        self.memory.pressure() == MemoryPressure::Normal
            && self
                .memory
                .available_bytes()
                .is_none_or(|available| policy::memory_allows_start(model.model(), available))
    }

    /// Whether the recording in progress may be transcribed now, memory
    /// aside.
    fn live_allowed(&self, recording_id: &str) -> bool {
        let blocked = {
            let state = lock(&self.state);
            state.live_failed.contains(recording_id) || state.live_stopped.contains(recording_id)
        };
        !blocked && self.live_allowed.load(Ordering::SeqCst) && self.power.on_ac_power()
    }

    /// No more transcription during this recording: memory pressure rose.
    fn stop_live(&self, recording_id: &str) {
        log::info!(target: "recording", "memory pressure: transcription waits for Stop");
        lock(&self.state)
            .live_stopped
            .insert(recording_id.to_string());
    }
}

fn percent(done_ms: u64, total_ms: u64) -> u8 {
    if total_ms == 0 {
        return 100;
    }
    (done_ms.min(total_ms) * 100 / total_ms) as u8
}

/// The language whisper is told: English for the English-only Fast model,
/// detected for Accurate.
fn language(model: TranscriptionModelChoice) -> Option<String> {
    match model {
        TranscriptionModelChoice::Fast => Some("en".into()),
        TranscriptionModelChoice::Accurate => None,
    }
}

/// Whether every file a worker says it holds audio from is one of the
/// track's own segment files: the parent hands back only paths it would
/// plan itself.
fn held_is_planned(held: &Option<HeldAudio>, track: Track, audio_dir: &Path) -> bool {
    held.as_ref().is_none_or(|held| {
        !held.pieces.is_empty()
            && held
                .pieces
                .iter()
                .all(|piece| plan::is_segment_file(audio_dir, track, &piece.wav_path))
    })
}

impl RecordingRuntime {
    /// One unit of transcription work. Returns whether it did something, so
    /// the caller should call again at once.
    pub fn transcription_step(&self) -> bool {
        let parked = lock(&self.scheduler.state).parked.take();
        let job = match parked {
            Some(job) => job,
            None => match self.open_job() {
                Some(job) => job,
                None => return false,
            },
        };
        self.advance_job(job)
    }

    /// Waits for a reason to look for transcription work, or `timeout`.
    pub fn wait_for_transcription_work(&self, timeout: Duration) {
        self.scheduler.wait(timeout);
    }

    /// Picks the next job, if one is due and memory allows. Its first
    /// worker starts in [`Self::advance_job`].
    fn open_job(&self) -> Option<Job> {
        if lock(&self.scheduler.state).current.is_some() {
            return None;
        }
        let model = self.settings().transcription_model;
        if !self.scheduler.models.is_ready(model) {
            return None;
        }
        let rows = match self.database.recording_audio() {
            Ok(rows) => rows,
            Err(error) => {
                log::warn!(target: "recording", "listing recordings to transcribe: {error}");
                return None;
            }
        };
        let active = self.active_id();
        let waiting = rows.iter().find(|row| {
            matches!(
                row.state,
                RecordingState::Recorded | RecordingState::Interrupted
            ) && row.audio_deleted_at.is_none()
                && Some(&row.id) != active.as_ref()
        });
        let row = match waiting {
            Some(row) => {
                let audio_dir = Path::new(&row.audio_dir);
                if plan::standing(audio_dir, true, |_| false).total_ms == 0 {
                    // Nothing to transcribe (a crash before any audio was
                    // saved); it is no longer waiting, so look again.
                    self.fail_recording(&row.id, NO_AUDIO_MESSAGE);
                    return self.open_job();
                }
                row
            }
            None => {
                let id = active.clone()?;
                let row = rows.iter().find(|row| row.id == id)?;
                if !self.scheduler.live_allowed(&id) {
                    return None;
                }
                let waiting = plan::standing(Path::new(&row.audio_dir), false, |_| false);
                if !policy::live_batch_due(waiting.waiting_ms) {
                    return None;
                }
                row
            }
        };
        if !self.scheduler.memory_allows(model) {
            // Waits, still `recorded`, for pressure to fall or memory to
            // free up; the transcription thread looks again shortly.
            return None;
        }
        // A model that fails its check is marked failed; the recording
        // waits for it.
        let (config, model_name) = self.job_config(model)?;
        let live = Some(&row.id) == active.as_ref();
        let title = self
            .database
            .recording(&row.id)
            .ok()
            .flatten()
            .map(|r| r.title)
            .unwrap_or_default();
        let control = Arc::new(JobControl {
            cancel: AtomicBool::new(false),
            live: AtomicBool::new(live),
            live_allowed: self.scheduler.live_allowed.clone(),
            memory: self.scheduler.memory.clone(),
        });
        lock(&self.scheduler.state).current = Some((
            CurrentJob {
                recording_id: row.id.clone(),
                title,
                model,
                percent: None,
            },
            control.clone(),
        ));
        Some(Job {
            recording_id: row.id.clone(),
            audio_dir: PathBuf::from(&row.audio_dir),
            model,
            model_name,
            config: Some(config),
            control,
            session: None,
            done_through: HashMap::new(),
            lines: Vec::new(),
            done_ms: 0,
            announced: false,
            waiting_state: if live {
                RecordingState::Recorded
            } else {
                row.state
            },
            held: HashMap::new(),
            carry_pending: HashSet::new(),
        })
    }

    /// The worker's settings, with `model` checked against its pinned hash
    /// (before every load), and the model's file name. `None` when the
    /// model failed the check; it is then marked failed.
    fn job_config(&self, model: TranscriptionModelChoice) -> Option<(JobConfig, String)> {
        let (model_path, vad_model_path) = match self.scheduler.models.verified_paths(model) {
            Ok(paths) => paths,
            Err(error) => {
                log::warn!(target: "recording", "transcription model: {error}");
                return None;
            }
        };
        let model_name = model_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let config = JobConfig {
            model_path,
            vad_model_path,
            language: language(model),
            threads: policy::transcription_threads(threads) as u32,
            memory_bytes: policy::worker_memory_budget(model.model()),
        };
        Some((config, model_name))
    }

    /// Moves the job's row to `transcribing`.
    fn announce(&self, job: &mut Job) {
        if let Err(error) =
            self.database
                .set_recording_state(&job.recording_id, RecordingState::Transcribing, None)
        {
            log::warn!(target: "recording", "marking transcribing: {error}");
        }
        job.announced = true;
        let total = self.job_total_ms(job);
        self.scheduler
            .set_percent(Some(percent(job.done_ms, total)));
        self.notifier.recordings_changed(&job.recording_id);
        self.refresh_tray_now();
    }

    /// Moves a stopped job's row back to waiting.
    fn unannounce(&self, job: &mut Job) {
        if let Err(error) =
            self.database
                .set_recording_state(&job.recording_id, job.waiting_state, None)
        {
            log::warn!(target: "recording", "marking recorded: {error}");
        }
        job.announced = false;
        self.scheduler.set_percent(None);
        self.notifier.recordings_changed(&job.recording_id);
        self.refresh_tray_now();
    }

    fn job_total_ms(&self, job: &Job) -> u64 {
        plan::standing(&job.audio_dir, true, |_| true).total_ms
    }

    fn advance_job(&self, mut job: Job) -> bool {
        if self.scheduler.is_cancelled(&job.recording_id) {
            self.job_failed(job, false, JobError::Cancelled);
            return true;
        }
        let active = self.active_id().as_deref() == Some(job.recording_id.as_str());
        if !active {
            job.control.live.store(false, Ordering::SeqCst);
            if !job.announced {
                // Stopped since the job started during the recording, or
                // waiting again after an interruption.
                let state = self
                    .database
                    .recording(&job.recording_id)
                    .ok()
                    .flatten()
                    .map(|r| r.state);
                if state != Some(job.waiting_state) {
                    // Failed or gone: nothing to transcribe.
                    self.scheduler.end_job();
                    return true;
                }
            }
        }
        if active && job.session.is_some() && !self.live_may_continue(&job.recording_id) {
            // On battery, Transcribe During Meetings turned off, or memory
            // pressure: the batch ends here, and its worker with it.
            job.session = None;
            self.scheduler.park(job);
            return true;
        }
        let standing = job.standing(!active);
        let Some(segment) = standing.next else {
            if active {
                // The batch is done; its worker exits until the next one.
                job.session = None;
                self.scheduler.park(job);
                return false;
            }
            if !job.announced {
                self.announce(&mut job);
            }
            self.finish_job(job);
            return true;
        };
        if job.session.is_none() {
            let due = !active
                || (self.scheduler.live_allowed(&job.recording_id)
                    && policy::live_batch_due(standing.waiting_ms));
            if !due || !self.scheduler.memory_allows(job.model) {
                self.scheduler.park(job);
                return false;
            }
            let config = match job.config.take() {
                Some(config) => config,
                None => match self.job_config(job.model) {
                    Some((config, _)) => config,
                    None => {
                        self.model_unavailable(job);
                        return true;
                    }
                },
            };
            if !active && !job.announced {
                // Visible from the moment the model starts loading.
                self.announce(&mut job);
            }
            match self
                .scheduler
                .transcriber
                .open(&config, job.control.clone())
            {
                Ok(session) => {
                    job.session = Some(session);
                    job.carry_pending = job.held.keys().copied().collect();
                }
                Err(JobError::Interrupted) => {
                    self.interrupted(job, active);
                    return true;
                }
                Err(error) => {
                    self.job_failed(job, active, error);
                    return true;
                }
            }
        }
        if !active && !job.announced {
            // Stopped while a batch's worker was running; it goes on.
            self.announce(&mut job);
        }
        let track = segment.request.track;
        let mut request = segment.request.clone();
        if job.carry_pending.remove(&track) {
            request.carry_in = job.held.get(&track).cloned();
        }
        let Some(session) = job.session.as_mut() else {
            unreachable!("a worker was opened above");
        };
        match session.transcribe(&request, segment.audio_ms) {
            Ok(result) => {
                if !held_is_planned(&result.held, track, &job.audio_dir) {
                    let crash = JobError::Failed(super::worker::CRASH_MESSAGE.into());
                    self.job_failed(job, active, crash);
                    return true;
                }
                job.done_through.insert(track, segment.number);
                match result.held {
                    Some(held) => job.held.insert(track, held),
                    None => job.held.remove(&track),
                };
                job.lines.extend(result.lines);
                job.done_ms += segment.audio_ms;
                if job.announced {
                    let now = percent(job.done_ms, standing.total_ms);
                    let before = self.scheduler.current().and_then(|c| c.percent);
                    if before != Some(now) {
                        self.scheduler.set_percent(Some(now));
                        self.notifier.transcription_progress(&job.recording_id, now);
                        self.refresh_tray_now();
                    }
                }
                self.scheduler.park(job);
                true
            }
            Err(JobError::Interrupted) => {
                self.interrupted(job, active);
                true
            }
            Err(error) => {
                self.job_failed(job, active, error);
                true
            }
        }
    }

    /// Whether a batch in progress during the recording may take another
    /// file. Memory pressure also ends transcription during this recording.
    fn live_may_continue(&self, recording_id: &str) -> bool {
        if self.scheduler.memory.pressure() != MemoryPressure::Normal {
            self.scheduler.stop_live(recording_id);
            return false;
        }
        self.scheduler.live_allowed(recording_id)
    }

    /// The worker was stopped mid-file to give memory back, or because
    /// Transcribe During Meetings was turned off. The file it was on is
    /// redone by the next worker, from the last one it finished.
    fn interrupted(&self, mut job: Job, active: bool) {
        job.session = None;
        if active {
            if self.scheduler.memory.pressure() != MemoryPressure::Normal {
                self.scheduler.stop_live(&job.recording_id);
            }
        } else if job.announced {
            log::info!(target: "recording", "memory pressure: transcription waits");
            self.unannounce(&mut job);
        }
        self.scheduler.park(job);
    }

    /// The job's model failed its check before a worker loaded it. The
    /// model is marked failed, and the recording waits for it like any
    /// other.
    fn model_unavailable(&self, mut job: Job) {
        if job.announced {
            self.unannounce(&mut job);
        }
        drop(job);
        self.scheduler.end_job();
        self.refresh_tray_now();
    }

    fn job_failed(&self, job: Job, live: bool, error: JobError) {
        let id = job.recording_id.clone();
        drop(job);
        match error {
            JobError::Cancelled | JobError::Interrupted => {}
            JobError::Failed(message) if live => {
                log::warn!(target: "recording", "transcribing during the recording: {message}");
                lock(&self.scheduler.state).live_failed.insert(id);
            }
            JobError::Failed(message) => self.fail_recording(&id, &message),
        }
        self.scheduler.end_job();
        self.refresh_tray_now();
    }

    fn fail_recording(&self, id: &str, message: &str) {
        if let Err(error) =
            self.database
                .set_recording_state(id, RecordingState::Failed, Some(message))
        {
            log::warn!(target: "recording", "marking transcription failed: {error}");
        }
        self.notifier.recordings_changed(id);
    }

    fn finish_job(&self, mut job: Job) {
        let id = job.recording_id.clone();
        let model_name = std::mem::take(&mut job.model_name);
        let lines = std::mem::take(&mut job.lines);
        let system_start = plan::standing(&job.audio_dir, true, |_| true)
            .system_start_ms
            .map(|start| start as i64);
        drop(job);
        let rows: Vec<TranscriptRow> = merge::merge(lines)
            .into_iter()
            .map(|line| TranscriptRow {
                track: line.line.track,
                start_ms: line.line.start_ms as i64,
                end_ms: line.line.end_ms as i64,
                text: line.line.text,
                dropped_as_echo: line.dropped_as_echo,
            })
            .collect();
        let saved = self
            .database
            .save_transcript(&id, &rows, ENGINE, &model_name, system_start);
        drop(rows);
        self.scheduler.end_job();
        match saved {
            Ok(()) => {
                self.notifier.recordings_changed(&id);
                self.refresh_tray_now();
                // Keep Audio: Until Transcribed applies now.
                self.sweep();
            }
            Err(error) => {
                log::warn!(target: "recording", "saving the transcript: {error}");
                self.fail_recording(&id, "The transcript couldn't be saved.");
                self.refresh_tray_now();
            }
        }
    }
}

#[cfg(test)]
mod tests;
