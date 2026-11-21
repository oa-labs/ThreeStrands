//! Account-scoped Gmail thread evidence and atomic repair of legacy groups.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::ImapStateStore;
use crate::db::{serialization_error, Database, DatabaseError, DbResult};
use crate::mime::NormalizedMessage;

#[derive(Debug)]
pub(crate) struct ThreadProbe {
    pub message_id: String,
    pub mailbox: String,
    pub uidvalidity: u32,
    pub uid: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ThreadRepair {
    pub source: String,
    /// Every member of the source has verified native evidence.
    pub assignments: BTreeMap<String, String>,
    /// The prospective memberships of the affected groups, including
    /// existing members already assigned to destinations.
    pub groups: BTreeMap<String, BTreeSet<String>>,
    pub projected: BTreeSet<String>,
    bindings: BTreeMap<String, String>,
}

impl ThreadRepair {
    /// A complete move to a single destination is the only case where
    /// a thread-only reference can follow a retired ID.
    fn survivor(&self) -> Option<&str> {
        if self.groups.len() != 1 {
            return None;
        }
        self.groups.first_key_value().map(|(id, _)| id.as_str())
    }

    pub(crate) fn changes_membership(&self) -> bool {
        self.groups.len() != 1 || !self.groups.contains_key(&self.source)
    }
}

pub(super) fn record_evidence(
    tx: &Transaction<'_>,
    account: &str,
    message: &str,
    native_thread: &str,
) -> DbResult<bool> {
    Ok(tx.query_row(
        "INSERT INTO imap_gmail_message_threads(account_id,message_id,gmail_thread_id) VALUES(?1,?2,?3)
         ON CONFLICT(account_id,message_id) DO UPDATE SET
            conflicted=conflicted OR (gmail_thread_id IS NOT NULL AND gmail_thread_id<>excluded.gmail_thread_id),
            gmail_thread_id=COALESCE(gmail_thread_id,excluded.gmail_thread_id)
         RETURNING conflicted",
        params![account, message, native_thread],
        |row| row.get(0),
    )?)
}

pub(super) fn bind_thread(
    tx: &Transaction<'_>,
    account: &str,
    native: &str,
    local: &str,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO imap_gmail_threads(account_id,gmail_thread_id,thread_id) VALUES(?1,?2,?3)
         ON CONFLICT(account_id,gmail_thread_id) DO NOTHING",
        params![account, native, local],
    )?;
    let bound: String = tx.query_row(
        "SELECT thread_id FROM imap_gmail_threads WHERE account_id=?1 AND gmail_thread_id=?2",
        params![account, native],
        |row| row.get(0),
    )?;
    if bound != local {
        return Err(DatabaseError::invalid(
            "The native Gmail thread binding changed during repair.",
        ));
    }
    Ok(())
}

impl ImapStateStore {
    pub(crate) fn gmail_thread_probes(&self, limit: usize) -> DbResult<Vec<ThreadProbe>> {
        self.database.with_connection(|conn| {
            let mut query = conn.prepare(
                "WITH candidates AS (
                     SELECT t.message_id,COALESCE(e.probe_at,0) AS attempted
                     FROM imap_threads t
                     LEFT JOIN imap_gmail_message_threads e ON e.account_id=t.account_id AND e.message_id=t.message_id
                     WHERE t.account_id=?1 AND e.gmail_thread_id IS NULL AND COALESCE(e.conflicted,0)=0
                       AND EXISTS(SELECT 1 FROM imap_locations l JOIN imap_mailboxes m
                           ON m.account_id=l.account_id AND m.name=l.mailbox AND m.uidvalidity=l.uidvalidity
                           WHERE l.account_id=t.account_id AND l.message_id=t.message_id AND l.uidvalidity>0 AND l.uid>0)
                     ORDER BY attempted,t.message_id LIMIT ?2
                 ), ranked AS (
                     SELECT l.message_id,l.mailbox,l.uidvalidity,l.uid,c.attempted,
                            ROW_NUMBER() OVER (PARTITION BY l.message_id ORDER BY
                                CASE WHEN m.special_use='\\All' THEN 0 WHEN l.mailbox='INBOX' THEN 1 ELSE 2 END,
                                l.mailbox,l.uid DESC) AS position
                     FROM candidates c JOIN imap_locations l ON l.account_id=?1 AND l.message_id=c.message_id
                     JOIN imap_mailboxes m ON m.account_id=l.account_id AND m.name=l.mailbox AND m.uidvalidity=l.uidvalidity
                     WHERE l.uidvalidity>0 AND l.uid>0
                 ) SELECT message_id,mailbox,uidvalidity,uid FROM ranked
                   WHERE position=1 ORDER BY attempted,message_id",
            )?;
            let rows = query.query_map(params![self.account_id, limit as i64], |row| Ok(ThreadProbe {
                message_id: row.get(0)?, mailbox: row.get(1)?, uidvalidity: row.get(2)?, uid: row.get(3)?,
            }))?;
            Ok(rows.collect::<Result<_,_>>()?)
        })
    }

    pub(crate) fn record_gmail_thread_probe(
        &self,
        probe: &ThreadProbe,
        native: Option<(u64, u64)>,
    ) -> DbResult<()> {
        self.database.with_transaction(|tx| {
            let account = &self.account_id;
            tx.execute(
                "INSERT INTO imap_gmail_message_threads(account_id,message_id,probe_at) VALUES(?1,?2,?3)
                 ON CONFLICT(account_id,message_id) DO UPDATE SET probe_at=excluded.probe_at",
                params![account, probe.message_id, crate::clock::unix_millis()],
            )?;
            let Some((message, thread)) = native.filter(|(message, thread)| *message != 0 && *thread != 0) else {
                return Ok(());
            };
            let external = format!("{message:x}");
            // A previously unbound message may learn its Gmail identity only
            // at its exact persisted UID. Existing conflicting bindings win
            // over a new observation and quarantine the repair evidence.
            let (exact, conflict): (bool, bool) = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM imap_locations l JOIN imap_mailboxes m
                    ON m.account_id=l.account_id AND m.name=l.mailbox AND m.uidvalidity=l.uidvalidity
                    WHERE l.account_id=?1 AND l.message_id=?2 AND l.mailbox=?3 AND l.uidvalidity=?4 AND l.uid=?5),
                  EXISTS(SELECT 1 FROM imap_message_identities WHERE account_id=?1 AND namespace='gmail-message-id'
                    AND ((external_id=?6 AND message_id<>?2) OR (message_id=?2 AND external_id<>?6)))",
                params![account, probe.message_id, probe.mailbox, probe.uidvalidity, probe.uid, external],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if !exact {
                return Ok(());
            }
            if conflict {
                tx.execute("UPDATE imap_gmail_message_threads SET conflicted=1 WHERE account_id=?1 AND message_id=?2",
                    params![account, probe.message_id])?;
                return Ok(());
            }
            tx.execute("INSERT OR IGNORE INTO imap_message_identities(account_id,namespace,external_id,message_id)
                VALUES(?1,'gmail-message-id',?2,?3)", params![account, external, probe.message_id])?;
            record_evidence(tx, account, &probe.message_id, &format!("{thread:x}"))?;
            Ok(())
        })
    }

    pub(crate) fn next_gmail_thread_repair(&self, limit: usize) -> DbResult<Option<ThreadRepair>> {
        self.database.with_transaction(|tx| {
            let account = &self.account_id;
            let source: Option<String> = tx.query_row(
                "SELECT t.thread_id FROM imap_threads t
                 JOIN imap_gmail_message_threads e USING(account_id,message_id)
                 LEFT JOIN imap_gmail_threads b ON b.account_id=t.account_id AND b.thread_id=t.thread_id
                 WHERE t.account_id=?1 AND e.gmail_thread_id IS NOT NULL AND e.conflicted=0
                   AND (b.gmail_thread_id IS NULL OR b.gmail_thread_id<>e.gmail_thread_id)
                 GROUP BY t.thread_id ORDER BY MIN(e.repair_at), t.thread_id LIMIT 1",
                [account], |row| row.get(0),
            ).optional()?;
            let Some(source) = source else { return Ok(None); };
            tx.execute(
                "UPDATE imap_gmail_message_threads SET repair_at=?3 WHERE account_id=?1 AND message_id IN (
                    SELECT message_id FROM imap_threads WHERE account_id=?1 AND thread_id=?2
                 )", params![account, source, crate::clock::unix_millis()],
            )?;
            plan_repair(tx, account, &source, limit)
        })
    }
}

fn plan_repair(
    conn: &Connection,
    account: &str,
    source: &str,
    limit: usize,
) -> DbResult<Option<ThreadRepair>> {
    let mut query = conn.prepare(
        "SELECT t.message_id, e.gmail_thread_id
         FROM imap_threads t LEFT JOIN imap_gmail_message_threads e
            ON e.account_id=t.account_id AND e.message_id=t.message_id AND e.conflicted=0
         LEFT JOIN messages m ON m.id=t.message_id AND m.thread_id=?1||':'||t.thread_id
         WHERE t.account_id=?1 AND t.thread_id=?2
         ORDER BY m.sent_at IS NULL,m.sent_at,t.created_generation,t.message_id LIMIT ?3",
    )?;
    let evidence = query
        .query_map(
            params![account, source, limit.saturating_add(1) as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if evidence.is_empty()
        || evidence.len() > limit
        || evidence.iter().any(|(_, native)| native.is_none())
    {
        return Ok(None);
    }
    let source_is_bound: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM imap_gmail_threads WHERE account_id=?1 AND thread_id=?2)",
        params![account, source],
        |row| row.get(0),
    )?;
    let mut source_available = !source_is_bound;
    let mut bindings = BTreeMap::new();
    let mut assignments = BTreeMap::new();
    let mut groups = BTreeMap::<String, BTreeSet<String>>::new();
    for (message, native) in evidence {
        let native = native.expect("all messages have verified evidence");
        let destination = match bindings.get(&native) {
            Some(id) => String::clone(id),
            None => {
                let existing: Option<String> = conn.query_row(
                    "SELECT thread_id FROM imap_gmail_threads WHERE account_id=?1 AND gmail_thread_id=?2",
                    params![account, native], |row| row.get(0),
                ).optional()?;
                let id = if let Some(id) = existing {
                    id
                } else if source_available {
                    source_available = false;
                    source.to_string()
                } else {
                    crate::provider::imap::gmail_thread_id(account, &native)
                };
                bindings.insert(native, id.clone());
                id
            }
        };
        groups
            .entry(destination.clone())
            .or_default()
            .insert(message.clone());
        assignments.insert(message, destination);
    }
    // The old ID remains reserved for its verified Gmail conversation,
    // even when that conversation has no visible member at the moment.
    if source_is_bound && !groups.contains_key(source) {
        return Ok(None);
    }
    for (id, members) in groups.iter_mut().filter(|(id, _)| id.as_str() != source) {
        let mut query = conn.prepare(
            "SELECT message_id FROM imap_threads WHERE account_id=?1 AND thread_id=?2 LIMIT ?3",
        )?;
        for row in query.query_map(
            params![account, id, limit.saturating_add(1) as i64],
            |row| row.get::<_, String>(0),
        )? {
            members.insert(row?);
        }
    }
    if groups.values().map(BTreeSet::len).sum::<usize>() > limit {
        return Ok(None);
    }
    let touched: BTreeSet<_> = groups.keys().cloned().chain([source.to_string()]).collect();
    let mut projected = BTreeSet::new();
    for id in &touched {
        let local = super::super::threads::local_thread_id(account, id);
        let visible: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM threads WHERE account_id=?1 AND id=?2)
             OR EXISTS(SELECT 1 FROM imap_hot_threads WHERE account_id=?1 AND thread_id=?3)",
            params![account, local, id],
            |row| row.get(0),
        )?;
        if visible {
            projected.insert(id.clone());
        }
    }
    // A group split from visible mail must remain visible even when its
    // messages no longer have an Inbox or Sent location.
    if projected.contains(source) {
        projected.extend(groups.keys().cloned());
    }
    projected.retain(|id| groups.contains_key(id));
    let repair = ThreadRepair {
        source: source.into(),
        assignments,
        groups,
        projected,
        bindings,
    };
    if repair.changes_membership() && has_ambiguous_work(conn, account, &repair, &touched)? {
        return Ok(None);
    }
    Ok(Some(repair))
}

fn has_ambiguous_work(
    conn: &Connection,
    account: &str,
    repair: &ThreadRepair,
    touched: &BTreeSet<String>,
) -> DbResult<bool> {
    use super::super::threads::local_thread_id;

    // Conversation-wide writes already in progress cannot acquire a new
    // scope while a repair is in flight. Let existing workflows settle.
    for id in touched {
        let local = local_thread_id(account, id);
        let pending: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM mutations WHERE account_id=?1 AND thread_id=?2
                             AND state IN ('pending','running','failed'))
             OR EXISTS(SELECT 1 FROM conversation_snoozes WHERE account_id=?1 AND id=?2
                             AND (pending_apply=1 OR json_extract(record_json,'$.state')='active'))
             OR EXISTS(SELECT 1 FROM outbox_messages WHERE account=?1
                             AND (state NOT IN ('sent','canceled') OR sent_copy_state IN ('pending','verify'))
                             AND json_extract(payload,'$.threadId')=?3)",
            params![account, local, id], |row| row.get(0),
        )?;
        if pending {
            return Ok(true);
        }
    }
    if repair.groups.contains_key(&repair.source) || repair.survivor().is_some() {
        return Ok(false);
    }
    // A split with no surviving source ID has no honest whole-thread alias.
    // Keep any thread-only links recoverable while their scope is ambiguous.
    let origin = local_thread_id(account, &repair.source);
    let known = serde_json::to_string(&repair.assignments.keys().collect::<Vec<_>>())
        .map_err(serialization_error)?;
    let reference = crate::note_entities::NoteReference {
        version: 1,
        kind: "conversation".into(),
        id: repair.source.clone(),
        account: Some(account.into()),
        calendar: None,
        note: None,
        contact: None,
        series: None,
    }
    .target()
    .map_err(DatabaseError::invalid)?;
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE account_id=?1 AND thread_id=?2
                         AND (source_message_id IS NULL OR source_message_id NOT IN (SELECT value FROM json_each(?5))))
         OR EXISTS(SELECT 1 FROM note_links WHERE target_key=?4)
         OR EXISTS(SELECT 1 FROM imap_thread_aliases WHERE account_id=?1 AND new_id=?3)
         OR EXISTS(SELECT 1 FROM agent_tasks WHERE account_id=?1 AND source_thread_id=?2)
         OR EXISTS(SELECT 1 FROM drafts WHERE json_extract(payload,'$.account')=?1
                         AND json_extract(payload,'$.threadId')=?3
                         AND COALESCE(json_extract(payload,'$.sourceId'),'') NOT IN (SELECT value FROM json_each(?5)))
         OR EXISTS(SELECT 1 FROM discarded_drafts WHERE json_extract(payload,'$.account')=?1
                         AND json_extract(payload,'$.threadId')=?3
                         AND COALESCE(json_extract(payload,'$.sourceId'),'') NOT IN (SELECT value FROM json_each(?5)))",
        params![account, origin, repair.source, reference, known], |row| row.get(0),
    ).map_err(Into::into)
}

impl ImapStateStore {
    /// Publishes both views of the topology together. All already-projected
    /// messages must be in the fetched result before anything can move.
    pub(crate) fn commit_gmail_thread_repair(
        &self,
        repair: &ThreadRepair,
        fetched: &BTreeMap<String, Vec<NormalizedMessage>>,
        limit: usize,
    ) -> DbResult<bool> {
        self.database.with_transaction(|tx| {
            let account = &self.account_id;
            if plan_repair(tx, account, &repair.source, limit)?.as_ref() != Some(repair) {
                return Ok(false);
            }
            if !repair.changes_membership() {
                persist_bindings(tx, account, repair)?;
                return Ok(true);
            }
            if !complete_projection(tx, account, repair, fetched)? {
                return Ok(false);
            }
            persist_bindings(tx, account, repair)?;
            let source = super::super::threads::local_thread_id(account, &repair.source);
            for (message, destination) in &repair.assignments {
                if destination == &repair.source {
                    continue;
                }
                let target = super::super::threads::local_thread_id(account, destination);
                seed_destination(tx, account, &source, &target, destination, message)?;
                tx.execute(
                    "UPDATE imap_threads SET thread_id=?3 WHERE account_id=?1 AND message_id=?2",
                    params![account, message, destination],
                )?;
                tx.execute(
                    "UPDATE messages SET thread_id=?3 WHERE id=?1 AND thread_id=?2",
                    params![message, source, target],
                )?;
                tx.execute(
                    "UPDATE unsubscribe_requests SET thread_id=?3 WHERE message_id=?1 AND thread_id=?2",
                    params![message, source, target],
                )?;
                rebind_tasks(tx, account, &source, &target, Some(message))?;
                rebind_drafts(tx, account, &repair.source, destination, Some(message))?;
            }
            for (id, messages) in fetched {
                if !messages.is_empty() {
                    Database::apply_thread(tx, account, messages)?;
                }
                let local = super::super::threads::local_thread_id(account, id);
                tx.execute("UPDATE threads SET summary=NULL,summary_generated_at=NULL,summary_revision=NULL WHERE id=?1", [&local])?;
                tx.execute("DELETE FROM ai_thread_analyses WHERE thread_id=?1", [&local])?;
                tx.execute("DELETE FROM ai_result_provenance WHERE thread_id=?1", [&local])?;
            }
            if !repair.groups.contains_key(&repair.source) {
                if let Some(survivor) = repair.survivor() {
                    let target = super::super::threads::local_thread_id(account, survivor);
                    tx.execute(
                        "INSERT INTO imap_thread_aliases(account_id,old_id,new_id) VALUES(?1,?2,?3)",
                        params![account, repair.source, survivor],
                    )?;
                    rebind_tasks(tx, account, &source, &target, None)?;
                    rebind_drafts(tx, account, &repair.source, survivor, None)?;
                    tx.execute("UPDATE agent_tasks SET source_thread_id=?3 WHERE account_id=?1 AND source_thread_id=?2
                        AND EXISTS(SELECT 1 FROM threads WHERE id=?3)", params![account, source, target])?;
                }
                tx.execute("DELETE FROM imap_hot_threads WHERE account_id=?1 AND thread_id=?2", params![account, repair.source])?;
                tx.execute("DELETE FROM thread_search WHERE thread_id=?1", [&source])?;
                tx.execute("DELETE FROM threads WHERE id=?1 AND account_id=?2", params![source, account])?;
            }
            journal_repair(tx, account, repair)?;
            Ok(true)
        })
    }
}

fn persist_bindings(tx: &Transaction<'_>, account: &str, repair: &ThreadRepair) -> DbResult<()> {
    for (native, id) in &repair.bindings {
        bind_thread(tx, account, native, id)?;
    }
    Ok(())
}

fn complete_projection(
    tx: &Transaction<'_>,
    account: &str,
    repair: &ThreadRepair,
    fetched: &BTreeMap<String, Vec<NormalizedMessage>>,
) -> DbResult<bool> {
    if !fetched.keys().eq(repair.projected.iter()) {
        return Ok(false);
    }
    let mut received = BTreeMap::new();
    for (id, messages) in fetched {
        for message in messages {
            if message.thread_id != *id
                || !repair.groups[id].contains(&message.id)
                || received.insert(message.id.as_str(), id.as_str()).is_some()
            {
                return Ok(false);
            }
        }
    }
    let mut query = tx.prepare("SELECT id FROM messages WHERE thread_id=?1")?;
    for id in repair.groups.keys().chain(std::iter::once(&repair.source)) {
        let local = super::super::threads::local_thread_id(account, id);
        for row in query.query_map([local], |row| row.get::<_, String>(0))? {
            let message = row?;
            let expected = if id == &repair.source {
                repair.assignments.get(&message)
            } else {
                Some(id)
            };
            if received.get(message.as_str()).copied() != expected.map(String::as_str)
                || expected.is_none()
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Reserve the target row without deleting any message, so its file stars
/// and other message-keyed records survive the FK change.
fn seed_destination(
    tx: &Transaction<'_>,
    account: &str,
    source: &str,
    target: &str,
    provider: &str,
    message: &str,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO threads(id,provider_thread_id,subject,snippet,participants_json,last_message_at,account_id)
         SELECT ?3,?4,t.subject,t.snippet,t.participants_json,t.last_message_at,?1 FROM threads t
         WHERE t.id=?2 AND t.account_id=?1 AND EXISTS(SELECT 1 FROM messages WHERE id=?5 AND thread_id=?2)
         ON CONFLICT(id) DO NOTHING",
        params![account, source, target, provider, message],
    )?;
    Ok(())
}

fn rebind_tasks(
    tx: &Transaction<'_>,
    account: &str,
    source: &str,
    target: &str,
    message: Option<&String>,
) -> DbResult<()> {
    let mut statement = tx.prepare(
        "UPDATE tasks SET thread_id=?3 WHERE account_id=?1 AND thread_id<>?3
         AND ((?4 IS NULL AND thread_id=?2) OR (?4 IS NOT NULL AND source_message_id=?4))
         RETURNING id",
    )?;
    let ids = statement
        .query_map(params![account, source, target, message], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for id in ids {
        let task = super::super::tasks::task_by_id(tx, &id)?
            .ok_or(DatabaseError::NotFound("Repaired task"))?;
        super::super::tasks::record_task(tx, &task, Some(["threadId".into()].into()))?;
    }
    Ok(())
}

fn rebind_drafts(
    tx: &Transaction<'_>,
    account: &str,
    source: &str,
    target: &str,
    message: Option<&String>,
) -> DbResult<()> {
    for table in ["drafts", "discarded_drafts"] {
        tx.execute(
            &format!(
                "UPDATE {table} SET payload=json_set(payload,'$.threadId',?3)
                WHERE json_extract(payload,'$.account')=?1 AND json_extract(payload,'$.threadId')=?2
                  AND (?4 IS NULL OR json_extract(payload,'$.sourceId')=?4)"
            ),
            params![account, source, target, message],
        )?;
    }
    Ok(())
}

fn journal_repair(tx: &Transaction<'_>, account: &str, repair: &ThreadRepair) -> DbResult<()> {
    if repair.projected.is_empty() {
        return Ok(());
    }
    let generation: i64 = tx.query_row(
        "INSERT INTO imap_sync_state(account_id,generation,tokens_backfilled) VALUES (?1,1,1)
         ON CONFLICT(account_id) DO UPDATE SET generation=generation+1 RETURNING generation",
        [account],
        |row| row.get(0),
    )?;
    for id in &repair.projected {
        tx.execute(
            "INSERT INTO imap_hot_threads(account_id,thread_id) VALUES(?1,?2)
             ON CONFLICT(account_id,thread_id) DO UPDATE SET emptied_at_walk=NULL,expiry_journaled=0",
            params![account, id],
        )?;
        tx.execute(
            "INSERT INTO imap_change_journal(account_id,generation,thread_id) VALUES(?1,?2,?3)",
            params![account, generation, id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::imap::{ImapLocation, ImapMailbox, SyncRoundWrite};
    use crate::db::test_support::message;
    use crate::models::MailProviderKind;
    use std::sync::Arc;

    const ACCOUNT: &str = "reader@example.test";

    #[test]
    fn thread_repair_rejects_stale_and_conflicting_probes_and_incomplete_projection() {
        let database = Arc::new(Database::open_memory());
        database
            .adopt_mail_account(ACCOUNT, MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), ACCOUNT);
        store
            .upsert_mailbox(&ImapMailbox {
                name: "INBOX".into(),
                delimiter: Some("/".into()),
                special_use: None,
                uidvalidity: 7,
                uidnext: 3,
                highestmodseq: None,
                permanent_flags_json: None,
                permanent_keywords: None,
            })
            .unwrap();
        store
            .commit_sync_round(&SyncRoundWrite {
                locations: [(1, "root"), (2, "reply")]
                    .into_iter()
                    .map(|(uid, id)| ImapLocation {
                        mailbox: "INBOX".into(),
                        uidvalidity: 7,
                        uid,
                        message_id: id.into(),
                        flags_json: "[]".into(),
                        modseq: None,
                    })
                    .collect(),
                thread_assignments: vec![
                    ("root".into(), "legacy".into()),
                    ("reply".into(), "legacy".into()),
                ],
                hot_threads: vec!["legacy".into()],
                ..Default::default()
            })
            .unwrap();
        database
            .upsert_thread(
                ACCOUNT,
                &[
                    message("root", "legacy", "2025-10-01T09:00:00Z", "Catalog"),
                    message("reply", "legacy", "2025-10-02T09:00:00Z", "Shipping"),
                ],
            )
            .unwrap();
        let mut probe = ThreadProbe {
            message_id: "root".into(),
            mailbox: "INBOX".into(),
            uidvalidity: 6,
            uid: 1,
        };
        store
            .record_gmail_thread_probe(&probe, Some((800, 901)))
            .unwrap();
        assert_eq!(store.gmail_message_id("root").unwrap(), None);
        probe.uidvalidity = 7;
        store
            .record_gmail_thread_probe(&probe, Some((800, 901)))
            .unwrap();
        assert_eq!(store.gmail_message_id("root").unwrap(), Some(800));
        assert!(store.next_gmail_thread_repair(100).unwrap().is_none());
        let reply = ThreadProbe {
            message_id: "reply".into(),
            uid: 2,
            ..probe
        };
        store
            .record_gmail_thread_probe(&reply, Some((810, 902)))
            .unwrap();
        let repair = store.next_gmail_thread_repair(100).unwrap().unwrap();
        assert!(!store
            .commit_gmail_thread_repair(&repair, &BTreeMap::new(), 100)
            .unwrap());
        assert!(store.bound_gmail_thread(901).unwrap().is_none());
        assert_eq!(store.messages_in_thread("legacy").unwrap().len(), 2);
        store
            .record_gmail_thread_probe(&reply, Some((811, 903)))
            .unwrap();
        assert_eq!(store.gmail_message_id("reply").unwrap(), Some(810));
        assert!(store.next_gmail_thread_repair(100).unwrap().is_none());
        assert_eq!(
            database
                .get_thread(&format!("{ACCOUNT}:legacy"))
                .unwrap()
                .messages
                .len(),
            2
        );
    }
}
