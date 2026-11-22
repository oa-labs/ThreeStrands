//! Replication of verified IMAP mailbox identities. The mail account already
//! has a version-tolerant field map. Peers running an older app can still open
//! that snapshot, but cannot authorize structural edits because they never
//! publish a vote for the account's editor.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use threestrands_sync_protocol::EntityType;

use super::{label_edits::ImapLabelBinding, ImapStateStore};
use crate::db::{serialization_error, Database, DatabaseError, DbResult};
use crate::provider::imap::ImapAccountSettings;
use crate::signatures::sha256_hex;

const VOTE_PREFIX: &str = "imapLabelEditorVote:";
const HANDOFF_PREFIX: &str = "imapLabelEditorHandoff:";
const BINDING_PREFIX: &str = "imapLabelBinding:";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditorVote {
    initial_editor: String,
    desktop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SharedLabelBinding {
    pub binding: ImapLabelBinding,
    pub mailbox_scope: String,
    pub physical_folder: bool,
}

pub(crate) fn mailbox_scope(settings: &ImapAccountSettings) -> String {
    // Do not apply reservations to a different authenticated mailbox or a
    // device using different label namespaces on the same server.
    sha256_hex(
        json!([
            settings.imap_host.to_ascii_lowercase(),
            settings.imap_port,
            settings.imap_security,
            settings.imap_username,
            settings.label_storage,
            settings.label_container,
            settings.archive_mailbox,
            settings.mailbox_overrides,
        ])
        .to_string()
        .as_bytes(),
    )
}

fn initial_editor(connection: &Connection, account: &str) -> DbResult<Option<String>> {
    let account = account.to_ascii_lowercase();
    let exists: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM sync_values WHERE entity_type='mail_account'
         AND entity_id=?1 AND field='_entity' AND value='true')",
        [&account],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let mut query = connection.prepare(
        "SELECT DISTINCT json_extract(value,'$.initialEditor') FROM sync_values
         WHERE entity_type='mail_account' AND entity_id=?1 AND field=?2||device_id
           AND json_type(value,'$.initialEditor')='text' LIMIT 2",
    )?;
    let votes = query
        .query_map(params![account, VOTE_PREFIX], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if votes.len() > 1 {
        return Ok(None);
    }
    if let Some(vote) = votes.first() {
        // A device learns the already nominated editor before acknowledging
        // the account. A changed roster never silently replaces that editor.
        return Ok(Some(vote.clone()));
    }

    // Prefer an active device that introduced the shared account. Several
    // devices may have introduced it independently before they first synced.
    // Every device with the merged account and roster picks the same one.
    connection.query_row(
        "SELECT d.device_id FROM sync_devices d WHERE d.status='active'
         AND EXISTS (SELECT 1 FROM sync_values s WHERE s.entity_type='mail_account'
              AND s.entity_id=?1 AND s.field='_entity' AND s.value='true' AND s.device_id=d.device_id)
         ORDER BY d.device_id LIMIT 1",
        [account],
        |row| row.get(0),
    ).optional().map_err(Into::into)
}

fn field_written_by(
    connection: &Connection,
    account: &str,
    field: &str,
    writer: &str,
) -> DbResult<Option<String>> {
    let mut query = connection.prepare(
        "SELECT device_id,value FROM sync_values WHERE entity_type='mail_account'
         AND entity_id=?1 AND field=?2 LIMIT 2",
    )?;
    let mut rows = query.query(params![account.to_ascii_lowercase(), field])?;
    let row: Option<(String, Option<String>)> = rows
        .next()?
        .map(|row| Ok::<_, rusqlite::Error>((row.get(0)?, row.get(1)?)))
        .transpose()?;
    if rows.next()?.is_some() {
        return Ok(None);
    }
    Ok(row
        .filter(|(author, _)| author == writer)
        .and_then(|(_, value)| value))
}

fn publish_fields(
    tx: &Transaction<'_>,
    account: &str,
    fields: serde_json::Map<String, Value>,
) -> DbResult<()> {
    if fields.is_empty() {
        return Ok(());
    }
    let names = fields.keys().cloned().collect();
    crate::db::sync_state::record_replicated_write_in_transaction(
        tx,
        EntityType::MailAccount,
        &account.to_ascii_lowercase(),
        &names,
        &Value::Object(fields),
    )
}

pub(super) fn has_editor_vote(connection: &Connection, account: &str) -> DbResult<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='mail_account'
         AND entity_id=?1 AND field GLOB 'imapLabelEditorVote:*')",
        [account.to_ascii_lowercase()],
        |row| row.get(0),
    )?)
}

pub(super) fn active_peers(connection: &Connection) -> DbResult<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_devices WHERE is_self=0 AND status='active')",
        [],
        |row| row.get(0),
    )?)
}

fn local_device(connection: &Connection) -> DbResult<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT device_id FROM sync_devices WHERE is_self=1 AND status='active'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

#[derive(Debug, PartialEq, Eq)]
struct Editor {
    id: String,
    term: usize,
    acknowledged: bool,
    eligible_devices: Vec<String>,
}

fn editor(connection: &Connection, account: &str) -> DbResult<Option<Editor>> {
    let Some(origin) = initial_editor(connection, account)? else {
        return Ok(None);
    };
    let mut id = origin.clone();
    let mut term = 0;
    loop {
        let field = format!("{HANDOFF_PREFIX}{term}");
        let Some(handoff) = field_written_by(connection, account, &field, &id)? else {
            let exists: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='mail_account'
                 AND entity_id=?1 AND field=?2)",
                params![account.to_ascii_lowercase(), field],
                |row| row.get(0),
            )?;
            if exists {
                return Ok(None);
            }
            break;
        };
        let next: String = serde_json::from_str(&handoff).map_err(serialization_error)?;
        id = next;
        term += 1;
    }

    let mut active =
        connection.prepare("SELECT device_id FROM sync_devices WHERE status='active'")?;
    let active = active
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut acknowledged = active.contains(&id);
    let mut eligible_devices = Vec::new();
    for device in active {
        let vote = field_written_by(
            connection,
            account,
            &format!("{VOTE_PREFIX}{device}"),
            &device,
        )?;
        let vote = vote
            .and_then(|vote| serde_json::from_str::<EditorVote>(&vote).ok())
            .filter(|vote| vote.initial_editor == origin);
        acknowledged &= vote.is_some();
        if vote.is_some_and(|vote| vote.desktop) {
            eligible_devices.push(device);
        }
    }
    Ok(Some(Editor {
        id,
        term,
        acknowledged,
        eligible_devices,
    }))
}

pub(super) fn local_editor(connection: &Connection, account: &str) -> DbResult<Option<String>> {
    let Some(local) = local_device(connection)? else {
        return Ok(None);
    };
    Ok(editor(connection, account)?
        .filter(|editor| editor.acknowledged && editor.id == local)
        .map(|editor| editor.id))
}

impl Database {
    /// Publish one sticky acknowledgement per account from this build. The
    /// device that introduced the shared account is its initial editor.
    pub(crate) fn acknowledge_imap_label_editors(&self) -> DbResult<()> {
        self.with_transaction(|tx| {
            let Some(device) = local_device(tx)? else {
                return Ok(());
            };
            let field = format!("{VOTE_PREFIX}{device}");
            let mut query = tx.prepare(
                "SELECT DISTINCT entity_id FROM sync_values AS creation
                 WHERE entity_type='mail_account' AND field='_entity' AND value='true'
                 AND NOT EXISTS (
                    SELECT 1 FROM accounts WHERE lower(email)=creation.entity_id
                      AND on_this_device=1 AND provider='imap'
                 )
                 AND NOT EXISTS (
                    SELECT 1 FROM sync_values AS vote WHERE vote.entity_type='mail_account'
                    AND vote.entity_id=creation.entity_id AND vote.field=?1
                 )",
            )?;
            let accounts = query
                .query_map([&field], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for account in accounts {
                acknowledge_editor(tx, &account)?;
            }
            Ok(())
        })
    }

    pub(crate) fn imap_label_editor(&self, account: &str) -> DbResult<ImapLabelEditorStatus> {
        let enabled = self.replicated_sync_active()?;
        self.with_connection(|connection| {
            let local = local_device(connection)?;
            let editor = editor(connection, account)?;
            let has_peers = active_peers(connection)?;
            let designated = editor.as_ref().map(|editor| editor.id.clone());
            let is_editor = designated.is_some() && designated == local;
            let ready = enabled && editor.as_ref().is_some_and(|editor| editor.acknowledged);
            let eligible_devices = editor
                .map(|editor| editor.eligible_devices)
                .unwrap_or_default();
            let can_edit = if has_peers {
                ready && is_editor
            } else {
                !has_editor_vote(connection, account)? || is_editor
            };
            Ok(ImapLabelEditorStatus {
                has_peers,
                designated,
                is_editor,
                ready,
                can_edit,
                eligible_devices,
            })
        })
    }

    pub(crate) fn handoff_imap_label_editor(&self, account: &str, recipient: &str) -> DbResult<()> {
        if !self.replicated_sync_active()? {
            return Err(DatabaseError::invalid(
                "Enable Replicated Sync to change the editing device.",
            ));
        }
        self.with_transaction(|tx| {
            let Some(editor) = editor(tx, account)?.filter(|editor| editor.acknowledged) else {
                return Err(DatabaseError::invalid(
                    "Sync all enrolled devices before changing the editing device.",
                ));
            };
            if local_device(tx)?.as_deref() != Some(editor.id.as_str()) {
                return Err(DatabaseError::invalid(
                    "Change the editing device from the currently designated device.",
                ));
            }
            if recipient == editor.id {
                return Ok(());
            }
            if !editor
                .eligible_devices
                .iter()
                .any(|device| device == recipient)
            {
                return Err(DatabaseError::invalid("Choose an enrolled desktop device."));
            }
            let pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM imap_label_edits WHERE account_id=?1)",
                [account],
                |row| row.get(0),
            )?;
            if pending {
                return Err(DatabaseError::invalid(
                    "Finish checking the pending label edit before changing the editing device.",
                ));
            }
            // This transaction disables local writes before the recipient can
            // learn about its authority. Scope the handoff to the current term.
            publish_fields(
                tx,
                account,
                [(format!("{HANDOFF_PREFIX}{}", editor.term), json!(recipient))]
                    .into_iter()
                    .collect(),
            )
        })
    }
}

pub(super) fn acknowledge_editor(tx: &Transaction<'_>, account: &str) -> DbResult<()> {
    let (Some(device), Some(origin)) = (local_device(tx)?, initial_editor(tx, account)?) else {
        return Ok(());
    };
    let field = format!("{VOTE_PREFIX}{device}");
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='mail_account'
         AND entity_id=?1 AND field=?2)",
        params![account.to_ascii_lowercase(), field],
        |row| row.get(0),
    )?;
    if !exists {
        let vote = EditorVote {
            initial_editor: origin,
            desktop: cfg!(desktop),
        };
        let value = serde_json::to_value(vote).map_err(serialization_error)?;
        publish_fields(tx, account, [(field, value)].into_iter().collect())?;
    }
    Ok(())
}

pub(crate) fn is_imap_label_field(field: &str) -> bool {
    [VOTE_PREFIX, HANDOFF_PREFIX, BINDING_PREFIX]
        .iter()
        .any(|prefix| field.starts_with(prefix))
}

pub(crate) fn affects_imap_catalog(field: &str) -> bool {
    is_imap_label_field(field) || field == "_entity"
}

pub(super) fn is_designated_locally(connection: &Connection, account: &str) -> DbResult<bool> {
    Ok(
        match (local_device(connection)?, editor(connection, account)?) {
            (Some(local), Some(editor)) => local == editor.id,
            _ => false,
        },
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImapLabelEditorStatus {
    has_peers: bool,
    designated: Option<String>,
    is_editor: bool,
    ready: bool,
    can_edit: bool,
    eligible_devices: Vec<String>,
}

pub(super) fn publish_label_bindings(
    tx: &Transaction<'_>,
    account: &str,
    scope: &str,
    physical_folder: bool,
    bindings: impl IntoIterator<Item = ImapLabelBinding>,
) -> DbResult<()> {
    let mut fields = serde_json::Map::new();
    for binding in bindings {
        let field = binding_field(scope, physical_folder, &binding.label_id);
        let record = SharedLabelBinding {
            binding,
            mailbox_scope: scope.into(),
            physical_folder,
        };
        let value = serde_json::to_value(record).map_err(serialization_error)?;
        let known: bool = tx.query_row(
            "SELECT COUNT(*)=1 AND MIN(value)=?3 FROM sync_values WHERE entity_type='mail_account'
             AND entity_id=?1 AND field=?2",
            params![account.to_ascii_lowercase(), field, value.to_string()],
            |row| row.get(0),
        )?;
        if !known {
            fields.insert(field, value);
        }
    }
    publish_fields(tx, account, fields)
}

fn binding_field(scope: &str, physical_folder: bool, label_id: &str) -> String {
    format!(
        "{BINDING_PREFIX}{}",
        sha256_hex(
            json!([scope, physical_folder, label_id])
                .to_string()
                .as_bytes()
        )
    )
}

impl ImapStateStore {
    pub(crate) fn shared_label_bindings(
        &self,
        settings: &ImapAccountSettings,
        physical_folder: bool,
    ) -> DbResult<Vec<ImapLabelBinding>> {
        let scope = mailbox_scope(settings);
        self.database.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT field, value FROM sync_values WHERE entity_type='mail_account'
                 AND entity_id=?1 AND field GLOB 'imapLabelBinding:*'
                 GROUP BY field HAVING COUNT(*)=1",
            )?;
            let mut bindings = BTreeMap::new();
            let rows = query.query_map([self.account_id.to_ascii_lowercase()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            for row in rows {
                let (field, Some(value)) = row? else { continue };
                let Ok(record) = serde_json::from_str::<SharedLabelBinding>(&value) else {
                    continue;
                };
                let binding = record.binding;
                if field == binding_field(&scope, physical_folder, &binding.label_id)
                    && record.mailbox_scope == scope
                    && record.physical_folder == physical_folder
                    && (binding.label_id.starts_with("lf:")
                        || binding.label_id.starts_with("folder:")
                        || binding.label_id.starts_with("imap-label:"))
                    && binding.mailbox.is_some() == binding.uidvalidity.is_some_and(|uid| uid != 0)
                {
                    bindings.insert(binding.label_id.clone(), binding);
                }
            }
            Ok(bindings.into_values().collect())
        })
    }
    /// An unverified shared change must not let an older write target the
    /// previous path. The next catalog sweep checks the live destination.
    pub(crate) fn can_write_label_binding(
        &self,
        id: &str,
        settings: &ImapAccountSettings,
        physical_folder: bool,
    ) -> DbResult<bool> {
        let field = binding_field(&mailbox_scope(settings), physical_folder, id);
        self.database.with_connection(|connection| {
            let (count, matches): (i64, i64) = connection.query_row(
                "SELECT COUNT(*), COALESCE(SUM(
                    local.mailbox IS NOT NULL
                    AND local.mailbox IS json_extract(remote.value,'$.binding.mailbox')
                    AND local.uidvalidity IS json_extract(remote.value,'$.binding.uidvalidity')
                 ),0)
                 FROM sync_values remote
                 LEFT JOIN imap_label_bindings local ON local.account_id=?1 AND local.label_id=?3
                 WHERE remote.entity_type='mail_account' AND remote.entity_id=lower(?1) AND remote.field=?2",
                params![self.account_id, field, id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok(count == 0 || (count == 1 && matches == 1))
        })
    }
}
