//! Meeting preparation: local, read-only context for one meeting and counts
//! for a day's meetings. Nothing here writes, sends, or calls a provider; see
//! `db/meetings.rs` for what each result contains and how it is bounded.
use tauri::State;

use crate::db::meetings::{
    MeetingContext, MeetingPrepSelection, MeetingReadiness, MeetingSourcePreview,
};
use crate::note_entities::NoteReference;
use crate::{run_database_task, AppResult, AppState};

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
