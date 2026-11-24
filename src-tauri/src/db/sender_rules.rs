//! Muted and blocked senders. A rule names an address, or everyone at a
//! domain, in one account. While a mute exists, sync marks that sender's new
//! mail read and archives it; while a block exists, sync moves it to Spam.
//! Both act through the ordinary mutation queue, so the provider follows.
//! Mail older than the rule is left alone; muting can archive what is
//! already in the Inbox when the user asks.
//!
//! Blocks come only from Mark Spam, one address at a time (a domain block
//! could catch everyone at a shared mail provider). Spam senders come and go,
//! so blocks are pruned: each rule records when its sender's mail last
//! arrived, a block quiet for [`BLOCK_IDLE_DAYS`] is removed, and an account
//! keeps at most [`MAX_BLOCKED_SENDERS`]. Mutes are never pruned.

use super::{normalize_sender, Database, DatabaseError, DbResult};
use crate::mime::NormalizedMessage;
use crate::models::{MuteResult, SenderRule, ThreadMutation};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use threestrands_sync_protocol::EntityType;
use uuid::Uuid;

/// Most muted senders a settings export may carry.
pub(crate) const MAX_SENDER_RULES: usize = 5_000;

/// A block whose sender has sent nothing for longer than this many days is
/// removed. The clock runs from the latest of when the block was made (or
/// last refreshed by Mark Spam), when this device received it, and the date
/// of the newest mail it matched, so a sender that keeps writing stays
/// blocked and one that has gone away is forgotten.
pub(crate) const BLOCK_IDLE_DAYS: i64 = 180;

/// Most blocked senders one account keeps. Adding a block beyond this
/// removes the ones idle longest, by the same clock as [`BLOCK_IDLE_DAYS`].
/// Settings exports are held to it per account too.
pub(crate) const MAX_BLOCKED_SENDERS: usize = 2_000;
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
            && domain.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
            })
    };
    match match_kind {
        "address" => {
            let valid = value.len() <= MAX_ADDRESS_LEN
                && value.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty()
                        && !local.chars().any(|character| {
                            character.is_whitespace() || matches!(character, '<' | '>' | ',')
                        })
                        && valid_domain(domain)
                });
            if !valid {
                return Err(DatabaseError::invalid(
                    "Mute needs a complete email address",
                ));
            }
            Ok(value)
        }
        "domain" => {
            let domain = value.strip_prefix('@').unwrap_or(&value).to_string();
            if !valid_domain(&domain) {
                return Err(DatabaseError::invalid(
                    "Mute needs a domain such as example.com",
                ));
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
    match (
        DateTime::parse_from_rfc3339(sent_at),
        DateTime::parse_from_rfc3339(created_at),
    ) {
        (Ok(sent), Ok(created)) => sent >= created,
        _ => false,
    }
}

/// A timestamp as stored in `last_matched_at`: UTC, whole seconds, so
/// stored values compare correctly as text.
pub(crate) fn utc_stamp(value: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(value).ok().map(|date| {
        date.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

fn rule_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SenderRule> {
    Ok(SenderRule {
        id: row.get(0)?,
        account_id: row.get(1)?,
        match_kind: row.get(2)?,
        match_value: row.get(3)?,
        action: row.get(4)?,
        created_at: row.get(5)?,
        last_matched_at: row.get(6)?,
    })
}

const RULE_COLUMNS: &str =
    "id, account_id, match_kind, match_value, action, created_at, last_matched_at";

/// What Mark Spam, Not spam, Undo, or pruning did to blocked senders, so
/// the caller can tell other devices and offer Undo.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SenderBlockChanges {
    /// Blocks added or refreshed. Other devices receive these.
    pub written: Vec<SenderRule>,
    /// Those of them that did not exist before: what Undo removes.
    pub created_ids: Vec<String>,
    /// Blocks removed.
    pub removed_ids: Vec<String>,
    /// The full rows Not spam removed, so the caller can name the unblocked
    /// addresses and Undo can put the same rows back.
    pub unblocked: Vec<SenderRule>,
}

/// How a call to [`Database::apply_user_mutations_with`] relates to blocks.
#[derive(Debug, Clone, Copy, Default)]
pub enum BlockUndo<'a> {
    /// A direct user action: Mark Spam blocks, Not spam unblocks.
    #[default]
    None,
    /// Undo of a Mark Spam: Not spam keeps the senders' blocks, and only
    /// these blocks, the ones that Mark Spam created, go.
    MarkSpam(&'a [String]),
    /// Undo of a Not spam: marking spam adds no new block; these exact rows,
    /// the ones Not spam removed, come back with their ids and dates.
    NotSpam(&'a [SenderRule]),
}

impl SenderBlockChanges {
    pub fn is_empty(&self) -> bool {
        self.written.is_empty() && self.removed_ids.is_empty()
    }
}

/// The account and normalized address of the newest message in a
/// conversation that the user did not send. `None` when the conversation is
/// unknown or every message is the user's own.
fn latest_inbound_sender(
    transaction: &Transaction<'_>,
    thread_id: &str,
) -> DbResult<Option<(String, String)>> {
    let account_id: Option<String> = transaction
        .query_row(
            "SELECT account_id FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(account_id) = account_id else {
        return Ok(None);
    };
    let mine = super::sending_addresses_for_account(transaction, &account_id);
    let mut statement = transaction.prepare_cached(
        "SELECT sender FROM messages WHERE thread_id = ?1 ORDER BY sent_at DESC, id DESC",
    )?;
    let senders = statement
        .query_map([thread_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(senders
        .iter()
        .map(|sender| normalize_sender(sender).0)
        .find(|email| !email.is_empty() && !mine.contains(email))
        .map(|email| (account_id, email)))
}

/// Blocks the sender of a conversation's latest inbound message. An
/// existing block for that address is refreshed rather than duplicated.
/// Returns the rule and whether it is new.
fn block_thread_sender(
    transaction: &Transaction<'_>,
    thread_id: &str,
    now: &str,
) -> DbResult<Option<(SenderRule, bool)>> {
    let Some((account_id, email)) = latest_inbound_sender(transaction, thread_id)? else {
        return Ok(None);
    };
    // An address the rules can't hold (a malformed From) is not blocked.
    let Ok(address) = normalize_rule_match("address", &email) else {
        return Ok(None);
    };
    let existing = transaction
        .query_row(
            &format!(
                "SELECT {RULE_COLUMNS} FROM sender_rules
                 WHERE lower(account_id) = lower(?1) AND action = 'block' AND match_value = ?2
                 ORDER BY created_at, id LIMIT 1"
            ),
            params![account_id, address],
            rule_from_row,
        )
        .optional()?;
    if let Some(mut rule) = existing {
        transaction.execute(
            "UPDATE sender_rules SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![rule.id, now],
        )?;
        rule.created_at = now.to_string();
        return Ok(Some((rule, false)));
    }
    let rule = SenderRule {
        id: Uuid::new_v4().to_string(),
        account_id,
        match_kind: "address".into(),
        match_value: address,
        action: "block".into(),
        created_at: now.to_string(),
        last_matched_at: None,
    };
    transaction.execute(
        "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![rule.id, rule.account_id, rule.match_kind, rule.match_value, rule.action, rule.created_at],
    )?;
    Ok(Some((rule, true)))
}

/// Not spam: removes every block on the sender of a conversation's latest
/// inbound message. Returns the removed rules.
fn unblock_thread_sender(
    transaction: &Transaction<'_>,
    thread_id: &str,
) -> DbResult<Vec<SenderRule>> {
    let Some((account_id, email)) = latest_inbound_sender(transaction, thread_id)? else {
        return Ok(Vec::new());
    };
    let rules = {
        let mut statement = transaction.prepare_cached(&format!(
            "SELECT {RULE_COLUMNS} FROM sender_rules
             WHERE lower(account_id) = lower(?1) AND action = 'block' AND match_value = ?2
             ORDER BY created_at, id"
        ))?;
        let rules = statement
            .query_map(params![account_id, email], rule_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        rules
    };
    for rule in &rules {
        transaction.execute("DELETE FROM sender_rules WHERE id = ?1", [&rule.id])?;
    }
    Ok(rules)
}

/// Undo of Not spam: puts a removed block back as it was. Skipped when the
/// sender has been blocked again meanwhile, so there is one block per address.
fn restore_block(transaction: &Transaction<'_>, rule: &SenderRule, now: &str) -> DbResult<bool> {
    if rule.action != "block" {
        return Ok(false);
    }
    let present: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sender_rules WHERE id = ?1
            OR (lower(account_id) = lower(?2) AND action = 'block' AND match_value = ?3))",
        params![rule.id, rule.account_id, rule.match_value],
        |row| row.get(0),
    )?;
    if present {
        return Ok(false);
    }
    transaction.execute(
        "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at, last_matched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            rule.id,
            rule.account_id,
            rule.match_kind,
            rule.match_value,
            rule.action,
            rule.created_at,
            now,
            rule.last_matched_at
        ],
    )?;
    Ok(true)
}

/// Removes blocks idle longer than [`BLOCK_IDLE_DAYS`], then, in each
/// account over [`MAX_BLOCKED_SENDERS`], the ones idle longest. Limited to
/// one account when `account_id` is given. Returns the removed ids.
fn prune_blocks(
    transaction: &Transaction<'_>,
    account_id: Option<&str>,
    now: DateTime<Utc>,
) -> DbResult<Vec<String>> {
    let rows = {
        let mut statement = transaction.prepare_cached(
            "SELECT id, lower(account_id), created_at, last_matched_at, updated_at FROM sender_rules
             WHERE action = 'block' AND (?1 IS NULL OR lower(account_id) = lower(?1))",
        )?;
        let rows = statement
            .query_map([account_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let cutoff = now - Duration::days(BLOCK_IDLE_DAYS);
    let mut removed = Vec::new();
    let mut kept: BTreeMap<String, Vec<(DateTime<Utc>, String)>> = BTreeMap::new();
    for (id, account, created_at, last_matched_at, updated_at) in rows {
        // The block's clock: its newest evidence that the sender is still
        // around. A date that doesn't parse never ages a block out.
        let active = [Some(created_at), last_matched_at, updated_at]
            .into_iter()
            .flatten()
            .filter_map(|value| DateTime::parse_from_rfc3339(&value).ok())
            .map(|date| date.with_timezone(&Utc))
            .max()
            .unwrap_or(now);
        if active < cutoff {
            removed.push(id);
        } else {
            kept.entry(account).or_default().push((active, id));
        }
    }
    for mut blocks in kept.into_values() {
        if blocks.len() > MAX_BLOCKED_SENDERS {
            blocks.sort();
            let excess = blocks.len() - MAX_BLOCKED_SENDERS;
            removed.extend(blocks.into_iter().take(excess).map(|(_, id)| id));
        }
    }
    for id in &removed {
        transaction.execute("DELETE FROM sender_rules WHERE id = ?1", [id])?;
    }
    Ok(removed)
}

impl Database {
    pub fn list_sender_rules(&self) -> DbResult<Vec<SenderRule>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {RULE_COLUMNS} FROM sender_rules ORDER BY account_id, match_value, id"
            ))?;
            let rules = statement
                .query_map([], rule_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
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
                         WHERE lower(account_id) = lower(?1) AND match_kind = ?2 AND match_value = ?3
                             AND action = 'mute'"
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
                        last_matched_at: None,
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
    /// than stored half-checked. `updated_at` records when it arrived, so a
    /// block this device has only just learned of is not pruned as idle
    /// before this device has seen any of the sender's mail. A block
    /// refreshed by Mark Spam elsewhere arrives with its new `created_at`.
    pub(crate) fn upsert_synced_sender_rule(&self, rule: &SenderRule) -> DbResult<()> {
        let value = normalize_rule_match(&rule.match_kind, &rule.match_value)?;
        let known_action =
            rule.action == "mute" || (rule.action == "block" && rule.match_kind == "address");
        if !known_action || DateTime::parse_from_rfc3339(&rule.created_at).is_err() {
            return Err(DatabaseError::invalid("Invalid synchronized sender rule"));
        }
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET account_id = excluded.account_id, match_kind = excluded.match_kind,
                     match_value = excluded.match_value, action = excluded.action,
                     created_at = excluded.created_at, updated_at = excluded.updated_at",
                params![rule.id, rule.account_id, rule.match_kind, value, rule.action, rule.created_at, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// Runs at the end of ingesting one conversation. Every rule matching
    /// the newest message's sender, for mail newer than the rule, records
    /// the message's date as when it last matched. Then, when that message
    /// is still unread, a block moves the conversation to Spam unless it is
    /// already there or in Trash; otherwise a mute marks it read and
    /// archives it if it is in the Inbox. A block outranks a mute. The
    /// message is logged so a rule acts on it once. Mail the user has read
    /// is left alone, so a conversation moved back stays there on every
    /// device: the first device to act also changes it everywhere.
    pub(super) fn apply_sender_rules(
        transaction: &Transaction<'_>,
        account_id: &str,
        thread_id: &str,
        latest: &NormalizedMessage,
        mine: &BTreeSet<String>,
    ) -> DbResult<()> {
        let (email, domain) = normalize_sender(&latest.from);
        if email.is_empty() || mine.contains(&email) {
            return Ok(());
        }
        let matching = {
            let mut statement = transaction.prepare_cached(&format!(
                "SELECT {RULE_COLUMNS} FROM sender_rules WHERE lower(account_id) = lower(?1) ORDER BY created_at, id"
            ))?;
            let rules = statement
                .query_map([account_id], rule_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            rules
                .into_iter()
                .filter(|rule| {
                    rule_matches(rule, &email, &domain)
                        && arrived_since(&latest.date, &rule.created_at)
                })
                .collect::<Vec<_>>()
        };
        if matching.is_empty() {
            return Ok(());
        }
        if let Some(seen) = utc_stamp(&latest.date) {
            for rule in &matching {
                transaction.execute(
                    "UPDATE sender_rules SET last_matched_at = ?2
                     WHERE id = ?1 AND (last_matched_at IS NULL OR last_matched_at < ?2)",
                    params![rule.id, seen],
                )?;
            }
        }
        if !latest.labels.iter().any(|label| label == "UNREAD") {
            return Ok(());
        }
        let rule = matching
            .iter()
            .find(|rule| rule.action == "block")
            .unwrap_or(&matching[0]);
        let block = rule.action == "block";
        let actionable: bool = transaction
            .query_row(
                if block {
                    "SELECT trashed = 0 AND NOT EXISTS(SELECT 1 FROM json_each(labels_json) WHERE value = 'SPAM')
                     FROM threads WHERE id = ?1"
                } else {
                    "SELECT archived = 0 AND trashed = 0 FROM threads WHERE id = ?1"
                },
                [thread_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !actionable {
            return Ok(());
        }
        let first_time = transaction.execute(
            "INSERT OR IGNORE INTO sender_rule_actions(account_id, message_id, rule_id, acted_at) VALUES (?1, ?2, ?3, ?4)",
            params![account_id, latest.id, rule.id, Utc::now().to_rfc3339()],
        )? > 0;
        if !first_time {
            return Ok(());
        }
        if block {
            return Self::apply_mutation(
                transaction,
                &ThreadMutation::Spam {
                    thread_id: thread_id.to_string(),
                    value: true,
                },
            );
        }
        Self::apply_mutation(
            transaction,
            &ThreadMutation::Read {
                thread_id: thread_id.to_string(),
                value: true,
            },
        )?;
        Self::apply_mutation(
            transaction,
            &ThreadMutation::Archive {
                thread_id: thread_id.to_string(),
                value: true,
            },
        )
    }

    /// Applies the user's conversation changes, as [`Self::mutate_threads`]
    /// does, and keeps blocked senders in step with Spam in the same
    /// transaction. Mark Spam blocks each conversation's latest inbound
    /// sender (the user's own addresses never), and prunes that account's
    /// blocks. Not spam unblocks that sender. When `undo_blocks` is given
    /// the call is Undo of a Mark Spam: Not spam keeps the senders' blocks,
    /// and only the listed blocks, the ones that Mark Spam created, go.
    #[cfg(test)]
    pub fn apply_user_mutations(
        &self,
        mutations: &[ThreadMutation],
        undo_blocks: Option<&[String]>,
    ) -> DbResult<SenderBlockChanges> {
        self.apply_user_mutations_with(
            mutations,
            undo_blocks.map_or(BlockUndo::None, BlockUndo::MarkSpam),
        )
    }

    /// [`Self::apply_user_mutations`] that can also be Undo of a Not spam
    /// ([`BlockUndo::NotSpam`]).
    pub fn apply_user_mutations_with(
        &self,
        mutations: &[ThreadMutation],
        undo: BlockUndo<'_>,
    ) -> DbResult<SenderBlockChanges> {
        let now = Utc::now();
        let stamp = now.to_rfc3339();
        self.with_transaction(|transaction| {
            let mut changes = SenderBlockChanges::default();
            let mut blocked_accounts = BTreeSet::new();
            for mutation in mutations {
                Self::cancel_snooze_for_mutation(transaction, mutation)?;
                Self::apply_mutation(transaction, mutation)?;
                let ThreadMutation::Spam { thread_id, value } = mutation else {
                    continue;
                };
                if *value {
                    if matches!(undo, BlockUndo::NotSpam(_)) {
                        continue;
                    }
                    if let Some((rule, created)) =
                        block_thread_sender(transaction, thread_id, &stamp)?
                    {
                        if created {
                            changes.created_ids.push(rule.id.clone());
                        }
                        blocked_accounts.insert(rule.account_id.clone());
                        changes.written.retain(|written| written.id != rule.id);
                        changes.written.push(rule);
                    }
                } else if matches!(undo, BlockUndo::None) {
                    for rule in unblock_thread_sender(transaction, thread_id)? {
                        if !changes.removed_ids.contains(&rule.id) {
                            changes.removed_ids.push(rule.id.clone());
                            changes.unblocked.push(rule);
                        }
                    }
                }
            }
            if let BlockUndo::NotSpam(rules) = undo {
                for rule in rules {
                    if restore_block(transaction, rule, &stamp)? {
                        let mut restored = rule.clone();
                        restored.action = "block".into();
                        changes.written.push(restored);
                    }
                }
            }
            let mark_spam_blocks = match undo {
                BlockUndo::MarkSpam(ids) => ids,
                _ => &[],
            };
            for id in mark_spam_blocks {
                let removed = transaction.execute(
                    "DELETE FROM sender_rules WHERE id = ?1 AND action = 'block'",
                    [id],
                )? > 0;
                if removed && !changes.removed_ids.contains(id) {
                    changes.removed_ids.push(id.clone());
                }
            }
            for account in &blocked_accounts {
                changes
                    .removed_ids
                    .extend(prune_blocks(transaction, Some(account), now)?);
            }
            let removed = changes.removed_ids.clone();
            changes.written.retain(|rule| !removed.contains(&rule.id));
            changes.created_ids.retain(|id| !removed.contains(id));
            Ok(changes)
        })
    }

    /// Tells other devices what [`Self::apply_user_mutations`] or pruning
    /// changed. A no-op unless replicated sync is active.
    pub fn record_sender_block_changes(&self, changes: &SenderBlockChanges) -> DbResult<()> {
        for rule in &changes.written {
            self.record_local_entity_write(
                EntityType::SenderRule,
                &rule.id,
                rule.sync_payload(),
                None,
            )?;
        }
        for id in &changes.removed_ids {
            self.record_local_entity_deletion(EntityType::SenderRule, id)?;
        }
        Ok(())
    }

    /// Periodic maintenance: removes blocks idle longer than
    /// [`BLOCK_IDLE_DAYS`], and any over [`MAX_BLOCKED_SENDERS`] (blocks
    /// arriving from other devices can push an account past it), and tells
    /// other devices. Returns how many were removed.
    pub fn prune_idle_blocked_senders(&self) -> DbResult<usize> {
        self.prune_blocked_senders_at(Utc::now())
    }

    pub(crate) fn prune_blocked_senders_at(&self, now: DateTime<Utc>) -> DbResult<usize> {
        let removed_ids =
            self.with_transaction(|transaction| prune_blocks(transaction, None, now))?;
        let count = removed_ids.len();
        self.record_sender_block_changes(&SenderBlockChanges {
            removed_ids,
            ..SenderBlockChanges::default()
        })?;
        Ok(count)
    }
}

impl Database {
    /// Removes a mute or block another device deleted.
    pub(crate) fn delete_synced_sender_rule(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sender_rules WHERE id=?1", [id])?;
            Ok(())
        })
    }
}

/// Settings import: clears every rule with this action (`mute` or `block`)
/// before the file's are added. Removing a rule drops its action log.
pub(super) fn delete_all_sender_rules(
    connection: &rusqlite::Connection,
    action: &str,
) -> DbResult<()> {
    connection.execute("DELETE FROM sender_rules WHERE action = ?1", [action])?;
    Ok(())
}

/// Settings import: adds one blocked sender from the file, with its address
/// already normalized. `updated_at` is the import time, so an imported block
/// is not pruned before this device has had a chance to see its mail.
pub(super) fn insert_imported_blocked_sender(
    connection: &rusqlite::Connection,
    block: &crate::transfer::TransferBlockedSender,
    address: &str,
    now: &str,
) -> DbResult<()> {
    connection.execute(
        "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at, last_matched_at)
         VALUES (?1, ?2, 'address', ?3, 'block', ?4, ?5, ?6)",
        params![
            block.id,
            block.account_id,
            address,
            block.created_at,
            now,
            block.last_matched_at.as_deref().and_then(utc_stamp),
        ],
    )?;
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
