use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use threestrands_sync_protocol::goal_period_matches;
use uuid::Uuid;

use super::{Database, DbResult};
use crate::models::{CreateGoalRequest, Goal, ThreadTask, UpdateGoalRequest};

const MAX_TITLE: usize = 240;
const MAX_NOTES: usize = 8_000;

/// What deleting a goal unlinked, so the caller can replicate each change.
#[derive(Debug, Default)]
pub struct GoalDeletion {
    pub tasks: Vec<ThreadTask>,
    pub children: Vec<Goal>,
}

fn validate_goal_fields(title: &str, notes: Option<&str>, horizon: &str, period: &str, status: &str) -> DbResult<()> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return Err(format!("Goal title must be between 1 and {MAX_TITLE} characters").into());
    }
    if notes.is_some_and(|value| value.chars().count() > MAX_NOTES) {
        return Err(format!("Goal notes exceed {MAX_NOTES} characters").into());
    }
    if !goal_period_matches(horizon, period) {
        return Err("Choose a period that matches the goal's horizon, such as 2026, 2026-H2, or 2026-Q4".into());
    }
    if !matches!(status, "active" | "achieved" | "dropped") {
        return Err("Unknown goal status".into());
    }
    Ok(())
}

/// Longer horizons rank lower: a goal can support only a lower-ranked one.
fn horizon_rank(horizon: &str) -> u8 {
    match horizon {
        "year" => 0,
        "half" => 1,
        _ => 2,
    }
}

/// Whether the period `outer` (of a longer horizon) encloses `inner`.
fn period_encloses(outer: &str, inner: &str) -> bool {
    let (outer_year, inner_year) = (&outer[..4.min(outer.len())], &inner[..4.min(inner.len())]);
    if outer_year != inner_year {
        return false;
    }
    match (outer.get(5..), inner.get(5..)) {
        (None, Some(_)) => true,
        (Some("H1"), Some("Q1" | "Q2")) | (Some("H2"), Some("Q3" | "Q4")) => true,
        _ => false,
    }
}

fn goal_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Goal> {
    Ok(Goal {
        id: row.get(0)?,
        account_id: row.get(1)?,
        title: row.get(2)?,
        notes: row.get(3)?,
        horizon: row.get(4)?,
        period: row.get(5)?,
        status: row.get(6)?,
        parent_goal_id: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        closed_at: row.get(10)?,
    })
}

const SELECT_GOALS: &str = "SELECT id, account_id, title, notes, horizon, period, status,
        parent_goal_id, created_at, updated_at, closed_at
     FROM goals";

fn goal_by_id(connection: &Connection, id: &str) -> DbResult<Option<Goal>> {
    Ok(connection
        .query_row(&format!("{SELECT_GOALS} WHERE id = ?1"), [id], goal_from_row)
        .optional()?)
}

/// A task may support only a goal that exists in the task's own account.
pub(super) fn ensure_goal_link(connection: &Connection, account_id: &str, goal_id: Option<&str>) -> DbResult<()> {
    let Some(goal_id) = goal_id else { return Ok(()) };
    let goal = goal_by_id(connection, goal_id)?.ok_or_else(|| "Goal not found".to_string())?;
    if goal.account_id != account_id {
        return Err("A task can only support a goal in its own account".into());
    }
    Ok(())
}

fn ensure_parent(connection: &Connection, goal_id: Option<&str>, account_id: &str, horizon: &str, period: &str, parent_id: Option<&str>) -> DbResult<()> {
    let Some(parent_id) = parent_id else { return Ok(()) };
    if Some(parent_id) == goal_id {
        return Err("A goal cannot support itself".into());
    }
    let parent = goal_by_id(connection, parent_id)?.ok_or_else(|| "Parent goal not found".to_string())?;
    if parent.account_id != account_id {
        return Err("A goal can only support a goal in its own account".into());
    }
    if horizon_rank(&parent.horizon) >= horizon_rank(horizon) || !period_encloses(&parent.period, period) {
        return Err("A goal can only support a longer-term goal whose period includes it".into());
    }
    Ok(())
}

impl Database {
    pub fn list_goals(&self, account_id: Option<&str>) -> DbResult<Vec<Goal>> {
        self.with_connection(|connection| {
            let order = " ORDER BY CASE horizon WHEN 'year' THEN 0 WHEN 'half' THEN 1 ELSE 2 END, period DESC, created_at";
            let goals = match account_id {
                Some(account) => connection
                    .prepare(&format!("{SELECT_GOALS} WHERE account_id = ?1{order}"))?
                    .query_map([account], goal_from_row)?
                    .collect::<Result<Vec<_>, _>>()?,
                None => connection
                    .prepare(&format!("{SELECT_GOALS}{order}"))?
                    .query_map([], goal_from_row)?
                    .collect::<Result<Vec<_>, _>>()?,
            };
            Ok(goals)
        })
    }

    pub fn create_goal(&self, request: &CreateGoalRequest) -> DbResult<Goal> {
        validate_goal_fields(&request.title, request.notes.as_deref(), &request.horizon, &request.period, "active")?;
        self.with_connection(|connection| {
            ensure_parent(connection, None, &request.account_id, &request.horizon, &request.period, request.parent_goal_id.as_deref())?;
            let id = Uuid::new_v4().to_string();
            let now = Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO goals(id, account_id, title, notes, horizon, period, status, parent_goal_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?8, ?8)",
                params![
                    id,
                    request.account_id,
                    request.title.trim(),
                    request.notes.as_deref().map(str::trim).filter(|notes| !notes.is_empty()),
                    request.horizon,
                    request.period,
                    request.parent_goal_id,
                    now,
                ],
            )?;
            Ok(goal_by_id(connection, &id)?.expect("inserted goal"))
        })
    }

    pub fn update_goal(&self, request: &UpdateGoalRequest) -> DbResult<Goal> {
        self.with_connection(|connection| {
            let current = goal_by_id(connection, &request.id)?.ok_or_else(|| "Goal not found".to_string())?;
            let title = request.title.as_deref().unwrap_or(&current.title);
            let notes = request.notes.as_ref().map_or(current.notes.as_deref(), |value| value.as_deref());
            let notes = notes.map(str::trim).filter(|notes| !notes.is_empty());
            let horizon = request.horizon.as_deref().unwrap_or(&current.horizon);
            let period = request.period.as_deref().unwrap_or(&current.period);
            let status = request.status.as_deref().unwrap_or(&current.status);
            let parent = request.parent_goal_id.as_ref().map_or(current.parent_goal_id.as_deref(), |value| value.as_deref());
            validate_goal_fields(title, notes, horizon, period, status)?;
            let moved = horizon != current.horizon || period != current.period;
            if moved || request.parent_goal_id.is_some() {
                ensure_parent(connection, Some(&current.id), &current.account_id, horizon, period, parent)?;
            }
            if moved {
                let children: Vec<Goal> = connection
                    .prepare(&format!("{SELECT_GOALS} WHERE parent_goal_id = ?1"))?
                    .query_map([&current.id], goal_from_row)?
                    .collect::<Result<_, _>>()?;
                if children.iter().any(|child| horizon_rank(horizon) >= horizon_rank(&child.horizon) || !period_encloses(period, &child.period)) {
                    return Err("Some goals that support this one fall outside its new period. Unlink them first.".into());
                }
            }
            let closed_at = match (status, current.status.as_str()) {
                ("active", _) => None,
                (next, previous) if next == previous => current.closed_at.clone(),
                _ => Some(Utc::now().to_rfc3339()),
            };
            connection.execute(
                "UPDATE goals SET title=?1, notes=?2, horizon=?3, period=?4, status=?5, parent_goal_id=?6,
                    closed_at=?7, updated_at=?8 WHERE id=?9",
                params![title.trim(), notes, horizon, period, status, parent, closed_at, Utc::now().to_rfc3339(), current.id],
            )?;
            Ok(goal_by_id(connection, &current.id)?.expect("updated goal"))
        })
    }

    /// Deletes a goal and unlinks the tasks and goals that supported it. Also
    /// how a synced deletion lands, so every device unlinks the same rows.
    pub fn delete_goal(&self, id: &str) -> DbResult<GoalDeletion> {
        let now = Utc::now().to_rfc3339();
        let (task_ids, child_ids): (Vec<String>, Vec<String>) = self.with_connection(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let ids = |sql: &str| -> rusqlite::Result<Vec<String>> {
                transaction.prepare(sql)?.query_map([id], |row| row.get(0))?.collect()
            };
            let task_ids = ids("SELECT id FROM tasks WHERE goal_id = ?1")?;
            let child_ids = ids("SELECT id FROM goals WHERE parent_goal_id = ?1")?;
            transaction.execute("UPDATE tasks SET goal_id = NULL, updated_at = ?2 WHERE goal_id = ?1", params![id, now])?;
            transaction.execute("UPDATE goals SET parent_goal_id = NULL, updated_at = ?2 WHERE parent_goal_id = ?1", params![id, now])?;
            transaction.execute("DELETE FROM goals WHERE id = ?1", [id])?;
            transaction.commit()?;
            Ok((task_ids, child_ids))
        })?;
        let children = self.list_goals(None)?.into_iter().filter(|goal| child_ids.contains(&goal.id)).collect();
        let tasks = self.list_tasks(None, None)?.into_iter().filter(|task| task_ids.contains(&task.id)).collect();
        Ok(GoalDeletion { tasks, children })
    }

    /// Writes a goal received through replicated sync, as resolved.
    pub(crate) fn upsert_synced_goal(&self, goal: &Goal) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO goals(id, account_id, title, notes, horizon, period, status, parent_goal_id, created_at, updated_at, closed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id, title=excluded.title, notes=excluded.notes,
                    horizon=excluded.horizon, period=excluded.period, status=excluded.status,
                    parent_goal_id=excluded.parent_goal_id, updated_at=excluded.updated_at, closed_at=excluded.closed_at",
                params![
                    goal.id, goal.account_id, goal.title, goal.notes, goal.horizon, goal.period, goal.status,
                    goal.parent_goal_id, goal.created_at, goal.updated_at, goal.closed_at
                ],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CreateTaskRequest, UpdateTaskRequest};

    const ACCOUNT: &str = "you@example.com";

    fn create(database: &Database, title: &str, horizon: &str, period: &str, parent: Option<&str>) -> DbResult<Goal> {
        database.create_goal(&CreateGoalRequest {
            account_id: ACCOUNT.into(),
            title: title.into(),
            notes: None,
            horizon: horizon.into(),
            period: period.into(),
            parent_goal_id: parent.map(Into::into),
        })
    }

    fn update(id: &str, json: serde_json::Value) -> UpdateGoalRequest {
        let mut value = json;
        value["id"] = serde_json::Value::String(id.into());
        serde_json::from_value(value).unwrap()
    }

    fn task(database: &Database, account: &str, goal: Option<&str>) -> DbResult<ThreadTask> {
        database.create_task(&CreateTaskRequest {
            account_id: account.into(),
            thread_id: None,
            source_message_id: None,
            subject_snapshot: None,
            title: "Draft the plan".into(),
            notes: None,
            kind: "action".into(),
            due_kind: "none".into(),
            due_value: None,
            time_zone: None,
            repeat_interval_days: None,
            evidence_text: None,
            goal_id: goal.map(Into::into),
        })
    }

    #[test]
    fn goals_round_trip_and_list_longer_horizons_first_then_newest_period() {
        let database = Database::open_memory();
        let year = create(&database, "  Grow the practice  ", "year", "2026", None).unwrap();
        let quarter = create(&database, "Land two clients", "quarter", "2026-Q4", Some(&year.id)).unwrap();
        let half = create(&database, "Hire", "half", "2026-H2", Some(&year.id)).unwrap();
        let old = create(&database, "Old", "quarter", "2026-Q1", None).unwrap();
        create(&database, "Other account", "year", "2026", None).unwrap();
        database.with_connection(|connection| {
            connection.execute("UPDATE goals SET account_id='other@example.com' WHERE title='Other account'", [])?;
            Ok(())
        }).unwrap();

        assert_eq!(year.title, "Grow the practice");
        assert_eq!(year.status, "active");
        assert_eq!(quarter.parent_goal_id.as_deref(), Some(year.id.as_str()));
        let ids: Vec<String> = database.list_goals(Some(ACCOUNT)).unwrap().into_iter().map(|goal| goal.id).collect();
        assert_eq!(ids, [year.id, half.id, quarter.id, old.id]);
        assert_eq!(database.list_goals(None).unwrap().len(), 5);
    }

    #[test]
    fn rejects_periods_that_do_not_match_the_horizon_and_invalid_titles() {
        let database = Database::open_memory();
        assert!(create(&database, "Grow", "quarter", "2026", None).is_err());
        assert!(create(&database, "Grow", "half", "2026-Q2", None).is_err());
        assert!(create(&database, "Grow", "month", "2026-10", None).is_err());
        assert!(create(&database, " ", "year", "2026", None).is_err());
        assert!(create(&database, &"x".repeat(MAX_TITLE), "year", "2026", None).is_ok());
        assert!(create(&database, &"x".repeat(MAX_TITLE + 1), "year", "2026", None).is_err());
    }

    #[test]
    fn a_goal_supports_only_a_longer_goal_whose_period_encloses_it() {
        let database = Database::open_memory();
        let year = create(&database, "Year", "year", "2026", None).unwrap();
        let first_half = create(&database, "H1", "half", "2026-H1", Some(&year.id)).unwrap();
        let q4 = create(&database, "Q4", "quarter", "2026-Q4", Some(&year.id)).unwrap();

        assert!(create(&database, "Q2", "quarter", "2026-Q2", Some(&first_half.id)).is_ok());
        assert!(create(&database, "Q3 under H1", "quarter", "2026-Q3", Some(&first_half.id)).is_err());
        assert!(create(&database, "Next year", "quarter", "2027-Q1", Some(&year.id)).is_err());
        assert!(create(&database, "Same horizon", "quarter", "2026-Q4", Some(&q4.id)).is_err());
        assert!(create(&database, "Upside down", "year", "2026", Some(&q4.id)).is_err());
        assert!(create(&database, "Missing", "quarter", "2026-Q4", Some("nope")).is_err());
        assert!(database.update_goal(&update(&year.id, serde_json::json!({ "parentGoalId": year.id }))).is_err());
    }

    #[test]
    fn moving_a_goal_keeps_its_supporting_goals_inside_its_period() {
        let database = Database::open_memory();
        let year = create(&database, "Year", "year", "2026", None).unwrap();
        create(&database, "Q4", "quarter", "2026-Q4", Some(&year.id)).unwrap();

        let error = database.update_goal(&update(&year.id, serde_json::json!({ "period": "2027" }))).unwrap_err();
        assert!(error.to_string().contains("Unlink them first"));
        assert!(database.update_goal(&update(&year.id, serde_json::json!({ "title": "Renamed year" }))).is_ok());
    }

    #[test]
    fn closing_a_goal_stamps_closed_at_and_reopening_clears_it() {
        let database = Database::open_memory();
        let goal = create(&database, "Ship", "quarter", "2026-Q4", None).unwrap();
        let achieved = database.update_goal(&update(&goal.id, serde_json::json!({ "status": "achieved" }))).unwrap();
        assert!(achieved.closed_at.is_some());
        let renamed = database.update_goal(&update(&goal.id, serde_json::json!({ "title": "Shipped" }))).unwrap();
        assert_eq!(renamed.closed_at, achieved.closed_at);
        let reopened = database.update_goal(&update(&goal.id, serde_json::json!({ "status": "active", "notes": null }))).unwrap();
        assert_eq!(reopened.closed_at, None);
        assert!(database.update_goal(&update(&goal.id, serde_json::json!({ "status": "done" }))).is_err());
    }

    #[test]
    fn tasks_link_only_to_goals_in_their_own_account() {
        let database = Database::open_memory();
        let goal = create(&database, "Ship", "quarter", "2026-Q4", None).unwrap();
        let linked = task(&database, ACCOUNT, Some(&goal.id)).unwrap();
        assert_eq!(linked.goal_id.as_deref(), Some(goal.id.as_str()));
        assert!(task(&database, "other@example.com", Some(&goal.id)).is_err());
        assert!(task(&database, ACCOUNT, Some("missing")).is_err());

        let unlinked = task(&database, ACCOUNT, None).unwrap();
        let link: UpdateTaskRequest = serde_json::from_value(serde_json::json!({ "id": unlinked.id, "goalId": goal.id })).unwrap();
        assert_eq!(database.update_task(&link).unwrap().goal_id.as_deref(), Some(goal.id.as_str()));
        // An update that does not mention the goal leaves the link alone.
        let rename: UpdateTaskRequest = serde_json::from_value(serde_json::json!({ "id": unlinked.id, "title": "Renamed" })).unwrap();
        assert_eq!(database.update_task(&rename).unwrap().goal_id.as_deref(), Some(goal.id.as_str()));
        let clear: UpdateTaskRequest = serde_json::from_value(serde_json::json!({ "id": unlinked.id, "goalId": null })).unwrap();
        assert_eq!(database.update_task(&clear).unwrap().goal_id, None);
    }

    #[test]
    fn deleting_a_goal_unlinks_its_tasks_and_supporting_goals() {
        let database = Database::open_memory();
        let year = create(&database, "Year", "year", "2026", None).unwrap();
        let quarter = create(&database, "Q4", "quarter", "2026-Q4", Some(&year.id)).unwrap();
        let linked = task(&database, ACCOUNT, Some(&year.id)).unwrap();
        let other = task(&database, ACCOUNT, Some(&quarter.id)).unwrap();

        let deletion = database.delete_goal(&year.id).unwrap();
        assert_eq!(deletion.tasks.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(), [linked.id.as_str()]);
        assert_eq!(deletion.tasks[0].goal_id, None);
        assert_eq!(deletion.children.iter().map(|goal| goal.id.as_str()).collect::<Vec<_>>(), [quarter.id.as_str()]);
        assert_eq!(deletion.children[0].parent_goal_id, None);
        let remaining = database.list_goals(None).unwrap();
        assert_eq!(remaining.iter().map(|goal| goal.id.as_str()).collect::<Vec<_>>(), [quarter.id.as_str()]);
        let tasks = database.list_tasks(None, None).unwrap();
        assert_eq!(tasks.iter().find(|task| task.id == other.id).unwrap().goal_id.as_deref(), Some(quarter.id.as_str()));
        // Deleting an unknown goal (a repeated synced deletion) changes nothing.
        let repeat = database.delete_goal(&year.id).unwrap();
        assert!(repeat.tasks.is_empty() && repeat.children.is_empty());
    }

    #[test]
    fn period_enclosure_follows_the_calendar() {
        assert!(period_encloses("2026", "2026-H2"));
        assert!(period_encloses("2026", "2026-Q1"));
        assert!(period_encloses("2026-H1", "2026-Q2"));
        assert!(period_encloses("2026-H2", "2026-Q3"));
        assert!(!period_encloses("2026-H2", "2026-Q2"));
        assert!(!period_encloses("2026", "2027-Q1"));
        assert!(!period_encloses("2026-H1", "2026-H1"));
    }
}
