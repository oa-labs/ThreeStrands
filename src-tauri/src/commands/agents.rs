//! Explicit local fixture-workflow commands. No external services in phase 1.
use crate::agents::{AgentDetail, CreateAgentTask};
use crate::{run_database_task, AppResult, AppState};
use tauri::State;

#[tauri::command]
pub(crate) async fn agent_runner_enabled(state: State<'_, AppState>) -> AppResult<bool> {
    let db = state.database.clone();
    run_database_task(move || db.agent_runner_enabled()).await
}
#[tauri::command]
pub(crate) async fn set_agent_runner_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let db = state.database.clone();
    run_database_task(move || db.set_agent_runner_enabled(enabled, chrono::Utc::now().timestamp()))
        .await
}
#[tauri::command]
pub(crate) async fn create_agent_task(
    request: CreateAgentTask,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    let db = state.database.clone();
    run_database_task(move || db.create_agent_task(&request, chrono::Utc::now().timestamp())).await
}
#[tauri::command]
pub(crate) async fn get_agent_task(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    let db = state.database.clone();
    run_database_task(move || db.get_agent_task(&id)).await
}
#[tauri::command]
pub(crate) async fn list_agent_tasks(
    account_id: Option<String>,
    include_archived: bool,
    state: State<'_, AppState>,
) -> AppResult<Vec<AgentDetail>> {
    let db = state.database.clone();
    run_database_task(move || db.list_agent_tasks(account_id.as_deref(), include_archived)).await
}
#[tauri::command]
pub(crate) async fn control_agent_task(
    id: String,
    action: String,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    let db = state.database.clone();
    run_database_task(move || db.control_agent_task(&id, &action, chrono::Utc::now().timestamp()))
        .await
}
#[tauri::command]
pub(crate) async fn answer_agent_question(
    id: String,
    question_id: String,
    answer: String,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    let db = state.database.clone();
    run_database_task(move || {
        db.answer_agent_question(&id, &question_id, &answer, chrono::Utc::now().timestamp())
    })
    .await
}
#[tauri::command]
pub(crate) async fn delete_agent_task(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let db = state.database.clone();
    run_database_task(move || db.delete_agent_task(&id)).await
}

#[tauri::command]
pub(crate) async fn interact_agent_task(
    id: String,
    message: String,
    mode: String,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    let db = state.database.clone();
    run_database_task(move || {
        db.interact_agent_task(&id, &message, &mode, chrono::Utc::now().timestamp())
    })
    .await
}
#[tauri::command]
pub(crate) async fn link_agent_task(
    id: String,
    linked_task_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    let db = state.database.clone();
    run_database_task(move || db.link_agent_task(&id, linked_task_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn mark_agent_task_read(
    id: String,
    generation: i64,
    turn_count: usize,
    artifact_count: usize,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    let db = state.database.clone();
    run_database_task(move || db.mark_agent_task_read(&id, generation, turn_count, artifact_count))
        .await
}
