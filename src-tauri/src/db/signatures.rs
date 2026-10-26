//! Signature templates, per-address defaults, and their replicated writes.
//! Gmail imports save a reviewed copy and its optional default in one
//! transaction; reading Gmail settings never touches the library.

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::signatures::{
    embed_images, signature_sync_payload, DraftSignature, Signature, SignatureDefaults,
    SignatureLibrary,
};
use rusqlite::params;
use threestrands_sync_protocol::EntityType;

impl Database {
    pub fn signature_library(&self) -> DbResult<SignatureLibrary> {
        Ok(SignatureLibrary {
            signatures: self.list_signature_records("signatures")?,
            defaults: self.list_signature_records("signature_defaults")?,
        })
    }
    fn list_signature_records<T: serde::de::DeserializeOwned>(
        &self,
        table: &str,
    ) -> DbResult<Vec<T>> {
        self.with_connection(|c| {
            let mut query = c.prepare(&format!("SELECT payload FROM {table} ORDER BY id"))?;
            let rows = query.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|row| serde_json::from_str(&row?).map_err(serialization_error))
                .collect()
        })
    }
    pub fn store_signature(&self, item: &Signature) -> DbResult<()> {
        let value = serde_json::to_value(item).map_err(serialization_error)?;
        EntityType::Signature
            .validate_payload(&value)
            .map_err(DatabaseError::invalid)?;
        embed_images(&item.html, &item.id).map_err(DatabaseError::invalid)?;
        signature_sync_payload(item).map_err(DatabaseError::invalid)?;
        self.store_signature_record("signatures", &item.id, &value)
    }
    pub fn store_signature_defaults(&self, item: &SignatureDefaults) -> DbResult<()> {
        let value = serde_json::to_value(item).map_err(serialization_error)?;
        EntityType::SignatureDefaults
            .validate_payload(&value)
            .map_err(DatabaseError::invalid)?;
        if item.id != SignatureDefaults::key(&item.account, &item.address) {
            return Err(DatabaseError::invalid(
                "Invalid signature defaults identity",
            ));
        }
        self.store_signature_record("signature_defaults", &item.id, &value)
    }
    fn store_signature_record(
        &self,
        table: &str,
        id: &str,
        value: &serde_json::Value,
    ) -> DbResult<()> {
        let enabled = self.replicated_sync_active()? && !self.is_projecting_remote_operation();
        let entity_type = if table == "signatures" {
            EntityType::Signature
        } else {
            EntityType::SignatureDefaults
        };
        self.with_transaction(|tx| {
            tx.execute(&format!("INSERT INTO {table}(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload"), params![id, value.to_string()])?;
            if enabled { record_signature_write(tx, entity_type, id, value)?; }
            Ok(())
        })
    }
    pub fn delete_signature(&self, id: &str) -> DbResult<()> {
        self.delete_signature_record("signatures", EntityType::Signature, id)
    }
    pub fn delete_signature_defaults(&self, id: &str) -> DbResult<()> {
        self.delete_signature_record("signature_defaults", EntityType::SignatureDefaults, id)
    }
    fn delete_signature_record(
        &self,
        table: &str,
        entity_type: EntityType,
        id: &str,
    ) -> DbResult<()> {
        let enabled = self.replicated_sync_active()? && !self.is_projecting_remote_operation();
        self.with_transaction(|tx| {
            tx.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])?;
            if enabled {
                crate::db::sync_state::record_replicated_deletion_in_transaction(tx, entity_type, id)?;
            }
            Ok(())
        })
    }
    pub fn default_draft_signature(
        &self,
        account: &str,
        address: &str,
        mode: &str,
    ) -> Result<Option<DraftSignature>, String> {
        let library = self.signature_library()?;
        let alias = SignatureDefaults::key(account, address);
        let base = SignatureDefaults::key(account, "");
        let defaults = library
            .defaults
            .iter()
            .find(|d| d.id == alias)
            .or_else(|| library.defaults.iter().find(|d| d.id == base));
        let id = defaults.and_then(|d| {
            if mode == "new" {
                d.new_signature_id.as_ref()
            } else {
                d.reply_signature_id.as_ref()
            }
        });
        Ok(id
            .and_then(|id| library.signatures.iter().find(|s| &s.id == id))
            .map(|s| DraftSignature {
                id: Some(s.id.clone()),
                name: s.name.clone(),
                html: s.html.clone(),
                text: s.text.clone(),
                automatic: true,
                html_offset: 0,
                text_offset: 0,
            }))
    }
    pub fn replace_automatic_signature(
        &self,
        draft: &mut crate::correspondence::Draft,
    ) -> Result<(), String> {
        if draft.signature.as_ref().is_some_and(|s| !s.automatic) {
            return Ok(());
        }
        let previous = draft
            .signature
            .as_ref()
            .map(|s| (s.html_offset, s.text_offset));
        draft.signature =
            self.default_draft_signature(&draft.account, draft.from_address(), &draft.mode)?;
        if let (Some(s), Some((html, text))) = (&mut draft.signature, previous) {
            s.html_offset = html;
            s.text_offset = text;
        }
        Ok(())
    }

    pub(crate) fn require_local_gmail_account(&self, email: &str) -> DbResult<()> {
        match self.get_account(email)? {
            Some(account) if account.provider == "gmail" && account.on_this_device => Ok(()),
            _ => Err(DatabaseError::invalid(
                "Choose a Gmail account connected on this device.",
            )),
        }
    }

    pub(crate) fn import_gmail_signature(
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
        let library = SignatureLibrary {
            signatures: vec![signature.clone()],
            defaults: vec![],
        };
        library.validate().map_err(DatabaseError::invalid)?;
        let value = serde_json::to_value(signature).map_err(serialization_error)?;
        let enabled = self.replicated_sync_active()? && !self.is_projecting_remote_operation();
        self.with_transaction(|tx| {
            tx.execute("INSERT INTO signatures(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![signature.id, value.to_string()])?;
            if enabled { record_signature_write(tx, EntityType::Signature, &signature.id, &value)?; }
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
                if enabled { record_signature_write(tx, EntityType::SignatureDefaults, &id, &value)?; }
            }
            Ok(())
        })
    }
}

pub(crate) fn record_signature_write(
    tx: &rusqlite::Transaction,
    entity_type: EntityType,
    id: &str,
    value: &serde_json::Value,
) -> DbResult<()> {
    let value = if entity_type == EntityType::Signature {
        signature_sync_payload(&serde_json::from_value(value.clone()).map_err(serialization_error)?)
            .map_err(DatabaseError::invalid)?
    } else {
        value.clone()
    };
    let fields = value
        .as_object()
        .ok_or_else(|| DatabaseError::invalid("Invalid signature record"))?
        .keys()
        .cloned()
        .collect();
    crate::db::sync_state::record_replicated_write_in_transaction(
        tx,
        entity_type,
        id,
        &fields,
        &value,
    )?;
    if entity_type == EntityType::Signature {
        let mut query = tx.prepare(
            "SELECT DISTINCT field FROM sync_values WHERE entity_type=?1 AND entity_id=?2",
        )?;
        let old = query
            .query_map(params![entity_type.as_str(), id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for field in old.into_iter().filter(|field| {
            (field.starts_with("html:") || field.starts_with("text:")) && value.get(field).is_none()
        }) {
            // Replica context retains the removed fields' dots, just as it
            // does for an entity deletion. Older snapshots cannot revive them.
            tx.execute(
                "DELETE FROM sync_values WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
                params![entity_type.as_str(), id, field],
            )?;
        }
    }
    Ok(())
}

/// Settings import: replaces the signature library with `library`. When
/// `sync` is set, templates and defaults that leave are recorded as
/// replicated deletions and every imported one as a replicated write.
pub(super) fn replace_signature_library(
    transaction: &rusqlite::Transaction,
    library: &SignatureLibrary,
    sync: bool,
) -> DbResult<()> {
    library.validate().map_err(DatabaseError::invalid)?;
    if sync {
        for (table, entity_type, keep) in [
            ("signatures", threestrands_sync_protocol::EntityType::Signature, library.signatures.iter().map(|s| s.id.as_str()).collect::<std::collections::HashSet<_>>()),
            ("signature_defaults", threestrands_sync_protocol::EntityType::SignatureDefaults, library.defaults.iter().map(|d| d.id.as_str()).collect::<std::collections::HashSet<_>>()),
        ] {
            let mut query = transaction.prepare(&format!("SELECT id FROM {table}"))?;
            let ids = query.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            for id in ids.into_iter().filter(|id| !keep.contains(id.as_str())) {
                crate::db::sync_state::record_replicated_deletion_in_transaction(transaction, entity_type, &id)?;
            }
        }
    }
    transaction.execute("DELETE FROM signatures", [])?;
    transaction.execute("DELETE FROM signature_defaults", [])?;
    for signature in &library.signatures {
        transaction.execute("INSERT INTO signatures(id,payload) VALUES(?1,?2)", params![signature.id, serde_json::to_string(signature).map_err(serialization_error)?])?;
        if sync { record_signature_write(transaction, threestrands_sync_protocol::EntityType::Signature, &signature.id, &serde_json::to_value(signature).map_err(serialization_error)?)?; }
    }
    for defaults in &library.defaults {
        transaction.execute("INSERT INTO signature_defaults(id,payload) VALUES(?1,?2)", params![defaults.id, serde_json::to_string(defaults).map_err(serialization_error)?])?;
        if sync { record_signature_write(transaction, threestrands_sync_protocol::EntityType::SignatureDefaults, &defaults.id, &serde_json::to_value(defaults).map_err(serialization_error)?)?; }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/signatures.rs"]
mod tests;
