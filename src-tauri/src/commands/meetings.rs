//! Meeting preparation: local, read-only context for one meeting and counts
//! for a day's meetings. Nothing here writes, sends, or calls a provider; see
//! `db/meetings.rs` for what each result contains and how it is bounded.
use tauri::State;

use crate::ai;
use crate::db::meeting_agenda::{MeetingAgendaSave, MeetingAgendaUndo};
use crate::db::meetings::{
    MeetingContext, MeetingPrepSelection, MeetingReadiness, MeetingSourcePreview,
};
use crate::note_entities::NoteReference;
use crate::note_meetings::MeetingEntry;
use crate::{kick_replicated_sync, run_database_task, AppError, AppResult, AppState};

#[tauri::command]
pub(crate) async fn meeting_context(
    reference: NoteReference,
    mail_scope: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> AppResult<MeetingContext> {
    let database = state.database.clone();
    run_database_task(move || {
        database.meeting_context(&reference, mail_scope.as_deref(), chrono::Utc::now())
    })
    .await
}

#[tauri::command]
pub(crate) async fn meeting_source_preview(
    reference: NoteReference,
    mail_scope: Option<Vec<String>>,
    selection: Option<MeetingPrepSelection>,
    state: State<'_, AppState>,
) -> AppResult<MeetingSourcePreview> {
    let database = state.database.clone();
    run_database_task(move || {
        database
            .meeting_source_preview(
                &reference,
                mail_scope.as_deref(),
                selection.as_ref(),
                chrono::Utc::now(),
            )
            .map(|(preview, _, _)| preview)
    })
    .await
}

#[tauri::command]
pub(crate) async fn prepare_meeting_brief(
    reference: NoteReference,
    mail_scope: Option<Vec<String>>,
    selection: Option<MeetingPrepSelection>,
    expected_payload_revision: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    reasoning: Option<ai::Reasoning>,
    control: Option<ai::requests::RequestControl>,
    state: State<'_, AppState>,
) -> AppResult<ai::MeetingPrepBrief> {
    let api_key = ai::get_key()?.ok_or_else(|| AppError::from("No AI API key configured"))?;
    let database = state.database.clone();
    let reference_for_db = reference.clone();
    let (preview, prepared, snapshot) = run_database_task(move || {
        database.meeting_source_preview(
            &reference_for_db,
            mail_scope.as_deref(),
            selection.as_ref(),
            chrono::Utc::now(),
        )
    })
    .await?;
    if preview.payload_revision != expected_payload_revision {
        return Err(AppError::from(
            "One or more sources changed since you reviewed them. Review the updated sources and try again.",
        ));
    }
    if preview.over_budget {
        return Err(AppError::from(
            "Selected sources exceed the preparation size budget. Deselect some sources and try again.",
        ));
    }
    let sources: Vec<ai::MeetingPrepSourceMeta> = preview
        .sources
        .iter()
        .filter(|item| item.selected && !item.excluded_by_note_setting)
        .map(|item| ai::MeetingPrepSourceMeta {
            source_id: item.source_id.clone(),
            kind: item.kind.clone(),
            id: item.id.clone(),
            title: item.title.clone(),
        })
        .collect();
    if sources.is_empty() {
        return Err(AppError::from(
            "Select at least one source before preparing an agenda.",
        ));
    }
    let mut purpose =
        ai::RequestPurpose::about("meeting_prep", &snapshot.title, Some(sources.len()));
    purpose.control = control;
    purpose.scope = prepared.manifest.scope.clone();
    let request = ai::MeetingPrepBriefRequest {
        provider,
        model,
        endpoint,
        reasoning: reasoning.unwrap_or_default(),
        payload_json: prepared.payload,
        payload_revision: preview.payload_revision,
        sources,
    };
    ai::with_purpose(purpose, ai::prepare_meeting_brief(request, &api_key))
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn save_meeting_agenda(
    reference: NoteReference,
    note_id: Option<String>,
    date: String,
    expected_revision: Option<String>,
    operation_id: String,
    entry: MeetingEntry,
    items: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<MeetingAgendaSave> {
    let database = state.database.clone();
    let saved = run_database_task(move || {
        database.save_meeting_agenda(
            &reference,
            note_id.as_deref(),
            &date,
            expected_revision.as_deref(),
            &operation_id,
            &entry,
            &items,
        )
    })
    .await?;
    if !saved.replayed {
        kick_replicated_sync(&state);
    }
    Ok(saved)
}

#[tauri::command]
pub(crate) async fn undo_meeting_agenda(
    operation_id: String,
    state: State<'_, AppState>,
) -> AppResult<MeetingAgendaUndo> {
    let database = state.database.clone();
    let undone = run_database_task(move || database.undo_meeting_agenda(&operation_id)).await?;
    kick_replicated_sync(&state);
    Ok(undone)
}

#[tauri::command]
pub(crate) async fn meeting_readiness(
    references: Vec<NoteReference>,
    mail_scope: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> AppResult<Vec<MeetingReadiness>> {
    let database = state.database.clone();
    run_database_task(move || {
        database.meeting_readiness(&references, mail_scope.as_deref(), chrono::Utc::now())
    })
    .await
}

/// Reserve Prep Time: a round prep task under the meeting's line in its
/// day's note, writing the line first when needed. Repeating it returns the
/// task already there.
#[tauri::command]
pub(crate) async fn reserve_meeting_prep(
    date: String,
    expected: Option<String>,
    entry: crate::note_meetings::MeetingEntry,
    state: State<'_, AppState>,
) -> AppResult<crate::db::note_entities::PrepReservation> {
    let database = state.database.clone();
    run_database_task(move || database.reserve_meeting_prep(&date, expected.as_deref(), &entry))
        .await
}

/// Review Follow-through: appends the accepted decisions and actions to one
/// of the occurrence's notes in a guarded transaction with a receipt, so a
/// retry with the same `operation_id` returns the first result.
#[tauri::command]
pub(crate) async fn save_meeting_follow_through(
    reference: NoteReference,
    destination: crate::db::meeting_follow_through::FollowThroughDestination,
    expected_revision: String,
    operation_id: String,
    accepted: crate::db::meeting_follow_through::AcceptedFollowThrough,
    state: State<'_, AppState>,
) -> AppResult<crate::db::meeting_follow_through::FollowThroughSave> {
    let database = state.database.clone();
    run_database_task(move || {
        database.save_meeting_follow_through(
            &reference,
            &destination,
            &expected_revision,
            &operation_id,
            &accepted,
        )
    })
    .await
}

/// Undo for a saved follow-through: removes exactly that section while the
/// note is unchanged since it was saved.
#[tauri::command]
pub(crate) async fn undo_meeting_follow_through(
    operation_id: String,
    state: State<'_, AppState>,
) -> AppResult<crate::db::meeting_follow_through::FollowThroughUndo> {
    let database = state.database.clone();
    run_database_task(move || database.undo_meeting_follow_through(&operation_id)).await
}
