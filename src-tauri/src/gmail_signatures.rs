//! Explicit Gmail imports. Reading settings never mutates Gmail or the library;
//! saving a reviewed copy and its optional default is one local transaction.
use crate::{
    db::{serialization_error, Database, DatabaseError, DbResult},
    signatures::{Signature, SignatureDefaults},
};
use rusqlite::params;
use tauri::State;
use threestrands_sync_protocol::EntityType;

impl Database {
    fn require_local_gmail_account(&self, email: &str) -> DbResult<()> {
        match self.get_account(email)? {
            Some(account) if account.provider == "gmail" && account.on_this_device => Ok(()),
            _ => Err(DatabaseError::invalid(
                "Choose a Gmail account connected on this device.",
            )),
        }
    }

    fn import_gmail_signature(
        &self,
        account: &str,
        signature: &Signature,
        use_as_default: bool,
    ) -> DbResult<()> {
        self.require_local_gmail_account(account)?;
        if signature.name.trim().is_empty()
            || signature
                .name
                .chars()
                .any(|c| matches!(c, '\r' | '\n' | '\0'))
        {
            return Err(DatabaseError::invalid("Enter a valid signature name."));
        }
        let library = crate::signatures::SignatureLibrary {
            signatures: vec![signature.clone()],
            defaults: vec![],
        };
        library.validate().map_err(DatabaseError::invalid)?;
        let value = serde_json::to_value(signature).map_err(serialization_error)?;
        let enabled = self.replicated_sync_active()? && !self.is_projecting_remote_operation();
        self.with_transaction(|tx| {
            tx.execute("INSERT INTO signatures(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![signature.id, value.to_string()])?;
            if enabled { crate::signatures::record_signature_write(tx, EntityType::Signature, &signature.id, &value)?; }
            if use_as_default {
                let id = SignatureDefaults::key(account, "");
                // Read inside the transaction so a concurrent defaults edit is preserved.
                let previous = tx.query_row("SELECT payload FROM signature_defaults WHERE id=?1", [&id], |row| row.get::<_, String>(0));
                let mut defaults = match previous {
                    Ok(payload) => serde_json::from_str::<SignatureDefaults>(&payload).map_err(serialization_error)?,
                    Err(rusqlite::Error::QueryReturnedNoRows) => SignatureDefaults { id: id.clone(), account: account.into(), address: String::new(), new_signature_id: None, reply_signature_id: None },
                    Err(error) => return Err(error.into()),
                };
                defaults.new_signature_id = Some(signature.id.clone());
                let value = serde_json::to_value(&defaults).map_err(serialization_error)?;
                tx.execute("INSERT INTO signature_defaults(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![id, value.to_string()])?;
                if enabled { crate::signatures::record_signature_write(tx, EntityType::SignatureDefaults, &id, &value)?; }
            }
            Ok(())
        })
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::MailProviderKind,
        signatures::tests::{defaults, template},
    };

    fn database() -> Database {
        let db = Database::open_memory();
        db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
            .unwrap();
        db
    }

    #[test]
    fn imported_templates_and_defaults_are_both_recorded_for_sync() {
        let db = database();
        db.set_beta_features_enabled(true).unwrap();
        db.import_gmail_signature("me@example.com", &template("gmail"), true)
            .unwrap();
        let id = SignatureDefaults::key("me@example.com", "");
        let connection = db.connection().unwrap();
        for (entity_type, entity_id) in [
            (EntityType::Signature.as_str(), "gmail"),
            (EntityType::SignatureDefaults.as_str(), id.as_str()),
        ] {
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_values WHERE entity_type=?1 AND entity_id=?2",
                    params![entity_type, entity_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(count > 0, "{entity_type} must replicate");
        }
    }

    #[test]
    fn importing_preserves_existing_reply_and_alias_defaults() {
        let db = database();
        db.store_signature(&template("old")).unwrap();
        db.store_signature_defaults(&defaults("me@example.com", "", Some("old"), Some("old")))
            .unwrap();
        db.store_signature_defaults(&defaults(
            "me@example.com",
            "alias@example.com",
            Some("old"),
            None,
        ))
        .unwrap();
        db.import_gmail_signature("me@example.com", &template("gmail"), true)
            .unwrap();
        let library = db.signature_library().unwrap();
        let base = library
            .defaults
            .iter()
            .find(|d| d.address.is_empty())
            .unwrap();
        assert_eq!(base.new_signature_id.as_deref(), Some("gmail"));
        assert_eq!(base.reply_signature_id.as_deref(), Some("old"));
        assert_eq!(
            library
                .defaults
                .iter()
                .find(|d| !d.address.is_empty())
                .unwrap()
                .new_signature_id
                .as_deref(),
            Some("old")
        );
        assert_eq!(library.signatures.len(), 2);
        // A retry uses the same identity rather than duplicating the template.
        db.import_gmail_signature("me@example.com", &template("gmail"), true)
            .unwrap();
        assert_eq!(db.signature_library().unwrap().signatures.len(), 2);
    }

    #[test]
    fn importing_without_a_default_leaves_all_defaults_unchanged() {
        let db = database();
        db.store_signature_defaults(&defaults("me@example.com", "", None, Some("short")))
            .unwrap();
        db.import_gmail_signature("me@example.com", &template("gmail"), false)
            .unwrap();
        let library = db.signature_library().unwrap();
        assert_eq!(library.signatures.len(), 1);
        assert_eq!(library.defaults.len(), 1);
        assert_eq!(library.defaults[0].new_signature_id, None);
        assert_eq!(
            library.defaults[0].reply_signature_id.as_deref(),
            Some("short")
        );
    }

    #[test]
    fn importing_creates_a_new_message_default_only() {
        let db = database();
        db.import_gmail_signature("me@example.com", &template("gmail"), true)
            .unwrap();
        let library = db.signature_library().unwrap();
        assert_eq!(library.defaults.len(), 1);
        assert_eq!(
            library.defaults[0].new_signature_id.as_deref(),
            Some("gmail")
        );
        assert_eq!(library.defaults[0].reply_signature_id, None);
    }

    #[test]
    fn invalid_input_or_a_failed_default_write_cannot_leave_a_partial_import() {
        let db = database();
        let mut invalid = template("invalid");
        invalid.name = "bad\nname".into();
        assert!(db
            .import_gmail_signature("me@example.com", &invalid, true)
            .is_err());
        db.with_connection(|c| { c.execute_batch("CREATE TRIGGER reject_import_defaults BEFORE INSERT ON signature_defaults BEGIN SELECT RAISE(ABORT, 'storage failure'); END;")?; Ok(()) }).unwrap();
        assert!(db
            .import_gmail_signature("me@example.com", &template("gmail"), true)
            .is_err());
        let library = db.signature_library().unwrap();
        assert!(library.signatures.is_empty());
        assert!(library.defaults.is_empty());
    }

    #[test]
    fn importing_rejects_unknown_imap_and_other_device_accounts_before_writes() {
        let db = database();
        db.adopt_mail_account("imap@example.com", MailProviderKind::Imap)
            .unwrap();
        db.with_connection(|c| {
            c.execute(
                "UPDATE accounts SET on_this_device=0 WHERE email='me@example.com'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        for account in ["unknown@example.com", "imap@example.com", "me@example.com"] {
            assert!(db.require_local_gmail_account(account).is_err());
            assert!(db
                .import_gmail_signature(account, &template("gmail"), true)
                .is_err());
        }
        assert!(db.signature_library().unwrap().signatures.is_empty());
    }
}
