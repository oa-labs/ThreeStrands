//! Explicit delegated-work commands; setup, credentials and grants stay native.
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResearchSettings {
    config: crate::agents::research::ResearchConfig,
    policy: serde_json::Value,
    model_key_configured: bool,
    search_key_configured: bool,
}
#[tauri::command]
pub(crate) async fn agent_research_settings(
    state: State<'_, AppState>,
) -> AppResult<ResearchSettings> {
    let db = state.database.clone();
    tokio::task::spawn_blocking(move || {
        Ok(ResearchSettings {
            config: db.research_config()?,
            policy: crate::agents::policy::client_contract(),
            model_key_configured: crate::agents::service::key("model")?.is_some(),
            search_key_configured: crate::agents::service::key("search")?.is_some(),
        })
    })
    .await
    .map_err(|_| crate::AppError::from("Research setup interrupted".to_string()))?
}
#[tauri::command]
pub(crate) async fn set_agent_research_settings(
    config: crate::agents::research::ResearchConfig,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let _setup = crate::agents::service::SETUP_LOCK.lock().await;
    let db = state.database.clone();
    run_database_task(move || {
        db.set_research_config(&config, false, chrono::Utc::now().timestamp())
    })
    .await
}
#[tauri::command]
pub(crate) async fn set_agent_research_key(
    provider: String,
    key: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let _setup = crate::agents::service::SETUP_LOCK.lock().await;
    let db = state.database.clone();
    tokio::task::spawn_blocking(move || {
        // Revoke in-flight permission before touching the keychain.
        if provider == "model" {
            db.revoke_research_model_check()?;
        }
        let cfg = db.research_config()?;
        db.set_research_config(&cfg, false, chrono::Utc::now().timestamp())?;
        crate::agents::service::set_key(&provider, &key).map_err(crate::AppError::from)
    })
    .await
    .map_err(|_| crate::AppError::from("Research setup interrupted".to_string()))?
}
#[tauri::command]
pub(crate) async fn check_agent_research_connection(state: State<'_, AppState>) -> AppResult<()> {
    let (cfg, keys) = {
        let _setup = crate::agents::service::SETUP_LOCK.lock().await;
        (
            state.database.research_config()?,
            crate::agents::service::keys().await?,
        )
    };
    if !cfg.search_enabled {
        return Err("Configure Exa before checking connections"
            .to_string()
            .into());
    }
    crate::agents::service::check_connection(&keys.0,&keys.1).await.map_err(|_|crate::AppError::from("Research connection check failed; verify model availability, keys, credits and account access".to_string()))?;
    // Snapshot comparison and certification cannot interleave with key/setup writes.
    let _setup = crate::agents::service::SETUP_LOCK.lock().await;
    // An edit while the check ran cannot certify a different configuration.
    let current = state.database.research_config()?;
    if serde_json::to_value(&cfg).ok() != serde_json::to_value(&current).ok() {
        return Err("Research setup changed during the check".to_string().into());
    }
    let current_keys = crate::agents::service::keys().await?;
    if keys != current_keys {
        return Err("Research keys changed during the check".to_string().into());
    }
    state
        .database
        .set_research_config(&cfg, true, chrono::Utc::now().timestamp())?;
    Ok(())
}
#[tauri::command]
pub(crate) async fn create_agent_research_task(
    request: crate::agents::research::ResearchRequest,
    state: State<'_, AppState>,
) -> AppResult<AgentDetail> {
    crate::agents::service::keys().await?;
    let db = state.database.clone();
    run_database_task(move || db.create_research_task(&request, chrono::Utc::now().timestamp()))
        .await
}
#[tauri::command]
pub(crate) async fn open_agent_research_source(
    id: String,
    source_id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let d = state.database.get_agent_task(&id)?;
    let s = d
        .research
        .as_ref()
        .and_then(|w| w.sources.iter().find(|s| s.id == source_id))
        .ok_or_else(|| crate::AppError::from("Source unavailable".to_string()))?;
    let url = crate::agents::transport::validate_url(&s.url)
        .map_err(|_| crate::AppError::from("Source URL is unavailable".to_string()))?;
    // Only source IDs retrieved by native tools can open; no model URL is executable.
    tokio::task::spawn_blocking(move || {
        open::that(url.as_str())
            .map_err(|_| crate::AppError::from("Could not open source".to_string()))
    })
    .await
    .map_err(|_| crate::AppError::from("Could not open source".to_string()))?
}
