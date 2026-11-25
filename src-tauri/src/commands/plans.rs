//! The day's plan in its daily note: read it with each item's state, and add
//! or remove items. Writes are version-checked like other daily-note edits.
use tauri::State;

use crate::daily_plan::PlanAddition;
use crate::db::daily_plan::{DailyPlan, PlanSave};
use crate::db::plan_save::{PlanChanges, PlanSaveResult};
use crate::db::plan_sources::{PlanSource, PlanSourcePage};
use crate::models::Note;
use crate::{run_database_task, AppResult, AppState};

#[tauri::command]
pub(crate) async fn daily_plan(
    date: String,
    day_start: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<DailyPlan> {
    let database = state.database.clone();
    run_database_task(move || database.daily_plan_from(&date, day_start.as_deref())).await
}

#[tauri::command]
pub(crate) async fn add_to_daily_plan(
    date: String,
    expected: Option<String>,
    additions: Vec<PlanAddition>,
    state: State<'_, AppState>,
) -> AppResult<PlanSave> {
    let database = state.database.clone();
    run_database_task(move || database.add_to_daily_plan(&date, expected.as_deref(), &additions))
        .await
}

#[tauri::command]
pub(crate) async fn remove_from_daily_plan(
    date: String,
    expected: Option<String>,
    key: String,
    state: State<'_, AppState>,
) -> AppResult<PlanSave> {
    let database = state.database.clone();
    run_database_task(move || database.remove_from_daily_plan(&date, expected.as_deref(), &key))
        .await
}

/// Plan Today's save: every change to `date`'s plan in one guarded
/// transaction, with a local receipt under `operation_id`.
#[tauri::command]
pub(crate) async fn save_daily_plan(
    date: String,
    expected: Option<String>,
    operation_id: String,
    changes: PlanChanges,
    state: State<'_, AppState>,
) -> AppResult<PlanSaveResult> {
    let database = state.database.clone();
    run_database_task(move || {
        database.save_daily_plan(&date, expected.as_deref(), &operation_id, &changes)
    })
    .await
}

/// Undoes a plan save while the note is still as that save left it.
#[tauri::command]
pub(crate) async fn undo_daily_plan_save(
    date: String,
    operation_id: String,
    state: State<'_, AppState>,
) -> AppResult<Note> {
    let database = state.database.clone();
    run_database_task(move || database.undo_daily_plan_save(&date, &operation_id)).await
}

/// One page of plan candidates from a source.
#[tauri::command]
pub(crate) async fn plan_sources(
    date: String,
    source: PlanSource,
    account_id: Option<String>,
    cursor: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<PlanSourcePage> {
    let database = state.database.clone();
    run_database_task(move || {
        database.plan_sources(&date, source, account_id.as_deref(), cursor.as_deref())
    })
    .await
}
