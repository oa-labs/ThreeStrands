//! The app's handle on meeting recording: the [`RecordingService`] behind a
//! lock, plus everything around it that needs the file system or tells the
//! rest of the app what changed. Commands, the tray, and the tick thread all
//! go through it.
//!
//! Every method that can change the recording ends by publishing: the
//! `recording-state` event goes out when the status changed (anything but
//! the elapsed time), `recordings-changed` when a recording was created,
//! finished, interrupted, deleted, or moved through transcription, and the
//! tray is refreshed. Publishing happens after the service lock is
//! released, so a listener can call back in.
//!
//! Transcription (stage 3) lives here too: the model store, and the
//! scheduler whose step runs on its own thread
//! ([`super::transcribe::scheduler`]).

use super::capture::{AudioInputDevice, CaptureBackend, SystemAudioSupport, Track};
use super::offers::{self, MicActivityProbe, OfferEngine, OfferInput, TrayOffer};
use super::retention;
use super::segments;
use super::settings::{RecordingSettings, TranscriptionModelChoice};
use super::transcribe::cloud::CloudTranscriptionBackend;
use super::transcribe::memory::MemoryProbe;
use super::transcribe::models::{ModelStore, TranscriptionModelStatus};
use super::transcribe::plan;
use super::transcribe::power::PowerSource;
use super::transcribe::scheduler::{Scheduler, TrayTranscription};
use super::transcribe::Transcriber;
use super::vocabulary::{self, VocabularySuggestion};
use super::{policy, DiskProbe, RecordingPhase, RecordingService, RecordingStatus};
use crate::clock::Clock;
use crate::db::note_entities::{OfferedMeeting, TrayMeeting};
use crate::db::recordings::{MeetingRecording, RecordingState};
use crate::db::Database;
use crate::note_entities::NoteReference;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

/// Unreviewed transcribed recording ready for the tray's Review action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayReview {
    pub recording_id: String,
    pub title: String,
}

/// Where the runtime reports changes. The app sends Tauri events and
/// refreshes the tray; tests record the calls.
pub trait RecordingNotifier: Send + Sync {
    fn state_changed(&self, status: &RecordingStatus);
    fn recordings_changed(&self, recording_id: &str);
    /// Called after every published change and every tick while a
    /// recording is active, so the tray's elapsed time stays current, and
    /// whenever a transcription after Stop changes.
    fn refresh_tray(
        &self,
        status: &RecordingStatus,
        settings: &RecordingSettings,
        transcribing: Option<&TrayTranscription>,
        review: Option<&TrayReview>,
        offer: Option<&TrayOffer>,
        upcoming: &[TrayMeeting],
        now: SystemTime,
        supported: bool,
    );
    /// A transcription's whole percent changed.
    fn transcription_progress(&self, recording_id: &str, percent: u8);
    /// A transcription model's state or download progress changed.
    fn models_changed(&self, statuses: &[TranscriptionModelStatus]);
}

/// What transcription needs: the worker, cloud backend, power source, memory
/// probe, and models.
pub struct TranscriptionParts {
    pub transcriber: Arc<dyn Transcriber>,
    pub cloud: Arc<dyn CloudTranscriptionBackend>,
    pub power: Arc<dyn PowerSource>,
    pub memory: Arc<dyn MemoryProbe>,
    pub models: Arc<ModelStore>,
}

/// A transcript as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingTranscript {
    pub recording_id: String,
    pub speaker_labels: bool,
    pub lines: Vec<RecordingTranscriptLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptSpeaker {
    You,
    Others,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingTranscriptLine {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: Option<TranscriptSpeaker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingCapabilities {
    pub supported: bool,
    pub system_audio: SystemAudioSupport,
    pub speech_analyzer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingsUsage {
    pub recordings: u64,
    pub audio_bytes: u64,
}

const ACTIVE_REFUSAL: &str = "This recording is still in progress. Stop it first.";
const TRANSCRIBING_REFUSAL: &str =
    "Wait for the transcription to finish, or delete the whole recording.";
const UNSUPPORTED_TRANSCRIPTION: &str = "Transcription isn't available on this platform yet.";

fn rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub struct RecordingRuntime {
    service: Mutex<RecordingService>,
    backend: Arc<dyn CaptureBackend>,
    pub(super) database: Arc<Database>,
    supported: bool,
    pub(super) speech_analyzer: bool,
    audio_root: PathBuf,
    clock: Arc<dyn Clock + Send + Sync>,
    pub(super) notifier: Arc<dyn RecordingNotifier>,
    pub(super) scheduler: Arc<Scheduler>,
    pub(super) cloud: Arc<dyn CloudTranscriptionBackend>,
    settings: Mutex<RecordingSettings>,
    /// The last status published, with its elapsed time zeroed.
    published: Mutex<Option<RecordingStatus>>,
    last_sweep: Mutex<Option<SystemTime>>,
    mic_probe: Arc<dyn MicActivityProbe>,
    offers: Mutex<OfferEngine>,
    current_offer: Mutex<Option<TrayOffer>>,
    cached_meeting: Mutex<Option<(SystemTime, Option<OfferedMeeting>)>>,
    cached_upcoming: Mutex<Option<(SystemTime, Vec<TrayMeeting>)>>,
    last_upcoming_snapshot: Mutex<Vec<TrayMeeting>>,
    last_tray_minute: Mutex<Option<i64>>,
    cached_event_end: Mutex<Option<(String, Option<SystemTime>)>>,
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
        transcription: TranscriptionParts,
    ) -> Self {
        Self::with_mic_probe(
            database,
            backend,
            clock,
            disk,
            audio_root,
            notifier,
            transcription,
            offers::platform_mic_probe(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_mic_probe(
        database: Arc<Database>,
        backend: Arc<dyn CaptureBackend>,
        clock: Arc<dyn Clock + Send + Sync>,
        disk: Arc<dyn DiskProbe>,
        audio_root: PathBuf,
        notifier: Arc<dyn RecordingNotifier>,
        transcription: TranscriptionParts,
        mic_probe: Arc<dyn MicActivityProbe>,
    ) -> Self {
        let settings = database.recording_settings().unwrap_or_else(|error| {
            log::warn!(target: "recording", "reading recording settings: {error}");
            RecordingSettings::default()
        });
        let supported = backend.supported();
        let speech_analyzer = supported && backend.speech_analyzer();
        let scheduler = Arc::new(Scheduler::new(
            transcription.transcriber,
            transcription.power,
            transcription.memory,
            transcription.models.clone(),
        ));
        scheduler.set_transcribe_during_meetings(settings.transcribe_during_meetings);
        // A model becoming ready can start waiting recordings.
        let listener_notifier = notifier.clone();
        let listener_scheduler = Arc::downgrade(&scheduler);
        transcription
            .models
            .set_listener(Arc::new(move |statuses, ready| {
                listener_notifier.models_changed(statuses);
                if ready.is_some() {
                    if let Some(scheduler) = listener_scheduler.upgrade() {
                        scheduler.wake();
                    }
                }
            }));
        Self {
            service: Mutex::new(RecordingService::new(
                database.clone(),
                backend.clone(),
                clock.clone(),
                disk,
                audio_root.clone(),
            )),
            backend,
            database,
            supported,
            speech_analyzer,
            audio_root,
            clock,
            notifier,
            scheduler,
            cloud: transcription.cloud,
            settings: Mutex::new(settings),
            published: Mutex::new(None),
            last_sweep: Mutex::new(None),
            mic_probe,
            offers: Mutex::new(OfferEngine::default()),
            current_offer: Mutex::new(None),
            cached_meeting: Mutex::new(None),
            cached_upcoming: Mutex::new(None),
            last_upcoming_snapshot: Mutex::new(Vec::new()),
            last_tray_minute: Mutex::new(None),
            cached_event_end: Mutex::new(None),
        }
    }

    pub fn capabilities(&self) -> RecordingCapabilities {
        RecordingCapabilities {
            supported: self.supported,
            system_audio: lock(&self.service).status().system_audio,
            speech_analyzer: self.speech_analyzer,
        }
    }

    pub fn input_devices(&self) -> Vec<AudioInputDevice> {
        if !self.supported {
            return Vec::new();
        }
        self.backend.list_input_devices()
    }

    pub fn status(&self) -> RecordingStatus {
        lock(&self.service).status()
    }

    pub fn settings(&self) -> RecordingSettings {
        lock(&self.settings).clone()
    }

    /// Whether closing the window hides it (Record Meetings is on or upcoming
    /// meetings are shown in the tray, and this platform has the tray to bring
    /// it back).
    pub fn hide_on_close(&self) -> bool {
        if !self.supported {
            return false;
        }
        let settings = self.settings();
        settings.record_meetings
            || (settings.show_upcoming_meetings_in_tray
                && !lock(&self.last_upcoming_snapshot).is_empty())
    }

    pub fn is_active(&self) -> bool {
        lock(&self.service).status().state != RecordingPhase::Idle
    }

    pub(super) fn active_id(&self) -> Option<String> {
        lock(&self.service).status().recording_id
    }

    fn upcoming_meetings_for(
        &self,
        now: SystemTime,
        settings: &RecordingSettings,
    ) -> Vec<TrayMeeting> {
        if !self.supported || !settings.show_upcoming_meetings_in_tray {
            lock(&self.last_upcoming_snapshot).clear();
            *lock(&self.cached_upcoming) = None;
            return Vec::new();
        }
        if let Some((fetched_at, cached)) = lock(&self.cached_upcoming).as_ref() {
            if now.duration_since(*fetched_at).unwrap_or_default()
                < policy::OFFER_CALENDAR_CACHE_TTL
            {
                *lock(&self.last_upcoming_snapshot) = cached.clone();
                return cached.clone();
            }
        }
        let local_now = chrono::DateTime::<chrono::Local>::from(now);
        let fresh = self
            .database
            .upcoming_tray_meetings(local_now, policy::MAX_TRAY_MEETINGS)
            .unwrap_or_default();
        *lock(&self.cached_upcoming) = Some((now, fresh.clone()));
        *lock(&self.last_upcoming_snapshot) = fresh.clone();
        fresh
    }

    pub fn upcoming_meeting(&self, index: usize) -> Option<TrayMeeting> {
        lock(&self.last_upcoming_snapshot).get(index).cloned()
    }

    /// Sends what changed since the last publish. Returns true when it refreshed the tray.
    fn publish(&self) -> bool {
        let status = self.status();
        let mut comparable = status.clone();
        comparable.elapsed_ms = 0;
        let previous = lock(&self.published).replace(comparable.clone());
        let changed = previous.as_ref() != Some(&comparable);
        if changed {
            self.notifier.state_changed(&status);
            let before = previous.and_then(|p| p.recording_id);
            if let Some(ended) = before.filter(|id| Some(id) != status.recording_id.as_ref()) {
                self.scheduler.recording_stopped(&ended);
                self.notifier.recordings_changed(&ended);
            }
        }
        let refreshed = changed || status.state != RecordingPhase::Idle;
        if refreshed {
            let now = self.clock.now();
            let settings = self.settings();
            let review = self.review_candidate();
            let offer = lock(&self.current_offer).clone();
            let upcoming = self.upcoming_meetings_for(now, &settings);
            self.notifier.refresh_tray(
                &status,
                &settings,
                self.scheduler.tray().as_ref(),
                review.as_ref(),
                offer.as_ref(),
                &upcoming,
                now,
                self.supported,
            );
        }
        refreshed
    }

    fn review_candidate(&self) -> Option<TrayReview> {
        self.database
            .latest_unreviewed_transcribed_recording()
            .ok()
            .flatten()
            .map(|rec| TrayReview {
                recording_id: rec.id,
                title: rec.title,
            })
    }

    pub fn latest_unreviewed_review(&self) -> Option<TrayReview> {
        self.review_candidate()
    }

    pub fn current_offer(&self) -> Option<TrayOffer> {
        lock(&self.current_offer).clone()
    }

    pub fn invalidate_calendar_offers(&self) {
        *lock(&self.cached_meeting) = None;
        *lock(&self.cached_event_end) = None;
        *lock(&self.cached_upcoming) = None;
        let offer_changed = self.evaluate_offers();
        let previous_upcoming = lock(&self.last_upcoming_snapshot).clone();
        let upcoming = self.upcoming_meetings_for(self.clock.now(), &self.settings());
        let upcoming_changed = upcoming != previous_upcoming;
        if offer_changed || upcoming_changed {
            self.refresh_tray_now();
        }
    }

    pub fn notify_recordings_changed(&self, recording_id: &str) {
        self.notifier.recordings_changed(recording_id);
        self.refresh_tray_now();
    }

    pub fn refresh_tray_for_review(&self) {
        self.refresh_tray_now();
    }

    /// Refreshes the tray outside a publish (transcription progress, review, or upcoming meetings change).
    pub(super) fn refresh_tray_now(&self) {
        let now = self.clock.now();
        let settings = self.settings();
        let review = self.review_candidate();
        let offer = lock(&self.current_offer).clone();
        let upcoming = self.upcoming_meetings_for(now, &settings);
        self.notifier.refresh_tray(
            &self.status(),
            &settings,
            self.scheduler.tray().as_ref(),
            review.as_ref(),
            offer.as_ref(),
            &upcoming,
            now,
            self.supported,
        );
    }

    fn scheduled_meeting(&self, now: SystemTime) -> Option<OfferedMeeting> {
        if let Some((fetched_at, cached)) = lock(&self.cached_meeting).as_ref() {
            if now.duration_since(*fetched_at).unwrap_or_default()
                < policy::OFFER_CALENDAR_CACHE_TTL
            {
                return cached.clone();
            }
        }
        let fresh = self
            .database
            .active_or_upcoming_meeting(now, policy::OFFER_WINDOW)
            .ok()
            .flatten();
        *lock(&self.cached_meeting) = Some((now, fresh.clone()));
        fresh
    }

    fn active_recording_end(
        &self,
        recording_id: &str,
        event_ref: &NoteReference,
    ) -> Option<SystemTime> {
        if let Some((cached_id, end)) = lock(&self.cached_event_end).as_ref() {
            if cached_id == recording_id {
                return *end;
            }
        }
        let end = self.database.cached_event_end(event_ref).ok().flatten();
        *lock(&self.cached_event_end) = Some((recording_id.to_string(), end));
        end
    }

    /// Evaluates active conferencing/calendar offers and post-call silence nudges.
    /// Returns true if `current_offer` changed.
    fn evaluate_offers(&self) -> bool {
        if !self.supported {
            return false;
        }
        let now = self.clock.now();
        let settings = self.settings();
        if !settings.record_meetings || !settings.offer_to_record {
            let empty_mic = offers::MicActivitySnapshot::default();
            let outcome = lock(&self.offers).evaluate(OfferInput {
                now,
                settings: &settings,
                phase: RecordingPhase::Idle,
                recording_id: None,
                scheduled_meeting: None,
                recording_event_end: None,
                both_tracks_silent_for: Duration::ZERO,
                mic: &empty_mic,
            });
            let _ = outcome;
            lock(&self.service).set_still_recording_nudge(false);
            let prev = lock(&self.current_offer).take();
            return prev.is_some();
        }

        let (phase, recording_id, event_ref, silent_for) = {
            let service = lock(&self.service);
            let status = service.status();
            (
                status.state,
                status.recording_id,
                status.event_ref,
                service.both_tracks_silent_for(),
            )
        };
        let scheduled = if phase == RecordingPhase::Idle {
            self.scheduled_meeting(now)
        } else {
            None
        };
        let recording_event_end = match (recording_id.as_deref(), event_ref.as_ref()) {
            (Some(id), Some(ref_val)) if phase != RecordingPhase::Idle => {
                self.active_recording_end(id, ref_val)
            }
            _ => None,
        };
        let mic = self.mic_probe.snapshot();
        let outcome = lock(&self.offers).evaluate(OfferInput {
            now,
            settings: &settings,
            phase,
            recording_id: recording_id.as_deref(),
            scheduled_meeting: scheduled.as_ref(),
            recording_event_end,
            both_tracks_silent_for: silent_for,
            mic: &mic,
        });
        if phase != RecordingPhase::Idle {
            lock(&self.service).set_still_recording_nudge(outcome.still_recording_nudge);
        }
        let mut current = lock(&self.current_offer);
        let changed = *current != outcome.offer;
        *current = outcome.offer;
        changed
    }

    #[cfg(test)]
    pub fn start(
        &self,
        event_ref: Option<NoteReference>,
        title: &str,
    ) -> Result<RecordingStatus, String> {
        self.start_meeting(event_ref, title, &[])
    }

    pub fn start_meeting(
        &self,
        event_ref: Option<NoteReference>,
        title: &str,
        attendees: &[String],
    ) -> Result<RecordingStatus, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("A recording needs a title.".into());
        }
        if !self.supported {
            return Err(super::capture::UNSUPPORTED_MESSAGE.into());
        }
        let microphone_uid = self.settings().microphone_device_uid;
        let (result, failed) = {
            let mut service = lock(&self.service);
            let result = service.start_with_options(event_ref, title, microphone_uid, attendees);
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
        // The stopped recording, or what it was waiting behind, can go on.
        self.scheduler.wake();
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

    /// Stops any active recording when the Mac enters system sleep and queues
    /// transcription immediately.
    pub fn stop_for_sleep(&self) {
        if self.is_active() {
            lock(&self.service).stop_for_sleep();
            self.publish();
            self.scheduler.wake();
        }
    }

    /// The once-a-second work: capture health and limits, offers, upcoming
    /// tray meetings countdown, then the daily retention sweep.
    pub fn tick(&self) {
        lock(&self.service).tick();
        let offer_changed = self.evaluate_offers();
        let now = self.clock.now();
        let settings = self.settings();
        let previous_upcoming = lock(&self.last_upcoming_snapshot).clone();
        let upcoming = self.upcoming_meetings_for(now, &settings);
        let upcoming_changed = upcoming != previous_upcoming;
        let now_minute = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            / 60;
        let prev_minute = lock(&self.last_tray_minute).replace(now_minute);
        let minute_changed = prev_minute != Some(now_minute)
            && settings.show_upcoming_meetings_in_tray
            && !upcoming.is_empty();
        let refreshed = self.publish();
        if !refreshed && (offer_changed || upcoming_changed || minute_changed) {
            self.refresh_tray_now();
        }
        let due = lock(&self.last_sweep).is_none_or(|last| {
            now.duration_since(last).unwrap_or_default() >= policy::RETENTION_SWEEP_INTERVAL
        });
        if due {
            self.sweep();
        }
    }

    pub fn set_settings(&self, mut settings: RecordingSettings) -> Result<(), String> {
        settings.custom_vocabulary =
            policy::sanitize_custom_vocabulary(&settings.custom_vocabulary);
        if !settings.cloud_transcription_enabled {
            settings.cloud_transcription_default = false;
        }
        self.database
            .set_recording_settings(&settings)
            .map_err(String::from)?;
        let keep_changed = lock(&self.settings).keep_audio != settings.keep_audio;
        self.scheduler
            .set_transcribe_during_meetings(settings.transcribe_during_meetings);
        *lock(&self.settings) = settings.clone();
        *lock(&self.cached_meeting) = None;
        *lock(&self.cached_upcoming) = None;
        self.evaluate_offers();
        self.refresh_tray_now();
        if keep_changed {
            self.sweep();
        }
        // Another model may be chosen, and ready.
        self.scheduler.wake();
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
        // A transcription ended with the process that ran it; start over.
        if let Err(error) = self.database.reset_transcribing_recordings() {
            log::warn!(target: "recording", "resetting transcriptions: {error}");
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
        let transcribing = self.scheduler.current().map(|job| job.recording_id);
        let mut deleted = 0;
        for row in rows {
            let busy = Some(&row.id) == active.as_ref()
                || Some(&row.id) == transcribing.as_ref()
                || row.state == RecordingState::Transcribing;
            if busy || !retention::audio_expired(keep, &row, now) {
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
        self.scheduler.cancel(id);
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
        if self
            .scheduler
            .current()
            .is_some_and(|job| job.recording_id == id)
        {
            return Err(TRANSCRIBING_REFUSAL.into());
        }
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

    /// Deletes every recording's audio except the one in progress and the
    /// one being transcribed.
    pub fn delete_all_audio(&self) -> Result<(), String> {
        let active = self.active_id();
        let transcribing = self.scheduler.current().map(|job| job.recording_id);
        let rows = self.database.recording_audio().map_err(String::from)?;
        let mut first_error = None;
        for row in rows {
            if Some(&row.id) == active.as_ref()
                || Some(&row.id) == transcribing.as_ref()
                || row.audio_deleted_at.is_some()
            {
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
            self.scheduler.cancel(&row.id);
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

    /// Whether a recording has no audio on disk: deleted, or never saved.
    fn audio_missing(recording: &MeetingRecording) -> bool {
        recording.audio_deleted
            || (recording.state != RecordingState::Recording
                && Track::ALL.iter().all(|&track| {
                    segments::saved_track(Path::new(&recording.audio_dir), track).audio_ms == 0
                }))
    }

    /// An event occurrence's recordings, newest first, with what only the
    /// runtime knows: missing audio and transcription progress.
    pub fn list_recordings(&self, event: &NoteReference) -> Result<Vec<MeetingRecording>, String> {
        let mut recordings = self.database.list_recordings(event).map_err(String::from)?;
        for recording in &mut recordings {
            self.annotate(recording);
        }
        Ok(recordings)
    }

    /// Fills in a listed recording's missing audio and progress.
    pub(super) fn annotate(&self, recording: &mut MeetingRecording) {
        recording.audio_deleted = Self::audio_missing(recording);
        if recording.state == RecordingState::Transcribing {
            recording.transcription_percent = self
                .scheduler
                .current()
                .filter(|job| job.recording_id == recording.id)
                .and_then(|job| job.percent)
                .or(Some(0));
        }
    }

    fn refuse_unsupported(&self) -> Result<(), String> {
        if self.supported && cfg!(target_os = "macos") {
            Ok(())
        } else {
            Err(UNSUPPORTED_TRANSCRIPTION.into())
        }
    }

    /// Queues a recording for transcription again (Retry), or nudges one
    /// already waiting.
    pub fn transcribe_recording(&self, id: &str) -> Result<(), String> {
        self.refuse_unsupported()?;
        let recording = self
            .database
            .recording(id)
            .map_err(String::from)?
            .ok_or("This recording no longer exists.")?;
        match recording.state {
            RecordingState::Recorded | RecordingState::Interrupted | RecordingState::Failed => {}
            RecordingState::Recording => return Err(ACTIVE_REFUSAL.into()),
            RecordingState::Transcribing => {
                return Err("This recording is already being transcribed.".into())
            }
            RecordingState::Transcribed => {
                return Err("This recording is already transcribed.".into())
            }
        }
        if recording.audio_deleted || plan::plan(Path::new(&recording.audio_dir), true).is_empty() {
            return Err("This recording has no audio to transcribe.".into());
        }
        if recording.state == RecordingState::Failed {
            self.database
                .set_recording_state(id, RecordingState::Recorded, None)
                .map_err(String::from)?;
            self.notifier.recordings_changed(id);
        }
        self.scheduler.wake();
        Ok(())
    }

    /// A transcribed recording's transcript, in time order, without the
    /// lines dropped as echo.
    pub fn recording_transcript(&self, id: &str) -> Result<RecordingTranscript, String> {
        let recording = self
            .database
            .recording(id)
            .map_err(String::from)?
            .ok_or("This recording no longer exists.")?;
        if recording.state != RecordingState::Transcribed {
            return Err("This recording hasn't been transcribed yet.".into());
        }
        let speaker_labels = recording
            .tracks
            .iter()
            .any(|track| track.track == Track::System && track.first_frame_offset_ms.is_some());
        let lines = self
            .database
            .transcript_rows(id)
            .map_err(String::from)?
            .into_iter()
            .filter(|row| !row.dropped_as_echo)
            .map(|row| RecordingTranscriptLine {
                start_ms: row.start_ms,
                end_ms: row.end_ms,
                speaker: speaker_labels.then_some(match row.track {
                    Track::Mic => TranscriptSpeaker::You,
                    Track::System => TranscriptSpeaker::Others,
                }),
                speaker_label: if speaker_labels && row.track == Track::System {
                    row.speaker_label
                } else {
                    None
                },
                text: row.text,
            })
            .collect();
        Ok(RecordingTranscript {
            recording_id: id.to_string(),
            speaker_labels,
            lines,
        })
    }

    /// Scans Sent email locally for recurring domain terms, acronyms, and
    /// proper nouns. Never saves suggestions automatically.
    pub fn suggest_vocabulary(&self) -> Result<Vec<VocabularySuggestion>, String> {
        let existing = self.settings().custom_vocabulary;
        let messages = self
            .database
            .sent_message_texts_for_vocabulary(policy::SENT_VOCAB_SCAN_LIMIT)
            .map_err(String::from)?;
        Ok(vocabulary::extract_vocabulary_suggestions(
            &messages, &existing,
        ))
    }

    /// Explicitly uploads an existing recording's per-track audio for cloud
    /// transcription, preserving any prior transcript if the upload fails.
    pub fn cloud_transcribe_recording(
        &self,
        id: &str,
        provider: crate::ai::AiProvider,
        model: &str,
        endpoint: Option<&str>,
        diarize: bool,
        api_key: &str,
    ) -> Result<RecordingTranscript, String> {
        self.cloud_transcribe_recording_with_requests(
            id,
            provider,
            model,
            endpoint,
            diarize,
            api_key,
            &crate::ai::requests::RUNTIME,
        )
    }

    pub(crate) fn cloud_transcribe_recording_with_requests(
        &self,
        id: &str,
        provider: crate::ai::AiProvider,
        model: &str,
        endpoint: Option<&str>,
        diarize: bool,
        api_key: &str,
        requests: &crate::ai::requests::Runtime,
    ) -> Result<RecordingTranscript, String> {
        self.refuse_unsupported()?;
        let settings = self.settings();
        if !settings.cloud_transcription_enabled
            || !settings.cloud_transcription_disclosure_acknowledged
        {
            return Err(
                "Turn on Cloud Transcription in Settings → Meeting Recording and confirm the privacy disclosure first."
                    .into(),
            );
        }
        if self
            .scheduler
            .current()
            .is_some_and(|job| job.recording_id == id)
        {
            return Err("This recording is already being transcribed.".into());
        }
        let recording = self
            .database
            .recording(id)
            .map_err(String::from)?
            .ok_or("This recording no longer exists.")?;
        match recording.state {
            RecordingState::Recording => return Err(ACTIVE_REFUSAL.into()),
            RecordingState::Transcribing => {
                return Err("This recording is already being transcribed.".into())
            }
            RecordingState::Recorded
            | RecordingState::Interrupted
            | RecordingState::Transcribed
            | RecordingState::Failed => {}
        }
        if recording.audio_deleted || plan::plan(Path::new(&recording.audio_dir), true).is_empty() {
            return Err("This recording has no audio to transcribe.".into());
        }

        let previous_state = recording.state;
        let previous_error = recording.error.clone();
        let attendees = self.database.recording_attendees(id).unwrap_or_default();
        let headphones = recording
            .tracks
            .iter()
            .any(|t| t.track == Track::System && t.is_headphones == Some(true));
        let audio_dir = PathBuf::from(&recording.audio_dir);
        let system_start = plan::standing(&audio_dir, true, |_| true)
            .system_start_ms
            .map(|start| start as i64);

        self.database
            .set_recording_state(id, RecordingState::Transcribing, None)
            .map_err(String::from)?;
        self.notifier.recordings_changed(id);
        self.notifier.transcription_progress(id, 0);

        let result = super::transcribe::cloud::transcribe_recording_cloud(
            self.cloud.as_ref(),
            requests,
            super::transcribe::cloud::CloudJobInput {
                recording_id: id,
                title: &recording.title,
                attendees: &attendees,
                audio_dir: &audio_dir,
                provider,
                model,
                endpoint,
                api_key,
                language: settings.transcription_language.as_deref(),
                custom_vocabulary: &settings.custom_vocabulary,
                diarize,
                headphones,
            },
            None,
            |percent| self.notifier.transcription_progress(id, percent),
        );

        match result {
            Ok(rows) => {
                self.database
                    .save_transcript(
                        id,
                        &rows,
                        policy::CLOUD_TRANSCRIPTION_ENGINE,
                        model.trim(),
                        system_start,
                    )
                    .map_err(String::from)?;
                if settings.keep_audio != super::settings::KeepAudio::UntilTranscribed {
                    if let Err(error) =
                        super::transcribe::transcode::transcode_recording_dir(&audio_dir)
                    {
                        log::warn!(target: "recording", "transcoding completed recording: {error}");
                    }
                }
                self.notifier.recordings_changed(id);
                self.refresh_tray_now();
                self.sweep();
                self.recording_transcript(id)
            }
            Err(error) => {
                let message = match error {
                    super::transcribe::JobError::Failed(msg) => msg,
                    super::transcribe::JobError::Cancelled => {
                        "Cloud transcription was cancelled.".into()
                    }
                    super::transcribe::JobError::Interrupted => {
                        "Cloud transcription was interrupted.".into()
                    }
                };
                if previous_state == RecordingState::Transcribed {
                    let _ = self.database.set_recording_state(
                        id,
                        RecordingState::Transcribed,
                        previous_error.as_deref(),
                    );
                } else {
                    let _ = self.database.set_recording_state(
                        id,
                        RecordingState::Failed,
                        Some(&message),
                    );
                }
                self.notifier.recordings_changed(id);
                self.refresh_tray_now();
                Err(message)
            }
        }
    }

    pub fn transcription_models(&self) -> Result<Vec<TranscriptionModelStatus>, String> {
        self.refuse_unsupported()?;
        Ok(self.scheduler.models.statuses())
    }

    /// Starts a model download in the background. Returns the download's
    /// thread so tests can wait for it.
    pub fn download_transcription_model(
        &self,
        model: TranscriptionModelChoice,
    ) -> Result<std::thread::JoinHandle<()>, String> {
        self.refuse_unsupported()?;
        self.scheduler.models.start_download(model)
    }

    pub fn cancel_transcription_model_download(
        &self,
        model: TranscriptionModelChoice,
    ) -> Result<(), String> {
        self.refuse_unsupported()?;
        self.scheduler.models.cancel_download(model);
        Ok(())
    }

    pub fn delete_transcription_model(
        &self,
        model: TranscriptionModelChoice,
    ) -> Result<(), String> {
        self.refuse_unsupported()?;
        self.scheduler.check_model_unused(model)?;
        self.scheduler.models.delete(model)
    }

    /// Stops a running transcription (the app is quitting); its recording
    /// is transcribed again at the next launch.
    pub fn cancel_transcription(&self) {
        self.scheduler.cancel_all();
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
    impl TestClock {
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }
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
        fn refresh_tray(
            &self,
            status: &RecordingStatus,
            settings: &RecordingSettings,
            _: Option<&TrayTranscription>,
            _: Option<&TrayReview>,
            _: Option<&TrayOffer>,
            _: &[TrayMeeting],
            _: SystemTime,
            _: bool,
        ) {
            self.0
                .lock()
                .unwrap()
                .push(Call::Tray(status.state, settings.record_meetings));
        }
        fn transcription_progress(&self, _: &str, _: u8) {}
        fn models_changed(&self, _: &[TranscriptionModelStatus]) {}
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
            Self::with_backend_and_probe(FakeBackend::new(), Arc::new(offers::NullMicProbe))
        }

        fn with_backend(backend: FakeBackend) -> Self {
            Self::with_backend_and_probe(backend, Arc::new(offers::NullMicProbe))
        }

        fn with_backend_and_probe(
            backend: FakeBackend,
            mic_probe: Arc<dyn MicActivityProbe>,
        ) -> Self {
            let database = Arc::new(Database::open_memory());
            let clock = Arc::new(TestClock(Mutex::new(at("2026-11-27T10:00:00Z"))));
            let calls = Arc::new(Recorder::default());
            let root = TempDir::new();
            let runtime = RecordingRuntime::with_mic_probe(
                database.clone(),
                Arc::new(backend.clone()),
                clock.clone(),
                Arc::new(PlentyOfDisk(AtomicU64::new(u64::MAX))),
                root.path().to_path_buf(),
                calls.clone(),
                crate::recording::test_support::idle_transcription(root.path()),
                mic_probe,
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
                    attendees: &[],
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
            crate::recording::test_support::idle_transcription(root.path()),
        );
        assert_eq!(
            runtime.capabilities(),
            RecordingCapabilities {
                supported: false,
                system_audio: SystemAudioSupport::Unsupported,
                speech_analyzer: false,
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
                "systemAudio": { "kind": "needsMacos", "major": 14, "minor": 2 },
                "speechAnalyzer": h.runtime.capabilities().speech_analyzer,
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
                attendees: &[],
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
                attendees: &[],
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
                attendees: &[],
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

    struct FakeProbe(Mutex<offers::MicActivitySnapshot>);
    impl FakeProbe {
        fn set(&self, snap: offers::MicActivitySnapshot) {
            *self.0.lock().unwrap() = snap;
        }
    }
    impl MicActivityProbe for FakeProbe {
        fn snapshot(&self) -> offers::MicActivitySnapshot {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn offers_and_post_call_silence_nudge_update_across_ticks_without_stopping_recording() {
        let probe = Arc::new(FakeProbe(Mutex::new(offers::MicActivitySnapshot {
            mic_running: true,
            processes: vec![offers::ActiveMicProcess {
                pid: 4242,
                bundle_id: Some("us.zoom.xos".into()),
                name: Some("zoom.us".into()),
            }],
        })));
        let h = Harness::with_backend_and_probe(FakeBackend::new(), probe.clone());
        h.runtime
            .set_settings(RecordingSettings {
                record_meetings: true,
                offer_to_record: true,
                ..Default::default()
            })
            .unwrap();

        // First tick starts the delay clock: no unscheduled offer yet.
        h.runtime.tick();
        assert_eq!(h.runtime.current_offer(), None);

        // Past OFFER_MIC_DELAY: raises badged "Zoom Call" offer.
        h.advance(policy::OFFER_MIC_DELAY);
        h.runtime.tick();
        let offer = h.runtime.current_offer().unwrap();
        assert_eq!(offer.title, "Zoom Call");
        assert!(offer.badged);

        // Starting the recording transitions into recording phase.
        h.runtime.start(None, &offer.title).unwrap();
        h.runtime.tick();
        assert!(h.runtime.is_active());
        assert!(!h.runtime.status().still_recording_nudge);

        // Call releases mic; track stays silent for 3 minutes -> nudge rises, recording keeps running!
        probe.set(offers::MicActivitySnapshot::default());
        h.runtime.tick();
        h.advance(policy::STILL_RECORDING_SILENCE);
        h.runtime.tick();
        assert!(h.runtime.status().still_recording_nudge);
        assert!(h.runtime.is_active());

        // Turning offer_to_record off clears the nudge immediately.
        h.runtime
            .set_settings(RecordingSettings {
                record_meetings: true,
                offer_to_record: false,
                ..Default::default()
            })
            .unwrap();
        assert!(!h.runtime.status().still_recording_nudge);
        assert!(h.runtime.is_active());
    }

    #[test]
    fn cloud_transcribe_requires_opt_in_and_preserves_prior_transcript_on_failure() {
        use crate::recording::transcribe::cloud::{
            CloudChunkRequest, CloudChunkResponse, CloudSegmentLine,
        };
        use std::sync::atomic::AtomicBool;

        struct ScriptedCloud(AtomicBool);
        impl CloudTranscriptionBackend for ScriptedCloud {
            fn transcribe_chunk(
                &self,
                req: &CloudChunkRequest,
            ) -> Result<CloudChunkResponse, String> {
                if self.0.load(Ordering::SeqCst) {
                    return Err("OpenRouter 503".into());
                }
                Ok(CloudChunkResponse {
                    lines: vec![CloudSegmentLine {
                        start_ms: 0,
                        end_ms: 900,
                        text: match req.track {
                            Track::Mic => "Cloud mic line".into(),
                            Track::System => "Cloud remote line".into(),
                        },
                        speaker_label: (req.track == Track::System).then(|| "Speaker 1".into()),
                    }],
                    cost_usd: Some(0.001),
                    input_tokens: 0,
                    output_tokens: 0,
                    raw_body: "{}".into(),
                })
            }
        }

        let database = Arc::new(Database::open_memory());
        let clock = Arc::new(TestClock(Mutex::new(at("2026-11-27T10:00:00Z"))));
        let calls = Arc::new(Recorder::default());
        let root = TempDir::new();
        let mut parts = crate::recording::test_support::idle_transcription(root.path());
        let cloud_script = Arc::new(ScriptedCloud(AtomicBool::new(true)));
        parts.cloud = cloud_script.clone();
        let runtime = RecordingRuntime::with_mic_probe(
            database.clone(),
            Arc::new(FakeBackend::new()),
            clock,
            Arc::new(PlentyOfDisk(AtomicU64::new(u64::MAX))),
            root.path().to_path_buf(),
            calls,
            parts,
            Arc::new(offers::NullMicProbe),
        );

        let dir = root.path().join("rec-cloud");
        database
            .create_recording(&crate::db::recordings::NewRecording {
                id: "rec-cloud",
                event_ref: None,
                title: "Design Sync",
                attendees: &[],
                started_at: "2026-11-27T10:00:00Z",
                audio_dir: &dir.to_string_lossy(),
                tracks: &[
                    (Track::Mic, crate::db::recordings::TrackState::Active),
                    (Track::System, crate::db::recordings::TrackState::Active),
                ],
            })
            .unwrap();
        let mut mic = SegmentWriter::open(&dir, Track::Mic).unwrap();
        mic.write(&vec![2_000; 16_000], 0).unwrap();
        drop(mic);
        let mut sys = SegmentWriter::open(&dir, Track::System).unwrap();
        sys.write(&vec![2_000; 16_000], 0).unwrap();
        drop(sys);
        database
            .save_transcript(
                "rec-cloud",
                &[crate::db::recordings::TranscriptRow {
                    track: Track::Mic,
                    start_ms: 0,
                    end_ms: 900,
                    text: "Existing local line".into(),
                    rms: Some(0.1),
                    dropped_as_echo: false,
                    speaker_label: None,
                }],
                "speech_analyzer",
                "apple-on-device",
                Some(0),
            )
            .unwrap();

        let requests = crate::ai::requests::Runtime::new();

        // Refused without opt-in and disclosure acknowledgment.
        let gate_err = runtime
            .cloud_transcribe_recording_with_requests(
                "rec-cloud",
                crate::ai::AiProvider::OpenRouter,
                policy::CLOUD_DEFAULT_MODEL_OPENROUTER,
                None,
                true,
                "key",
                &requests,
            )
            .unwrap_err();
        assert!(gate_err.contains("Cloud Transcription"));

        runtime
            .set_settings(RecordingSettings {
                cloud_transcription_enabled: true,
                cloud_transcription_disclosure_acknowledged: true,
                ..Default::default()
            })
            .unwrap();

        // When cloud fails on an already-transcribed recording, its prior transcript stays intact.
        let fail_err = runtime
            .cloud_transcribe_recording_with_requests(
                "rec-cloud",
                crate::ai::AiProvider::OpenRouter,
                policy::CLOUD_DEFAULT_MODEL_OPENROUTER,
                None,
                true,
                "key",
                &requests,
            )
            .unwrap_err();
        assert!(fail_err.contains("OpenRouter 503"));
        assert_eq!(
            database.recording("rec-cloud").unwrap().unwrap().state,
            RecordingState::Transcribed
        );
        assert_eq!(
            runtime.recording_transcript("rec-cloud").unwrap().lines[0].text,
            "Existing local line"
        );

        // Once cloud succeeds, transcript is replaced and remote speaker labels are kept.
        cloud_script.0.store(false, Ordering::SeqCst);
        let transcript = runtime
            .cloud_transcribe_recording_with_requests(
                "rec-cloud",
                crate::ai::AiProvider::OpenRouter,
                policy::CLOUD_DEFAULT_MODEL_OPENROUTER,
                None,
                true,
                "key",
                &requests,
            )
            .unwrap();
        assert!(transcript.speaker_labels);
        assert_eq!(transcript.lines.len(), 2);
        assert_eq!(transcript.lines[0].speaker, Some(TranscriptSpeaker::You));
        assert_eq!(transcript.lines[0].speaker_label, None);
        assert_eq!(transcript.lines[1].speaker, Some(TranscriptSpeaker::Others));
        assert_eq!(
            transcript.lines[1].speaker_label.as_deref(),
            Some("Speaker 1")
        );
    }

    #[test]
    fn suggest_vocabulary_reads_sent_mail_without_saving_automatically() {
        let h = Harness::new();
        h.database
            .test_with_connection(|c| {
                for (tid, mid, body) in [
                    ("t1", "m1", "Shipping CoreAudio update."),
                    ("t2", "m2", "Verifying CoreAudio behavior."),
                    ("t3", "m3", "Documenting CoreAudio flags."),
                ] {
                    c.execute(
                        "INSERT INTO threads(id,provider_thread_id,subject,snippet,participants_json,last_message_at,labels_json,account_id)
                         VALUES(?1,?1,'Sync','','[]','2026-11-27T10:00:00Z','[]','me@example.com')",
                        [tid],
                    )?;
                    c.execute(
                        "INSERT INTO messages(id,thread_id,sender,recipients_json,sent_at,body_html,body_text)
                         VALUES(?1,?2,'me@example.com','[]','2026-11-27T10:00:00Z','',?3)",
                        [mid, tid, body],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let suggested = h.runtime.suggest_vocabulary().unwrap();
        assert_eq!(suggested.len(), 1);
        assert_eq!(suggested[0].term, "CoreAudio");
        assert!(
            h.runtime.settings().custom_vocabulary.is_empty(),
            "suggestions must stay un-saved until the user selects them"
        );
    }

    #[test]
    fn upcoming_meetings_keep_tray_hiding_refresh_on_invalidate_and_tick_per_minute() {
        let h = Harness::new();
        h.database
            .adopt_calendar_account("you@example.com")
            .unwrap();

        // Recording is OFF, default settings have show_upcoming_meetings_in_tray = true.
        assert!(!h.runtime.hide_on_close());
        h.runtime.tick();
        h.calls.take();

        // Next tick within the same minute with no meetings does not refresh the tray.
        h.clock.advance(Duration::from_secs(10));
        h.runtime.tick();
        assert!(h.calls.take().is_empty());

        let event: crate::models::ScheduleEvent = serde_json::from_value(serde_json::json!({
            "id": "m1",
            "seriesId": null,
            "accountId": "you@example.com",
            "calendarId": "work",
            "title": "Product Sync",
            "start": "2026-11-27T10:20:00Z",
            "end": "2026-11-27T10:50:00Z",
            "allDay": false,
            "attendees": ["ann@example.org"],
            "attendeeResponses": {},
            "responseStatus": "accepted",
            "canRespond": true,
            "canEdit": false,
            "conferenceUrl": "https://meet.google.com/abc-defg-hij"
        }))
        .unwrap();
        h.database
            .cache_note_events("you@example.com", &[], "", "", &[event])
            .unwrap();

        // Invalidating calendar cache updates snapshot, hide_on_close, and refreshes tray immediately.
        h.runtime.invalidate_calendar_offers();
        assert_eq!(
            h.calls.take(),
            vec![Call::Tray(RecordingPhase::Idle, false)]
        );
        assert!(h.runtime.hide_on_close());
        let first = h.runtime.upcoming_meeting(0).unwrap();
        assert_eq!(first.title, "Product Sync");
        assert_eq!(
            first.join_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );

        // Same minute: no redundant refresh.
        h.clock.advance(Duration::from_secs(15));
        h.runtime.tick();
        assert!(h.calls.take().is_empty());

        // Minute rollover: refreshes tray countdown.
        h.clock.advance(Duration::from_secs(60));
        h.runtime.tick();
        assert_eq!(
            h.calls.take(),
            vec![Call::Tray(RecordingPhase::Idle, false)]
        );

        // Disabling show_upcoming_meetings_in_tray stops hide_on_close and clears snapshot.
        h.runtime
            .set_settings(RecordingSettings {
                show_upcoming_meetings_in_tray: false,
                ..Default::default()
            })
            .unwrap();
        assert!(!h.runtime.hide_on_close());
        assert_eq!(h.runtime.upcoming_meeting(0), None);
    }
}
