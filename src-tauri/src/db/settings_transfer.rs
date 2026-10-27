//! Atomic application of native settings transfers across feature tables.
//! This orchestrator keeps one transaction and calls each feature module's
//! import row functions on it; none of them takes the connection lock or
//! commits on its own.

use super::{
    accounts, compose_settings, contact_groups, contacts, sender_rules, signatures, snippets, split_inboxes, Database,
    DbResult,
};
use crate::transfer::{
    TransferAccount, TransferContact, TransferContactGroup, TransferSenderRule, TransferSnippet,
    TransferSplitInbox,
};
use chrono::Utc;

impl Database {
    /// Applies the native portion of a settings transfer atomically. Existing
    /// destination accounts keep their connection status because their
    /// keychain credentials are deliberately not part of the transfer. An
    /// account seen only in the imported file is created as `needs_reauth`.
    #[allow(clippy::too_many_arguments)]
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
            accounts::shift_account_order(transaction, accounts.len() as i64)?;
            let connected_at = Utc::now().to_rfc3339();
            for account in accounts {
                accounts::upsert_imported_account(
                    transaction,
                    account,
                    &connected_at,
                    !not_here.contains(&account.email.to_ascii_lowercase()),
                )?;
                // An IMAP account carries its non-secret server settings
                // (format version 5). Persist them so a reconnect after the
                // password is re-entered has the host/port/pins ready. A V4
                // export, and every Gmail account, has `imap: None` and writes
                // nothing here. The password is never in the transfer.
                if let Some(imap) = &account.imap {
                    super::imap::write_settings_row(transaction, &account.email, imap)?;
                }
            }

            split_inboxes::delete_all_split_inboxes(transaction)?;
            for split in split_inboxes {
                split_inboxes::insert_split_inbox_row(
                    transaction,
                    &split.id,
                    &split.name,
                    &split.match_kind,
                    &split.match_value,
                    split.sort_order,
                    &split.created_at,
                    &split.account_id,
                )?;
            }

            snippets::delete_all_snippets(transaction)?;
            for snippet in snippets {
                snippets::insert_snippet_row(transaction, &snippet.id, &snippet.name, &snippet.body, &snippet.created_at)?;
            }

            if let Some(library) = signatures {
                signatures::replace_signature_library(transaction, library, sync_signatures)?;
            }

            contacts::delete_all_contacts(transaction)?;
            for contact in contacts {
                contacts::insert_imported_contact(transaction, contact)?;
                for (position,email) in contact.addresses.iter().enumerate() { contacts::insert_contact_address(transaction, &contact.id, email, position as i64)?; }
            }

            // An export from before contact groups has none to offer, so
            // the groups already here stay.
            if let Some(groups) = contact_groups {
                contact_groups::delete_all_contact_groups(transaction)?;
                let now = Utc::now().to_rfc3339();
                for group in groups {
                    contact_groups::insert_imported_group(transaction, group, &now)?;
                    for member in &group.member_ids {
                        contact_groups::insert_group_member(transaction, &group.id, member)?;
                    }
                }
            }

            // An export from before muted senders has none to offer, so the
            // ones already here stay. Removing a rule drops its action log.
            if let Some(rules) = sender_rules {
                sender_rules::delete_all_sender_rules(transaction)?;
                let now = Utc::now().to_rfc3339();
                for rule in rules {
                    let value = sender_rules::normalize_rule_match(&rule.match_kind, &rule.match_value)?;
                    sender_rules::insert_imported_sender_rule(transaction, rule, &value, &now)?;
                }
            }

            // Imports replace retention, including older exports' Forever
            // value; an installation default must not reappear underneath it.
            compose_settings::write_retention_days(transaction, retention_days)?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "tests/settings_transfer.rs"]
mod tests;
