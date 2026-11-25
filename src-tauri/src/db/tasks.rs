use std::collections::BTreeSet;

use chrono::{DateTime, Days, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::{params, OptionalExtension, Transaction};
use threestrands_sync_protocol::EntityType;
use uuid::Uuid;

use super::DatabaseError;
use super::{
    display_error, goals::ensure_goal_link, notes::sync_recording, serialization_error,
    sync_state::record_replicated_write_in_transaction, Database, DbResult,
};
use crate::models::{CreateTaskRequest, ThreadTask, UpdateTaskRequest};

const MAX_TITLE: usize = 240;
const MAX_NOTES: usize = 8_000;
const MAX_EVIDENCE: usize = 4_000;
/// Most recipients kept on a record of the email that finished a task.
const MAX_RECORDED_RECIPIENTS: usize = 20;

/// The draft's To and Cc addresses, normalized, de-duplicated and bounded.
/// Bcc is left out: the synced copy doesn't show it.
fn sent_recipients(draft: &crate::correspondence::Draft) -> Vec<String> {
    let mut seen = BTreeSet::new();
    [&draft.to, &draft.cc]
        .into_iter()
        .flat_map(|list| crate::correspondence::stored_addresses(list))
        .map(|(_, email)| email.trim().to_ascii_lowercase())
        .filter(|email| email.len() <= 320 && seen.insert(email.clone()))
        .take(MAX_RECORDED_RECIPIENTS)
        .collect()
}

/// Bounds the open tasks returned for one person's conversations.
pub(crate) const MAX_CONTACT_TASKS: usize = 50;

fn validate_task_fields(
    title: &str,
    kind: &str,
    due_kind: &str,
    due_value: Option<&str>,
    repeat_interval_days: Option<i64>,
) -> DbResult<()> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return Err(DatabaseError::invalid(format!(
            "Task title must be between 1 and {MAX_TITLE} characters"
        )));
    }
    if !matches!(kind, "action" | "follow_up" | "waiting_for") {
        return Err(DatabaseError::invalid("Unknown task kind"));
    }
    if !matches!(due_kind, "none" | "date" | "datetime") {
        return Err(DatabaseError::invalid("Unknown task due kind"));
    }
    if due_kind == "none" && due_value.is_some_and(|value| !value.trim().is_empty()) {
        return Err(DatabaseError::invalid(
            "A task without a due date cannot have a due value",
        ));
    }
    if due_kind != "none" && due_value.is_none_or(|value| value.trim().is_empty()) {
        return Err(DatabaseError::invalid("A dated task must have a due value"));
    }
    if repeat_interval_days.is_some_and(|value| !(1..=365).contains(&value)) {
        return Err(DatabaseError::invalid(
            "Repeat interval must be between 1 and 365 days",
        ));
    }
    Ok(())
}

fn is_active_status(status: &str) -> bool {
    matches!(status, "open" | "in_progress")
}

pub(super) fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadTask> {
    Ok(ThreadTask {
        id: row.get(0)?,
        account_id: row.get(1)?,
        thread_id: row.get(2)?,
        source_message_id: row.get(3)?,
        subject_snapshot: row.get(4)?,
        title: row.get(5)?,
        notes: row.get(6)?,
        kind: row.get(7)?,
        due_kind: row.get(8)?,
        due_value: row.get(9)?,
        time_zone: row.get(10)?,
        repeat_interval_days: row.get(11)?,
        status: row.get(12)?,
        completion_source: row.get(13)?,
        evidence_text: row.get(14)?,
        wait_after: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        completed_at: row.get(18)?,
        goal_id: row.get(19)?,
        completed_by_email: row
            .get::<_, Option<String>>(20)?
            .and_then(|json| serde_json::from_str(&json).ok()),
    })
}

pub(super) fn select_sql() -> &'static str {
    "SELECT id, account_id, thread_id, source_message_id, subject_snapshot,
            title, notes, kind, due_kind, due_value, time_zone,
            repeat_interval_days, status, completion_source, evidence_text,
            wait_after, created_at, updated_at, completed_at, goal_id, completed_by_email
     FROM tasks"
}

pub(super) fn task_by_id(
    connection: &rusqlite::Connection,
    id: &str,
) -> DbResult<Option<ThreadTask>> {
    Ok(connection
        .query_row(
            &format!("{} WHERE id = ?1", select_sql()),
            [id],
            task_from_row,
        )
        .optional()?)
}

pub(super) fn record_task(
    tx: &Transaction<'_>,
    task: &ThreadTask,
    fields: Option<BTreeSet<String>>,
) -> DbResult<()> {
    if sync_recording(tx)? {
        let payload = serde_json::to_value(task).map_err(serialization_error)?;
        // An offline task can already exist locally when sync is enabled.
        // Its first replica write must publish all required fields.
        let recorded: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM sync_values WHERE entity_type='task' AND entity_id=?1)",
            [&task.id],
            |row| row.get(0),
        )?;
        let fields = fields.filter(|_| recorded).unwrap_or_else(|| {
            payload
                .as_object()
                .into_iter()
                .flat_map(|map| map.keys().cloned())
                .collect()
        });
        record_replicated_write_in_transaction(tx, EntityType::Task, &task.id, &fields, &payload)?;
    }
    Ok(())
}

impl Database {
    pub fn list_tasks(
        &self,
        account_id: Option<&str>,
        status: Option<&str>,
    ) -> DbResult<Vec<ThreadTask>> {
        self.with_connection(|connection| {
            let mut sql = format!("{} WHERE 1=1", select_sql());
            if account_id.is_some() {
                sql.push_str(" AND account_id = ?1");
            }
            if status.is_some() {
                sql.push_str(if account_id.is_some() {
                    " AND status = ?2"
                } else {
                    " AND status = ?1"
                });
            }
            sql.push_str(
                " ORDER BY CASE WHEN status IN ('open', 'in_progress') THEN 0 ELSE 1 END,
                          CASE WHEN due_value IS NULL THEN 1 ELSE 0 END,
                          due_value ASC, updated_at DESC",
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = match (account_id, status) {
                (Some(account), Some(status)) => {
                    statement.query_map(params![account, status], task_from_row)
                }
                (Some(account), None) => statement.query_map(params![account], task_from_row),
                (None, Some(status)) => statement.query_map(params![status], task_from_row),
                (None, None) => statement.query_map([], task_from_row),
            }?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub(crate) fn list_tasks_for_thread(&self, thread_id: &str) -> DbResult<Vec<ThreadTask>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare(&format!("{} WHERE thread_id = ?1", select_sql()))?;
            let rows = statement.query_map([thread_id], task_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Open tasks linked to any conversation that includes one of the
    /// contact's addresses. `id` is a saved contact id or `derived:<email>`.
    pub fn list_contact_tasks(&self, id: &str) -> DbResult<Vec<ThreadTask>> {
        let addresses = self.contact_address_list(id)?;
        if addresses.is_empty() {
            return Ok(Vec::new());
        }
        self.with_connection(|connection| {
            let marks = vec!["?"; addresses.len()].join(",");
            let sql = format!(
                "{} WHERE status IN ('open', 'in_progress')
                   AND thread_id IN (SELECT thread_id FROM contact_interactions WHERE email IN ({marks}))
                 ORDER BY CASE WHEN due_value IS NULL THEN 1 ELSE 0 END, due_value ASC, updated_at DESC
                 LIMIT {MAX_CONTACT_TASKS}",
                select_sql()
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(
                rusqlite::params_from_iter(addresses.iter().map(|email| email.to_ascii_lowercase())),
                task_from_row,
            )?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn create_task(&self, request: &CreateTaskRequest) -> DbResult<ThreadTask> {
        validate_task_fields(
            &request.title,
            &request.kind,
            &request.due_kind,
            request.due_value.as_deref(),
            request.repeat_interval_days,
        )?;
        match (&request.thread_id, &request.subject_snapshot) {
            (Some(_), Some(subject)) if !subject.trim().is_empty() => {}
            (Some(_), _) => {
                return Err(DatabaseError::invalid(
                    "Task subject snapshot cannot be empty",
                ))
            }
            (None, None) => {}
            (None, Some(_)) => {
                return Err(DatabaseError::invalid(
                    "A standalone task cannot have a subject snapshot",
                ))
            }
        }
        if request.thread_id.is_none()
            && (request.source_message_id.is_some() || request.evidence_text.is_some())
        {
            return Err(DatabaseError::invalid(
                "A standalone task cannot have conversation context",
            ));
        }
        if request
            .notes
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_NOTES)
        {
            return Err(DatabaseError::invalid(format!(
                "Task notes exceed {MAX_NOTES} characters"
            )));
        }
        if request
            .evidence_text
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_EVIDENCE)
        {
            return Err(DatabaseError::invalid(format!(
                "Task evidence exceeds {MAX_EVIDENCE} characters"
            )));
        }
        self.with_transaction(|connection| {
            ensure_goal_link(connection, &request.account_id, request.goal_id.as_deref())?;
            let wait_after: Option<String> = match request.thread_id.as_deref() {
                Some(thread_id) => connection
                    .query_row(
                        "SELECT last_received_at FROM threads WHERE id = ?1 AND account_id = ?2",
                        params![thread_id, request.account_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or(DatabaseError::NotFound("Source thread"))?,
                None => None,
            };
            let now = Utc::now().to_rfc3339();
            let id = Uuid::new_v4().to_string();
            connection.execute(
                "INSERT INTO tasks(
                        id, account_id, thread_id, source_message_id, subject_snapshot,
                        title, notes, kind, due_kind, due_value, time_zone,
                        repeat_interval_days, status, evidence_text, wait_after,
                        created_at, updated_at, goal_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                               'open', ?13, ?14, ?15, ?15, ?16)",
                params![
                    id,
                    request.account_id,
                    request.thread_id,
                    request.source_message_id,
                    request.subject_snapshot.as_deref().map(str::trim),
                    request.title.trim(),
                    request.notes.as_deref().map(str::trim),
                    request.kind,
                    request.due_kind,
                    request.due_value,
                    request.time_zone,
                    request.repeat_interval_days,
                    request.evidence_text.as_deref().map(str::trim),
                    wait_after,
                    now,
                    request.goal_id,
                ],
            )?;
            let task = connection.query_row(
                &format!("{} WHERE id = ?1", select_sql()),
                [id],
                task_from_row,
            )?;
            record_task(connection, &task, None)?;
            Ok(task)
        })
    }

    pub fn update_task(&self, request: &UpdateTaskRequest) -> DbResult<ThreadTask> {
        self.with_transaction(|connection| {
            let current = task_by_id(connection, &request.id)?
                .ok_or(DatabaseError::NotFound("Task"))?;
            let title = request.title.as_deref().unwrap_or(&current.title);
            let kind = request.kind.as_deref().unwrap_or(&current.kind);
            let due_kind = request.due_kind.as_deref().unwrap_or(&current.due_kind);
            let due_value = request
                .due_value
                .as_ref()
                .map_or(current.due_value.as_deref(), |value| value.as_deref());
            let repeat_interval_days = request
                .repeat_interval_days
                .unwrap_or(current.repeat_interval_days);
            validate_task_fields(title, kind, due_kind, due_value, repeat_interval_days)?;
            let notes = request
                .notes
                .as_ref()
                .map_or(current.notes.as_deref(), |value| value.as_deref());
            let time_zone = request
                .time_zone
                .as_ref()
                .map_or(current.time_zone.as_deref(), |value| value.as_deref());
            let goal_id = request
                .goal_id
                .as_ref()
                .map_or(current.goal_id.as_deref(), |value| value.as_deref());
            if request.goal_id.is_some() {
                ensure_goal_link(connection, &current.account_id, goal_id)?;
            }
            if notes.is_some_and(|value| value.chars().count() > MAX_NOTES) {
                return Err(DatabaseError::invalid(format!("Task notes exceed {MAX_NOTES} characters")));
            }
            let now = Utc::now().to_rfc3339();
            connection
                .execute(
                    "UPDATE tasks SET title=?1, notes=?2, kind=?3, due_kind=?4, due_value=?5,
                        time_zone=?6, repeat_interval_days=?7, updated_at=?8, goal_id=?10 WHERE id=?9",
                    params![
                        title.trim(), notes.map(str::trim), kind, due_kind, due_value,
                        time_zone, repeat_interval_days, now, request.id, goal_id
                    ],
                )?;
            let task = connection.query_row(&format!("{} WHERE id = ?1", select_sql()), [&request.id], task_from_row)?;
            let fields = [
                ("title", request.title.is_some()),
                ("notes", request.notes.is_some()),
                ("kind", request.kind.is_some()),
                ("dueKind", request.due_kind.is_some()),
                ("dueValue", request.due_value.is_some()),
                ("timeZone", request.time_zone.is_some()),
                ("repeatIntervalDays", request.repeat_interval_days.is_some()),
                ("goalId", request.goal_id.is_some()),
                ("updatedAt", true),
            ].into_iter().filter(|(_, changed)| *changed).map(|(field, _)| field.to_owned()).collect();
            record_task(connection, &task, Some(fields))?;
            Ok(task)
        })
    }

    pub fn set_task_status(&self, id: &str, status: &str, source: &str) -> DbResult<ThreadTask> {
        self.set_task_status_if_current(id, status, source, None)
    }

    pub(crate) fn set_task_status_if_current(
        &self,
        id: &str,
        status: &str,
        source: &str,
        expected: Option<&str>,
    ) -> DbResult<ThreadTask> {
        if !matches!(status, "open" | "in_progress" | "completed" | "cancelled") {
            return Err(DatabaseError::invalid("Unknown task status"));
        }
        if !matches!(source, "user" | "reply" | "external") {
            return Err(DatabaseError::invalid("Unknown task completion source"));
        }
        let now = Utc::now().to_rfc3339();
        let completed_at = (status == "completed").then_some(now.as_str());
        let completion_source = if is_active_status(status) {
            None
        } else {
            Some(source)
        };
        self.with_transaction(|connection| {
            let changed = connection.execute(
                "UPDATE tasks SET status=?1, completion_source=?2, completed_at=?3, updated_at=?4,
                        completed_by_email=CASE WHEN ?1 IN ('open', 'in_progress') THEN NULL ELSE completed_by_email END
                 WHERE id=?5 AND (?6 IS NULL OR updated_at=?6)",
                params![status, completion_source, completed_at, now, id, expected],
            )?;
            if changed == 0 && expected.is_some() {
                return Err(DatabaseError::invalid(
                    "This task changed. Refresh Tasks and try again.",
                ));
            }
            let task = connection.query_row(
                &format!("{} WHERE id = ?1", select_sql()),
                [id],
                task_from_row,
            )?;
            let fields = ["status", "completionSource", "completedAt", "completedByEmail", "updatedAt"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            record_task(connection, &task, Some(fields))?;
            Ok(task)
        })
    }

    pub fn record_follow_up(&self, id: &str) -> DbResult<ThreadTask> {
        self.with_connection(|connection| Self::record_follow_up_on(connection, id))
    }

    pub(crate) fn record_follow_up_on(
        connection: &rusqlite::Connection,
        id: &str,
    ) -> DbResult<ThreadTask> {
        let current = task_by_id(connection, id)?.ok_or(DatabaseError::NotFound("Task"))?;
        if !is_active_status(&current.status)
            || current.kind != "follow_up"
            || current.repeat_interval_days.is_none()
        {
            return Err(DatabaseError::invalid(
                "Only active repeating follow-up tasks can be recorded",
            ));
        }
        let interval = current.repeat_interval_days.unwrap_or_default() as i64;
        let due_value = current
            .due_value
            .as_deref()
            .ok_or_else(|| DatabaseError::invalid("Repeating follow-up has no due date"))?;
        let next_due = match current.due_kind.as_str() {
            "date" => NaiveDate::parse_from_str(due_value, "%Y-%m-%d")
                .map_err(display_error)?
                .checked_add_days(Days::new(interval as u64))
                .ok_or_else(|| DatabaseError::invalid("Follow-up due date is out of range"))?
                .format("%Y-%m-%d")
                .to_string(),
            "datetime" => {
                let parsed = DateTime::parse_from_rfc3339(due_value).map_err(display_error)?;
                let time_zone = current
                    .time_zone
                    .as_deref()
                    .unwrap_or("UTC")
                    .parse::<Tz>()
                    .map_err(display_error)?;
                let local = parsed.with_timezone(&time_zone);
                let next_local = local
                    .naive_local()
                    .checked_add_days(Days::new(interval as u64))
                    .ok_or_else(|| DatabaseError::invalid("Follow-up due date is out of range"))?;
                time_zone
                    .from_local_datetime(&next_local)
                    .single()
                    .or_else(|| time_zone.from_local_datetime(&next_local).earliest())
                    .or_else(|| time_zone.from_local_datetime(&next_local).latest())
                    .ok_or_else(|| {
                        DatabaseError::invalid("Follow-up due date is invalid in its timezone")
                    })?
                    .to_rfc3339()
            }
            _ => {
                return Err(DatabaseError::invalid(
                    "Repeating follow-up must have a date or datetime due value",
                ))
            }
        };
        let now = Utc::now().to_rfc3339();
        connection
                .execute(
                    "UPDATE tasks SET due_value=?1, wait_after=(SELECT last_received_at FROM threads WHERE threads.id=tasks.thread_id), completion_source=NULL, completed_at=NULL, updated_at=?2 WHERE id=?3 AND status IN ('open', 'in_progress')",
                    params![next_due, now, id],
                )?;
        Ok(connection.query_row(
            &format!("{} WHERE id = ?1", select_sql()),
            [id],
            task_from_row,
        )?)
    }

    /// Finishes what a message that just left was linked to. Called in the
    /// transaction that moves an outbox row to `sent`, once per row, for an
    /// immediate send and a scheduled one alike.
    ///
    /// `follow_up_task_id` records a follow-up. `completes_task_id` records a
    /// follow-up when the task repeats, and otherwise completes the task as
    /// the user's (`completion_source = 'user'`, since they chose the link)
    /// and notes the message that did it. A task that is missing, finished, or
    /// can't take the change is left alone: the message is already gone, so a
    /// task problem must never fail the send. The savepoint keeps a failed
    /// half-write out of the transaction.
    pub(crate) fn finish_linked_tasks_on(
        tx: &Transaction<'_>,
        outbox_id: &str,
        at: i64,
    ) -> DbResult<()> {
        let row: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT payload, provider_id FROM outbox_messages WHERE id=?1",
                [outbox_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((payload, provider_id)) = row else {
            return Ok(());
        };
        let Ok(draft) = serde_json::from_str::<crate::correspondence::Draft>(&payload) else {
            return Ok(());
        };
        if draft.follow_up_task_id.is_none() && draft.completes_task_id.is_none() {
            return Ok(());
        }
        tx.execute_batch("SAVEPOINT finish_linked_tasks")?;
        match Self::finish_linked_tasks_inner(tx, &draft, provider_id, at) {
            Ok(()) => tx.execute_batch("RELEASE finish_linked_tasks")?,
            Err(_) => {
                tx.execute_batch("ROLLBACK TO finish_linked_tasks; RELEASE finish_linked_tasks")?
            }
        }
        Ok(())
    }

    fn finish_linked_tasks_inner(
        tx: &Transaction<'_>,
        draft: &crate::correspondence::Draft,
        provider_id: Option<String>,
        at: i64,
    ) -> DbResult<()> {
        const FOLLOW_UP_FIELDS: [&str; 5] = [
            "dueValue",
            "waitAfter",
            "completionSource",
            "completedAt",
            "updatedAt",
        ];
        let record_follow_up = |tx: &Transaction<'_>, id: &str| -> DbResult<()> {
            match Self::record_follow_up_on(tx, id) {
                Ok(task) => record_task(
                    tx,
                    &task,
                    Some(FOLLOW_UP_FIELDS.into_iter().map(str::to_owned).collect()),
                ),
                Err(DatabaseError::NotFound(_)) | Err(DatabaseError::Validation(_)) => Ok(()),
                Err(error) => Err(error),
            }
        };
        if let Some(id) = &draft.follow_up_task_id {
            record_follow_up(tx, id)?;
        }
        let Some(id) = draft
            .completes_task_id
            .as_ref()
            .filter(|id| draft.follow_up_task_id.as_ref() != Some(*id))
        else {
            return Ok(());
        };
        let Some(task) = task_by_id(tx, id)?.filter(|task| is_active_status(&task.status)) else {
            return Ok(());
        };
        if task.kind == "follow_up" && task.repeat_interval_days.is_some() {
            return record_follow_up(tx, id);
        }
        let record = crate::models::CompletedByEmail {
            account_id: draft.account.clone(),
            subject: draft.subject.chars().take(MAX_TITLE).collect(),
            sent_at: at,
            message_id: provider_id,
            thread_id: draft.thread_id.clone(),
            recipients: sent_recipients(draft),
        };
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE tasks SET status='completed', completion_source='user', completed_at=?1, updated_at=?1, completed_by_email=?2
             WHERE id=?3 AND status IN ('open', 'in_progress')",
            params![now, serde_json::to_string(&record).map_err(serialization_error)?, id],
        )?;
        let done = tx.query_row(
            &format!("{} WHERE id = ?1", select_sql()),
            [id],
            task_from_row,
        )?;
        let fields = [
            "status",
            "completionSource",
            "completedAt",
            "completedByEmail",
            "updatedAt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        record_task(tx, &done, Some(fields))
    }

    pub fn reconcile_waiting_tasks(&self) -> DbResult<usize> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let changed = connection
                .execute(
                    "UPDATE tasks SET status='completed', completion_source='reply', completed_at=?1, updated_at=?1
                     WHERE status IN ('open', 'in_progress') AND kind = 'waiting_for'
                       AND wait_after IS NOT NULL
                       AND EXISTS (
                         SELECT 1 FROM threads
                         WHERE threads.id = tasks.thread_id
                           AND threads.last_received_at > tasks.wait_after
                       )",
                    [&now],
                )?;
            Ok(changed)
        })
    }
}

impl Database {
    /// Stores a task another device wrote. An existing row keeps its
    /// `created_at`.
    pub(crate) fn upsert_synced_task(&self, mut task: ThreadTask) -> DbResult<()> {
        self.with_connection(|connection| {
            if let (Some(_), Some(message)) = (&task.thread_id, &task.source_message_id) {
                let verified: Option<String> = connection.query_row(
                    "SELECT t.thread_id FROM imap_threads t
                     JOIN imap_gmail_message_threads e USING(account_id,message_id)
                     JOIN imap_gmail_threads b ON b.account_id=t.account_id AND b.thread_id=t.thread_id
                       AND b.gmail_thread_id=e.gmail_thread_id
                     WHERE t.account_id=?1 AND t.message_id=?2 AND e.conflicted=0",
                    params![task.account_id, message], |row| row.get(0),
                ).optional()?;
                if let Some(provider_id) = verified {
                    task.thread_id = Some(super::threads::local_thread_id(&task.account_id, &provider_id));
                }
            }
            // Only a finished task keeps the record. A device that predates it
            // can reopen a task without knowing to clear it.
            let completed_by_email = task
                .completed_by_email
                .as_ref()
                .filter(|_| !is_active_status(&task.status))
                .map(serde_json::to_string)
                .transpose()
                .map_err(serialization_error)?;
            connection.execute(
                "INSERT INTO tasks(id,account_id,thread_id,source_message_id,subject_snapshot,title,notes,kind,due_kind,due_value,time_zone,repeat_interval_days,status,completion_source,evidence_text,wait_after,created_at,updated_at,completed_at,goal_id,completed_by_email) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21) ON CONFLICT(id) DO UPDATE SET goal_id=excluded.goal_id, completed_by_email=excluded.completed_by_email, account_id=excluded.account_id,thread_id=excluded.thread_id,source_message_id=excluded.source_message_id,subject_snapshot=excluded.subject_snapshot,title=excluded.title,notes=excluded.notes,kind=excluded.kind,due_kind=excluded.due_kind,due_value=excluded.due_value,time_zone=excluded.time_zone,repeat_interval_days=excluded.repeat_interval_days,status=excluded.status,completion_source=excluded.completion_source,evidence_text=excluded.evidence_text,wait_after=excluded.wait_after,updated_at=excluded.updated_at,completed_at=excluded.completed_at",
                params![
                    task.id,
                    task.account_id,
                    task.thread_id,
                    task.source_message_id,
                    task.subject_snapshot,
                    task.title,
                    task.notes,
                    task.kind,
                    task.due_kind,
                    task.due_value,
                    task.time_zone,
                    task.repeat_interval_days,
                    task.status,
                    task.completion_source,
                    task.evidence_text,
                    task.wait_after,
                    task.created_at,
                    task.updated_at,
                    task.completed_at,
                    task.goal_id,
                    completed_by_email,
                ],
            )?;
            Ok(())
        })
    }

    /// Removes a task another device deleted.
    pub(crate) fn delete_synced_task(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM tasks WHERE id=?1", [id])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn database_with_thread() -> Database {
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO threads(
                    id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json,
                    trashed, account_id, summary, summary_generated_at,
                    has_attachments, last_received_at
                 ) VALUES ('account:thread', 'thread', 'Planning', 'Please reply', '[]',
                           '2026-09-19T10:00:00Z', 0, 0, 0, '[]', 0, 'account@example.com',
                           NULL, NULL, 0, '2026-09-19T10:00:00Z')",
                [],
            )
            .unwrap();
        database
    }

    #[test]
    fn task_round_trip_preserves_thread_snapshot_and_reply_baseline() {
        let database = database_with_thread();
        let task = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: Some("account:thread".into()),
                source_message_id: Some("message".into()),
                subject_snapshot: Some("Planning".into()),
                title: "Follow up with the team".into(),
                notes: Some("Ask for an update".into()),
                kind: "waiting_for".into(),
                due_kind: "date".into(),
                due_value: Some("2026-09-25".into()),
                time_zone: Some("America/New_York".into()),
                repeat_interval_days: None,
                evidence_text: Some("Please reply".into()),
                goal_id: None,
            })
            .unwrap();
        assert_eq!(task.status, "open");
        assert_eq!(task.wait_after.as_deref(), Some("2026-09-19T10:00:00Z"));
        assert_eq!(
            database
                .list_tasks(Some("account@example.com"), Some("open"))
                .unwrap()
                .len(),
            1
        );

        database
            .connection()
            .unwrap()
            .execute(
                "UPDATE threads SET last_received_at='2026-09-20T10:00:00Z' WHERE id='account:thread'",
                [],
            )
            .unwrap();
        assert_eq!(database.reconcile_waiting_tasks().unwrap(), 1);
        let completed = database.list_tasks(None, Some("completed")).unwrap();
        assert_eq!(completed[0].completion_source.as_deref(), Some("reply"));
    }

    #[test]
    fn in_progress_tasks_stay_active_and_complete_on_reply() {
        let database = database_with_thread();
        let task = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: Some("account:thread".into()),
                source_message_id: None,
                subject_snapshot: Some("Planning".into()),
                title: "Waiting on the contract".into(),
                notes: None,
                kind: "waiting_for".into(),
                due_kind: "none".into(),
                due_value: None,
                time_zone: None,
                repeat_interval_days: None,
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();

        let started = database
            .set_task_status(&task.id, "in_progress", "user")
            .unwrap();
        assert_eq!(started.status, "in_progress");
        assert_eq!(started.completion_source, None);
        assert_eq!(started.completed_at, None);
        assert!(database
            .set_task_status(&task.id, "blocked", "user")
            .is_err());

        database
            .connection()
            .unwrap()
            .execute(
                "UPDATE threads SET last_received_at='2026-09-20T10:00:00Z' WHERE id='account:thread'",
                [],
            )
            .unwrap();
        assert_eq!(database.reconcile_waiting_tasks().unwrap(), 1);
        let completed = database.list_tasks(None, Some("completed")).unwrap();
        assert_eq!(completed[0].completion_source.as_deref(), Some("reply"));

        let reopened = database.set_task_status(&task.id, "open", "user").unwrap();
        assert_eq!(reopened.completion_source, None);
        assert_eq!(reopened.completed_at, None);
    }

    #[test]
    fn version_checked_completion_can_restore_in_progress_without_replaying_stale_undo() {
        let database = database_with_thread();
        let request = serde_json::from_value(serde_json::json!({
            "accountId": "account@example.com", "threadId": "account:thread",
            "subjectSnapshot": "Planning", "title": "Call the library", "kind": "action",
        }))
        .unwrap();
        let task = database.create_task(&request).unwrap();
        let started = database
            .set_task_status(&task.id, "in_progress", "user")
            .unwrap();
        let completed = database
            .set_task_status_if_current(&task.id, "completed", "user", Some(&started.updated_at))
            .unwrap();
        let restored = database
            .set_task_status_if_current(
                &task.id,
                "in_progress",
                "user",
                Some(&completed.updated_at),
            )
            .unwrap();
        assert_eq!(restored.status, "in_progress");
        assert_eq!(restored.completed_at, None);

        assert!(database
            .set_task_status_if_current(&task.id, "open", "user", Some(&completed.updated_at))
            .is_err());
        assert_eq!(
            database.list_tasks(None, Some("in_progress")).unwrap()[0].updated_at,
            restored.updated_at
        );
    }

    #[test]
    fn replies_do_not_complete_action_or_follow_up_tasks() {
        for kind in ["action", "follow_up"] {
            for status in ["open", "in_progress"] {
                let database = database_with_thread();
                let request = serde_json::from_value(serde_json::json!({
                    "accountId": "account@example.com", "threadId": "account:thread",
                    "subjectSnapshot": "Planning", "title": "Send the draft", "kind": kind,
                }))
                .unwrap();
                let task = database.create_task(&request).unwrap();
                database.set_task_status(&task.id, status, "user").unwrap();
                database.connection().unwrap().execute(
                    "UPDATE threads SET last_received_at='2026-09-20T10:00:00Z' WHERE id='account:thread'", [],
                ).unwrap();
                assert_eq!(database.reconcile_waiting_tasks().unwrap(), 0);
                let tasks = database.list_tasks(None, Some(status)).unwrap();
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].completion_source, None);
                assert_eq!(tasks[0].completed_at, None);
            }
        }
    }

    #[test]
    fn standalone_task_round_trip_has_no_conversation_context() {
        let database = database_with_thread();
        let task = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: None,
                source_message_id: None,
                subject_snapshot: None,
                title: "Buy printer paper".into(),
                notes: None,
                kind: "action".into(),
                due_kind: "none".into(),
                due_value: None,
                time_zone: None,
                repeat_interval_days: None,
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();

        assert_eq!(task.thread_id, None);
        assert_eq!(task.subject_snapshot, None);
        assert_eq!(task.source_message_id, None);
        assert_eq!(task.evidence_text, None);
        assert_eq!(task.wait_after, None);
    }

    #[test]
    fn task_validation_rejects_invalid_kind_and_due_shape() {
        let database = database_with_thread();
        let mut request = CreateTaskRequest {
            account_id: "account@example.com".into(),
            thread_id: Some("account:thread".into()),
            source_message_id: None,
            subject_snapshot: Some("Planning".into()),
            title: "Do it".into(),
            notes: None,
            kind: "unknown".into(),
            due_kind: "none".into(),
            due_value: Some("2026-09-25".into()),
            time_zone: None,
            repeat_interval_days: None,
            evidence_text: None,
            goal_id: None,
        };
        assert!(database.create_task(&request).is_err());
        request.kind = "action".into();
        assert!(database.create_task(&request).is_err());
    }

    #[test]
    fn a_synced_task_keeps_the_email_that_finished_it_and_an_older_write_still_applies() {
        let source = Database::open_memory();
        let task = source
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: None,
                source_message_id: None,
                subject_snapshot: None,
                title: "Send the contract".into(),
                notes: None,
                kind: "action".into(),
                due_kind: "none".into(),
                due_value: None,
                time_zone: None,
                repeat_interval_days: None,
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();
        let record = crate::models::CompletedByEmail {
            account_id: "account@example.com".into(),
            subject: "The contract".into(),
            sent_at: 1_900_000_000_000,
            message_id: Some("m-1".into()),
            thread_id: None,
            recipients: vec![],
        };
        let mut finished = task.clone();
        finished.status = "completed".into();
        finished.completion_source = Some("user".into());
        finished.completed_at = Some("2026-11-25T10:00:00Z".into());
        finished.completed_by_email = Some(record.clone());

        let peer = Database::open_memory();
        let payload = serde_json::to_value(&finished).unwrap();
        assert_eq!(payload["completedByEmail"]["messageId"], "m-1");
        threestrands_sync_protocol::EntityType::Task
            .validate_payload(&payload)
            .unwrap();
        peer.upsert_synced_task(serde_json::from_value(payload).unwrap())
            .unwrap();
        assert_eq!(
            peer.list_tasks(None, None).unwrap()[0].completed_by_email,
            Some(record)
        );

        // A device that predates the field writes none, and still applies.
        let mut older = serde_json::to_value(&task).unwrap();
        older.as_object_mut().unwrap().remove("completedByEmail");
        threestrands_sync_protocol::EntityType::Task
            .validate_payload(&older)
            .unwrap();
        let older: ThreadTask = serde_json::from_value(older).unwrap();
        assert_eq!(older.completed_by_email, None);
        peer.upsert_synced_task(older).unwrap();
        let reopened = &peer.list_tasks(None, None).unwrap()[0];
        assert_eq!(reopened.status, "open");
        assert_eq!(
            reopened.completed_by_email, None,
            "reopening elsewhere drops the record"
        );

        // A record that arrives on a task that is open is not kept.
        let mut stale = finished;
        stale.status = "open".into();
        peer.upsert_synced_task(stale).unwrap();
        assert_eq!(
            peer.list_tasks(None, None).unwrap()[0].completed_by_email,
            None
        );
    }

    #[test]
    fn recording_a_repeating_follow_up_advances_its_due_date() {
        let database = database_with_thread();
        let task = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: Some("account:thread".into()),
                source_message_id: None,
                subject_snapshot: Some("Planning".into()),
                title: "Check in with the client".into(),
                notes: None,
                kind: "follow_up".into(),
                due_kind: "date".into(),
                due_value: Some("2026-09-25".into()),
                time_zone: Some("America/New_York".into()),
                repeat_interval_days: Some(7),
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();

        let next = database.record_follow_up(&task.id).unwrap();
        assert_eq!(next.status, "open");
        assert_eq!(next.due_value.as_deref(), Some("2026-10-02"));
        assert_eq!(next.completion_source, None);
    }

    #[test]
    fn updating_a_task_can_clear_optional_details() {
        let database = database_with_thread();
        let task = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: Some("account:thread".into()),
                source_message_id: None,
                subject_snapshot: Some("Planning".into()),
                title: "Check in".into(),
                notes: Some("Bring the status report".into()),
                kind: "follow_up".into(),
                due_kind: "date".into(),
                due_value: Some("2026-09-25".into()),
                time_zone: Some("America/New_York".into()),
                repeat_interval_days: Some(7),
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();

        let request: UpdateTaskRequest = serde_json::from_value(serde_json::json!({
            "id": task.id,
            "notes": null,
            "kind": "action",
            "dueKind": "none",
            "dueValue": null,
            "timeZone": null,
            "repeatIntervalDays": null
        }))
        .unwrap();
        let updated = database.update_task(&request).unwrap();

        assert_eq!(updated.notes, None);
        assert_eq!(updated.due_kind, "none");
        assert_eq!(updated.due_value, None);
        assert_eq!(updated.time_zone, None);
        assert_eq!(updated.repeat_interval_days, None);
    }

    #[test]
    fn task_mutations_do_not_decode_unrelated_rows() {
        let database = database_with_thread();
        let target = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: Some("account:thread".into()),
                source_message_id: None,
                subject_snapshot: Some("Planning".into()),
                title: "Check in with the client".into(),
                notes: None,
                kind: "follow_up".into(),
                due_kind: "date".into(),
                due_value: Some("2026-09-25".into()),
                time_zone: Some("America/New_York".into()),
                repeat_interval_days: Some(7),
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();
        let unrelated = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: None,
                source_message_id: None,
                subject_snapshot: None,
                title: "Unrelated task".into(),
                notes: None,
                kind: "action".into(),
                due_kind: "none".into(),
                due_value: None,
                time_zone: None,
                repeat_interval_days: None,
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "UPDATE tasks SET title = X'80' WHERE id = ?1",
                [&unrelated.id],
            )
            .unwrap();
        assert!(database.list_tasks(None, None).is_err());

        let update: UpdateTaskRequest = serde_json::from_value(serde_json::json!({
            "id": target.id,
            "notes": "Reached out by phone"
        }))
        .unwrap();
        let updated = database.update_task(&update).unwrap();
        assert_eq!(updated.notes.as_deref(), Some("Reached out by phone"));

        let next = database.record_follow_up(&updated.id).unwrap();
        assert_eq!(next.due_value.as_deref(), Some("2026-10-02"));
    }
}
