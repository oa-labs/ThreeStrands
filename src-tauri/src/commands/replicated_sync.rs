//! Cross-device (replicated) sync, enrollment, and synced preferences.

use tauri::State;

use crate::{enrollment, ipfs_transport, replicated_sync, s3_transport, sync_connectors};
use crate::{kick_replicated_sync, run_database_task, AppState};

#[tauri::command]
pub(crate) async fn synced_preferences(state: State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    let database = state.database.clone();
    run_database_task(move || database.synced_preferences()).await
}

#[tauri::command(async)]
pub(crate) fn update_synced_preferences(preferences: serde_json::Value, state: State<'_, AppState>) -> Result<(), String> {
    if state.database.update_synced_preferences(preferences)? {
        kick_replicated_sync(&state);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn replicated_sync_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let database = state.database.clone();
    run_database_task(move || database.replicated_sync_active()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_beta_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let database = state.database.clone();
    run_database_task(move || database.beta_features_enabled()).await
}

#[tauri::command(async)]
pub(crate) fn replicated_sync_set_beta_enabled(on: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.database.set_beta_features_enabled(on)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn replicated_sync_status(
    state: State<'_, AppState>,
) -> Result<Vec<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    state.replicated_sync.status().await
}

#[tauri::command]
pub(crate) async fn replicated_sync_add_folder(
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    let Some(folder) = rfd::AsyncFileDialog::new()
        .set_title("Select a folder to sync through")
        .pick_folder()
        .await
    else {
        return Ok(None);
    };
    let instance_id = format!("folder-{}", uuid::Uuid::new_v4());
    state.database.add_folder_transport(&instance_id, folder.path())?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
pub(crate) async fn replicated_sync_add_ipfs_rpc(
    base_url: String,
    token: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    let instance_id = format!("ipfs-rpc-{}", uuid::Uuid::new_v4());
    state.database.add_ipfs_rpc_transport(&instance_id, &base_url, token.as_deref())?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
pub(crate) async fn replicated_sync_probe_s3(
    config: s3_transport::S3Config,
    credentials: s3_transport::S3Credentials,
    state: State<'_, AppState>,
) -> Result<replicated_sync::S3ConnectionTest, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.probe_s3(&config, &credentials).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_add_s3(
    config: s3_transport::S3Config,
    credentials: s3_transport::S3Credentials,
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    let instance_id = format!("s3-{}", uuid::Uuid::new_v4());
    state.database.add_s3_transport(&instance_id, &config, &credentials)?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
pub(crate) async fn replicated_sync_update_connector(
    instance_id: String,
    label: Option<String>,
    credentials: Option<sync_connectors::ConnectorCredentials>,
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.update_connector(&instance_id, label.as_deref(), credentials)?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
pub(crate) async fn replicated_sync_create_join_code(
    expires_in_hours: u32,
    connectors: Vec<enrollment::JoinCodeConnectorChoice>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.create_join_code(&connectors, expires_in_hours).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_list_join_codes(state: State<'_, AppState>) -> Result<Vec<enrollment::OutstandingJoinCode>, String> {
    let database = state.database.clone();
    run_database_task(move || database.outstanding_join_codes()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_cancel_join_code(invitation_cid: String, state: State<'_, AppState>) -> Result<(), String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.cancel_join_code(&invitation_cid).await
}

/// Parses pasted join code text without saving or contacting anything.
#[tauri::command]
pub(crate) fn replicated_sync_preview_join_code(code: String) -> Result<enrollment::JoinCodePreview, String> {
    Ok(enrollment::preview_join_code(&code, chrono::Utc::now().timestamp_millis())?)
}

/// Opens the native folder picker for a join code's shared-folder
/// connector. `None` if the user cancels.
#[tauri::command]
pub(crate) async fn replicated_sync_pick_join_folder() -> Result<Option<String>, String> {
    Ok(rfd::AsyncFileDialog::new()
        .set_title("Choose this device's copy of the shared sync folder")
        .pick_folder()
        .await
        .map(|folder| folder.path().to_string_lossy().into_owned()))
}

#[tauri::command]
pub(crate) async fn replicated_sync_join_with_code(
    code: String,
    folders: Vec<enrollment::JoinFolderChoice>,
    credentials: Vec<enrollment::JoinCredentialsChoice>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.replicated_sync.join_with_code(&code, &folders, credentials).await?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn replicated_sync_join_code_notices(state: State<'_, AppState>) -> Result<Vec<enrollment::JoinCodeNotice>, String> {
    let database = state.database.clone();
    run_database_task(move || database.join_code_notices()).await
}

#[tauri::command(async)]
pub(crate) fn replicated_sync_dismiss_join_code_notice(redemption_cid: String, state: State<'_, AppState>) -> Result<(), String> {
    Ok(state.database.dismiss_join_code_notice(&redemption_cid)?)
}

#[tauri::command]
pub(crate) async fn replicated_sync_probe_ipfs_rpc(
    base_url: String,
    token: Option<String>,
    state: State<'_, AppState>,
) -> Result<ipfs_transport::ProbeReport, String> {
    state.replicated_sync.probe_ipfs_rpc_endpoint(&base_url, token.as_deref()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_enrollment_status(state: State<'_, AppState>) -> Result<enrollment::EnrollmentStatus, String> {
    let database = state.database.clone();
    run_database_task(move || database.enrollment_status()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_pending_requests(state: State<'_, AppState>) -> Result<Vec<enrollment::IncomingEnrollmentRequest>, String> {
    let database = state.database.clone();
    run_database_task(move || database.pending_incoming_enrollment_requests()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_device_roster(state: State<'_, AppState>) -> Result<Vec<enrollment::DeviceRosterEntry>, String> {
    let database = state.database.clone();
    run_database_task(move || database.device_roster()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_begin_genesis(allow_existing_space: bool, state: State<'_, AppState>) -> Result<String, String> {
    let phrase = state.replicated_sync.begin_genesis(allow_existing_space).await?;
    kick_replicated_sync(&state);
    Ok(phrase)
}

#[tauri::command]
pub(crate) async fn replicated_sync_inspect_space(state: State<'_, AppState>) -> Result<enrollment::SyncSpacePresence, String> {
    Ok(state.replicated_sync.inspect_sync_space().await)
}

#[tauri::command]
pub(crate) async fn replicated_sync_protocol_reset_notice(state: State<'_, AppState>) -> Result<bool, String> {
    let database = state.database.clone();
    run_database_task(move || database.protocol_reset_notice()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_dismiss_protocol_reset_notice(state: State<'_, AppState>) -> Result<(), String> {
    let database = state.database.clone();
    run_database_task(move || database.dismiss_protocol_reset_notice()).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_request_enrollment(state: State<'_, AppState>) -> Result<String, String> {
    let fingerprint = state.replicated_sync.request_enrollment().await?;
    kick_replicated_sync(&state);
    Ok(fingerprint)
}

#[tauri::command]
pub(crate) async fn replicated_sync_approve_request(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.approve_enrollment_request(&request_id).await?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn replicated_sync_reject_request(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.reject_enrollment_request(&request_id).await?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn replicated_sync_confirm_enrollment(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.confirm_enrollment(&request_id).await?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn replicated_sync_rotate_epoch(revoke_device_id: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.rotate_epoch(revoke_device_id.as_deref()).await
}

#[tauri::command(async)]
pub(crate) fn replicated_sync_set_device_label(device_id: String, label: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.set_device_label(&device_id, &label)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn replicated_sync_leave(state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.leave_sync_space().await
}

#[tauri::command]
pub(crate) fn replicated_sync_check_recovery_phrase(phrase: String) -> threestrands_sync_envelope::RecoveryPhraseCheck {
    threestrands_sync_envelope::check_recovery_phrase(&phrase)
}

#[tauri::command]
pub(crate) async fn replicated_sync_join_with_recovery_phrase(phrase: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.join_with_recovery_phrase(&phrase).await
}

#[tauri::command]
pub(crate) async fn replicated_sync_remove_transport(
    instance_id: String,
    delete_data: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if delete_data {
        if let Some(row) = state
            .database
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
        {
            if row.config().is_some_and(|config| config.supports_delete_data()) {
                if let Some(connector) = row.open_connector().await {
                    connector.delete_all_corpus_data().await?;
                }
            }
        }
    }
    state.database.remove_transport(&instance_id)
}

#[tauri::command]
pub(crate) async fn replicated_sync_now(state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.sync_once().await
}

#[tauri::command]
pub(crate) async fn replicated_sync_conflicts(state: State<'_, AppState>) -> Result<Vec<replicated_sync::FrontierConflict>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_frontier_conflicts()).await
}

#[tauri::command(async)]
pub(crate) fn replicated_sync_resolve_conflict(
    entity_type: String,
    entity_id: String,
    field: String,
    operation_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let entity_type: threestrands_sync_protocol::EntityType =
        entity_type.parse().map_err(|error: String| error)?;
    state.database.resolve_frontier_conflict(entity_type, &entity_id, &field, &operation_id)?;
    kick_replicated_sync(&state);
    Ok(())
}
