//! Meeting Recording commands. Settings here are device-local: they are
//! stored in this device's database only and never enter settings transfer
//! or sync.
//!
//! Recording work can block (starting or stopping a track waits for its
//! capture thread; deleting removes files), so each command runs on the
//! blocking pool.
use tauri::{Emitter, State};

use crate::ai;
use crate::db::recordings::{MeetingRecording, RecordingState};
use crate::note_entities::NoteReference;
use crate::recording::capture::{AudioInputDevice, SystemAudioSupport, Track};
use crate::recording::permissions::{self, RecordingPermissions};
use crate::recording::policy;
use crate::recording::runtime::{RecordingCapabilities, RecordingsUsage};
use crate::recording::runtime::{RecordingTranscript, TranscriptSpeaker};
use crate::recording::settings::{RecordingSettings, TranscriptionModelChoice};
use crate::recording::transcribe::models::TranscriptionModelStatus;
use crate::recording::RecordingStatus;
use crate::{run_database_task, AppError, AppResult, AppState};

#[tauri::command]
pub(crate) async fn recording_settings(state: State<'_, AppState>) -> AppResult<RecordingSettings> {
    let database = state.database.clone();
    run_database_task(move || database.recording_settings()).await
}

/// Saves the settings and shows or hides the tray icon to match.
#[tauri::command]
pub(crate) async fn set_recording_settings(
    settings: RecordingSettings,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.set_settings(settings)).await
}

#[tauri::command]
pub(crate) async fn recording_capabilities(
    state: State<'_, AppState>,
) -> AppResult<RecordingCapabilities> {
    Ok(state.recording.capabilities())
}

#[tauri::command]
pub(crate) async fn recording_input_devices(
    state: State<'_, AppState>,
) -> AppResult<Vec<AudioInputDevice>> {
    let runtime = state.recording.clone();
    run_database_task(move || Ok::<_, String>(runtime.input_devices())).await
}

#[tauri::command]
pub(crate) async fn open_system_audio_settings() -> AppResult<()> {
    #[cfg(target_os = "macos")]
    {
        open::that(policy::SYSTEM_AUDIO_SETTINGS_URL).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn recording_status(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    Ok(state.recording.status())
}

#[tauri::command]
pub(crate) async fn recording_start(
    event_ref: Option<NoteReference>,
    title: String,
    attendees: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    let attendees = attendees.unwrap_or_default();
    run_database_task(move || runtime.start_meeting(event_ref, &title, &attendees)).await
}

#[tauri::command]
pub(crate) async fn recording_pause(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.pause()).await
}

#[tauri::command]
pub(crate) async fn recording_resume(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.resume()).await
}

#[tauri::command]
pub(crate) async fn recording_stop(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.stop()).await
}

#[tauri::command]
pub(crate) async fn list_recordings(
    event_ref: NoteReference,
    state: State<'_, AppState>,
) -> AppResult<Vec<MeetingRecording>> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.list_recordings(&event_ref)).await
}

#[tauri::command]
pub(crate) async fn delete_recording(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_recording(&id)).await
}

#[tauri::command]
pub(crate) async fn delete_recording_audio(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_recording_audio(&id)).await
}

#[tauri::command]
pub(crate) async fn recordings_usage(state: State<'_, AppState>) -> AppResult<RecordingsUsage> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.usage()).await
}

#[tauri::command]
pub(crate) async fn delete_all_recording_audio(state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_all_audio()).await
}

#[tauri::command]
pub(crate) async fn delete_all_recordings(state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_all_recordings()).await
}

/// Asks for microphone permission (waiting for the user's answer) and, where
/// system audio is available, makes macOS ask for it now too.
#[tauri::command]
pub(crate) async fn request_recording_permissions(
    state: State<'_, AppState>,
) -> AppResult<RecordingPermissions> {
    let system_audio = state.recording.capabilities().system_audio == SystemAudioSupport::Available;
    run_database_task(move || Ok::<_, String>(permissions::request(system_audio))).await
}

/// The user confirmed quitting during a recording: stop it, then continue
/// the usual exit handshake (`compose-before-exit`, then `finish_exit`).
#[tauri::command]
pub(crate) async fn recording_confirm_quit(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || {
        runtime.stop_if_active();
        Ok::<_, String>(())
    })
    .await?;
    let _ = app.emit("compose-before-exit", ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn transcription_models(
    state: State<'_, AppState>,
) -> AppResult<Vec<TranscriptionModelStatus>> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.transcription_models()).await
}

/// Starts the download in the background and returns.
#[tauri::command]
pub(crate) async fn download_transcription_model(
    model: TranscriptionModelChoice,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.download_transcription_model(model).map(drop)).await
}

#[tauri::command]
pub(crate) async fn cancel_transcription_model_download(
    model: TranscriptionModelChoice,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.cancel_transcription_model_download(model)).await
}

#[tauri::command]
pub(crate) async fn delete_transcription_model(
    model: TranscriptionModelChoice,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_transcription_model(model)).await
}

/// Queues a recording for transcription (Retry).
#[tauri::command]
pub(crate) async fn transcribe_recording(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.transcribe_recording(&id)).await
}

#[tauri::command]
pub(crate) async fn recording_transcript(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<RecordingTranscript> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.recording_transcript(&id)).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordingReviewContext {
    pub recording: MeetingRecording,
    pub recap: Option<ai::MeetingRecap>,
    pub transcript_bytes: usize,
    pub over_budget: bool,
    pub duration_ms: u64,
    pub has_user_notes: bool,
    pub notes_excluded_from_ai: bool,
}

#[tauri::command]
pub(crate) async fn latest_transcribed_recording(
    state: State<'_, AppState>,
) -> AppResult<Option<MeetingRecording>> {
    let database = state.database.clone();
    run_database_task(move || database.latest_transcribed_recording()).await
}

fn recap_segments_for(
    database: &crate::db::Database,
    recording: &MeetingRecording,
    range: Option<ai::RecapTimeRange>,
) -> Result<(bool, Vec<ai::RecapTranscriptSegment>), String> {
    let speaker_labels = recording
        .tracks
        .iter()
        .any(|track| track.track == Track::System && track.first_frame_offset_ms.is_some());
    let rows = database
        .transcript_rows(&recording.id)
        .map_err(|e| e.to_string())?;
    let segments = rows
        .into_iter()
        .filter(|row| !row.dropped_as_echo)
        .enumerate()
        .filter_map(|(index, row)| {
            let start_ms = row.start_ms.max(0) as u64;
            let end_ms = row.end_ms.max(row.start_ms).max(0) as u64;
            if let Some(r) = range {
                if start_ms < r.start_ms || start_ms > r.end_ms {
                    return None;
                }
            }
            let speaker = if speaker_labels {
                Some(match row.track {
                    Track::Mic => TranscriptSpeaker::You,
                    Track::System => TranscriptSpeaker::Others,
                })
            } else {
                None
            };
            Some(ai::RecapTranscriptSegment {
                id: index + 1,
                speaker,
                start_ms,
                end_ms,
                text: row.text,
            })
        })
        .collect();
    Ok((speaker_labels, segments))
}

#[tauri::command]
pub(crate) async fn recording_review_context(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<RecordingReviewContext> {
    let database = state.database.clone();
    run_database_task(move || -> Result<RecordingReviewContext, String> {
        let recording = database
            .recording(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "This recording no longer exists.".to_string())?;
        let recap = database
            .recording_recap_json(&id)
            .map_err(|e| e.to_string())?
            .and_then(|json| serde_json::from_str::<ai::MeetingRecap>(&json).ok());
        let (speaker_labels, segments) = recap_segments_for(&database, &recording, None)?;
        let raw_bytes: usize = segments.iter().map(|s| s.text.len()).sum();
        let over_budget = if segments.is_empty() {
            false
        } else {
            let probe = ai::MeetingRecapRequest {
                provider: ai::AiProvider::OpenAi,
                model: String::new(),
                endpoint: None,
                reasoning: ai::Reasoning::Default,
                title: recording.title.clone(),
                started_at: recording.started_at.clone(),
                duration_ms: recording.duration_ms.max(0) as u64,
                speaker_labels,
                attendees: database.recording_attendees(&id).unwrap_or_default(),
                own_addresses: Vec::new(),
                segments,
                user_notes: None,
                range: None,
            };
            ai::meeting_recap::build_recap_payload(&probe).is_err()
        };
        let (has_user_notes, notes_excluded_from_ai) = match recording.event_ref.as_ref() {
            Some(reference) => {
                let (notes, excluded) = database
                    .occurrence_notes_for_recap(reference)
                    .map_err(|e| e.to_string())?;
                (notes.is_some(), excluded)
            }
            None => (false, false),
        };
        Ok(RecordingReviewContext {
            duration_ms: recording.duration_ms.max(0) as u64,
            recording,
            recap,
            transcript_bytes: raw_bytes,
            over_budget,
            has_user_notes,
            notes_excluded_from_ai,
        })
    })
    .await
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn summarize_recording(
    id: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    reasoning: Option<ai::Reasoning>,
    include_my_notes: Option<bool>,
    range: Option<ai::RecapTimeRange>,
    control: Option<ai::requests::RequestControl>,
    state: State<'_, AppState>,
) -> AppResult<ai::MeetingRecap> {
    let api_key = ai::get_key()?.ok_or_else(|| AppError::from("No AI API key configured"))?;
    let database = state.database.clone();
    let rec_id = id.clone();
    let include_notes = include_my_notes.unwrap_or(false);
    let prepared = run_database_task(move || -> Result<ai::MeetingRecapRequest, String> {
        let recording = database
            .recording(&rec_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "This recording no longer exists.".to_string())?;
        if recording.state != RecordingState::Transcribed {
            return Err("This recording hasn't been transcribed yet.".into());
        }
        let (speaker_labels, segments) = recap_segments_for(&database, &recording, range)?;
        if segments.is_empty() {
            return Err("There are no transcript segments in that range to summarize.".into());
        }
        let mut attendees = database.recording_attendees(&rec_id).unwrap_or_default();
        if let Some(reference) = recording.event_ref.as_ref() {
            if let Ok(ctx) = database.meeting_context(reference, None, chrono::Utc::now()) {
                if let Some(ev) = ctx.event {
                    for attendee in ev.attendees {
                        if !attendees.iter().any(|a| a.eq_ignore_ascii_case(&attendee)) {
                            attendees.push(attendee);
                        }
                    }
                }
            }
        }
        let own_addresses = database
            .list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.email)
            .collect();
        let user_notes = if include_notes {
            match recording.event_ref.as_ref() {
                Some(reference) => {
                    database
                        .occurrence_notes_for_recap(reference)
                        .map_err(|e| e.to_string())?
                        .0
                }
                None => None,
            }
        } else {
            None
        };
        Ok(ai::MeetingRecapRequest {
            provider,
            model,
            endpoint,
            reasoning: reasoning.unwrap_or_default(),
            title: recording.title,
            started_at: recording.started_at,
            duration_ms: recording.duration_ms.max(0) as u64,
            speaker_labels,
            attendees,
            own_addresses,
            segments,
            user_notes,
            range,
        })
    })
    .await?;

    let mut purpose = ai::RequestPurpose::about(
        "meeting_recap",
        &prepared.title,
        Some(prepared.segments.len()),
    );
    purpose.control = control;
    let recap = ai::with_purpose(purpose, ai::summarize_meeting_recap(prepared, &api_key))
        .await
        .map_err(AppError::from)?;

    let database = state.database.clone();
    let rec_id = id.clone();
    let recap_to_save = recap.clone();
    run_database_task(move || -> Result<(), String> {
        let json = serde_json::to_string(&recap_to_save).map_err(|e| e.to_string())?;
        database
            .save_recording_recap(&rec_id, &json)
            .map_err(|e| e.to_string())
    })
    .await?;
    state.recording.notify_recordings_changed(&id);
    Ok(recap)
}
