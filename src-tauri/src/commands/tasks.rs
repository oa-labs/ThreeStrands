//! Tasks and goals.

use crate::models::{
    CreateGoalRequest, CreateTaskRequest, Goal, ThreadTask, UpdateGoalRequest, UpdateTaskRequest,
};
use tauri::State;

use crate::{kick_replicated_sync, record_synced_value, run_database_task, AppResult, AppState};

#[tauri::command]
pub(crate) async fn list_tasks(
    account_id: Option<String>,
    status: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<ThreadTask>> {
    let database = state.database.clone();
    run_database_task(move || database.list_tasks(account_id.as_deref(), status.as_deref())).await
}

#[tauri::command(async)]
pub(crate) fn create_task(request: CreateTaskRequest, state: State<'_, AppState>) -> AppResult<ThreadTask> {
    let task = state.database.create_task(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task, None)?;
    Ok(task)
}

#[tauri::command(async)]
pub(crate) fn update_task(request: UpdateTaskRequest, state: State<'_, AppState>) -> AppResult<ThreadTask> {
    let mut fields = std::collections::BTreeSet::new();
    if request.title.is_some() { fields.insert("title".to_string()); }
    if request.notes.is_some() { fields.insert("notes".to_string()); }
    if request.kind.is_some() { fields.insert("kind".to_string()); }
    if request.due_kind.is_some() { fields.insert("dueKind".to_string()); }
    if request.due_value.is_some() { fields.insert("dueValue".to_string()); }
    if request.time_zone.is_some() { fields.insert("timeZone".to_string()); }
    if request.repeat_interval_days.is_some() { fields.insert("repeatIntervalDays".to_string()); }
    if request.goal_id.is_some() { fields.insert("goalId".to_string()); }
    fields.insert("updatedAt".to_string());
    let task = state.database.update_task(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task, Some(fields))?;
    Ok(task)
}

#[tauri::command]
pub(crate) async fn list_goals(account_id: Option<String>, state: State<'_, AppState>) -> AppResult<Vec<Goal>> {
    let database = state.database.clone();
    run_database_task(move || database.list_goals(account_id.as_deref())).await
}

#[tauri::command(async)]
pub(crate) fn create_goal(request: CreateGoalRequest, state: State<'_, AppState>) -> AppResult<Goal> {
    let goal = state.database.create_goal(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Goal, &goal.id, &goal, None)?;
    Ok(goal)
}

#[tauri::command(async)]
pub(crate) fn update_goal(request: UpdateGoalRequest, state: State<'_, AppState>) -> AppResult<Goal> {
    let mut fields = std::collections::BTreeSet::from(["updatedAt".to_string()]);
    if request.title.is_some() { fields.insert("title".to_string()); }
    if request.notes.is_some() { fields.insert("notes".to_string()); }
    if request.horizon.is_some() { fields.insert("horizon".to_string()); }
    if request.period.is_some() { fields.insert("period".to_string()); }
    if request.status.is_some() { fields.extend(["status".to_string(), "closedAt".to_string()]); }
    if request.parent_goal_id.is_some() { fields.insert("parentGoalId".to_string()); }
    let goal = state.database.update_goal(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Goal, &goal.id, &goal, Some(fields))?;
    Ok(goal)
}

#[tauri::command(async)]
pub(crate) fn delete_goal(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let deletion = state.database.delete_goal(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::Goal, &id)?;
    for task in &deletion.tasks {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, task,
            Some(std::collections::BTreeSet::from(["goalId".to_string(), "updatedAt".to_string()])))?;
    }
    for goal in &deletion.children {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::Goal, &goal.id, goal,
            Some(std::collections::BTreeSet::from(["parentGoalId".to_string(), "updatedAt".to_string()])))?;
    }
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn set_task_status(
    id: String,
    status: String,
    source: String,
    state: State<'_, AppState>,
) -> AppResult<ThreadTask> {
    let task = state.database.set_task_status(&id, &status, &source)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task,
        Some(std::collections::BTreeSet::from(["status".to_string(), "completionSource".to_string(), "completedAt".to_string(), "updatedAt".to_string()])))?;
    Ok(task)
}

#[tauri::command(async)]
pub(crate) fn record_follow_up(id: String, state: State<'_, AppState>) -> AppResult<ThreadTask> {
    let task = state.database.record_follow_up(&id)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task,
        Some(std::collections::BTreeSet::from(["dueValue".to_string(), "waitAfter".to_string(), "completionSource".to_string(), "completedAt".to_string(), "updatedAt".to_string()])))?;
    Ok(task)
}

#[tauri::command(async)]
pub(crate) fn reconcile_tasks(state: State<'_, AppState>) -> AppResult<usize> {
    let count = state.database.reconcile_waiting_tasks()?;
    if count > 0 {
        for task in state.database.list_tasks(None, None)? {
            record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task,
                Some(std::collections::BTreeSet::from(["status".to_string(), "completionSource".to_string(), "completedAt".to_string(), "updatedAt".to_string()])))?;
        }
    }
    Ok(count)
}
