//! The day's plan in its daily note: read it with each item's state, and add
//! or remove items. Writes are version-checked like other daily-note edits.
use tauri::State;

use crate::daily_plan::PlanAddition;
use crate::db::daily_plan::{DailyPlan, PlanSave};
use crate::db::plan_review::{PlanHistory, PlanReview};
use crate::db::plan_sources::{PlanSource, PlanSourcePage};
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

/// Carry counts and planned days for plan item keys over `from..=to`.
#[tauri::command]
pub(crate) async fn plan_history(
    targets: Vec<String>,
    from: String,
    to: String,
    state: State<'_, AppState>,
) -> AppResult<Vec<PlanHistory>> {
    let database = state.database.clone();
    run_database_task(move || database.plan_history(&targets, &from, &to)).await
}

/// The Done and Didn't get to columns for planning `date`.
#[tauri::command]
pub(crate) async fn plan_review(date: String, state: State<'_, AppState>) -> AppResult<PlanReview> {
    let database = state.database.clone();
    run_database_task(move || database.plan_review(&date)).await
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
