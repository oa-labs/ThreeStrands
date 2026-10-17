//! Atomic application of native settings transfers across feature tables.
//! This orchestrator keeps one transaction; feature CRUD methods must not be
//! called here if they would acquire another lock or commit independently.

use super::{serialization_error, Database, DbResult};
use crate::transfer::{
    TransferAccount, TransferContact, TransferContactGroup, TransferSenderRule, TransferSnippet,
    TransferSplitInbox,
};
use chrono::Utc;
use rusqlite::params;

impl Database {
    /// Applies the native portion of a settings transfer atomically. Existing
    /// destination accounts keep their connection status because their
    /// keychain credentials are deliberately not part of the transfer. An
    /// account seen only in the imported file is created as `needs_reauth`.
    pub(crate) fn import_transfer_data(
        &self,
        accounts: &[TransferAccount],
        // Lowercased emails the user chose not to set up on this device.
        not_here: &std::collections::HashSet<String>,
        split_inboxes: &[TransferSplitInbox],
        snippets: &[TransferSnippet],
        contacts: &[TransferContact],
        contact_groups: Option<&[TransferContactGroup]>,
        sender_rules: Option<&[TransferSenderRule]>,
        retention_days: Option<i64>,
        signatures: Option<&crate::signatures::SignatureLibrary>,
    ) -> DbResult<()> {
        let sync_signatures = signatures.is_some() && self.replicated_sync_active()?;
        self.with_transaction(|transaction| {
            transaction
                .execute(
                    "UPDATE accounts SET sort_order = sort_order + ?1",
                    [accounts.len() as i64],
                )?;
            let connected_at = Utc::now().to_rfc3339();
            for account in accounts {
                transaction
                    .execute(
                        "INSERT INTO accounts(
                             email, display_name, color, status, provider, sort_order, connected_at, last_synced_at, nickname, on_this_device
                         ) VALUES (?1, ?2, ?3, 'needs_reauth', ?4, ?5, ?6, NULL, ?7, ?8)
                         ON CONFLICT(email) DO UPDATE SET
                             display_name = excluded.display_name,
                             nickname = excluded.nickname,
                             color = excluded.color,
                             sort_order = excluded.sort_order,
                             -- Choosing an account adds it here; leaving it
                             -- out never disconnects one already set up.
                             on_this_device = MAX(on_this_device, excluded.on_this_device)",
                        params![
                            account.email,
                            account.display_name,
                            account.color,
                            account.provider,
                            account.sort_order,
                            connected_at,
                            account.nickname,
                            !not_here.contains(&account.email.to_ascii_lowercase()),
                        ],
                    )?;
                // An IMAP account carries its non-secret server settings
                // (format version 5). Persist them so a reconnect after the
                // password is re-entered has the host/port/pins ready. A V4
                // export, and every Gmail account, has `imap: None` and writes
                // nothing here. The password is never in the transfer.
                if let Some(imap) = &account.imap {
                    crate::provider::imap::write_settings_row(&*transaction, &account.email, imap)?;
                }
            }

            transaction
                .execute("DELETE FROM split_inboxes", [])?;
            for split in split_inboxes {
                transaction
                    .execute(
                        "INSERT INTO split_inboxes(
                             id, name, match_kind, match_value, sort_order, created_at, account_id
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            split.id,
                            split.name,
                            split.match_kind,
                            split.match_value,
                            split.sort_order,
                            split.created_at,
                            split.account_id,
                        ],
                    )?;
            }

            transaction
                .execute("DELETE FROM snippets", [])?;
            for snippet in snippets {
                transaction
                    .execute(
                        "INSERT INTO snippets(id, name, body, created_at) VALUES (?1, ?2, ?3, ?4)",
                        params![snippet.id, snippet.name, snippet.body, snippet.created_at],
                    )?;
            }

            if let Some(library) = signatures {
                library.validate().map_err(super::DatabaseError::invalid)?;
                if sync_signatures {
                    for (table, entity_type, keep) in [
                        ("signatures", threestrands_sync_protocol::EntityType::Signature, library.signatures.iter().map(|s| s.id.as_str()).collect::<std::collections::HashSet<_>>()),
                        ("signature_defaults", threestrands_sync_protocol::EntityType::SignatureDefaults, library.defaults.iter().map(|d| d.id.as_str()).collect::<std::collections::HashSet<_>>()),
                    ] {
                        let mut query = transaction.prepare(&format!("SELECT id FROM {table}"))?;
                        let ids = query.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                        for id in ids.into_iter().filter(|id| !keep.contains(id.as_str())) {
                            crate::sync_state::record_replicated_deletion_in_transaction(transaction, entity_type, &id)?;
                        }
                    }
                }
                transaction.execute("DELETE FROM signatures", [])?;
                transaction.execute("DELETE FROM signature_defaults", [])?;
                for signature in &library.signatures {
                    transaction.execute("INSERT INTO signatures(id,payload) VALUES(?1,?2)", params![signature.id, serde_json::to_string(signature).map_err(serialization_error)?])?;
                    if sync_signatures { crate::signatures::record_signature_write(transaction, threestrands_sync_protocol::EntityType::Signature, &signature.id, &serde_json::to_value(signature).map_err(serialization_error)?)?; }
                }
                for defaults in &library.defaults {
                    transaction.execute("INSERT INTO signature_defaults(id,payload) VALUES(?1,?2)", params![defaults.id, serde_json::to_string(defaults).map_err(serialization_error)?])?;
                    if sync_signatures { crate::signatures::record_signature_write(transaction, threestrands_sync_protocol::EntityType::SignatureDefaults, &defaults.id, &serde_json::to_value(defaults).map_err(serialization_error)?)?; }
                }
            }

            transaction.execute("DELETE FROM contacts", [])?;
            for contact in contacts {
                let kit=&contact.keep_in_touch;
                transaction.execute("INSERT INTO contacts(id,display_name,role,company,location,bio,notes,links_json,photo_data,favorite,updated_at,birthday,kit_interval_days,kit_started_at,kit_snoozed_until,kit_snoozed_at,kit_last_touch_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",params![contact.id,contact.display_name,contact.role,contact.company,contact.location,contact.bio,contact.notes,serde_json::to_string(&contact.links).map_err(serialization_error)?,contact.photo_data,contact.favorite,Utc::now().to_rfc3339(),contact.birthday,kit.interval_days,kit.started_at,kit.snoozed_until,kit.snoozed_at,kit.last_touch_at])?;
                for (position,email) in contact.addresses.iter().enumerate() { transaction.execute("INSERT INTO contact_addresses(contact_id,email,position) VALUES(?1,?2,?3)",params![contact.id,email,position as i64])?; }
            }

            // An export from before contact groups has none to offer, so
            // the groups already here stay.
            if let Some(groups) = contact_groups {
                transaction.execute("DELETE FROM contact_groups", [])?;
                transaction.execute("DELETE FROM contact_group_members", [])?;
                let now = Utc::now().to_rfc3339();
                for group in groups {
                    transaction.execute(
                        "INSERT INTO contact_groups(id,name,created_at,updated_at) VALUES(?1,?2,?3,?4)",
                        params![group.id, group.name.trim(), group.created_at, now],
                    )?;
                    for member in &group.member_ids {
                        transaction.execute(
                            "INSERT INTO contact_group_members(group_id,contact_id) VALUES(?1,?2)",
                            params![group.id, member],
                        )?;
                    }
                }
            }

            // An export from before muted senders has none to offer, so the
            // ones already here stay. Removing a rule drops its action log.
            if let Some(rules) = sender_rules {
                transaction.execute("DELETE FROM sender_rules", [])?;
                let now = Utc::now().to_rfc3339();
                for rule in rules {
                    let value = super::sender_rules::normalize_rule_match(&rule.match_kind, &rule.match_value)?;
                    transaction.execute(
                        "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![rule.id, rule.account_id, rule.match_kind, value, rule.action, rule.created_at, now],
                    )?;
                }
            }

            // Imports replace retention, including older exports' Forever
            // value; an installation default must not reappear underneath it.
            transaction.execute("DELETE FROM compose_settings WHERE key='retention_days_default'", [])?;
            match retention_days {
                Some(days) => transaction
                    .execute(
                        "INSERT INTO compose_settings(key, value) VALUES ('retention_days', ?1)
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        [days.to_string()],
                    )?,
                None => transaction
                    .execute(
                        "DELETE FROM compose_settings WHERE key = 'retention_days'",
                        [],
                    )?,
            };
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "tests/settings_transfer.rs"]
mod tests;
