//! Muted senders. A rule names an address, or everyone at a domain, in one
//! account. While it exists, sync marks that sender's new mail read and
//! archives it through the ordinary mutation queue, so the provider's own
//! Inbox follows. Mail older than the rule is left alone; muting can archive
//! what is already in the Inbox when the user asks.

use super::{normalize_sender, Database, DatabaseError, DbResult};
use crate::mime::NormalizedMessage;
use crate::models::{MuteResult, SenderRule, ThreadMutation};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Most muted senders a settings export may carry.
pub(crate) const MAX_SENDER_RULES: usize = 5_000;
const MAX_ADDRESS_LEN: usize = 320;
const MAX_DOMAIN_LEN: usize = 253;

/// Checks and lowercases a rule's match: a whole address for `address`, a
/// bare domain (a leading `@` is dropped) for `domain`.
pub(crate) fn normalize_rule_match(match_kind: &str, match_value: &str) -> DbResult<String> {
    let value = match_value.trim().to_ascii_lowercase();
    let valid_domain = |domain: &str| {
        !domain.is_empty()
            && domain.len() <= MAX_DOMAIN_LEN
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && domain.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
    };
    match match_kind {
        "address" => {
            let valid = value.len() <= MAX_ADDRESS_LEN
                && value.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty()
                        && !local.chars().any(|character| character.is_whitespace() || matches!(character, '<' | '>' | ','))
                        && valid_domain(domain)
                });
            if !valid {
                return Err(DatabaseError::invalid("Mute needs a complete email address"));
            }
            Ok(value)
        }
        "domain" => {
            let domain = value.strip_prefix('@').unwrap_or(&value).to_string();
            if !valid_domain(&domain) {
                return Err(DatabaseError::invalid("Mute needs a domain such as example.com"));
            }
            Ok(domain)
        }
        _ => Err(DatabaseError::invalid("Unknown mute rule")),
    }
}

fn rule_matches(rule: &SenderRule, email: &str, domain: &str) -> bool {
    match rule.match_kind.as_str() {
        "address" => rule.match_value == email,
        "domain" => rule.match_value == domain,
        _ => false,
    }
}

/// Whether a message dated `sent_at` arrived at or after the rule was made.
/// An unreadable date never counts, so a rule cannot act on it.
fn arrived_since(sent_at: &str, created_at: &str) -> bool {
    match (DateTime::parse_from_rfc3339(sent_at), DateTime::parse_from_rfc3339(created_at)) {
        (Ok(sent), Ok(created)) => sent >= created,
        _ => false,
    }
}

fn rule_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SenderRule> {
    Ok(SenderRule {
        id: row.get(0)?,
        account_id: row.get(1)?,
        match_kind: row.get(2)?,
        match_value: row.get(3)?,
        action: row.get(4)?,
        created_at: row.get(5)?,
    })
}

const RULE_COLUMNS: &str = "id, account_id, match_kind, match_value, action, created_at";

impl Database {
    pub fn list_sender_rules(&self) -> DbResult<Vec<SenderRule>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {RULE_COLUMNS} FROM sender_rules ORDER BY account_id, match_value, id"
            ))?;
            let rules = statement.query_map([], rule_from_row)?.collect::<Result<Vec<_>, _>>()?;
            Ok(rules)
        })
    }

    /// Mutes a sender in `account_id`, reusing an identical rule, and marks
    /// read and archives the listed conversations that are still in that
    /// account's Inbox, all in one transaction. The user's own addresses
    /// cannot be muted.
    pub fn mute_sender(
        &self,
        account_id: &str,
        match_kind: &str,
        match_value: &str,
        archive_thread_ids: &[String],
    ) -> DbResult<MuteResult> {
        let value = normalize_rule_match(match_kind, match_value)?;
        self.with_transaction(|transaction| {
            let known: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE lower(email) = lower(?1))",
                [account_id],
                |row| row.get(0),
            )?;
            if !known {
                return Err(DatabaseError::NotFound("Account"));
            }
            if match_kind == "address" && super::sending_addresses_for_account(transaction, account_id).contains(&value) {
                return Err(DatabaseError::invalid("You can't mute your own address"));
            }
            let existing = transaction
                .query_row(
                    &format!(
                        "SELECT {RULE_COLUMNS} FROM sender_rules
                         WHERE lower(account_id) = lower(?1) AND match_kind = ?2 AND match_value = ?3"
                    ),
                    params![account_id, match_kind, value],
                    rule_from_row,
                )
                .optional()?;
            let rule = match existing {
                Some(rule) => rule,
                None => {
                    let rule = SenderRule {
                        id: Uuid::new_v4().to_string(),
                        account_id: account_id.to_string(),
                        match_kind: match_kind.to_string(),
                        match_value: value.clone(),
                        action: "mute".into(),
                        created_at: Utc::now().to_rfc3339(),
                    };
                    transaction.execute(
                        "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                        params![rule.id, rule.account_id, rule.match_kind, rule.match_value, rule.action, rule.created_at],
                    )?;
                    rule
                }
            };
            let mut archived_thread_ids = Vec::new();
            let mut unread_thread_ids = Vec::new();
            for thread_id in archive_thread_ids {
                if archived_thread_ids.contains(thread_id) {
                    continue;
                }
                let unread: Option<bool> = transaction
                    .query_row(
                        "SELECT unread FROM threads
                         WHERE id = ?1 AND lower(account_id) = lower(?2) AND archived = 0 AND trashed = 0",
                        params![thread_id, account_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let Some(unread) = unread else { continue };
                // Read first: an IMAP archive moves the message, and the flag
                // is set where it is before the move.
                Self::apply_mutation(transaction, &ThreadMutation::Read { thread_id: thread_id.clone(), value: true })?;
                Self::apply_mutation(transaction, &ThreadMutation::Archive { thread_id: thread_id.clone(), value: true })?;
                if unread {
                    unread_thread_ids.push(thread_id.clone());
                }
                archived_thread_ids.push(thread_id.clone());
            }
            Ok(MuteResult { rule, archived_thread_ids, unread_thread_ids })
        })
    }

    pub fn delete_sender_rule(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sender_rules WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Stores a rule another device made. An invalid one is refused rather
    /// than stored half-checked.
    pub(crate) fn upsert_synced_sender_rule(&self, rule: &SenderRule) -> DbResult<()> {
        let value = normalize_rule_match(&rule.match_kind, &rule.match_value)?;
        if rule.action != "mute" || DateTime::parse_from_rfc3339(&rule.created_at).is_err() {
            return Err(DatabaseError::invalid("Invalid synchronized mute rule"));
        }
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET account_id = excluded.account_id, match_kind = excluded.match_kind,
                     match_value = excluded.match_value, action = excluded.action, updated_at = excluded.updated_at",
                params![rule.id, rule.account_id, rule.match_kind, value, rule.action, rule.created_at, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// Runs at the end of ingesting one conversation. When its newest message
    /// is from a muted sender, still unread, newer than the rule, and in the
    /// Inbox, the conversation is marked read and archived, and the message
    /// is logged so the rule acts on it once. Mail the user has read is left
    /// alone, so a conversation moved back to the Inbox stays there on every
    /// device: the first device to act also marks it read everywhere.
    pub(super) fn apply_sender_rules(
        transaction: &Transaction<'_>,
        account_id: &str,
        thread_id: &str,
        latest: &NormalizedMessage,
        mine: &BTreeSet<String>,
    ) -> DbResult<()> {
        let (email, domain) = normalize_sender(&latest.from);
        if email.is_empty() || mine.contains(&email) || !latest.labels.iter().any(|label| label == "UNREAD") {
            return Ok(());
        }
        let rule = {
            let mut statement = transaction.prepare_cached(&format!(
                "SELECT {RULE_COLUMNS} FROM sender_rules WHERE lower(account_id) = lower(?1) ORDER BY created_at, id"
            ))?;
            let rules = statement.query_map([account_id], rule_from_row)?.collect::<Result<Vec<_>, _>>()?;
            rules.into_iter().find(|rule| rule_matches(rule, &email, &domain) && arrived_since(&latest.date, &rule.created_at))
        };
        let Some(rule) = rule else { return Ok(()) };
        let in_inbox: bool = transaction
            .query_row("SELECT archived = 0 AND trashed = 0 FROM threads WHERE id = ?1", [thread_id], |row| row.get(0))
            .optional()?
            .unwrap_or(false);
        if !in_inbox {
            return Ok(());
        }
        let first_time = transaction.execute(
            "INSERT OR IGNORE INTO sender_rule_actions(account_id, message_id, rule_id, acted_at) VALUES (?1, ?2, ?3, ?4)",
            params![account_id, latest.id, rule.id, Utc::now().to_rfc3339()],
        )? > 0;
        if !first_time {
            return Ok(());
        }
        Self::apply_mutation(transaction, &ThreadMutation::Read { thread_id: thread_id.to_string(), value: true })?;
        Self::apply_mutation(transaction, &ThreadMutation::Archive { thread_id: thread_id.to_string(), value: true })
    }
}

impl Database {
    /// Removes a mute rule another device deleted.
    pub(crate) fn delete_synced_sender_rule(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sender_rules WHERE id=?1", [id])?;
            Ok(())
        })
    }
}

/// Settings import: clears every mute rule before the file's are added.
/// Removing a rule drops its action log.
pub(super) fn delete_all_sender_rules(connection: &rusqlite::Connection) -> DbResult<()> {
    connection.execute("DELETE FROM sender_rules", [])?;
    Ok(())
}

/// Settings import: adds one mute rule from the file, with its match value
/// already normalized.
pub(super) fn insert_imported_sender_rule(
    connection: &rusqlite::Connection,
    rule: &crate::transfer::TransferSenderRule,
    match_value: &str,
    now: &str,
) -> DbResult<()> {
    connection.execute(
        "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![rule.id, rule.account_id, rule.match_kind, match_value, rule.action, rule.created_at, now],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/sender_rules.rs"]
mod tests;
