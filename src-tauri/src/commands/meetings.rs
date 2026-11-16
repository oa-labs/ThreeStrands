//! Meeting preparation: local, read-only context for one meeting and counts
//! for a day's meetings. Nothing here writes, sends, or calls a provider; see
//! `db/meetings.rs` for what each result contains and how it is bounded.
use tauri::State;

use crate::db::meetings::{MeetingContext, MeetingReadiness};
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
