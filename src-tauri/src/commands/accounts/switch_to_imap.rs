use std::time::Duration;

use tauri::State;

use super::{register_finished_imap_account, AccountAuth, MailProviderKind};
use crate::db::{GmailImapSwitchPreview, SplitLabelChoice};
use crate::models::{Account, Label};
use crate::provider::imap::{
    self,
    plan::{build_native_plan, CatalogMailbox},
    ImapSession as _, SecurityMode,
};
use crate::{
    kick_replicated_sync, log_failure, spawn_synced_account, AppError, AppResult, AppState,
};

#[tauri::command]
pub(crate) async fn preview_gmail_imap_switch(
    email: String,
    password: String,
    state: State<'_, AppState>,
) -> AppResult<GmailImapSwitchPreview> {
    state.database.require_gmail_imap_switch(&email)?;
    let (_, labels) = test_connection(&email, &password).await?;
    // Refresh saved label names when OAuth still works. The app-password
    // switch also works with expired OAuth, using cached names or the user's
    // explicit selection from the authenticated IMAP catalog.
    let previous = state
        .accounts
        .lock()
        .await
        .get(&email)
        .map(|account| account.auth.clone());
    let current = if let Some(provider) = previous
        .filter(AccountAuth::available)
        .and_then(|auth| auth.provider(&state.database).ok())
    {
        tokio::time::timeout(Duration::from_secs(5), provider.list_labels())
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let _guard = state.correspondence.gate.lock().await;
    state.database.require_gmail_imap_switch(&email)?;
    if let Some(current) = current {
        crate::db::cache_label_listing(&state.database, &email, &current);
    }
    Ok(state.database.preview_gmail_imap_switch(&email, labels)?)
}

#[tauri::command]
pub(crate) async fn switch_gmail_to_imap(
    email: String,
    password: String,
    choices: Vec<SplitLabelChoice>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> AppResult<Account> {
    state.database.require_gmail_imap_switch(&email)?;
    let (settings, labels) = test_connection(&email, &password).await?;
    let _guard = state.correspondence.gate.lock().await;
    state.database.require_gmail_imap_switch(&email)?;
    let auth = state
        .auth_config
        .mail_account(MailProviderKind::Imap, &email)?;
    let AccountAuth::Imap(credential) = &auth else {
        return Err("Expected an IMAP credential.".into());
    };
    // The correspondence worker shares this gate. Retire the old sync engine
    // too, so neither work stream can finish a Gmail operation after commit.
    let old_service = state
        .accounts
        .lock()
        .await
        .get(&email)
        .map(|account| account.sync.clone());
    if let Some(service) = &old_service {
        service.retire().await;
    }
    let previous_password = credential.load().ok();
    let switched = (|| -> AppResult<Account> {
        credential.save(&crate::credentials::ImapPassword {
            imap: password,
            smtp: None,
        })?;
        match state
            .database
            .switch_gmail_to_imap(&email, settings, &labels, &choices)
        {
            Ok(account) => Ok(account),
            Err(error) => {
                let restored = match previous_password {
                    Some(previous) => credential.save(&previous),
                    None => credential.disconnect(),
                };
                log_failure("restoring the previous IMAP credential", restored);
                Err(error.into())
            }
        }
    })();
    let account = match switched {
        Ok(account) => account,
        Err(error) => {
            if let Some(service) = &old_service {
                service.resume();
            }
            return Err(error);
        }
    };
    register_finished_imap_account(&state.accounts, email.clone(), || {
        spawn_synced_account(state.database.clone(), auth, true, app)
    })
    .await;

    // Clear this app's old mail credential only after a successful switch.
    // Revoking the Google grant could also invalidate Calendar access.
    if let Ok(old_auth) = state
        .auth_config
        .mail_account(MailProviderKind::Gmail, &email)
    {
        log_failure("clearing the old Gmail credential", old_auth.disconnect());
    }
    log_failure(
        "clearing unused outgoing attachments",
        state.correspondence.cleanup(),
    );
    kick_replicated_sync(&state);
    Ok(account)
}

/// Fixed TLS endpoints and the existing account as the username ensure that
/// native IDs are carried across only for the same Gmail mailbox.
async fn test_connection(
    email: &str,
    password: &str,
) -> AppResult<(imap::ImapAccountSettings, Vec<Label>)> {
    let settings = imap::ImapAccountSettings {
        imap_host: "imap.gmail.com".into(),
        imap_port: 993,
        imap_security: SecurityMode::ImplicitTls,
        imap_username: email.into(),
        smtp_host: "smtp.gmail.com".into(),
        smtp_port: 587,
        smtp_security: SecurityMode::StartTls,
        smtp_username: email.into(),
        mailbox_overrides: Default::default(),
        archive_mailbox: None,
        label_storage: imap::LabelStorage::Folders,
        label_container: None,
        identities: vec![imap::Identity {
            address: email.into(),
            ..Default::default()
        }],
        pinned_fingerprints: Default::default(),
        server_saves_sent: false,
    };
    tokio::time::timeout(Duration::from_secs(45), async {
        let mut session = imap::connect_with_settings(&settings, password).await.map_err(connection_error)?;
        session.capabilities().await.map_err(connection_error)?;
        if !session.supports_label_extensions() {
            return Err("Gmail’s IMAP server did not offer the identities needed to preserve links to mail.".into());
        }
        session.examine("INBOX").await.map_err(connection_error)?;
        let mailboxes = session.list_mailboxes().await.map_err(connection_error)?;
        let _ = session.logout().await;
        let catalog: Vec<_> = mailboxes.iter().filter(|entry| !entry.no_select).map(|entry| {
            CatalogMailbox { name: entry.name.clone(), special_use: entry.special_use.clone() }
        }).collect();
        if !catalog.iter().any(|entry| entry.special_use.as_deref() == Some("\\All")) {
            return Err("Turn on Show in IMAP for All Mail in Gmail settings so ThreeStrands can find your archived conversations.".into());
        }
        let delimiter = mailboxes.iter().find_map(|entry| entry.delimiter.as_deref());
        let plan = build_native_plan(&catalog, delimiter, &settings);
        let labels = plan.label_catalog(
            true,
            catalog.iter().filter(|entry| entry.special_use.is_none()).map(|entry| entry.name.as_str()),
        );
        imap::test_smtp(&settings.smtp_host, settings.smtp_port, settings.smtp_security, None, email, password)
            .await.map_err(connection_error)?;
        Ok((settings, labels))
    }).await.map_err(|_| AppError::message("The Gmail connection test timed out. Try again."))?
}

fn connection_error(error: crate::provider::ProviderError) -> AppError {
    AppError::message(imap::plain_language(&error))
}
