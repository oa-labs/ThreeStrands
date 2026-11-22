use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use super::{account_from_row, purge_imap_cache, purge_mail_traces, purge_outgoing_mail};
use crate::db::split_inboxes::{bind_split_label, local_split_inbox_on};
use crate::db::{Database, DatabaseError, DbResult};
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
                let local = local_split_inbox_on(connection, &split.id)?;
                let old_label_name: Option<String> = connection
                    .query_row(
                        "SELECT name FROM account_labels WHERE account_id=?1 AND label_id=?2",
                        params![email, local.match_value],
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
                    .or_else(|| labels.iter().find(|label| label.id == split.match_value))
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

    /// Recover a canonical conversation reserved by the switch or referenced
    /// by a task that a Gmail API peer added after this device switched.
    pub(crate) fn converted_gmail_thread(&self, id: &str) -> DbResult<Option<(String, String)>> {
        let Some((email, thread)) = id.rsplit_once(':') else {
            return Ok(None);
        };
        if !is_canonical_gmail_id(thread) {
            return Ok(None);
        }
        self.with_connection(|connection| {
            let available: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts a
                 WHERE a.email=?1 AND a.provider='imap' AND a.on_this_device=1 AND (
                   EXISTS(SELECT 1 FROM imap_gmail_threads g WHERE g.account_id=a.email
                     AND g.gmail_thread_id=?2 AND g.thread_id=?2)
                   OR (a.gmail_imap_converted=1 AND EXISTS(
                     SELECT 1 FROM tasks WHERE account_id=a.email AND thread_id=?3))))",
                params![email, thread, id],
                |row| row.get(0),
            )?;
            Ok(available.then(|| (email.into(), thread.into())))
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
        schedule_owner: Option<&str>,
    ) -> DbResult<Account> {
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
            // Only rows backed by a local, owned delivery can be cancelled.
            // The projected schedules and snoozes are shared workflow state.
            if let Some(owner) = schedule_owner {
                crate::db::scheduled_send::cancel_schedules_for_connection_switch(tx, email, owner)?;
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
                "sync_recovery", "quarantined_messages", "gmail_sending_identities"]
            {
                tx.execute(&format!("DELETE FROM {table} WHERE account_id=?1"), [email])?;
            }
            tx.execute("INSERT INTO sync_state(account_id) VALUES(?1)", [email])?;

            tx.execute(
                "UPDATE conversation_snoozes SET pending_apply=1 WHERE account_id=?1
                 AND json_extract(record_json,'$.state')='active'",
                [email],
            )?;
            for split in splits {
                bind_split_label(tx, &split.id, &split.match_value, "imap", selected[split.id.as_str()].label_id.as_deref())?;
            }
            super::super::imap::write_settings_row(tx, email, &settings)?;
            let mut account = tx.query_row(
                "UPDATE accounts SET provider='imap', gmail_imap_converted=1, status='connected', last_synced_at=NULL
                 WHERE email=?1 RETURNING email,display_name,color,status,provider,sort_order,
                    connected_at,last_synced_at,nickname,on_this_device",
                [email], account_from_row,
            )?;
            tx.execute(
                "INSERT INTO imap_server_capabilities(account_id,native_labels,native_labels_seen) VALUES(?1,1,1)",
                [email],
            )?;
            account.label_model = crate::provider::LabelModel::ImapNativeLabels;
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

pub(in crate::db) fn canonical_gmail_id(id: &str) -> Option<u64> {
    u64::from_str_radix(id, 16)
        .ok()
        .filter(|number| format!("{number:x}") == id)
}

fn is_canonical_gmail_id(id: &str) -> bool {
    canonical_gmail_id(id).is_some()
}

fn reserve_gmail_ids(tx: &Transaction<'_>, email: &str) -> DbResult<()> {
    // Gmail API IDs are the canonical hexadecimal spelling of Gmail's native
    // IMAP IDs. Ingest uses the same bindings after observing the native IDs
    // on the wire. No RFC Message-ID, subject or sender heuristic is used here.
    let mut threads = tx.prepare(
        "SELECT provider_thread_id FROM threads WHERE account_id=?1
         UNION SELECT substr(thread_id,length(?1)+2) FROM tasks
         WHERE account_id=?1 AND substr(thread_id,1,length(?1)+1)=?1||':'
         UNION SELECT json_extract(record_json,'$.providerThreadId')
         FROM conversation_snoozes WHERE account_id=?1",
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

        db.snooze_threads(&[format!("{ACCOUNT}:unknown")], "2099-01-01T00:00:00Z")
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
                None,
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
        let splits = db.list_local_split_inboxes().unwrap();
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
            ""
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
        let snoozes = db.list_snooze_records(Some(ACCOUNT)).unwrap();
        assert_eq!(snoozes[0].id, thread_id);
        assert_eq!(snoozes[0].state, "active");
        assert_eq!(snoozes[1].provider_thread_id, "unknown");
        assert_eq!(snoozes[1].state, "active");
        assert_eq!(
            db.snooze_refresh_ids(ACCOUNT, chrono::Utc::now()).unwrap(),
            ["7d1"]
        );
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
        assert_eq!(store.resolve_gmail_thread("44f", 2005).unwrap(), "7d5");
        assert_eq!(store.gmail_thread_lookup_id("7d9").unwrap(), Some(2009));
        assert_eq!(store.gmail_thread_lookup_id("imap:t:legacy").unwrap(), None);
        let inputs = crate::provider::imap::identity::IdentityInputs {
            gmail_message_id: Some(1103),
            ..Default::default()
        };
        assert_eq!(
            store
                .resolve_message_identity("All Mail", 1, 9, &inputs)
                .unwrap(),
            "44f"
        );
        assert!(db
            .switch_gmail_to_imap(ACCOUNT, settings(), &labels, &[], None)
            .is_err());
    }

    #[test]
    fn devices_switch_separately_without_rewriting_shared_rules_or_peer_schedules() {
        use threestrands_sync_protocol::EntityType;

        const OTHER_DEVICE: &str = "11111111111111111111111111111111";
        let first = Database::open_memory();
        let second = Database::open_memory();
        for device in [&first, &second] {
            device.adopt_account(ACCOUNT).unwrap();
            device.set_beta_features_enabled(true).unwrap();
            device
                .with_connection(|conn| {
                    conn.execute(
                        "INSERT INTO sync_devices(device_id,status,is_self) VALUES(?1,'active',0)",
                        [OTHER_DEVICE],
                    )?;
                    Ok(())
                })
                .unwrap();
        }
        let labels = vec![destination()];
        let api_label = Label {
            id: "Label_1".into(),
            name: labels[0].name.clone(),
            kind: "user".into(),
        };
        second
            .replace_account_labels(ACCOUNT, std::slice::from_ref(&api_label))
            .unwrap();
        first
            .replace_account_labels(ACCOUNT, std::slice::from_ref(&api_label))
            .unwrap();
        let rule = second
            .create_split_inbox("Projects", "label", &api_label.id, ACCOUNT)
            .unwrap();
        first
            .materialize_entity(
                EntityType::SplitInbox,
                &rule.id,
                Some(&serde_json::to_value(&rule).unwrap()),
                false,
            )
            .unwrap();

        // A real received scheduled-send summary and snooze remain visible
        // and keep their replicated state after the local cutover.
        let report = serde_json::json!({
            "operationId": "00000000-0000-4000-8000-0000000000a1",
            "ownerInstallationId": "00000000-0000-4000-8000-000000000001",
            "ownerSyncDeviceId": OTHER_DEVICE,
            "ownerNameAtCreation": "Other computer", "account": ACCOUNT,
            "subject": "Later", "scheduledAt": 1900000000000_i64, "timeZone": "UTC",
            "state": "scheduled", "blockedReason": null, "reportRevision": 1,
            "statusChangedAt": 1800000000000_i64
        });
        let report_id = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-0000000000a1";
        let snooze = serde_json::json!({"schedule": {
            "id": format!("{ACCOUNT}:7d9"), "accountId": ACCOUNT, "providerThreadId": "7d9",
            "until": "2099-01-01T00:00:00Z", "state": "active", "messageIds": ["44d"],
        }});
        for device in [&first, &second] {
            device
                .materialize_entity(
                    EntityType::ScheduledSendSummary,
                    report_id,
                    Some(&serde_json::json!({"report": report})),
                    false,
                )
                .unwrap();
            device
                .materialize_entity(
                    EntityType::ConversationSnooze,
                    snooze["schedule"]["id"].as_str().unwrap(),
                    Some(&snooze),
                    false,
                )
                .unwrap();
            device.reconcile_replicated_sync_backlog().unwrap();
        }
        let first_before = first.load_replica_state().unwrap();

        let switch = |device: &Database| {
            let preview = device
                .preview_gmail_imap_switch(ACCOUNT, labels.clone())
                .unwrap();
            assert_eq!(
                preview.split_inboxes[0].suggested_label_id.as_deref(),
                Some(labels[0].id.as_str())
            );
            device
                .switch_gmail_to_imap(
                    ACCOUNT,
                    settings(),
                    &labels,
                    &[SplitLabelChoice {
                        id: rule.id.clone(),
                        old_label_id: rule.match_value.clone(),
                        label_id: Some(labels[0].id.clone()),
                    }],
                    None,
                )
                .unwrap();
        };
        let ingest = |device: &Database, id: &str| {
            let mut item = message("44e", "7d3", "2026-01-01T00:00:00Z", "Orchard updates");
            item.labels = vec!["INBOX".into(), "UNREAD".into(), id.into()];
            device.upsert_thread(ACCOUNT, &[item]).unwrap();
            assert_eq!(
                device
                    .list_split_inbox_page(&rule.id, 0, 10)
                    .unwrap()
                    .threads
                    .len(),
                1
            );
            assert_eq!(
                device
                    .mailbox_unread_counts(Some(ACCOUNT))
                    .unwrap()
                    .splits
                    .get(&rule.id),
                Some(&1)
            );
            assert!(device
                .list_threads_page(Some(ACCOUNT), 0, 10)
                .unwrap()
                .threads
                .iter()
                .all(|thread| thread.provider_thread_id != "7d3"));
        };

        switch(&first);
        assert_eq!(
            first.get_account(ACCOUNT).unwrap().unwrap().provider,
            "imap"
        );
        assert_eq!(
            second.get_account(ACCOUNT).unwrap().unwrap().provider,
            "gmail"
        );
        assert_eq!(
            first.list_split_inboxes().unwrap()[0].match_value,
            api_label.id
        );
        assert_eq!(
            first.local_split_inbox(&rule.id).unwrap().match_value,
            labels[0].id
        );
        assert_eq!(
            second.local_split_inbox(&rule.id).unwrap().match_value,
            api_label.id
        );
        first.reconcile_replicated_sync_backlog().unwrap();
        assert_eq!(
            first.load_replica_state().unwrap().fields(),
            first_before.fields()
        );
        assert_eq!(
            first.scheduled_summaries().unwrap()[0]["state"],
            "scheduled"
        );
        assert_eq!(
            first.list_snooze_records(Some(ACCOUNT)).unwrap()[0].state,
            "active"
        );

        ingest(&first, &labels[0].id);
        ingest(&second, &api_label.id);
        switch(&second);
        ingest(&second, &labels[0].id);
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
            .switch_gmail_to_imap(ACCOUNT, settings(), &labels, &stale, None)
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
            .switch_gmail_to_imap(ACCOUNT, settings(), &labels, &valid, None)
            .is_err());
        assert_eq!(db.get_account(ACCOUNT).unwrap().unwrap().provider, "gmail");
        assert!(db.get_thread(&format!("{ACCOUNT}:7d1")).is_ok());
        assert_eq!(db.list_split_inboxes().unwrap()[0].match_value, "Label_1");
        assert!(table_rows(&db, "imap_gmail_threads").is_empty());
        assert!(db.imap_account_settings(ACCOUNT).unwrap().is_none());
    }
}
