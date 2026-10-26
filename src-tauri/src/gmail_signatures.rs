//! Explicit Gmail imports. Reading settings never mutates Gmail or the library;
//! the reviewed copy and its optional default are saved by the database layer.
use crate::signatures::Signature;
use tauri::State;

#[tauri::command]
pub async fn list_gmail_signatures(
    account: String,
    state: State<'_, crate::AppState>,
) -> Result<Vec<crate::provider::gmail::GmailSignature>, String> {
    let database = state.database.clone();
    let email = account.clone();
    crate::run_database_task(move || database.require_local_gmail_account(&email)).await?;
    let connected_auth = state
        .accounts
        .lock()
        .await
        .get(&account)
        .map(|connected| connected.auth.clone());
    let auth = match connected_auth {
        Some(auth) => auth,
        None => state
            .auth_config
            .mail_account(crate::models::MailProviderKind::Gmail, &account)?,
    };
    crate::provider::gmail::GmailClient::new(auth.require_oauth_credential()?.clone())
        .list_signatures()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub fn import_gmail_signature(
    account: String,
    signature: Signature,
    use_as_default: bool,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    state
        .database
        .import_gmail_signature(&account, &signature, use_as_default)?;
    crate::kick_replicated_sync(&state);
    Ok(())
}
