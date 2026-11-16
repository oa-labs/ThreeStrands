//! The day's plan in its daily note: read it with each item's state, and add
//! or remove items. Writes are version-checked like other daily-note edits.
use tauri::State;

use crate::daily_plan::PlanAddition;
use crate::db::daily_plan::{DailyPlan, PlanSave};
use crate::{run_database_task, AppResult, AppState};

#[tauri::command]
pub(crate) async fn daily_plan(date: String, state: State<'_, AppState>) -> AppResult<DailyPlan> {
    let database = state.database.clone();
    run_database_task(move || database.daily_plan(&date)).await
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
