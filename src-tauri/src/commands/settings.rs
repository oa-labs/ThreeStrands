//! Settings transfer, split inboxes, sender rules, signatures, snippets, and retention.

use crate::models::{
    CreateSnippetRequest, CreateSplitInboxRequest, Snippet, SplitInbox, UpdateSnippetRequest,
    UpdateSplitInboxRequest,
};
use tauri::State;

use crate::{kick_replicated_sync, record_synced_value, run_database_task, AppError, AppResult, AppState};
use crate::{models, signatures, transfer};

#[tauri::command]
pub(crate) fn export_settings(
    preferences: transfer::TransferPreferences,
    password: String,
    state: State<'_, AppState>,
) -> AppResult<Option<String>> {
    transfer::export(&state.database, preferences, &password)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn preview_settings_import(
    password: String,
    state: State<'_, AppState>,
) -> AppResult<Option<transfer::ImportPreview>> {
    transfer::preview_import(&state.database, &state.staged_import, &password)
        .map_err(AppError::from)
}

#[tauri::command(async)]
pub(crate) fn apply_settings_import(
    not_here: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<transfer::ImportResult> {
    transfer::apply_staged_import(&state.database, &state.staged_import, &not_here)
        .map_err(AppError::from)
}

#[tauri::command(async)]
pub(crate) fn cancel_settings_import(state: State<'_, AppState>) -> AppResult<()> {
    transfer::cancel_staged_import(&state.staged_import)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn list_split_inboxes(state: State<'_, AppState>) -> AppResult<Vec<SplitInbox>> {
    let database = state.database.clone();
    run_database_task(move || database.list_split_inboxes()).await
}

#[tauri::command(async)]
pub(crate) fn create_split_inbox(
    request: CreateSplitInboxRequest,
    state: State<'_, AppState>,
) -> AppResult<SplitInbox> {
    let item = state.database.create_split_inbox(
        &request.name,
        &request.match_kind,
        &request.match_value,
        &request.account_id,
    )?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item, None)?;
    Ok(item)
}

#[tauri::command(async)]
pub(crate) fn update_split_inbox(
    request: UpdateSplitInboxRequest,
    state: State<'_, AppState>,
) -> AppResult<SplitInbox> {
    let item = state.database.update_split_inbox(&request.id, &request.name)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item,
        Some(std::collections::BTreeSet::from(["name".to_string()])))?;
    Ok(item)
}

#[tauri::command(async)]
pub(crate) fn delete_split_inbox(id: String, state: State<'_, AppState>) -> AppResult<()> {
    state.database.delete_split_inbox(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::SplitInbox, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn reorder_split_inboxes(ids: Vec<String>, state: State<'_, AppState>) -> AppResult<()> {
    state.database.reorder_split_inboxes(&ids)?;
    for item in state.database.list_split_inboxes()? {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item,
            Some(std::collections::BTreeSet::from(["sortOrder".to_string()])))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_sender_rules(state: State<'_, AppState>) -> AppResult<Vec<models::SenderRule>> {
    let database = state.database.clone();
    run_database_task(move || database.list_sender_rules()).await
}

/// Mutes a sender in one account and archives the listed Inbox
/// conversations from them; sync delivers both to the provider.
#[tauri::command(async)]
pub(crate) fn mute_sender(
    account_id: String,
    match_kind: String,
    match_value: String,
    archive_thread_ids: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<models::MuteResult> {
    let result = state.database.mute_sender(&account_id, &match_kind, &match_value, &archive_thread_ids)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::SenderRule, &result.rule.id, &result.rule, None)?;
    Ok(result)
}

#[tauri::command(async)]
pub(crate) fn delete_sender_rule(id: String, state: State<'_, AppState>) -> AppResult<()> {
    state.database.delete_sender_rule(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::SenderRule, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn signature_library(state: State<'_, AppState>) -> AppResult<signatures::SignatureLibrary> {
    let database = state.database.clone();
    run_database_task(move || database.signature_library()).await
}

#[tauri::command(async)]
pub(crate) fn save_signature(signature: signatures::Signature, state: State<'_, AppState>) -> AppResult<()> {
    state.database.store_signature(&signature)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn delete_signature(id: String, state: State<'_, AppState>) -> AppResult<()> {
    state.database.delete_signature(&id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn save_signature_defaults(mut defaults: signatures::SignatureDefaults, state: State<'_, AppState>) -> AppResult<()> {
    defaults.id = signatures::SignatureDefaults::key(&defaults.account, &defaults.address);
    state.database.store_signature_defaults(&defaults)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn delete_signature_defaults(account: String, address: String, state: State<'_, AppState>) -> AppResult<()> {
    state.database.delete_signature_defaults(&signatures::SignatureDefaults::key(&account, &address))?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_snippets(state: State<'_, AppState>) -> AppResult<Vec<Snippet>> {
    let database = state.database.clone();
    run_database_task(move || database.list_snippets()).await
}

#[tauri::command(async)]
pub(crate) fn create_snippet(
    request: CreateSnippetRequest,
    state: State<'_, AppState>,
) -> AppResult<Snippet> {
    let item = state.database.create_snippet(&request.name, &request.body)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Snippet, &item.id, &item, None)?;
    Ok(item)
}

#[tauri::command(async)]
pub(crate) fn update_snippet(
    request: UpdateSnippetRequest,
    state: State<'_, AppState>,
) -> AppResult<Snippet> {
    let item = state
        .database
        .update_snippet(&request.id, &request.name, &request.body)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Snippet, &item.id, &item,
        Some(std::collections::BTreeSet::from(["name".to_string(), "body".to_string()])))?;
    Ok(item)
}

#[tauri::command(async)]
pub(crate) fn delete_snippet(id: String, state: State<'_, AppState>) -> AppResult<()> {
    state.database.delete_snippet(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::Snippet, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn get_retention_days(state: State<'_, AppState>) -> AppResult<Option<i64>> {
    let database = state.database.clone();
    run_database_task(move || database.retention_days()).await
}

#[tauri::command(async)]
pub(crate) fn set_retention_days(days: Option<i64>, state: State<'_, AppState>) -> AppResult<()> {
    state.database.set_retention_days(days)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Retention, "mail",
        &serde_json::json!({"days":days}), None)
}
