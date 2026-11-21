use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use threestrands_sync_protocol::EntityType;

use super::{account_from_row, purge_imap_cache, purge_mail_traces, purge_outgoing_mail};
use crate::db::sync_state::{
    record_replicated_deletion_in_transaction, record_replicated_write_in_transaction,
};
use crate::db::{serialization_error, Database, DatabaseError, DbResult};
use crate::models::{Account, Label, SplitInbox};
use crate::provider::imap::{Identity, ImapAccountSettings};
use crate::provider::SendingIdentity;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GmailImapSwitchPreview {
    task_count: i64,
    goal_count: i64,
    unverified_task_links: Vec<String>,
    split_inboxes: Vec<SplitLabel>,
    labels: Vec<Label>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SplitLabel {
    id: String,
    name: String,
    old_label_id: String,
    old_label_name: Option<String>,
    suggested_label_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SplitLabelChoice {
    pub(crate) id: String,
    pub(crate) old_label_id: String,
    pub(crate) label_id: Option<String>,
}

impl Database {
    pub(crate) fn require_gmail_imap_switch(&self, email: &str) -> DbResult<()> {
        self.with_connection(|connection| require_switchable_account(connection, email))
    }

    pub(crate) fn preview_gmail_imap_switch(
        &self,
        email: &str,
        labels: Vec<Label>,
    ) -> DbResult<GmailImapSwitchPreview> {
        self.with_connection(|connection| {
            require_switchable_account(connection, email)?;
            let task_count = connection.query_row(
                "SELECT COUNT(*) FROM tasks WHERE account_id=?1",
                [email],
                |row| row.get(0),
            )?;
            let goal_count = connection.query_row(
                "SELECT COUNT(*) FROM goals WHERE account_id=?1",
                [email],
                |row| row.get(0),
            )?;
            let mut statement = connection.prepare(
                "SELECT title,thread_id,source_message_id FROM tasks WHERE account_id=?1
                 AND (thread_id IS NOT NULL OR source_message_id IS NOT NULL) ORDER BY title,id",
            )?;
            let prefix = format!("{email}:");
            let mut unverified_task_links = Vec::new();
            for row in statement.query_map([email], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })? {
                let (title, thread, message) = row?;
                let valid_thread = thread
                    .as_deref()
                    .and_then(|id| id.strip_prefix(&prefix))
                    .is_some_and(is_canonical_gmail_id);
                if !valid_thread
                    || message
                        .as_deref()
                        .is_some_and(|id| !is_canonical_gmail_id(id))
                {
                    unverified_task_links.push(title);
                }
            }
            let mut split_inboxes = Vec::new();
            for split in label_splits(connection, email)? {
                let old_label_name: Option<String> = connection
                    .query_row(
                        "SELECT name FROM account_labels WHERE account_id=?1 AND label_id=?2",
                        params![email, split.match_value],
                        |row| row.get(0),
                    )
                    .optional()?;
                let mut candidates = labels.iter().filter(|label| {
                    if label.kind == "system" {
                        label.id == split.match_value
                    } else {
                        old_label_name.as_deref() == Some(label.name.as_str())
                    }
                });
                let suggested_label_id = candidates
                    .next()
                    .filter(|_| candidates.next().is_none())
                    .map(|label| label.id.clone());
                split_inboxes.push(SplitLabel {
                    id: split.id,
                    name: split.name,
                    old_label_id: split.match_value,
                    old_label_name,
                    suggested_label_id,
                });
            }
            Ok(GmailImapSwitchPreview {
                task_count,
                goal_count,
                unverified_task_links,
                split_inboxes,
                labels,
            })
        })
    }

    /// Canonical Gmail API keys are distinct from normal IMAP conversation
    /// IDs. Recover only a key the switch actually reserved for this account.
    pub(crate) fn converted_gmail_thread(&self, id: &str) -> DbResult<Option<(String, String)>> {
        let Some((email, thread)) = id.rsplit_once(':') else {
            return Ok(None);
        };
        if !is_canonical_gmail_id(thread) {
            return Ok(None);
        }
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT g.account_id,g.thread_id FROM imap_gmail_threads g
                 JOIN accounts a ON a.email=g.account_id
                 WHERE g.account_id=?1 AND g.gmail_thread_id=?2 AND g.thread_id=?2
                    AND a.provider='imap' AND a.on_this_device=1",
                    params![email, thread],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?)
        })
    }

    /// One database commit changes the provider, the saved connection, and the
    /// identity bindings. The account row and workflow primary keys stay put.
    pub(crate) fn switch_gmail_to_imap(
        &self,
        email: &str,
        mut settings: ImapAccountSettings,
        labels: &[Label],
        choices: &[SplitLabelChoice],
    ) -> DbResult<Account> {
        let sync_active = self.replicated_sync_active()?;
        self.with_transaction(|tx| {
            require_switchable_account(tx, email)?;
            let splits = label_splits(tx, email)?;
            let selected: BTreeMap<_, _> = choices.iter().map(|choice| (choice.id.as_str(), choice)).collect();
            if selected.len() != choices.len() || selected.len() != splits.len() ||
                splits.iter().any(|split| selected.get(split.id.as_str()).is_none_or(|choice| {
                    choice.old_label_id != split.match_value ||
                    choice.label_id.as_ref().is_some_and(|id| !labels.iter().any(|label| label.id == *id))
                }))
            {
                return Err(DatabaseError::invalid("The labels or Split Inboxes changed. Review the switch again."));
            }

            if let Some(identities) = crate::db::gmail_identities::cached_gmail_identities(tx, email)? {
                settings.identities = identities.into_iter().map(|SendingIdentity { address, display_name, reply_to, is_default }| {
                    Identity { address, display_name, reply_to, is_default }
                }).collect();
            }
            // A paused, previously enrolled replica also needs the updated
            // provider and rules when it next resumes.
            let publish = sync_active || tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_devices WHERE is_self=1)", [], |row| row.get::<_, bool>(0),
            )?;
            if publish {
                discard_replica_schedules(tx, email)?;
            }
            purge_imap_cache(tx, email)?;
            reserve_gmail_ids(tx, email)?;

            tx.execute(
                "DELETE FROM thread_search WHERE thread_id IN (SELECT id FROM threads WHERE account_id=?1)",
                [email],
            )?;
            purge_mail_traces(tx, email)?;
            purge_outgoing_mail(tx, email)?;
            for table in ["threads", "mutations", "sync_state", "sync_recovery_threads",
                "sync_recovery", "quarantined_messages", "conversation_snoozes", "gmail_sending_identities"]
            {
                tx.execute(&format!("DELETE FROM {table} WHERE account_id=?1"), [email])?;
            }
            tx.execute("INSERT INTO sync_state(account_id) VALUES(?1)", [email])?;

            for mut split in splits {
                let Some(id) = &selected[split.id.as_str()].label_id else { continue };
                if split.match_value == *id { continue; }
                tx.execute("UPDATE split_inboxes SET match_value=?1 WHERE id=?2", params![id, split.id])?;
                split.match_value.clone_from(id);
                if publish {
                    record_replica_fields(tx, EntityType::SplitInbox, &split.id, &split)?;
                }
            }
            super::super::imap::write_settings_row(tx, email, &settings)?;
            let mut account = tx.query_row(
                "UPDATE accounts SET provider='imap', status='connected', last_synced_at=NULL
                 WHERE email=?1 RETURNING email,display_name,color,status,provider,sort_order,
                    connected_at,last_synced_at,nickname,on_this_device",
                [email], account_from_row,
            )?;
            tx.execute(
                "INSERT INTO imap_server_capabilities(account_id,native_labels,native_labels_seen) VALUES(?1,1,1)",
                [email],
            )?;
            account.label_model = crate::provider::LabelModel::ImapNativeLabels;
            if publish {
                record_replica_fields(tx, EntityType::MailAccount, &email.to_ascii_lowercase(), &serde_json::json!({
                    "email": email,
                    "displayName": account.display_name,
                    "nickname": account.nickname,
                    "color": account.color,
                    "provider": account.provider,
                    "sortOrder": account.sort_order,
                }))?;
            }
            Ok(account)
        })
    }
}

fn require_switchable_account(connection: &Connection, email: &str) -> DbResult<()> {
    let gmail = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE email=?1 AND provider='gmail' AND on_this_device=1)",
        [email], |row| row.get::<_, bool>(0),
    )?;
    if !gmail {
        return Err(DatabaseError::invalid(
            "This account is no longer set up with Gmail on this device.",
        ));
    }
    let has_other_device: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_devices WHERE is_self=0 AND status='active')",
        [],
        |row| row.get(0),
    )?;
    if has_other_device {
        return Err(DatabaseError::invalid("Switching this connection is available when this is the only device enrolled in Replicated Sync."));
    }
    Ok(())
}

fn label_splits(connection: &Connection, email: &str) -> DbResult<Vec<SplitInbox>> {
    let mut statement = connection.prepare(
        "SELECT id,name,match_kind,match_value,sort_order,created_at,account_id
         FROM split_inboxes WHERE account_id=?1 AND match_kind='label' ORDER BY sort_order,id",
    )?;
    let rows = statement.query_map([email], |row| {
        Ok(SplitInbox {
            id: row.get(0)?,
            name: row.get(1)?,
            match_kind: row.get(2)?,
            match_value: row.get(3)?,
            sort_order: row.get(4)?,
            created_at: row.get(5)?,
            account_id: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn is_canonical_gmail_id(id: &str) -> bool {
    u64::from_str_radix(id, 16).is_ok_and(|number| format!("{number:x}") == id)
}

fn reserve_gmail_ids(tx: &Transaction<'_>, email: &str) -> DbResult<()> {
    // Gmail API IDs are the canonical hexadecimal spelling of Gmail's native
    // IMAP IDs. Ingest uses the same bindings after observing the native IDs
    // on the wire. No RFC Message-ID, subject or sender heuristic is used here.
    let mut threads = tx.prepare(
        "SELECT provider_thread_id FROM threads WHERE account_id=?1
         UNION SELECT substr(thread_id,length(?1)+2) FROM tasks
         WHERE account_id=?1 AND substr(thread_id,1,length(?1)+1)=?1||':'",
    )?;
    let mut insert = tx.prepare(
        "INSERT INTO imap_gmail_threads(account_id,gmail_thread_id,thread_id) VALUES(?1,?2,?2)",
    )?;
    for row in threads.query_map([email], |row| row.get::<_, String>(0))? {
        let id = row?;
        if is_canonical_gmail_id(&id) {
            insert.execute(params![email, id])?;
        }
    }
    let mut messages = tx.prepare(
        "SELECT m.id FROM messages m JOIN threads t ON t.id=m.thread_id WHERE t.account_id=?1
         UNION SELECT source_message_id FROM tasks WHERE account_id=?1 AND source_message_id IS NOT NULL",
    )?;
    let mut insert = tx.prepare(
        "INSERT INTO imap_message_identities(account_id,namespace,external_id,message_id) VALUES(?1,'gmail-message-id',?2,?2)",
    )?;
    for row in messages.query_map([email], |row| row.get::<_, String>(0))? {
        let id = row?;
        if is_canonical_gmail_id(&id) {
            insert.execute(params![email, id])?;
        }
    }
    Ok(())
}

fn record_replica_fields(
    tx: &Transaction<'_>,
    kind: EntityType,
    id: &str,
    value: &impl Serialize,
) -> DbResult<()> {
    let payload = serde_json::to_value(value).map_err(serialization_error)?;
    let fields: BTreeSet<_> = payload
        .as_object()
        .into_iter()
        .flat_map(|fields| fields.keys().cloned())
        .collect();
    record_replicated_write_in_transaction(tx, kind, id, &fields, &payload)
}

fn discard_replica_schedules(tx: &Transaction<'_>, email: &str) -> DbResult<()> {
    for (kind, sql) in [
        (EntityType::ConversationSnooze, "SELECT id FROM conversation_snoozes WHERE account_id=?1"),
        (EntityType::ScheduledSendSummary, "SELECT id FROM scheduled_send_summaries WHERE json_valid(report) AND lower(json_extract(report, '$.account'))=lower(?1)"),
    ] {
        let mut statement = tx.prepare(sql)?;
        let ids = statement.query_map([email], |row| row.get::<_, String>(0))?;
        for id in ids {
            record_replicated_deletion_in_transaction(tx, kind, &id?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{message, table_rows};
    use crate::models::{CreateGoalRequest, CreateTaskRequest};
    use crate::provider::imap::{ImapStateStore, LabelStorage, SecurityMode};
    use std::sync::Arc;

    const ACCOUNT: &str = "switch@example.com";
    const OTHER: &str = "other@example.com";

    fn settings() -> ImapAccountSettings {
        ImapAccountSettings {
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            imap_security: SecurityMode::ImplicitTls,
            imap_username: ACCOUNT.into(),
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            smtp_security: SecurityMode::StartTls,
            smtp_username: ACCOUNT.into(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::Folders,
            label_container: None,
            identities: vec![Identity {
                address: ACCOUNT.into(),
                ..Default::default()
            }],
            pinned_fingerprints: Default::default(),
            server_saves_sent: false,
        }
    }

    fn destination() -> Label {
        Label {
            id: "folder:Projects/Orchard".into(),
            name: "Projects/Orchard".into(),
            kind: "user".into(),
        }
    }

    #[test]
    fn switch_preserves_workflow_and_account_settings_and_reserves_existing_gmail_ids() {
        let db = Arc::new(Database::open_memory());
        db.adopt_account(ACCOUNT).unwrap();
        db.adopt_account(OTHER).unwrap();
        db.set_account_display_name(ACCOUNT, Some("Green Orchard"))
            .unwrap();
        db.set_account_nickname(ACCOUNT, Some("Orchard")).unwrap();
        db.set_account_color(ACCOUNT, "#6D4AFF").unwrap();
        let alias = SendingIdentity {
            address: "orchard@example.com".into(),
            is_default: true,
            ..Default::default()
        };
        db.store_gmail_sending_identities(
            ACCOUNT,
            &[
                SendingIdentity {
                    address: ACCOUNT.into(),
                    ..Default::default()
                },
                alias.clone(),
            ],
            1,
        )
        .unwrap();
        db.store_signature_defaults(&crate::signatures::SignatureDefaults {
            id: crate::signatures::SignatureDefaults::key(ACCOUNT, ""),
            account: ACCOUNT.into(),
            address: String::new(),
            new_signature_id: Some("orchard-signature".into()),
            reply_signature_id: None,
        })
        .unwrap();
        db.mute_sender(ACCOUNT, "domain", "example.net", &[])
            .unwrap();
        let labels = vec![destination()];
        db.replace_account_labels(
            ACCOUNT,
            &[Label {
                id: "Label_1".into(),
                name: labels[0].name.clone(),
                kind: "user".into(),
            }],
        )
        .unwrap();

        let mapped = db
            .create_split_inbox("Projects", "label", "Label_1", ACCOUNT)
            .unwrap();
        let unresolved = db
            .create_split_inbox("Writers", "label", "Label_2", ACCOUNT)
            .unwrap();
        let domain = db
            .create_split_inbox("Vendors", "domain", "example.net", ACCOUNT)
            .unwrap();
        db.create_split_inbox("Separate account", "label", "Label_1", OTHER)
            .unwrap();
        let goal = db
            .create_goal(&CreateGoalRequest {
                account_id: ACCOUNT.into(),
                title: "Orchard review".into(),
                notes: Some("Planning notes".into()),
                horizon: "year".into(),
                period: "2026".into(),
                parent_goal_id: None,
            })
            .unwrap();
        let thread_id = format!("{ACCOUNT}:7d1");
        db.upsert_thread(
            ACCOUNT,
            &[message(
                "44d",
                "7d1",
                "2026-01-01T00:00:00Z",
                "Orchard review",
            )],
        )
        .unwrap();
        db.upsert_thread(
            OTHER,
            &[message(
                "44e",
                "7d1",
                "2026-01-01T00:00:00Z",
                "Separate conversation",
            )],
        )
        .unwrap();
        let create_task = |thread: String, msg: &str, title: &str| {
            db.create_task(&CreateTaskRequest {
                account_id: ACCOUNT.into(),
                thread_id: Some(thread),
                source_message_id: Some(msg.into()),
                subject_snapshot: Some("Orchard review".into()),
                title: title.into(),
                notes: Some("Follow up by Friday".into()),
                kind: "follow_up".into(),
                due_kind: "date".into(),
                due_value: Some("2026-01-08".into()),
                time_zone: None,
                repeat_interval_days: None,
                evidence_text: None,
                goal_id: Some(goal.id.clone()),
            })
            .unwrap()
        };
        let task = create_task(thread_id.clone(), "44d", "Review the plan");
        db.set_task_status(&task.id, "completed", "user").unwrap();
        db.upsert_thread(
            ACCOUNT,
            &[message(
                "450",
                "7d2",
                "2026-01-01T00:00:00Z",
                "Archived mail",
            )],
        )
        .unwrap();
        let cold = create_task(format!("{ACCOUNT}:7d2"), "450", "Cold archive task");
        db.delete_thread(ACCOUNT, "7d2").unwrap();
        db.upsert_thread(
            ACCOUNT,
            &[message(
                "old-key",
                "unknown",
                "2026-01-01T00:00:00Z",
                "Legacy cached mail",
            )],
        )
        .unwrap();
        create_task(format!("{ACCOUNT}:unknown"), "old-key", "Legacy reference");
        db.snooze_threads(&[thread_id.clone()], "2099-01-01T00:00:00Z")
            .unwrap();

        let preview = db
            .preview_gmail_imap_switch(ACCOUNT, labels.clone())
            .unwrap();
        assert_eq!(preview.task_count, 3);
        assert_eq!(preview.goal_count, 1);
        assert_eq!(preview.unverified_task_links, ["Legacy reference"]);
        assert_eq!(
            preview.split_inboxes[0].suggested_label_id.as_deref(),
            Some("folder:Projects/Orchard")
        );
        assert_eq!(preview.split_inboxes[1].suggested_label_id, None);
        let preserved = ["tasks", "goals", "signature_defaults", "sender_rules"];
        let before = preserved.map(|table| table_rows(&db, table));

        let updated = db
            .switch_gmail_to_imap(
                ACCOUNT,
                settings(),
                &labels,
                &[
                    SplitLabelChoice {
                        id: mapped.id.clone(),
                        old_label_id: "Label_1".into(),
                        label_id: Some(labels[0].id.clone()),
                    },
                    SplitLabelChoice {
                        id: unresolved.id.clone(),
                        old_label_id: "Label_2".into(),
                        label_id: None,
                    },
                ],
            )
            .unwrap();
        assert_eq!(updated.email, ACCOUNT);
        assert_eq!(updated.provider, "imap");
        assert_eq!(updated.nickname.as_deref(), Some("Orchard"));
        assert_eq!(updated.display_name.as_deref(), Some("Green Orchard"));
        assert_eq!(updated.color, "#6D4AFF");
        assert_eq!(updated.sort_order, 0);
        assert_eq!(
            updated.label_model,
            crate::provider::LabelModel::ImapNativeLabels
        );
        assert_eq!(db.account_sending_identities(ACCOUNT).unwrap()[1], alias);
        assert_eq!(before, preserved.map(|table| table_rows(&db, table)));
        let splits = db.list_split_inboxes().unwrap();
        assert_eq!(
            splits
                .iter()
                .find(|split| split.id == mapped.id)
                .unwrap()
                .match_value,
            labels[0].id
        );
        assert_eq!(
            splits
                .iter()
                .find(|split| split.id == unresolved.id)
                .unwrap()
                .match_value,
            "Label_2"
        );
        assert_eq!(
            splits
                .iter()
                .find(|split| split.id == domain.id)
                .unwrap()
                .match_value,
            "example.net"
        );
        assert_eq!(
            splits
                .iter()
                .find(|split| split.account_id == OTHER)
                .unwrap()
                .match_value,
            "Label_1"
        );
        assert!(db.get_thread(&thread_id).is_err());
        assert!(db.get_thread(&format!("{OTHER}:7d1")).is_ok());
        assert!(db.list_snooze_records(Some(ACCOUNT)).unwrap().is_empty());
        assert!(db.claim_mutations(ACCOUNT, 10).unwrap().is_empty());
        assert_eq!(
            db.converted_gmail_thread(&thread_id).unwrap(),
            Some((ACCOUNT.into(), "7d1".into()))
        );
        assert_eq!(
            db.converted_gmail_thread(cold.thread_id.as_deref().unwrap())
                .unwrap(),
            Some((ACCOUNT.into(), "7d2".into()))
        );
        assert!(db
            .converted_gmail_thread(&format!("{ACCOUNT}:unknown"))
            .unwrap()
            .is_none());
        let store = ImapStateStore::new(db.clone(), ACCOUNT);
        assert_eq!(store.resolve_gmail_thread("44d", 2001).unwrap(), "7d1");
        assert_eq!(store.gmail_message_id("44d").unwrap(), Some(1101));
        assert_eq!(store.gmail_message_id("450").unwrap(), Some(1104));
        assert!(db
            .switch_gmail_to_imap(ACCOUNT, settings(), &labels, &[])
            .is_err());
    }

    #[test]
    fn switching_rejects_another_active_device_and_records_a_single_device_switch() {
        let db = Database::open_memory();
        db.adopt_account(ACCOUNT).unwrap();
        db.set_beta_features_enabled(true).unwrap();
        let labels = vec![destination()];
        let split = db
            .create_split_inbox("Projects", "label", "Label_1", ACCOUNT)
            .unwrap();
        let choices = [SplitLabelChoice {
            id: split.id.clone(),
            old_label_id: split.match_value.clone(),
            label_id: Some(labels[0].id.clone()),
        }];
        db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO sync_devices(device_id,status,is_self) VALUES(?1,'active',0)",
                ["11111111111111111111111111111111"],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db
            .preview_gmail_imap_switch(ACCOUNT, labels.clone())
            .is_err());
        assert!(db
            .switch_gmail_to_imap(ACCOUNT, settings(), &labels, &choices)
            .is_err());
        assert_eq!(db.get_account(ACCOUNT).unwrap().unwrap().provider, "gmail");
        db.with_connection(|conn| {
            conn.execute(
                "UPDATE sync_devices SET status='revoked' WHERE is_self=0",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        db.switch_gmail_to_imap(ACCOUNT, settings(), &labels, &choices)
            .unwrap();
        let replica = db.load_replica_state().unwrap();
        let account = replica
            .fields()
            .iter()
            .find(|(key, _)| {
                key.entity_type == EntityType::MailAccount
                    && key.entity_id == ACCOUNT
                    && key.field == "provider"
            })
            .unwrap();
        assert_eq!(account.1[0].value, Some(serde_json::json!("imap")));
        let rule = replica
            .fields()
            .iter()
            .find(|(key, _)| {
                key.entity_type == EntityType::SplitInbox
                    && key.entity_id == split.id
                    && key.field == "matchValue"
            })
            .unwrap();
        assert_eq!(rule.1[0].value, Some(serde_json::json!(labels[0].id)));
    }

    #[test]
    fn errors_leave_the_gmail_account_and_cache_intact() {
        let db = Database::open_memory();
        db.adopt_account(ACCOUNT).unwrap();
        db.upsert_thread(
            ACCOUNT,
            &[message(
                "44d",
                "7d1",
                "2026-01-01T00:00:00Z",
                "Orchard review",
            )],
        )
        .unwrap();
        let split = db
            .create_split_inbox("Projects", "label", "Label_1", ACCOUNT)
            .unwrap();
        let labels = vec![destination()];
        let stale = [SplitLabelChoice {
            id: split.id.clone(),
            old_label_id: split.match_value.clone(),
            label_id: Some("folder:missing".into()),
        }];
        assert!(db
            .switch_gmail_to_imap(ACCOUNT, settings(), &labels, &stale)
            .is_err());
        let valid = [SplitLabelChoice {
            id: split.id,
            old_label_id: split.match_value,
            label_id: Some(labels[0].id.clone()),
        }];
        // Fail after deleting the old cache and while committing the new settings.
        db.with_connection(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER reject_switch BEFORE INSERT ON imap_account_settings
                BEGIN SELECT RAISE(ABORT, 'injected switch failure'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db
            .switch_gmail_to_imap(ACCOUNT, settings(), &labels, &valid)
            .is_err());
        assert_eq!(db.get_account(ACCOUNT).unwrap().unwrap().provider, "gmail");
        assert!(db.get_thread(&format!("{ACCOUNT}:7d1")).is_ok());
        assert_eq!(db.list_split_inboxes().unwrap()[0].match_value, "Label_1");
        assert!(table_rows(&db, "imap_gmail_threads").is_empty());
        assert!(db.imap_account_settings(ACCOUNT).unwrap().is_none());
    }
}
