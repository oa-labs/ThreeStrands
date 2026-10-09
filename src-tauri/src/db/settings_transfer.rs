//! Atomic application of native settings transfers across feature tables.
//! This orchestrator keeps one transaction; feature CRUD methods must not be
//! called here if they would acquire another lock or commit independently.

use super::{serialization_error, Database, DbResult};
use crate::transfer::{
    TransferAccount, TransferContact, TransferContactGroup, TransferSnippet, TransferSplitInbox,
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
        split_inboxes: &[TransferSplitInbox],
        snippets: &[TransferSnippet],
        contacts: &[TransferContact],
        contact_groups: Option<&[TransferContactGroup]>,
        retention_days: Option<i64>,
    ) -> DbResult<()> {
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
                             email, display_name, color, status, provider, sort_order, connected_at, last_synced_at
                         ) VALUES (?1, ?2, ?3, 'needs_reauth', ?4, ?5, ?6, NULL)
                         ON CONFLICT(email) DO UPDATE SET
                             display_name = excluded.display_name,
                             color = excluded.color,
                             sort_order = excluded.sort_order",
                        params![
                            account.email,
                            account.display_name,
                            account.color,
                            account.provider,
                            account.sort_order,
                            connected_at,
                        ],
                    )?;
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
