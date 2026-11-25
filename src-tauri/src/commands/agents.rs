//! Tauri commands for standing Inbox Assistants.

use tauri::State;

use crate::agents::{
    AgentId, AiRunConfig, InboxAgentConfig, InboxAgentsSnapshot, ProposalState,
    UpdateInboxAgentConfig,
};
use crate::{run_database_task, AppError, AppResult, AppState};

#[tauri::command]
pub(crate) async fn get_inbox_agents_snapshot(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<InboxAgentsSnapshot> {
    let db = state.database.clone();
    run_database_task(move || db.inbox_agents_snapshot(account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn set_inbox_agent_runner_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> AppResult<InboxAgentsSnapshot> {
    let db = state.database.clone();
    let runner = state.agent_runner.clone();
    let snapshot = run_database_task(move || {
        db.set_inbox_agent_runner_enabled(enabled, chrono::Utc::now().timestamp())?;
        db.inbox_agents_snapshot(None)
    })
    .await?;
    if enabled {
        runner.wake();
    }
    Ok(snapshot)
}

#[tauri::command]
pub(crate) async fn update_inbox_agent_config(
    config: UpdateInboxAgentConfig,
    state: State<'_, AppState>,
) -> AppResult<InboxAgentConfig> {
    let db = state.database.clone();
    let runner = state.agent_runner.clone();
    let updated = run_database_task(move || {
        db.update_inbox_agent_config(&config, chrono::Utc::now().timestamp())
    })
    .await?;
    if updated.enabled && updated.schedule_hours.is_some() {
        runner.wake();
    }
    Ok(updated)
}

#[tauri::command]
pub(crate) async fn run_inbox_agent(
    agent_id: String,
    ai_config: Option<AiRunConfig>,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<InboxAgentsSnapshot> {
    let parsed = AgentId::parse(&agent_id)
        .ok_or_else(|| AppError::from(format!("Unknown assistant: {agent_id}")))?;
    let db = state.database.clone();
    let runner = state.agent_runner.clone();
    let snapshot = run_database_task(move || {
        db.enqueue_inbox_agent_run(
            parsed,
            ai_config.as_ref(),
            account_id.as_deref(),
            chrono::Utc::now().timestamp(),
        )?;
        db.inbox_agents_snapshot(account_id.as_deref())
    })
    .await?;
    runner.wake();
    Ok(snapshot)
}

#[tauri::command]
pub(crate) async fn run_enabled_inbox_agents(
    ai_config: Option<AiRunConfig>,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<InboxAgentsSnapshot> {
    let db = state.database.clone();
    let runner = state.agent_runner.clone();
    let snapshot = run_database_task(move || {
        db.enqueue_enabled_inbox_agent_runs(
            ai_config.as_ref(),
            account_id.as_deref(),
            chrono::Utc::now().timestamp(),
        )?;
        db.inbox_agents_snapshot(account_id.as_deref())
    })
    .await?;
    runner.wake();
    Ok(snapshot)
}

#[tauri::command]
pub(crate) async fn resolve_inbox_agent_proposal(
    id: String,
    state_value: ProposalState,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let db = state.database.clone();
    run_database_task(move || {
        db.resolve_inbox_agent_proposal(&id, state_value, chrono::Utc::now().timestamp())
    })
    .await
}

#[tauri::command]
pub(crate) async fn sync_inbox_agent_ai_config(
    ai_config: Option<AiRunConfig>,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let db = state.database.clone();
    run_database_task(move || db.save_inbox_agent_ai_config(ai_config.as_ref())).await
}
