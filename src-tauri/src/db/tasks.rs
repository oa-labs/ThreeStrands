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

pub(super) fn is_active_status(status: &str) -> bool {
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
        board_rank: row.get(21)?,
    })
}

pub(super) fn select_sql() -> &'static str {
    "SELECT id, account_id, thread_id, source_message_id, subject_snapshot,
            title, notes, kind, due_kind, due_value, time_zone,
            repeat_interval_days, status, completion_source, evidence_text,
            wait_after, created_at, updated_at, completed_at, goal_id, completed_by_email,
            board_rank
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

/// Writes a status change and what goes with it. A task that changes status
/// leaves its board column, so it becomes unplaced unless `place` gives it a
/// rank in the new one; one that keeps its status keeps its rank. Returns
/// the number of rows written: none when `expected` no longer matches.
fn write_status(
    connection: &rusqlite::Connection,
    id: &str,
    status: &str,
    source: &str,
    now: &str,
    expected: Option<&str>,
    place: Option<f64>,
) -> DbResult<usize> {
    let completed_at = (status == "completed").then_some(now);
    let completion_source = if is_active_status(status) {
        None
    } else {
        Some(source)
    };
    Ok(connection.execute(
        "UPDATE tasks SET status=?1, completion_source=?2, completed_at=?3, updated_at=?4,
                completed_by_email=CASE WHEN ?1 IN ('open', 'in_progress') THEN NULL ELSE completed_by_email END,
                board_rank=CASE WHEN ?7 IS NOT NULL THEN ?7 WHEN status=?1 THEN board_rank ELSE NULL END
         WHERE id=?5 AND (?6 IS NULL OR updated_at=?6)",
        params![status, completion_source, completed_at, now, id, expected, place],
    )?)
}

/// Ranks start this far apart, and a task placed at either end of a column
/// lands this far past its neighbor.
const BOARD_RANK_STEP: f64 = 1024.0;
/// Below this gap between neighbors, a midpoint is too close to tell apart
/// reliably and the column is renumbered instead.
const MIN_BOARD_RANK_GAP: f64 = 1e-6;
/// Most tasks one board placement can order.
const MAX_BOARD_COLUMN: usize = 10_000;

/// The rank writes that make a task board column read in `order`, top to
/// bottom, once `moved` is placed. `order` lists each task with its current
/// rank (`None` for unplaced) and includes `moved` exactly once; its other
/// tasks are already in the column.
///
/// Unplaced tasks show above placed ones, so giving only `moved` a rank
/// keeps the requested order just when every other task is placed and
/// already reads in that order. Then `moved` goes halfway between its
/// neighbors, or a step past the one at an edge, and keeps its rank when it
/// already sits between them. Otherwise, or when the neighbors are too
/// close to split, every task is renumbered one step apart and only the
/// ranks that change are returned.
pub(crate) fn plan_board_ranks(order: &[(&str, Option<f64>)], moved: &str) -> Vec<(String, f64)> {
    let Some(index) = order.iter().position(|(id, _)| *id == moved) else {
        return Vec::new();
    };
    let others: Option<Vec<f64>> = order
        .iter()
        .enumerate()
        .filter(|(position, _)| *position != index)
        .map(|(_, (_, rank))| *rank)
        .collect();
    if let Some(others) = others.filter(|ranks| ranks.windows(2).all(|pair| pair[0] < pair[1])) {
        let previous = index.checked_sub(1).map(|position| others[position]);
        let next = others.get(index).copied();
        let fits = |rank: f64| {
            previous.is_none_or(|previous| previous < rank) && next.is_none_or(|next| rank < next)
        };
        if order[index].1.is_some_and(fits) {
            return Vec::new();
        }
        let rank = match (previous, next) {
            (Some(previous), Some(next)) if next - previous > MIN_BOARD_RANK_GAP => {
                Some(previous + (next - previous) / 2.0)
            }
            (Some(_), Some(_)) => None,
            (Some(previous), None) => Some(previous + BOARD_RANK_STEP),
            (None, Some(next)) => Some(next - BOARD_RANK_STEP),
            (None, None) => Some(BOARD_RANK_STEP),
        };
        if let Some(rank) = rank.filter(|rank| rank.is_finite() && fits(*rank)) {
            return vec![(moved.to_owned(), rank)];
        }
    }
    order
        .iter()
        .enumerate()
        .filter_map(|(position, (id, rank))| {
            let wanted = (position + 1) as f64 * BOARD_RANK_STEP;
            (*rank != Some(wanted)).then(|| ((*id).to_owned(), wanted))
        })
        .collect()
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
        self.with_transaction(|connection| {
            let before: Option<String> = connection
                .query_row("SELECT status FROM tasks WHERE id=?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            let changed = write_status(connection, id, status, source, &now, expected, None)?;
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
            let mut fields: BTreeSet<String> = [
                "status",
                "completionSource",
                "completedAt",
                "completedByEmail",
                "updatedAt",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect();
            if before.as_deref() != Some(status) {
                fields.insert("boardRank".to_owned());
            }
            record_task(connection, &task, Some(fields))?;
            Ok(task)
        })
    }

    /// Places a task in a task board column (`open` is To Do, `in_progress`
    /// is In Progress), moving it there first when it is elsewhere, with the
    /// same side effects as [`Self::set_task_status`].
    ///
    /// `ordered_ids` is the column's intended order after the move, top to
    /// bottom, and lists `id` exactly once. Ids that are unknown or whose
    /// task is in another column are ignored, since the caller's list can be
    /// slightly out of date. Only the moved task gets a rank when that keeps
    /// the requested order; otherwise the column is renumbered (see
    /// [`plan_board_ranks`]). Returns the moved task and every task whose
    /// rank changed, in column order.
    pub fn place_task_on_board(
        &self,
        id: &str,
        status: &str,
        ordered_ids: &[String],
    ) -> DbResult<Vec<ThreadTask>> {
        if !is_active_status(status) {
            return Err(DatabaseError::invalid(
                "A task can only be placed in To Do or In Progress",
            ));
        }
        if ordered_ids.len() > MAX_BOARD_COLUMN {
            return Err(DatabaseError::invalid(format!(
                "A task board column can be ordered with at most {MAX_BOARD_COLUMN} tasks"
            )));
        }
        let mut seen = BTreeSet::new();
        if !ordered_ids
            .iter()
            .all(|listed| seen.insert(listed.as_str()))
        {
            return Err(DatabaseError::invalid(
                "A task board order lists a task more than once",
            ));
        }
        if !seen.contains(id) {
            return Err(DatabaseError::invalid(
                "A task board order must include the task being placed",
            ));
        }
        self.with_transaction(|connection| {
            let mut column = Vec::with_capacity(ordered_ids.len());
            let mut moved_from = None;
            {
                let mut statement =
                    connection.prepare("SELECT status, board_rank FROM tasks WHERE id=?1")?;
                for listed in ordered_ids {
                    let row: Option<(String, Option<f64>)> = statement
                        .query_row([listed], |row| Ok((row.get(0)?, row.get(1)?)))
                        .optional()?;
                    let Some((current, rank)) = row else {
                        continue;
                    };
                    if listed == id {
                        moved_from = Some(current);
                    } else if current != status {
                        continue;
                    }
                    column.push((listed.as_str(), rank.filter(|rank| rank.is_finite())));
                }
            }
            let moved_from = moved_from.ok_or(DatabaseError::NotFound("Task"))?;
            let moves = moved_from != status;
            let ranks = plan_board_ranks(&column, id);
            let now = Utc::now().to_rfc3339();
            let mut changed: BTreeSet<&str> =
                ranks.iter().map(|(ranked, _)| ranked.as_str()).collect();
            if moves {
                let rank = ranks
                    .iter()
                    .find(|(ranked, _)| ranked == id)
                    .map(|(_, rank)| *rank)
                    .or_else(|| column.iter().find(|(listed, _)| *listed == id)?.1);
                write_status(connection, id, status, "user", &now, None, rank)?;
                changed.insert(id);
            }
            for (ranked, rank) in &ranks {
                connection.execute(
                    "UPDATE tasks SET board_rank=?1, updated_at=?2 WHERE id=?3",
                    params![rank, now, ranked],
                )?;
            }
            let mut placed = Vec::new();
            for (listed, _) in &column {
                if *listed != id && !changed.contains(listed) {
                    continue;
                }
                let task =
                    task_by_id(connection, listed)?.ok_or(DatabaseError::NotFound("Task"))?;
                if changed.contains(listed) {
                    let mut fields: BTreeSet<String> = ["boardRank", "updatedAt"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect();
                    if *listed == id && moves {
                        fields.extend(
                            [
                                "status",
                                "completionSource",
                                "completedAt",
                                "completedByEmail",
                            ]
                            .into_iter()
                            .map(str::to_owned),
                        );
                    }
                    record_task(connection, &task, Some(fields))?;
                }
                placed.push(task);
            }
            Ok(placed)
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
            "UPDATE tasks SET status='completed', completion_source='user', completed_at=?1, updated_at=?1, completed_by_email=?2,
                    board_rank=NULL
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
            "boardRank",
            "updatedAt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        record_task(tx, &done, Some(fields))
    }

    /// Completes waiting-for tasks whose conversation got a reply after the
    /// wait began, and records each change for sync. Returns how many.
    pub fn reconcile_waiting_tasks(&self) -> DbResult<usize> {
        let now = Utc::now().to_rfc3339();
        self.with_transaction(|connection| {
            let ids = connection
                .prepare(
                    "UPDATE tasks SET status='completed', completion_source='reply', completed_at=?1, updated_at=?1,
                            board_rank=NULL
                     WHERE status IN ('open', 'in_progress') AND kind = 'waiting_for'
                       AND wait_after IS NOT NULL
                       AND EXISTS (
                         SELECT 1 FROM threads
                         WHERE threads.id = tasks.thread_id
                           AND threads.last_received_at > tasks.wait_after
                       )
                     RETURNING id",
                )?
                .query_map([&now], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for id in &ids {
                let task = task_by_id(connection, id)?.ok_or(DatabaseError::NotFound("Task"))?;
                let fields = [
                    "status",
                    "completionSource",
                    "completedAt",
                    "boardRank",
                    "updatedAt",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect();
                record_task(connection, &task, Some(fields))?;
            }
            Ok(ids.len())
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
                "INSERT INTO tasks(id,account_id,thread_id,source_message_id,subject_snapshot,title,notes,kind,due_kind,due_value,time_zone,repeat_interval_days,status,completion_source,evidence_text,wait_after,created_at,updated_at,completed_at,goal_id,completed_by_email,board_rank) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22) ON CONFLICT(id) DO UPDATE SET goal_id=excluded.goal_id, completed_by_email=excluded.completed_by_email, board_rank=excluded.board_rank, account_id=excluded.account_id,thread_id=excluded.thread_id,source_message_id=excluded.source_message_id,subject_snapshot=excluded.subject_snapshot,title=excluded.title,notes=excluded.notes,kind=excluded.kind,due_kind=excluded.due_kind,due_value=excluded.due_value,time_zone=excluded.time_zone,repeat_interval_days=excluded.repeat_interval_days,status=excluded.status,completion_source=excluded.completion_source,evidence_text=excluded.evidence_text,wait_after=excluded.wait_after,updated_at=excluded.updated_at,completed_at=excluded.completed_at",
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
                    task.board_rank.filter(|rank| rank.is_finite()),
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

    mod board {
        use super::*;

        fn plan(order: &[(&str, Option<f64>)], moved: &str) -> Vec<(String, f64)> {
            plan_board_ranks(order, moved)
        }

        fn updates(pairs: &[(&str, f64)]) -> Vec<(String, f64)> {
            pairs
                .iter()
                .map(|(id, rank)| ((*id).to_owned(), *rank))
                .collect()
        }

        fn standalone(database: &Database, title: &str) -> ThreadTask {
            database
                .create_task(
                    &serde_json::from_value(serde_json::json!({
                        "accountId": "account@example.com", "title": title, "kind": "action",
                    }))
                    .unwrap(),
                )
                .unwrap()
        }

        fn ids(tasks: &[&ThreadTask]) -> Vec<String> {
            tasks.iter().map(|task| task.id.clone()).collect()
        }

        fn get(database: &Database, id: &str) -> ThreadTask {
            database
                .list_tasks(None, None)
                .unwrap()
                .into_iter()
                .find(|task| task.id == id)
                .unwrap()
        }

        /// The board's order for a column: unplaced first, then by rank.
        fn column(database: &Database, status: &str, unplaced: &[&str]) -> Vec<String> {
            let mut tasks = database.list_tasks(None, Some(status)).unwrap();
            tasks.sort_by(|a, b| match (a.board_rank, b.board_rank) {
                (None, None) => {
                    let position =
                        |id: &str| unplaced.iter().position(|candidate| *candidate == id);
                    position(&a.id).cmp(&position(&b.id))
                }
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a), Some(b)) => a.total_cmp(&b),
            });
            tasks.into_iter().map(|task| task.id).collect()
        }

        #[test]
        fn a_first_placement_in_an_unplaced_column_numbers_every_task() {
            assert_eq!(
                plan(&[("a", None), ("m", None), ("b", None)], "m"),
                updates(&[("a", 1024.0), ("m", 2048.0), ("b", 3072.0)])
            );
            assert_eq!(plan(&[("m", None)], "m"), updates(&[("m", 1024.0)]));
        }

        #[test]
        fn a_move_among_placed_tasks_ranks_only_the_moved_task() {
            let placed = [("a", Some(1024.0)), ("b", Some(2048.0))];
            // To the top, the bottom, and between.
            assert_eq!(
                plan(&[("m", Some(4096.0)), placed[0], placed[1]], "m"),
                updates(&[("m", 0.0)])
            );
            assert_eq!(
                plan(&[placed[0], placed[1], ("m", Some(0.0))], "m"),
                updates(&[("m", 3072.0)])
            );
            assert_eq!(
                plan(&[placed[0], ("m", Some(4096.0)), placed[1]], "m"),
                updates(&[("m", 1536.0)])
            );
            // An unplaced task placed among placed ones.
            assert_eq!(
                plan(&[placed[0], ("m", None), placed[1]], "m"),
                updates(&[("m", 1536.0)])
            );
        }

        #[test]
        fn a_task_already_in_place_keeps_its_rank() {
            assert!(plan(
                &[
                    ("a", Some(1024.0)),
                    ("m", Some(1100.0)),
                    ("b", Some(2048.0))
                ],
                "m"
            )
            .is_empty());
            assert!(plan(&[("m", Some(7.0)), ("a", Some(1024.0))], "m").is_empty());
            assert!(plan(&[("m", Some(7.0))], "m").is_empty());
        }

        #[test]
        fn neighbors_too_close_to_split_renumber_the_column() {
            let close = 1024.0 + 1e-7;
            assert_eq!(
                plan(
                    &[("a", Some(1024.0)), ("m", Some(9000.0)), ("b", Some(close))],
                    "m"
                ),
                updates(&[("m", 2048.0), ("b", 3072.0)])
            );
            // Halving the same gap again and again ends in a renumber too.
            let mut ranks = [("a", Some(1024.0)), ("m", None), ("b", Some(2048.0))];
            let mut renumbered = false;
            for _ in 0..64 {
                let planned = plan(&ranks, "m");
                if planned.len() > 1 {
                    renumbered = true;
                    break;
                }
                // Moves to just above b, whose rank becomes the new upper bound.
                let rank = planned[0].1;
                ranks = [("a", ranks[0].1), ("m", None), ("b", Some(rank))];
            }
            assert!(renumbered);
        }

        #[test]
        fn unplaced_tasks_in_the_column_renumber_it() {
            // An unplaced task shows above placed ones, so a rank for only the
            // moved task can't keep it below the unplaced task or above one.
            assert_eq!(
                plan(&[("u", None), ("m", None), ("p", Some(1024.0))], "m"),
                updates(&[("u", 1024.0), ("m", 2048.0), ("p", 3072.0)])
            );
            assert_eq!(
                plan(&[("p", Some(1024.0)), ("m", None), ("u", None)], "m"),
                updates(&[("m", 2048.0), ("u", 3072.0)])
            );
            assert_eq!(
                plan(&[("m", Some(5.0)), ("u", None), ("p", Some(2048.0))], "m"),
                updates(&[("m", 1024.0), ("u", 2048.0), ("p", 3072.0)])
            );
        }

        #[test]
        fn placed_tasks_out_of_the_requested_order_renumber_the_column() {
            assert_eq!(
                plan(
                    &[("b", Some(2048.0)), ("m", None), ("a", Some(1024.0))],
                    "m"
                ),
                updates(&[("b", 1024.0), ("m", 2048.0), ("a", 3072.0)])
            );
        }

        #[test]
        fn a_task_missing_from_the_order_plans_nothing() {
            assert!(plan(&[("a", Some(1024.0))], "m").is_empty());
        }

        #[test]
        fn placing_tasks_reorders_the_column_and_writes_only_changed_rows() {
            let database = Database::open_memory();
            let a = standalone(&database, "A");
            let b = standalone(&database, "B");
            let c = standalone(&database, "C");

            // The first placement numbers the whole column.
            let placed = database
                .place_task_on_board(&c.id, "open", &ids(&[&c, &a, &b]))
                .unwrap();
            assert_eq!(placed.len(), 3);
            assert_eq!(column(&database, "open", &[]), ids(&[&c, &a, &b]));
            assert!(placed.iter().all(|task| task.board_rank.is_some()));

            // Moving b between c and a ranks only b.
            let a_before = get(&database, &a.id);
            let c_before = get(&database, &c.id);
            let placed = database
                .place_task_on_board(&b.id, "open", &ids(&[&c, &b, &a]))
                .unwrap();
            assert_eq!(ids(&placed.iter().collect::<Vec<_>>()), vec![b.id.clone()]);
            assert_eq!(column(&database, "open", &[]), ids(&[&c, &b, &a]));
            assert_eq!(get(&database, &a.id).updated_at, a_before.updated_at);
            assert_eq!(get(&database, &c.id).updated_at, c_before.updated_at);

            // To the top and back to the bottom.
            database
                .place_task_on_board(&a.id, "open", &ids(&[&a, &c, &b]))
                .unwrap();
            assert_eq!(column(&database, "open", &[]), ids(&[&a, &c, &b]));
            database
                .place_task_on_board(&a.id, "open", &ids(&[&c, &b, &a]))
                .unwrap();
            assert_eq!(column(&database, "open", &[]), ids(&[&c, &b, &a]));

            // Placing a task where it already is changes nothing.
            let b_before = get(&database, &b.id);
            let placed = database
                .place_task_on_board(&b.id, "open", &ids(&[&c, &b, &a]))
                .unwrap();
            assert_eq!(placed[0].updated_at, b_before.updated_at);
            assert_eq!(placed[0].board_rank, b_before.board_rank);
        }

        #[test]
        fn a_new_unplaced_task_shows_first_until_the_column_is_renumbered() {
            let database = Database::open_memory();
            let a = standalone(&database, "A");
            let b = standalone(&database, "B");
            database
                .place_task_on_board(&b.id, "open", &ids(&[&b, &a]))
                .unwrap();
            let new = standalone(&database, "New");
            assert_eq!(new.board_rank, None);
            assert_eq!(column(&database, "open", &[]), ids(&[&new, &b, &a]));
            // Moving a placed task below the unplaced one renumbers the column.
            let placed = database
                .place_task_on_board(&a.id, "open", &ids(&[&new, &a, &b]))
                .unwrap();
            assert_eq!(placed.len(), 3);
            assert_eq!(column(&database, "open", &[]), ids(&[&new, &a, &b]));
        }

        #[test]
        fn placing_a_task_in_another_column_moves_it_there() {
            let database = Database::open_memory();
            let a = standalone(&database, "A");
            let b = standalone(&database, "B");
            let moving = standalone(&database, "Moving");
            database
                .set_task_status(&a.id, "in_progress", "user")
                .unwrap();
            database
                .set_task_status(&b.id, "in_progress", "user")
                .unwrap();
            database
                .place_task_on_board(&a.id, "in_progress", &ids(&[&a, &b]))
                .unwrap();
            database
                .place_task_on_board(&moving.id, "open", &ids(&[&moving]))
                .unwrap();

            let placed = database
                .place_task_on_board(&moving.id, "in_progress", &ids(&[&a, &moving, &b]))
                .unwrap();
            assert_eq!(
                ids(&placed.iter().collect::<Vec<_>>()),
                vec![moving.id.clone()]
            );
            let moved = &placed[0];
            assert_eq!(moved.status, "in_progress");
            assert_eq!(moved.board_rank, Some(1536.0));
            assert_eq!(
                column(&database, "in_progress", &[]),
                ids(&[&a, &moving, &b])
            );

            // From Done: the completion is undone as a status change would.
            let done = database
                .set_task_status(&b.id, "completed", "user")
                .unwrap();
            assert!(done.completed_at.is_some());
            let placed = database
                .place_task_on_board(&b.id, "open", &ids(&[&b]))
                .unwrap();
            assert_eq!(placed[0].status, "open");
            assert_eq!(placed[0].completed_at, None);
            assert_eq!(placed[0].completion_source, None);
            assert_eq!(placed[0].board_rank, Some(1024.0));
        }

        #[test]
        fn stale_and_unknown_ids_in_the_order_are_ignored() {
            let database = Database::open_memory();
            let a = standalone(&database, "A");
            let b = standalone(&database, "B");
            let elsewhere = standalone(&database, "Elsewhere");
            database
                .set_task_status(&elsewhere.id, "in_progress", "user")
                .unwrap();
            let order = vec![
                "missing".to_owned(),
                a.id.clone(),
                elsewhere.id.clone(),
                b.id.clone(),
            ];
            let placed = database.place_task_on_board(&b.id, "open", &order).unwrap();
            assert_eq!(ids(&placed.iter().collect::<Vec<_>>()), ids(&[&a, &b]));
            assert_eq!(column(&database, "open", &[]), ids(&[&a, &b]));
            let untouched = get(&database, &elsewhere.id);
            assert_eq!(untouched.status, "in_progress");
            assert_eq!(untouched.board_rank, None);
        }

        #[test]
        fn a_placement_is_refused_for_done_or_an_order_that_misstates_the_task() {
            let database = Database::open_memory();
            let a = standalone(&database, "A");
            let b = standalone(&database, "B");
            for status in ["completed", "cancelled", "blocked"] {
                assert!(matches!(
                    database.place_task_on_board(&a.id, status, &ids(&[&a])),
                    Err(DatabaseError::Validation(_))
                ));
            }
            assert!(matches!(
                database.place_task_on_board(&a.id, "open", &ids(&[&b])),
                Err(DatabaseError::Validation(_))
            ));
            assert!(matches!(
                database.place_task_on_board(&a.id, "open", &ids(&[&a, &b, &a])),
                Err(DatabaseError::Validation(_))
            ));
            assert!(matches!(
                database.place_task_on_board("missing", "open", &["missing".to_owned()]),
                Err(DatabaseError::NotFound(_))
            ));
            assert_eq!(get(&database, &a.id).board_rank, None);
            assert_eq!(get(&database, &b.id).board_rank, None);
        }

        #[test]
        fn changing_column_clears_the_rank_and_other_writes_keep_it() {
            let database = database_with_thread();
            let request = serde_json::from_value(serde_json::json!({
                "accountId": "account@example.com", "threadId": "account:thread",
                "subjectSnapshot": "Planning", "title": "Waiting on the contract",
                "kind": "waiting_for",
            }))
            .unwrap();
            let task = database.create_task(&request).unwrap();
            let place = |status: &str| {
                database
                    .place_task_on_board(&task.id, status, std::slice::from_ref(&task.id))
                    .unwrap()
                    .remove(0)
            };
            assert_eq!(place("open").board_rank, Some(1024.0));

            // Same status, an edit, and a recorded follow-up keep the rank.
            let same = database.set_task_status(&task.id, "open", "user").unwrap();
            assert_eq!(same.board_rank, Some(1024.0));
            let edit: UpdateTaskRequest =
                serde_json::from_value(serde_json::json!({ "id": task.id, "title": "Renamed" }))
                    .unwrap();
            assert_eq!(
                database.update_task(&edit).unwrap().board_rank,
                Some(1024.0)
            );

            // Every status change clears it.
            for status in [
                "in_progress",
                "completed",
                "open",
                "cancelled",
                "in_progress",
            ] {
                place(if status == "in_progress" {
                    "open"
                } else {
                    "in_progress"
                });
                let changed = database.set_task_status(&task.id, status, "user").unwrap();
                assert_eq!(changed.board_rank, None, "moving to {status}");
            }
            let current = place("in_progress");
            assert!(current.board_rank.is_some());
            let checked = database
                .set_task_status_if_current(
                    &task.id,
                    "completed",
                    "user",
                    Some(&current.updated_at),
                )
                .unwrap();
            assert_eq!(checked.board_rank, None);

            // A reply that completes a waiting-for task clears it too.
            database.set_task_status(&task.id, "open", "user").unwrap();
            assert!(place("open").board_rank.is_some());
            database
                .connection()
                .unwrap()
                .execute(
                    "UPDATE threads SET last_received_at='2026-09-20T10:00:00Z' WHERE id='account:thread'",
                    [],
                )
                .unwrap();
            assert_eq!(database.reconcile_waiting_tasks().unwrap(), 1);
            let replied = get(&database, &task.id);
            assert_eq!(replied.status, "completed");
            assert_eq!(replied.board_rank, None);
        }

        #[test]
        fn a_repeating_follow_up_keeps_its_rank_when_recorded() {
            let database = database_with_thread();
            let request = serde_json::from_value(serde_json::json!({
                "accountId": "account@example.com", "threadId": "account:thread",
                "subjectSnapshot": "Planning", "title": "Check in", "kind": "follow_up",
                "dueKind": "date", "dueValue": "2026-09-25", "repeatIntervalDays": 7,
            }))
            .unwrap();
            let task = database.create_task(&request).unwrap();
            database
                .place_task_on_board(&task.id, "open", std::slice::from_ref(&task.id))
                .unwrap();
            let next = database.record_follow_up(&task.id).unwrap();
            assert_eq!(next.board_rank, Some(1024.0));
        }

        #[test]
        fn placements_and_column_changes_are_recorded_for_sync() {
            use threestrands_sync_protocol::EntityType;
            let database = Database::open_memory();
            database.set_beta_features_enabled(true).unwrap();
            let a = standalone(&database, "A");
            let b = standalone(&database, "B");
            database
                .place_task_on_board(&b.id, "open", &ids(&[&b, &a]))
                .unwrap();
            let winner = |id: &str, field: &str| {
                database
                    .resolve_field_winner(EntityType::Task, id, field)
                    .unwrap()
            };
            assert_eq!(winner(&b.id, "boardRank"), Some(serde_json::json!(1024.0)));
            assert_eq!(winner(&a.id, "boardRank"), Some(serde_json::json!(2048.0)));

            // Alone in its new column, the task keeps the rank it had.
            database
                .place_task_on_board(&a.id, "in_progress", &ids(&[&a]))
                .unwrap();
            assert_eq!(
                winner(&a.id, "status"),
                Some(serde_json::json!("in_progress"))
            );
            assert_eq!(winner(&a.id, "boardRank"), Some(serde_json::json!(2048.0)));

            database.set_task_status(&a.id, "open", "user").unwrap();
            assert_eq!(winner(&a.id, "boardRank"), Some(serde_json::Value::Null));
        }
    }
}
