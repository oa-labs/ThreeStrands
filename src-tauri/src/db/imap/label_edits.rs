//! IMAP mailbox label identity and the durable boundary around a server write.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use super::{label_replica, ImapStateStore};
use crate::db::{serialization_error, Database, DatabaseError, DbResult};
use crate::models::Label;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImapLabelBinding {
    pub label_id: String,
    pub mailbox: Option<String>,
    pub uidvalidity: Option<u32>,
}

impl ImapLabelBinding {
    fn needs_shared_reservation(
        &self,
        settings: &crate::provider::imap::ImapAccountSettings,
        delimiter: Option<&str>,
    ) -> bool {
        let Some(path) = &self.mailbox else {
            return true;
        };
        if let Some(original) = self.label_id.strip_prefix("folder:") {
            return original != path;
        }
        if let Some(original) = self.label_id.strip_prefix("lf:") {
            return settings.label_container.as_deref().zip(delimiter).and_then(
                |(container, delimiter)| path.strip_prefix(container)?.strip_prefix(delimiter),
            ) != Some(original);
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImapLabelEdit {
    // The root fields are flat in the persisted journal for compatibility
    // with pending edits written before hierarchy support.
    #[serde(flatten)]
    pub root: ImapLabelEditTarget,
    pub acknowledged: bool,
    #[serde(default)]
    pub physical_folder: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub descendants: Vec<ImapLabelEditTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImapLabelEditTarget {
    pub id: String,
    pub source: String,
    pub source_uidvalidity: u32,
    pub destination: Option<String>,
}

impl ImapLabelEdit {
    pub(crate) fn targets(&self) -> impl Iterator<Item = &ImapLabelEditTarget> {
        std::iter::once(&self.root).chain(&self.descendants)
    }
}

pub(crate) enum ImapLabelEditOutcome<'a> {
    Unchanged,
    Renamed {
        label: Label,
        provisional_id: String,
        mailbox: &'a str,
        delimiter: Option<&'a str>,
        uidvalidity: u32,
    },
    Unavailable,
}

pub(crate) fn read_bindings(
    connection: &Connection,
    account: &str,
) -> DbResult<Vec<ImapLabelBinding>> {
    let mut statement = connection.prepare(
        "SELECT label_id,mailbox,uidvalidity FROM imap_label_bindings WHERE account_id=?1 ORDER BY label_id",
    )?;
    let rows = statement.query_map([account], |row| {
        Ok(ImapLabelBinding {
            label_id: row.get(0)?,
            mailbox: row.get(1)?,
            uidvalidity: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn import_bindings(
    tx: &Transaction<'_>,
    account: &str,
    bindings: &[ImapLabelBinding],
) -> DbResult<()> {
    for binding in bindings {
        // Preserve the destination's more recent association when it already
        // knows this logical identity or has a different owner for this path.
        tx.execute(
            "INSERT OR IGNORE INTO imap_label_bindings(account_id,label_id,mailbox,uidvalidity)
                    VALUES(?1,?2,?3,?4)",
            params![
                account,
                binding.label_id,
                binding.mailbox,
                binding.uidvalidity
            ],
        )?;
    }
    Ok(())
}

/// An edit journal belongs to one authenticated mailbox. Keep its logical
/// identity reserved if settings replace the endpoint before recovery.
pub(crate) fn retire_changed_connection_bindings(
    tx: &Transaction<'_>,
    account: &str,
    settings: &crate::provider::imap::ImapAccountSettings,
) -> DbResult<()> {
    let previous = super::read_settings_row(tx, account)?;
    if !previous.is_some_and(|previous| {
        previous.imap_host != settings.imap_host
            || previous.imap_port != settings.imap_port
            || previous.imap_security != settings.imap_security
            || previous.imap_username != settings.imap_username
    }) {
        return Ok(());
    }
    tx.execute(
        "UPDATE accounts SET gmail_imap_converted=0 WHERE email=?1",
        [account],
    )?;
    tx.execute(
        "UPDATE split_inbox_label_bindings SET label_id=NULL WHERE provider='imap'
        AND split_id IN (SELECT id FROM split_inboxes WHERE account_id=?1)",
        [account],
    )?;
    let pending: Option<String> = tx
        .query_row(
            "SELECT pending_json FROM imap_label_edits WHERE account_id=?1",
            [account],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(pending) = pending {
        let edit: ImapLabelEdit = serde_json::from_str(&pending).map_err(serialization_error)?;
        for target in edit.targets() {
            tx.execute(
                "INSERT OR IGNORE INTO imap_label_bindings(account_id,label_id) VALUES(?1,?2)",
                params![account, target.id],
            )?;
        }
    }
    for binding in read_bindings(tx, account)? {
        remove_binding_references(tx, account, &binding.label_id, true)?;
    }
    tx.execute(
        "UPDATE imap_label_bindings SET mailbox=NULL,uidvalidity=NULL WHERE account_id=?1",
        [account],
    )?;
    tx.execute(
        "DELETE FROM imap_label_edits WHERE account_id=?1",
        [account],
    )?;
    Ok(())
}

impl Database {
    pub(crate) fn imap_label_bindings(&self, account: &str) -> DbResult<Vec<ImapLabelBinding>> {
        self.with_connection(|connection| read_bindings(connection, account))
    }
}

impl ImapStateStore {
    pub(crate) fn label_bindings(&self) -> DbResult<Vec<ImapLabelBinding>> {
        self.database.imap_label_bindings(&self.account_id)
    }

    pub(crate) fn pending_label_edit(&self) -> DbResult<Option<ImapLabelEdit>> {
        let json: Option<String> = self.database.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT pending_json FROM imap_label_edits WHERE account_id=?1",
                    [&self.account_id],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        json.map(|value| serde_json::from_str(&value).map_err(serialization_error))
            .transpose()
    }

    pub(crate) fn begin_label_edit(
        &self,
        edit: &mut ImapLabelEdit,
        settings: &crate::provider::imap::ImapAccountSettings,
    ) -> DbResult<()> {
        let sync_enabled = self.database.replicated_sync_active()?;
        self.database.with_transaction(|tx| {
            let editor = if sync_enabled {
                label_replica::local_editor(tx, &self.account_id)?
            } else {
                None
            };
            let blocked = if label_replica::active_peers(tx)? {
                editor.is_none()
            } else {
                label_replica::has_editor_vote(tx, &self.account_id)?
                    && !label_replica::is_designated_locally(tx, &self.account_id)?
            };
            if blocked {
                return Err(DatabaseError::invalid(
                    "Rename or delete IMAP folders and labels on the account’s designated editing device after its enrolled devices have synced.",
                ));
            }
            edit.mailbox_scope = editor.map(|_| label_replica::mailbox_scope(settings));
            let inserted = tx.execute("INSERT INTO imap_label_edits(account_id,pending_json) VALUES(?1,?2)
                ON CONFLICT(account_id) DO NOTHING", params![self.account_id, serialize(edit)?])?;
            if inserted == 0 {
                return Err(DatabaseError::invalid("ThreeStrands is still checking an earlier label change."));
            }
            for target in edit.targets() {
                tx.execute(
                    "INSERT INTO imap_label_bindings(account_id,label_id,mailbox,uidvalidity)
                     VALUES(?1,?2,?3,?4) ON CONFLICT(account_id,label_id) DO NOTHING",
                    params![self.account_id, target.id, target.source, target.source_uidvalidity],
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn acknowledge_label_edit(&self, edit: &ImapLabelEdit) -> DbResult<()> {
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE imap_label_edits SET pending_json=?2 WHERE account_id=?1",
                params![self.account_id, serialize(edit)?],
            )?;
            Ok(())
        })
    }

    pub(crate) fn cached_mailbox_descendants(&self, prefix: &str) -> DbResult<Vec<String>> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT name FROM imap_mailboxes WHERE account_id=?1 AND substr(name,1,length(?2))=?2
                 UNION
                 SELECT mailbox FROM imap_label_bindings WHERE account_id=?1 AND substr(mailbox,1,length(?2))=?2
                 UNION
                 SELECT label.value FROM imap_label_snapshots, json_each(labels_json) AS label
                  WHERE account_id=?1 AND substr(label.value,1,length(?2))=?2",
            )?;
            let rows = statement.query_map(params![self.account_id, prefix], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn retire_unavailable_label_binding(&self, id: &str) -> DbResult<()> {
        self.database
            .with_transaction(|tx| retire_binding(tx, &self.account_id, id, true))
    }

    pub(crate) fn suspend_unavailable_label_binding(&self, id: &str) -> DbResult<()> {
        self.database.with_transaction(|tx| {
            retire_binding(tx, &self.account_id, id, false)?;
            tx.execute(
                "DELETE FROM imap_mailbox_sync_state WHERE account_id=?1 AND mailbox=?2",
                params![self.account_id, Self::CATALOG_SWEEP_KEY],
            )?;
            Ok(())
        })
    }

    pub(crate) fn has_enrolled_peers(&self) -> DbResult<bool> {
        self.database
            .with_connection(|connection| label_replica::active_peers(connection))
    }

    pub(crate) fn remember_mailbox_label_bindings(
        &self,
        bindings: &[ImapLabelBinding],
        settings: &crate::provider::imap::ImapAccountSettings,
        physical_folder: bool,
        delimiter: Option<&str>,
    ) -> DbResult<()> {
        let sync_enabled = self.database.replicated_sync_active()?;
        self.database.with_transaction(|tx| {
            import_bindings(tx, &self.account_id, bindings)?;
            if sync_enabled {
                label_replica::acknowledge_editor(tx, &self.account_id)?;
                if label_replica::local_editor(tx, &self.account_id)?.is_some() {
                    // Backfill existing local edits only once this device is
                    // authoritative and has checked them against this mailbox.
                    label_replica::publish_label_bindings(
                        tx,
                        &self.account_id,
                        &label_replica::mailbox_scope(settings),
                        physical_folder,
                        read_bindings(tx, &self.account_id)?
                            .into_iter()
                            .filter(|binding| {
                                binding.needs_shared_reservation(settings, delimiter)
                            }),
                    )?;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn apply_shared_label_bindings(
        &self,
        verified: &[(ImapLabelBinding, Option<Label>)],
    ) -> DbResult<()> {
        let account = &self.account_id;
        self.database.with_transaction(|tx| {
            let local: BTreeMap<_, _> = read_bindings(tx, account)?
                .into_iter()
                .map(|binding| (binding.label_id.clone(), binding))
                .collect();
            // A full pull may include several sequential edits, including paths
            // that have changed owners. Release old coordinates before binding
            // any destinations.
            let destinations: BTreeMap<_, _> = verified.iter()
                .filter_map(|(binding, label)| {
                    label.as_ref()?;
                    Some((binding.mailbox.as_deref()?, binding.uidvalidity?))
                })
                .collect();
            let mut changed = Vec::new();
            let mut affected_paths = BTreeSet::new();
            for (desired, label) in verified {
                let prior = local.get(&desired.label_id);
                if label.is_some() && prior == Some(desired) {
                    continue;
                }
                let destination = label.as_ref().and(desired.mailbox.as_deref());
                if let Some(destination) = destination {
                    affected_paths.insert(destination);
                }
                if let Some(prior) = prior {
                    if let Some(path) = prior.mailbox.as_deref().filter(|path| Some(*path) != destination) {
                        affected_paths.insert(path);
                        if let Some(destination) = destination.filter(|destination| {
                            prior.uidvalidity == desired.uidvalidity
                                && !local.values().any(|other| other.mailbox.as_deref() == Some(*destination))
                        }) {
                            for table in ["imap_locations", "imap_search_pins"] {
                                tx.execute(
                                    &format!("UPDATE OR IGNORE {table} SET mailbox=?3
                                        WHERE account_id=?1 AND mailbox=?2 AND uidvalidity=?4"),
                                    params![account, path, destination, desired.uidvalidity],
                                )?;
                            }
                        }
                        // Replacing a provisional ID for the same verified
                        // mailbox changes its catalog identity, not its UIDs.
                        if destinations.get(path).copied() != prior.uidvalidity {
                            clear_mailbox_coordinates(tx, account, path)?;
                            tx.execute(
                                "DELETE FROM imap_copy_intents WHERE account_id=?1 AND destination=?2",
                                params![account, path],
                            )?;
                        }
                    }
                }
                reserve_binding(tx, account, &desired.label_id)?;
                if label.is_none() {
                    remove_binding_references(tx, account, &desired.label_id, desired.mailbox.is_none())?;
                }
                changed.push((desired, label));
            }
            for (desired, label) in changed {
                let Some(label) = label else { continue };
                // This peer may have discovered the destination before getting
                // the edit. Its provisional ID stays reserved for relinking.
                let displaced: Option<String> = tx.query_row(
                    "SELECT label_id FROM imap_label_bindings WHERE account_id=?1 AND mailbox=?2 AND label_id<>?3",
                    params![account, desired.mailbox, desired.label_id],
                    |row| row.get(0),
                ).optional()?;
                if let Some(displaced) = displaced {
                    retire_binding(tx, account, &displaced, true)?;
                }
                tx.execute(
                    "UPDATE imap_label_bindings SET mailbox=?3,uidvalidity=?4 WHERE account_id=?1 AND label_id=?2",
                    params![account, desired.label_id, desired.mailbox, desired.uidvalidity],
                )?;
                tx.execute(
                    "INSERT OR REPLACE INTO account_labels(account_id,label_id,name,kind) VALUES(?1,?2,?3,?4)",
                    params![account, label.id, label.name, label.kind],
                )?;
                tx.execute(
                    "UPDATE mutations SET next_attempt_at=NULL
                     WHERE account_id=?1 AND kind='label' AND state='pending'
                     AND json_extract(payload_json, '$.labelId')=?2",
                    params![account, label.id],
                )?;
            }
            journal_label_paths(tx, account, &affected_paths)
        })
    }

    /// Publish every verified path change together. An uncertain or deleted
    /// label leaves its saved rules attached to a retired ID for relinking.
    pub(crate) fn finish_label_edit(
        &self,
        edit: &ImapLabelEdit,
        outcomes: &[(&ImapLabelEditTarget, ImapLabelEditOutcome<'_>)],
    ) -> DbResult<()> {
        let account = &self.account_id;
        self.database.with_transaction(|tx| {
            let mut paths = BTreeMap::new();
            let mut changed_bindings = Vec::new();
            for (target, outcome) in outcomes {
                let (wire, validity) = match outcome {
                    ImapLabelEditOutcome::Unchanged => continue,
                    ImapLabelEditOutcome::Renamed { mailbox, uidvalidity, .. } => (Some(*mailbox), Some(*uidvalidity)),
                    ImapLabelEditOutcome::Unavailable => (None, None),
                };
                paths.insert(target.source.as_str(), wire);
                changed_bindings.push(ImapLabelBinding {
                    label_id: target.id.clone(),
                    mailbox: wire.map(str::to_owned),
                    uidvalidity: validity,
                });
                tx.execute(
                    "INSERT INTO imap_label_bindings(account_id,label_id,mailbox,uidvalidity)
                     VALUES(?1,?2,?3,?4) ON CONFLICT(account_id,label_id)
                     DO UPDATE SET mailbox=excluded.mailbox,uidvalidity=excluded.uidvalidity",
                    params![account, target.id, wire, validity],
                )?;
                if let ImapLabelEditOutcome::Renamed { label, provisional_id, mailbox, delimiter, uidvalidity } = outcome {
                    if provisional_id != &target.id {
                        // A peer can see the new path before its binding arrives.
                        // Keep that provisional ID from being reused for a
                        // different label if this destination is later recycled.
                        let reserved = tx.execute(
                            "INSERT OR IGNORE INTO imap_label_bindings(account_id,label_id) VALUES(?1,?2)",
                            params![account, provisional_id],
                        )?;
                        if reserved != 0 {
                            changed_bindings.push(ImapLabelBinding {
                                label_id: provisional_id.clone(),
                                mailbox: None,
                                uidvalidity: None,
                            });
                        }
                    }
                    // The destination was absent from the live listing before
                    // the edit. Discard any stale coordinates at that path.
                    clear_mailbox_coordinates(tx, account, mailbox)?;
                    tx.execute(
                        "INSERT INTO imap_mailboxes(account_id,name,delimiter,uidvalidity,uidnext)
                         VALUES(?1,?2,?3,?4,0) ON CONFLICT(account_id,name)
                         DO UPDATE SET delimiter=excluded.delimiter,uidvalidity=excluded.uidvalidity,uidnext=0,highestmodseq=NULL",
                        params![account, mailbox, delimiter, uidvalidity],
                    )?;
                    // Independent Gmail message IDs and cached bodies survive
                    // a reset. Only validated UIDs follow the renamed path.
                    if *uidvalidity == target.source_uidvalidity {
                        for table in ["imap_locations", "imap_search_pins"] {
                            tx.execute(
                                &format!("UPDATE {table} SET mailbox=?3
                                    WHERE account_id=?1 AND mailbox=?2 AND uidvalidity=?4"),
                                params![account, target.source, mailbox, uidvalidity],
                            )?;
                        }
                    }
                    if edit.physical_folder {
                        tx.execute(
                            "UPDATE OR IGNORE imap_copy_intents SET destination=?3
                             WHERE account_id=?1 AND destination=?2",
                            params![account, target.source, mailbox],
                        )?;
                    }
                    tx.execute(
                        "INSERT OR REPLACE INTO account_labels(account_id,label_id,name,kind) VALUES(?1,?2,?3,?4)",
                        params![account, label.id, label.name, label.kind],
                    )?;
                } else {
                    remove_binding_references(tx, account, &target.id, true)?;
                }
                clear_mailbox_coordinates(tx, account, &target.source)?;
                tx.execute(
                    "DELETE FROM imap_copy_intents WHERE account_id=?1 AND destination=?2",
                    params![account, target.source],
                )?;
                tx.execute(
                    "DELETE FROM imap_mailboxes WHERE account_id=?1 AND name=?2",
                    params![account, target.source],
                )?;
            }
            if !paths.is_empty() {
                // Normalize snapshot memberships in one pass for the entire
                // hierarchy. Unrelated labels retain their wire paths.
                tx.execute(
                    "WITH changes AS MATERIALIZED (SELECT key AS source, value AS destination FROM json_each(?2))
                     UPDATE imap_label_snapshots AS snapshot
                        SET labels_json=(SELECT json_group_array(DISTINCT
                            CASE WHEN changes.source IS NULL THEN label.value ELSE changes.destination END)
                          FROM json_each(snapshot.labels_json) AS label
                          LEFT JOIN changes ON changes.source=label.value
                          WHERE changes.source IS NULL OR changes.destination IS NOT NULL)
                      WHERE account_id=?1 AND EXISTS(SELECT 1 FROM json_each(snapshot.labels_json) AS label
                          JOIN changes ON changes.source=label.value)",
                    params![account, serde_json::to_string(&paths).map_err(serialization_error)?],
                )?;
            }
            if let Some(scope) = edit.mailbox_scope.as_ref()
                .filter(|_| !changed_bindings.is_empty())
            {
                if label_replica::is_designated_locally(tx, account)? {
                label_replica::publish_label_bindings(
                    tx, account, scope, edit.physical_folder, changed_bindings,
                )?;
                }
            }
            tx.execute("DELETE FROM imap_label_edits WHERE account_id=?1", [account])?;
            Ok(())
        })
    }
}

/// Raw memberships may already have arrived on this peer with provisional
/// IDs. A verified change in their interpretation still needs to refresh the
/// cached thread even if its next IMAP flag sweep finds identical data.
fn journal_label_paths(
    tx: &Transaction<'_>,
    account: &str,
    paths: &BTreeSet<&str>,
) -> DbResult<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let paths = serde_json::to_string(paths).map_err(serialization_error)?;
    let mut query = tx.prepare(
        "SELECT h.thread_id FROM imap_hot_threads h WHERE h.account_id=?1 AND EXISTS (
            SELECT 1 FROM imap_threads t WHERE t.account_id=h.account_id AND t.thread_id=h.thread_id
            AND (
                EXISTS (SELECT 1 FROM imap_locations l WHERE l.account_id=t.account_id
                    AND l.message_id=t.message_id AND l.mailbox IN (SELECT value FROM json_each(?2)))
                OR EXISTS (SELECT 1 FROM imap_label_snapshots s, json_each(s.labels_json) label
                    WHERE s.account_id=t.account_id AND s.message_id=t.message_id
                    AND label.value IN (SELECT value FROM json_each(?2)))
            )
         )",
    )?;
    let ids = query
        .query_map(params![account, paths], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Ok(());
    }
    let generation: i64 = tx.query_row(
        "UPDATE imap_sync_state SET generation=generation+1 WHERE account_id=?1 RETURNING generation",
        [account], |row| row.get(0),
    )?;
    for id in ids {
        tx.execute(
            "INSERT INTO imap_change_journal(account_id,generation,thread_id) VALUES(?1,?2,?3)",
            params![account, generation, id],
        )?;
    }
    Ok(())
}

fn clear_mailbox_coordinates(tx: &Transaction<'_>, account: &str, mailbox: &str) -> DbResult<()> {
    for table in [
        "imap_locations",
        "imap_mailbox_sync_state",
        "imap_search_pins",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE account_id=?1 AND mailbox=?2"),
            params![account, mailbox],
        )?;
    }
    Ok(())
}

fn retire_binding(tx: &Transaction<'_>, account: &str, id: &str, terminal: bool) -> DbResult<()> {
    reserve_binding(tx, account, id)?;
    remove_binding_references(tx, account, id, terminal)
}

fn reserve_binding(tx: &Transaction<'_>, account: &str, id: &str) -> DbResult<()> {
    tx.execute(
        "INSERT INTO imap_label_bindings(account_id,label_id) VALUES(?1,?2)
         ON CONFLICT(account_id,label_id) DO UPDATE SET mailbox=NULL,uidvalidity=NULL",
        params![account, id],
    )?;
    Ok(())
}

fn remove_binding_references(
    tx: &Transaction<'_>,
    account: &str,
    id: &str,
    terminal: bool,
) -> DbResult<()> {
    tx.execute(
        "DELETE FROM account_labels WHERE account_id=?1 AND label_id=?2",
        params![account, id],
    )?;
    tx.execute("UPDATE threads SET labels_json=(SELECT json_group_array(value)
        FROM json_each(threads.labels_json) WHERE value<>?2)
        WHERE account_id=?1 AND EXISTS(SELECT 1 FROM json_each(threads.labels_json) WHERE value=?2)",
        params![account, id])?;
    if terminal {
        tx.execute("UPDATE mutations SET state='failed',last_error='The label is no longer available.',next_attempt_at=NULL
            WHERE account_id=?1 AND kind='label' AND state IN ('pending','running')
                AND json_extract(payload_json, '$.labelId')=?2",
            params![account, id])?;
    }
    Ok(())
}

fn serialize(edit: &ImapLabelEdit) -> DbResult<String> {
    serde_json::to_string(edit).map_err(serialization_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_edits_from_before_hierarchy_support_keep_their_original_target() {
        let json = r#"{"id":"folder:Old","source":"Old","sourceUidvalidity":42,"destination":"New","acknowledged":true}"#;
        let edit: ImapLabelEdit = serde_json::from_str(json).unwrap();
        assert_eq!(edit.root.id, "folder:Old");
        assert_eq!(edit.root.source_uidvalidity, 42);
        assert_eq!(edit.root.destination.as_deref(), Some("New"));
        assert!(edit.acknowledged);
        assert!(!edit.physical_folder);
        assert_eq!(edit.targets().count(), 1);
    }
}
