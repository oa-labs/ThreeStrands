//! Which recording is transcribed when, one job at a time, and the job
//! itself from its first segment file to the saved transcript.
//!
//! A **job** is one recording with one worker ([`super::Transcriber`]) and
//! one model, kept from start to finish: a model chosen or deleted later
//! doesn't affect it. Jobs are picked, whenever the chosen model is ready:
//!
//! 1. the oldest `recorded` or `interrupted` recording whose audio exists;
//! 2. otherwise the recording in progress, while the Mac is on AC power:
//!    its closed segment files are transcribed as they appear, so the
//!    transcript is nearly done at Stop. On battery this waits for Stop.
//!    Until Stop the row stays `recording`; nothing of this is visible.
//!
//! After Stop the job's row goes `recorded` → `transcribing` (with a
//! percentage of audio done, what ran during the recording included) →
//! `transcribed`, or `failed` with a plain reason. A job that fails during
//! the recording is dropped and the recording starts over after Stop.
//!
//! [`RecordingRuntime::transcription_step`] does one unit of work (opens a
//! job, transcribes one segment file, or finishes) without holding a lock
//! during the slow part, so deletes, listings, and the tray read the job's
//! state at any time. The app runs it on its own thread; tests call it
//! directly.

use super::merge;
use super::models::ModelStore;
use super::plan::{self, PlannedSegment};
use super::power::PowerSource;
use super::protocol::TranscriptLine;
use super::{JobConfig, JobError, Transcriber, TranscriptionSession, ENGINE};
use crate::db::recordings::{RecordingState, TranscriptRow};
use crate::recording::capture::Track;
use crate::recording::policy;
use crate::recording::runtime::RecordingRuntime;
use crate::recording::settings::TranscriptionModelChoice;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The job in progress, as the rest of the app sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentJob {
    pub recording_id: String,
    pub title: String,
    pub model: TranscriptionModelChoice,
    /// Whole percent of audio done, once the recording has stopped; `None`
    /// while it is still being recorded.
    pub percent: Option<u8>,
}

/// What the tray shows while a job runs after Stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayTranscription {
    pub title: String,
    pub percent: u8,
}

struct Job {
    recording_id: String,
    audio_dir: PathBuf,
    model_name: String,
    session: Box<dyn TranscriptionSession>,
    done: HashSet<(Track, u32)>,
    lines: Vec<TranscriptLine>,
    done_ms: u64,
    /// The row has been moved to `transcribing`.
    announced: bool,
}

#[derive(Default)]
struct State {
    wake: bool,
    current: Option<(CurrentJob, Arc<AtomicBool>)>,
    /// The job between steps; `None` while a step has it.
    parked: Option<Job>,
    /// Recordings whose job failed while they were being recorded; they
    /// start over after Stop rather than during the recording again.
    live_failed: HashSet<String>,
}

pub struct Scheduler {
    transcriber: Arc<dyn Transcriber>,
    power: Arc<dyn PowerSource>,
    pub(crate) models: Arc<ModelStore>,
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
        models: Arc<ModelStore>,
    ) -> Self {
        Self {
            transcriber,
            power,
            models,
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
            let Some((job, flag)) = state.current.as_ref() else {
                return;
            };
            if job.recording_id != recording_id {
                return;
            }
            flag.store(true, Ordering::SeqCst);
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
        let cancelled = state.current.as_ref().is_none_or(|(current, flag)| {
            current.recording_id != job.recording_id || flag.load(Ordering::SeqCst)
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
            .is_none_or(|(job, flag)| {
                job.recording_id != recording_id || flag.load(Ordering::SeqCst)
            })
    }

    fn set_percent(&self, percent: Option<u8>) {
        if let Some((job, _)) = lock(&self.state).current.as_mut() {
            job.percent = percent;
        }
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

    /// Picks and opens the next job, if one is due.
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
                if plan::plan(std::path::Path::new(&row.audio_dir), true).is_empty() {
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
                let live_failed = lock(&self.scheduler.state).live_failed.contains(&id);
                let has_closed = plan::plan(std::path::Path::new(&row.audio_dir), false)
                    .iter()
                    .any(|segment| segment.closed);
                if live_failed || !has_closed || !self.scheduler.power.on_ac_power() {
                    return None;
                }
                row
            }
        };
        let live = Some(&row.id) == active.as_ref();
        let title = self
            .database
            .recording(&row.id)
            .ok()
            .flatten()
            .map(|r| r.title)
            .unwrap_or_default();
        let (model_path, vad_model_path) = match self.scheduler.models.verified_paths(model) {
            Ok(paths) => paths,
            Err(error) => {
                // The model is marked failed; the recording waits for it.
                log::warn!(target: "recording", "transcription model: {error}");
                return None;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.scheduler.state).current = Some((
            CurrentJob {
                recording_id: row.id.clone(),
                title,
                model,
                percent: None,
            },
            cancel.clone(),
        ));
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let config = JobConfig {
            model_path: model_path.clone(),
            vad_model_path,
            language: language(model),
            threads: policy::transcription_threads(threads) as u32,
        };
        let model_name = model_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut job = Job {
            recording_id: row.id.clone(),
            audio_dir: PathBuf::from(&row.audio_dir),
            model_name,
            // Replaced below once the worker is up.
            session: Box::new(Unopened),
            done: HashSet::new(),
            lines: Vec::new(),
            done_ms: 0,
            announced: false,
        };
        if !live {
            // Visible from the moment the model starts loading.
            self.announce(&mut job);
        }
        match self.scheduler.transcriber.open(&config, cancel) {
            Ok(session) => {
                job.session = session;
                Some(job)
            }
            Err(error) => {
                self.job_failed(job, live, error);
                None
            }
        }
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

    fn job_total_ms(&self, job: &Job) -> u64 {
        plan::plan(&job.audio_dir, true)
            .iter()
            .map(|segment| segment.audio_ms)
            .sum()
    }

    fn advance_job(&self, mut job: Job) -> bool {
        if self.scheduler.is_cancelled(&job.recording_id) {
            self.job_failed(job, false, JobError::Cancelled);
            return true;
        }
        let active = self.active_id().as_deref() == Some(job.recording_id.as_str());
        if !active && !job.announced {
            // Stopped since the job started during the recording.
            let state = self
                .database
                .recording(&job.recording_id)
                .ok()
                .flatten()
                .map(|r| r.state);
            if state != Some(RecordingState::Recorded) {
                // Failed or gone: nothing to transcribe.
                self.scheduler.end_job();
                return true;
            }
            self.announce(&mut job);
        }
        if active && !self.scheduler.power.on_ac_power() {
            self.scheduler.park(job);
            return false;
        }
        let planned = plan::plan(&job.audio_dir, !active);
        let next: Option<&PlannedSegment> = planned.iter().find(|segment| {
            segment.closed && !job.done.contains(&(segment.request.track, segment.number))
        });
        let Some(segment) = next.cloned() else {
            if active {
                self.scheduler.park(job);
                return false;
            }
            self.finish_job(job);
            return true;
        };
        match job.session.transcribe(&segment.request, segment.audio_ms) {
            Ok(lines) => {
                job.done.insert((segment.request.track, segment.number));
                job.lines.extend(lines);
                job.done_ms += segment.audio_ms;
                if job.announced {
                    let total: u64 = planned.iter().map(|s| s.audio_ms).sum();
                    let now = percent(job.done_ms, total);
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
            Err(error) => {
                self.job_failed(job, active, error);
                true
            }
        }
    }

    fn job_failed(&self, job: Job, live: bool, error: JobError) {
        let id = job.recording_id.clone();
        drop(job);
        match error {
            JobError::Cancelled => {}
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

    fn finish_job(&self, job: Job) {
        let id = job.recording_id.clone();
        let model_name = job.model_name.clone();
        let merged = merge::merge(job.lines.clone());
        let rows: Vec<TranscriptRow> = merged
            .into_iter()
            .map(|line| TranscriptRow {
                track: line.line.track,
                start_ms: line.line.start_ms as i64,
                end_ms: line.line.end_ms as i64,
                text: line.line.text,
                dropped_as_echo: line.dropped_as_echo,
            })
            .collect();
        let system_start = plan::plan(&job.audio_dir, true)
            .iter()
            .find(|segment| segment.request.track == Track::System)
            .map(|segment| segment.request.start_ms as i64);
        drop(job);
        let saved = self
            .database
            .save_transcript(&id, &rows, ENGINE, &model_name, system_start);
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

/// Placeholder session while the worker starts.
struct Unopened;

impl TranscriptionSession for Unopened {
    fn transcribe(
        &mut self,
        _request: &super::protocol::SegmentRequest,
        _audio_ms: u64,
    ) -> Result<Vec<TranscriptLine>, JobError> {
        Err(JobError::Failed(super::worker::CRASH_MESSAGE.into()))
    }
}

#[cfg(test)]
mod tests;
