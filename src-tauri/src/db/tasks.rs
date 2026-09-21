use chrono::{DateTime, Days, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use super::Database;
use crate::models::{CreateTaskRequest, ThreadTask, UpdateTaskRequest};

const MAX_TITLE: usize = 240;
const MAX_NOTES: usize = 8_000;
const MAX_EVIDENCE: usize = 4_000;

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn validate_task_fields(
    title: &str,
    kind: &str,
    due_kind: &str,
    due_value: Option<&str>,
    repeat_interval_days: Option<i64>,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return Err(format!("Task title must be between 1 and {MAX_TITLE} characters"));
    }
    if !matches!(kind, "action" | "follow_up" | "waiting_for") {
        return Err("Unknown task kind".to_string());
    }
    if !matches!(due_kind, "none" | "date" | "datetime") {
        return Err("Unknown task due kind".to_string());
    }
    if due_kind == "none" && due_value.is_some_and(|value| !value.trim().is_empty()) {
        return Err("A task without a due date cannot have a due value".to_string());
    }
    if due_kind != "none" && due_value.is_none_or(|value| value.trim().is_empty()) {
        return Err("A dated task must have a due value".to_string());
    }
    if repeat_interval_days.is_some_and(|value| !(1..=365).contains(&value)) {
        return Err("Repeat interval must be between 1 and 365 days".to_string());
    }
    Ok(())
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadTask> {
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
    })
}

fn select_sql() -> &'static str {
    "SELECT id, account_id, thread_id, source_message_id, subject_snapshot,
            title, notes, kind, due_kind, due_value, time_zone,
            repeat_interval_days, status, completion_source, evidence_text,
            wait_after, created_at, updated_at, completed_at
     FROM tasks"
}

impl Database {
    pub fn list_tasks(
        &self,
        account_id: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<ThreadTask>, String> {
        let connection = self.connection()?;
        let mut sql = format!("{} WHERE 1=1", select_sql());
        if account_id.is_some() {
            sql.push_str(" AND account_id = ?1");
        }
        if status.is_some() {
            sql.push_str(if account_id.is_some() { " AND status = ?2" } else { " AND status = ?1" });
        }
        sql.push_str(" ORDER BY CASE WHEN status = 'open' THEN 0 ELSE 1 END,
                      CASE WHEN due_value IS NULL THEN 1 ELSE 0 END,
                      due_value ASC, updated_at DESC");
        let mut statement = connection.prepare(&sql).map_err(error)?;
        let rows = match (account_id, status) {
            (Some(account), Some(status)) => statement.query_map(params![account, status], task_from_row),
            (Some(account), None) => statement.query_map(params![account], task_from_row),
            (None, Some(status)) => statement.query_map(params![status], task_from_row),
            (None, None) => statement.query_map([], task_from_row),
        }
        .map_err(error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(error)
    }

    pub fn create_task(&self, request: &CreateTaskRequest) -> Result<ThreadTask, String> {
        validate_task_fields(
            &request.title,
            &request.kind,
            &request.due_kind,
            request.due_value.as_deref(),
            request.repeat_interval_days,
        )?;
        if request.subject_snapshot.trim().is_empty() {
            return Err("Task subject snapshot cannot be empty".to_string());
        }
        if request.notes.as_deref().is_some_and(|value| value.chars().count() > MAX_NOTES) {
            return Err(format!("Task notes exceed {MAX_NOTES} characters"));
        }
        if request.evidence_text.as_deref().is_some_and(|value| value.chars().count() > MAX_EVIDENCE) {
            return Err(format!("Task evidence exceeds {MAX_EVIDENCE} characters"));
        }
        let connection = self.connection()?;
        let wait_after: Option<String> = connection
            .query_row(
                "SELECT last_received_at FROM threads WHERE id = ?1 AND account_id = ?2",
                params![request.thread_id, request.account_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(error)?
            .ok_or_else(|| "Source thread not found".to_string())?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO tasks(
                    id, account_id, thread_id, source_message_id, subject_snapshot,
                    title, notes, kind, due_kind, due_value, time_zone,
                    repeat_interval_days, status, evidence_text, wait_after,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                           'open', ?13, ?14, ?15, ?15)",
                params![
                    id,
                    request.account_id,
                    request.thread_id,
                    request.source_message_id,
                    request.subject_snapshot.trim(),
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
                ],
            )
            .map_err(error)?;
        connection
            .query_row(&format!("{} WHERE id = ?1", select_sql()), [id], task_from_row)
            .map_err(error)
    }

    pub fn update_task(&self, request: &UpdateTaskRequest) -> Result<ThreadTask, String> {
        let current = self
            .list_tasks(None, None)?
            .into_iter()
            .find(|task| task.id == request.id)
            .ok_or_else(|| "Task not found".to_string())?;
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
        if notes.is_some_and(|value| value.chars().count() > MAX_NOTES) {
            return Err(format!("Task notes exceed {MAX_NOTES} characters"));
        }
        let now = Utc::now().to_rfc3339();
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE tasks SET title=?1, notes=?2, kind=?3, due_kind=?4, due_value=?5,
                    time_zone=?6, repeat_interval_days=?7, updated_at=?8 WHERE id=?9",
                params![
                    title.trim(), notes.map(str::trim), kind, due_kind, due_value,
                    time_zone, repeat_interval_days, now, request.id
                ],
            )
            .map_err(error)?;
        connection
            .query_row(&format!("{} WHERE id = ?1", select_sql()), [&request.id], task_from_row)
            .map_err(error)
    }

    pub fn set_task_status(&self, id: &str, status: &str, source: &str) -> Result<ThreadTask, String> {
        if !matches!(status, "open" | "completed" | "cancelled") {
            return Err("Unknown task status".to_string());
        }
        if !matches!(source, "user" | "reply" | "external") {
            return Err("Unknown task completion source".to_string());
        }
        let now = Utc::now().to_rfc3339();
        let completed_at = (status == "completed").then_some(now.as_str());
        let completion_source = if status == "open" { None } else { Some(source) };
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE tasks SET status=?1, completion_source=?2, completed_at=?3, updated_at=?4 WHERE id=?5",
                params![status, completion_source, completed_at, now, id],
            )
            .map_err(error)?;
        connection
            .query_row(&format!("{} WHERE id = ?1", select_sql()), [id], task_from_row)
            .map_err(error)
    }

    pub fn record_follow_up(&self, id: &str) -> Result<ThreadTask, String> {
        let current = self
            .list_tasks(None, None)?
            .into_iter()
            .find(|task| task.id == id)
            .ok_or_else(|| "Task not found".to_string())?;
        if current.status != "open"
            || current.kind != "follow_up"
            || current.repeat_interval_days.is_none()
        {
            return Err("Only open repeating follow-up tasks can be recorded".to_string());
        }
        let interval = current.repeat_interval_days.unwrap_or_default() as i64;
        let due_value = current
            .due_value
            .as_deref()
            .ok_or_else(|| "Repeating follow-up has no due date".to_string())?;
        let next_due = match current.due_kind.as_str() {
            "date" => NaiveDate::parse_from_str(due_value, "%Y-%m-%d")
                .map_err(error)?
                .checked_add_days(Days::new(interval as u64))
                .ok_or_else(|| "Follow-up due date is out of range".to_string())?
                .format("%Y-%m-%d")
                .to_string(),
            "datetime" => {
                let parsed = DateTime::parse_from_rfc3339(due_value).map_err(error)?;
                let time_zone = current
                    .time_zone
                    .as_deref()
                    .unwrap_or("UTC")
                    .parse::<Tz>()
                    .map_err(error)?;
                let local = parsed.with_timezone(&time_zone);
                let next_local = local
                    .naive_local()
                    .checked_add_days(Days::new(interval as u64))
                    .ok_or_else(|| "Follow-up due date is out of range".to_string())?;
                time_zone
                    .from_local_datetime(&next_local)
                    .single()
                    .or_else(|| time_zone.from_local_datetime(&next_local).earliest())
                    .or_else(|| time_zone.from_local_datetime(&next_local).latest())
                    .ok_or_else(|| "Follow-up due date is invalid in its timezone".to_string())?
                    .to_rfc3339()
            }
            _ => return Err("Repeating follow-up must have a date or datetime due value".to_string()),
        };
        let now = Utc::now().to_rfc3339();
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE tasks SET due_value=?1, wait_after=(SELECT last_received_at FROM threads WHERE threads.id=tasks.thread_id), completion_source=NULL, completed_at=NULL, updated_at=?2 WHERE id=?3 AND status='open'",
                params![next_due, now, id],
            )
            .map_err(error)?;
        connection
            .query_row(&format!("{} WHERE id = ?1", select_sql()), [id], task_from_row)
            .map_err(error)
    }

    pub fn reconcile_waiting_tasks(&self) -> Result<usize, String> {
        let now = Utc::now().to_rfc3339();
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE tasks SET status='completed', completion_source='reply', completed_at=?1, updated_at=?1
                 WHERE status='open' AND kind IN ('waiting_for', 'follow_up')
                   AND wait_after IS NOT NULL
                   AND EXISTS (
                     SELECT 1 FROM threads
                     WHERE threads.id = tasks.thread_id
                       AND threads.last_received_at > tasks.wait_after
                   )",
                [&now],
            )
            .map_err(error)?;
        Ok(changed)
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
                thread_id: "account:thread".into(),
                source_message_id: Some("message".into()),
                subject_snapshot: "Planning".into(),
                title: "Follow up with the team".into(),
                notes: Some("Ask for an update".into()),
                kind: "waiting_for".into(),
                due_kind: "date".into(),
                due_value: Some("2026-09-25".into()),
                time_zone: Some("America/New_York".into()),
                repeat_interval_days: None,
                evidence_text: Some("Please reply".into()),
            })
            .unwrap();
        assert_eq!(task.status, "open");
        assert_eq!(task.wait_after.as_deref(), Some("2026-09-19T10:00:00Z"));
        assert_eq!(database.list_tasks(Some("account@example.com"), Some("open")).unwrap().len(), 1);

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
    fn task_validation_rejects_invalid_kind_and_due_shape() {
        let database = database_with_thread();
        let mut request = CreateTaskRequest {
            account_id: "account@example.com".into(),
            thread_id: "account:thread".into(),
            source_message_id: None,
            subject_snapshot: "Planning".into(),
            title: "Do it".into(),
            notes: None,
            kind: "unknown".into(),
            due_kind: "none".into(),
            due_value: Some("2026-09-25".into()),
            time_zone: None,
            repeat_interval_days: None,
            evidence_text: None,
        };
        assert!(database.create_task(&request).is_err());
        request.kind = "action".into();
        assert!(database.create_task(&request).is_err());
    }

    #[test]
    fn recording_a_repeating_follow_up_advances_its_due_date() {
        let database = database_with_thread();
        let task = database
            .create_task(&CreateTaskRequest {
                account_id: "account@example.com".into(),
                thread_id: "account:thread".into(),
                source_message_id: None,
                subject_snapshot: "Planning".into(),
                title: "Check in with the client".into(),
                notes: None,
                kind: "follow_up".into(),
                due_kind: "date".into(),
                due_value: Some("2026-09-25".into()),
                time_zone: Some("America/New_York".into()),
                repeat_interval_days: Some(7),
                evidence_text: None,
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
                thread_id: "account:thread".into(),
                source_message_id: None,
                subject_snapshot: "Planning".into(),
                title: "Check in".into(),
                notes: Some("Bring the status report".into()),
                kind: "follow_up".into(),
                due_kind: "date".into(),
                due_value: Some("2026-09-25".into()),
                time_zone: Some("America/New_York".into()),
                repeat_interval_days: Some(7),
                evidence_text: None,
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
}
