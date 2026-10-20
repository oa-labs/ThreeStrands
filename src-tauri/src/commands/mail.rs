//! Mail listing, threads, search, triage, labels, and unsubscribe.

use std::collections::HashMap;

use crate::models::{
    CreateLabelRequest, Label, MailboxUnreadCounts, SearchThreadsRequest, Thread, ThreadDetail,
    ThreadMutation, ThreadPage, TriageEvent, TriageSenderStats, UpdateLabelRequest,
};
use crate::sync::SyncService;
use tauri::State;

use crate::{correspondence, mail_links, models, sync, unsubscribe_service};
use crate::{resolve_account, run_database_task, sync_services, AppError, AppResult, AppState};

/// Accounts currently checking for mail, for a frontend that mounted after
/// their `mail-sync-activity` start event fired.
#[tauri::command]
pub(crate) async fn mail_sync_activity(activity: State<'_, sync::SyncActivity>) -> AppResult<Vec<String>> {
    Ok(activity.active_accounts())
}

#[tauri::command]
pub(crate) async fn correspondence_request(
    request: correspondence::Request,
    state: State<'_, AppState>,
) -> AppResult<serde_json::Value> {
    state.correspondence.request(request).await
        .map_err(AppError::from)
}

/// How often quitting re-checks for undo windows that are still open.
const EXIT_UNDO_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

#[tauri::command]
pub(crate) async fn finish_exit(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    // Give pending undo windows time to complete while keeping the UI responsive.
    while state.database.pending_undo()? {
        tokio::time::sleep(EXIT_UNDO_POLL_INTERVAL).await;
    }
    // Give drafts a bounded chance to reach the server; they stay saved here.
    state.correspondence.push_drafts_before_exit().await;
    // Do not terminate while a provider request is awaiting acknowledgement.
    let _send_guard = state.correspondence.gate.lock().await;
    state
        .exiting
        .store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_threads(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<Thread>> {
    let database = state.database.clone();
    run_database_task(move || database.list_threads(account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn list_all_mail(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<Thread>> {
    let database = state.database.clone();
    run_database_task(move || database.list_all_mail(account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn list_trash(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<Thread>> {
    let database = state.database.clone();
    run_database_task(move || database.list_trash(account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn list_threads_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<ThreadPage> {
    let database = state.database.clone();
    run_database_task(move || database.list_threads_page(account_id.as_deref(), offset, limit)).await
}

#[tauri::command]
pub(crate) async fn list_unread_counts(state: State<'_, AppState>) -> AppResult<HashMap<String, i64>> {
    let database = state.database.clone();
    run_database_task(move || database.list_unread_counts()).await
}

#[tauri::command]
pub(crate) async fn mailbox_unread_counts(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<MailboxUnreadCounts> {
    let database = state.database.clone();
    run_database_task(move || database.mailbox_unread_counts(account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn list_all_mail_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<ThreadPage> {
    let database = state.database.clone();
    run_database_task(move || database.list_all_mail_page(account_id.as_deref(), offset, limit)).await
}

#[tauri::command]
pub(crate) async fn list_trash_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<ThreadPage> {
    let database = state.database.clone();
    run_database_task(move || database.list_trash_page(account_id.as_deref(), offset, limit)).await
}

#[tauri::command]
pub(crate) async fn list_sent_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<ThreadPage> {
    let database = state.database.clone();
    run_database_task(move || database.list_sent_page(account_id.as_deref(), offset, limit)).await
}

#[tauri::command]
pub(crate) async fn get_thread(id: String, state: State<'_, AppState>) -> AppResult<ThreadDetail> {
    let database = state.database.clone();
    run_database_task(move || database.get_thread(&id)).await
}

#[tauri::command]
pub(crate) async fn search_threads(
    request: SearchThreadsRequest,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<Thread>> {
    let database = state.database.clone();
    run_database_task(move || database.search_threads(&request, account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn backfill_search_threads(
    query: String,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<crate::provider::SearchReport> {
    if query.trim().is_empty() {
        return Ok(crate::provider::SearchReport::not_searched());
    }

    let services = sync_services(&state, account_id.as_deref()).await;

    // Combining rule for a unified search over several accounts (Phase 5 run
    // 5): fold every account's report together with `SearchReport::combine` —
    // counts SUM, and the status takes the most-severe across accounts
    // (Complete < Partial < NoServerMatches < Skipped), keeping ALL partial /
    // skip reasons. So if any one account only managed a partial or skipped
    // search, the unified result says so rather than being masked by another
    // account's clean `Complete`. A provider ERROR (one account offline) is
    // still collected into `errors` and fails the command exactly as before —
    // the UI's existing "unavailable" state is unchanged.
    let mut errors = Vec::new();
    let mut report = crate::provider::SearchReport::not_searched();
    for service in services {
        match service.backfill_search(&query).await {
            Ok(account_report) => report = report.combine(account_report),
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(report)
    } else {
        Err(errors.join("\n").into())
    }
}

#[tauri::command]
pub(crate) async fn mutate_thread(mutation: ThreadMutation, state: State<'_, AppState>) -> AppResult<()> {
    let database = state.database.clone();
    run_database_task(move || database.mutate_thread(&mutation)).await
}

#[tauri::command]
pub(crate) async fn mutate_threads(
    mutations: Vec<ThreadMutation>,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let database = state.database.clone();
    run_database_task(move || database.mutate_threads(&mutations)).await
}

#[tauri::command]
pub(crate) async fn record_triage_event(event: TriageEvent, state: State<'_, AppState>) -> AppResult<()> {
    let database = state.database.clone();
    run_database_task(move || database.record_triage_event(&event)).await
}

#[tauri::command]
pub(crate) async fn list_triage_sender_stats(
    account_id: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> AppResult<Vec<TriageSenderStats>> {
    let database = state.database.clone();
    run_database_task(move || database.list_triage_sender_stats(&account_id, limit.unwrap_or(100))).await
}

#[tauri::command]
pub(crate) async fn unsubscribe(
    message_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<models::UnsubscribeResult> {
    let target = state.database.begin_unsubscribe(&message_id)?;
    match unsubscribe_service::execute(&target, |url| mail_links::deliver(&app, url)).await {
        Ok(result) => {
            state.database.finish_unsubscribe(
                &target.request_id,
                if result.outcome == "opened" {
                    "opened"
                } else {
                    "succeeded"
                },
                result.http_status,
                None,
            )?;
            Ok(result)
        }
        Err(error) => {
            state
                .database
                .finish_unsubscribe(&target.request_id, "failed", None, Some(&error))?;
            Err(error.into())
        }
    }
}

#[tauri::command]
pub(crate) async fn list_split_inbox_page(
    split_inbox_id: String,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<ThreadPage> {
    let database = state.database.clone();
    run_database_task(move || database.list_split_inbox_page(&split_inbox_id, offset, limit)).await
}

#[tauri::command]
pub(crate) async fn list_labels(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<Label>> {
    // Resolve the account id the same way `resolve_account` does so the cache
    // is keyed by the account the listing actually came from, defaulting to
    // the primary account when the caller didn't name one.
    let resolved_id = match account_id.as_deref() {
        Some(id) => id.to_string(),
        None => state.database.primary_account_id(),
    };
    let auth = resolve_account(&state, account_id.as_deref(), |account| account.auth.clone()).await?;

    let labels = auth
        .provider(&state.database)
        .map_err(|error| error.to_string())?
        .list_labels()
        .await
        .map_err(|error| error.to_string())?;

    // Best-effort write-through: a SUCCESSFUL listing refreshes the local
    // label-catalog cache (run 2b-1). A caching failure or an over-limit
    // catalog is logged and swallowed, never failing the user's action; a
    // FAILED listing returned above, leaving the cache untouched.
    crate::db::cache_label_listing(&state.database, &resolved_id, &labels);

    Ok(labels)
}

#[tauri::command]
pub(crate) async fn create_label(
    request: CreateLabelRequest,
    state: State<'_, AppState>,
) -> AppResult<Label> {
    resolve_sync(&state, request.account_id.as_deref())
        .await?
        .create_label(&request.name)
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn update_label(
    request: UpdateLabelRequest,
    state: State<'_, AppState>,
) -> AppResult<Label> {
    resolve_sync(&state, request.account_id.as_deref())
        .await?
        .update_label(&request.id, &request.name)
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn delete_label(
    id: String,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<()> {
    resolve_sync(&state, account_id.as_deref())
        .await?
        .delete_label(&id)
        .await
        .map_err(AppError::from)
}

/// Resolves the sync engine for a given account, falling back to the primary
/// account when `account_id` is `None`. Label mutations must run against the
/// account that actually owns the label, not always the primary account.
async fn resolve_sync(
    state: &State<'_, AppState>,
    account_id: Option<&str>,
) -> AppResult<SyncService> {
    resolve_account(state, account_id, |account| account.sync.clone()).await
        .map_err(AppError::from)
}
