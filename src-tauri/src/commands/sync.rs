//! Mail sync status and on-demand sync.

use std::collections::HashSet;

use crate::models::SyncStatus;
use tauri::State;

use crate::db;
use crate::{
    combined_sync_status, database_result, log_failure, resolve_account, run_database_task,
    AppError, AppResult, AppState,
};

#[tauri::command]
pub(crate) async fn sync_status(state: State<'_, AppState>) -> AppResult<SyncStatus> {
    let database = state.database.clone();
    run_database_task(move || combined_sync_status(database.as_ref())).await
}

#[tauri::command]
pub(crate) async fn retry_failed_mutations(state: State<'_, AppState>) -> AppResult<SyncStatus> {
    let database = state.database.clone();
    run_database_task(move || {
        database_result(database.retry_failed_mutations())?;
        combined_sync_status(database.as_ref())
    })
    .await
}

#[tauri::command]
pub(crate) async fn dismiss_sync_problems(state: State<'_, AppState>) -> AppResult<SyncStatus> {
    let database = state.database.clone();
    run_database_task(move || {
        database_result(database.dismiss_sync_problems())?;
        combined_sync_status(database.as_ref())
    })
    .await
}

/// `Some` only when this launch had to recover the local database (restored
/// from a backup, or recreated fresh) — see `db::open_with_recovery`. The
/// frontend uses this once at startup to explain a resync/empty inbox rather
/// than leaving it unexplained.
#[tauri::command]
pub(crate) fn recovery_status(state: State<'_, AppState>) -> Option<db::RecoveryOutcome> {
    state.recovery.clone()
}

#[tauri::command]
pub(crate) async fn sync_account(state: State<'_, AppState>) -> AppResult<SyncStatus> {
    log_failure(
        "refreshing account identity",
        state.correspondence.refresh_identity().await,
    );
    let services = {
        let accounts = state.accounts.lock().await;
        accounts
            .iter()
            .filter(|(_, connected)| connected.sync.is_connected())
            // Report against the account's live key rather than the map key,
            // which is still the placeholder until the first identity refresh
            // rekeys it.
            .map(|(_, connected)| (connected.sync.account_id(), connected.sync.clone()))
            .collect::<Vec<_>>()
    };
    if services.is_empty() {
        return Err("No connected Google accounts are available to sync".into());
    }

    // Accounts are independent Gmail sessions. Run them concurrently so a
    // full sync of one imported mailbox does not delay every other mailbox.
    let mut tasks = tokio::task::JoinSet::new();
    for (email, service) in services {
        tasks.spawn(async move { (email, service.sync().await) });
    }
    let mut task_errors = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((email, Err(error))) => task_errors.push(format!("{email}: {error}")),
            Err(error) => {
                task_errors.push(format!("A mail sync task stopped unexpectedly: {error}"))
            }
            Ok((_, Ok(_))) => {}
        }
    }

    let database = state.database.clone();
    let mut status = run_database_task(move || combined_sync_status(database.as_ref())).await?;
    if !task_errors.is_empty() {
        let mut errors = status.error.take().into_iter().collect::<Vec<_>>();
        errors.extend(task_errors);
        let mut seen = HashSet::new();
        errors.retain(|error| seen.insert(error.clone()));
        status.error = Some(errors.join("\n"));
        status.state = "error";
    }
    Ok(status)
}

#[tauri::command]
pub(crate) async fn flush_pending_mutations(state: State<'_, AppState>) -> AppResult<SyncStatus> {
    let primary = state.database.primary_account_id();
    match resolve_account(&state, Some(&primary), |account| account.sync.clone()).await {
        Ok(service) => service.flush_pending().await.map_err(AppError::from),
        Err(_) => database_result(state.database.sync_status(&primary)),
    }
}
