//! A day's plan, read from and written to the `## Plan` section of that day's
//! daily note (see `daily_plan.rs` for the format). A plan links work rather
//! than copying it: each item's state comes from the task, note task, or
//! contact it names. Writes are item-level and use the same version-checked
//! daily-note transaction as Add Note, so they never rewrite other text.
use super::note_entities::{begin_daily_insert, DailyInsertMessages};
use super::{Database, DatabaseError, DbResult};
use crate::daily_plan::{self, PlanAddition, PlanItemKind, PlanLine, PlanMarker};
use crate::models::Note;
use crate::note_entities::{normalize_contact_address, NoteReference};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Most items one save may add.
pub const MAX_PLAN_ADDITIONS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanItemState {
    Open,
    InProgress,
    Done,
    Cancelled,
    /// Free text: nothing to track.
    Text,
    /// The linked item isn't on this device.
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanGoal {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyPlanItem {
    /// The canonical target it names, or `line:<offset>` for free text.
    pub key: String,
    pub kind: PlanItemKind,
    pub reference: Option<NoteReference>,
    /// The linked item's current title, else the label written in the note.
    pub title: String,
    pub state: PlanItemState,
    pub marker: Option<PlanMarker>,
    /// The task's id as Tasks lists it (`note:<note>:<marker>` for a note task).
    pub task_id: Option<String>,
    pub account_id: Option<String>,
    pub goal: Option<PlanGoal>,
    /// A check-in's saved contact, when one owns the address.
    pub contact_id: Option<String>,
    /// Plan days within `PLAN_HISTORY_DAYS` up to this plan's date on which
    /// the same target was carried (see `plan_history`).
    pub carry_count: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyPlan {
    pub date: String,
    pub note_id: String,
    /// The daily note's version, to save against; `None` without a note.
    pub revision: Option<String>,
    pub items: Vec<DailyPlanItem>,
    pub not_today: Vec<DailyPlanItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSave {
    pub note: Note,
    /// Keys of the items this save added or removed.
    pub changed: Vec<String>,
    /// Keys of requested links already in the plan, left as they were.
    pub already: Vec<String>,
    pub previous_body: String,
}

pub(super) fn item_key(line: &PlanLine) -> String {
    line.reference
        .as_ref()
        .and_then(|reference| reference.target().ok())
        .unwrap_or_else(|| format!("line:{}", line.start))
}

pub(super) fn task_state(status: &str) -> PlanItemState {
    match status {
        "in_progress" => PlanItemState::InProgress,
        "completed" => PlanItemState::Done,
        "cancelled" => PlanItemState::Cancelled,
        _ => PlanItemState::Open,
    }
}

/// The saved contact owning a contact reference: its own id while it exists,
/// else the contact that now has the address.
fn contact_for(c: &Connection, reference: &NoteReference) -> DbResult<Option<String>> {
    if let Some(id) = &reference.contact {
        let exists: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM contacts WHERE id=?1)",
            [id],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(Some(id.clone()));
        }
    }
    Ok(c.query_row(
        "SELECT contact_id FROM contact_addresses WHERE email=?1",
        [normalize_contact_address(&reference.id)],
        |r| r.get(0),
    )
    .optional()?)
}

/// The instant a plan's day begins: `day_start`, local midnight as the app
/// computed it, or UTC midnight without one. Offsets run from UTC−12 to
/// UTC+14, so any other start is no time zone's midnight and is refused.
pub(super) fn plan_day_start(date: &str, day_start: Option<&str>) -> DbResult<DateTime<Utc>> {
    let midnight = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| DatabaseError::invalid("Invalid date"))?
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| DatabaseError::invalid("Invalid date"))?
        .and_utc();
    let Some(value) = day_start else {
        return Ok(midnight);
    };
    let start = DateTime::parse_from_rfc3339(value)
        .map_err(|_| DatabaseError::invalid("Invalid day start"))?
        .with_timezone(&Utc);
    let offset = start - midnight;
    if offset < -Duration::hours(14) || offset > Duration::hours(12) {
        return Err(DatabaseError::invalid("Invalid day start"));
    }
    Ok(start)
}

fn at_or_after(at: Option<&str>, start: DateTime<Utc>) -> bool {
    at.and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| at.with_timezone(&Utc) >= start)
}

pub(super) fn resolve(
    c: &Connection,
    day_start: DateTime<Utc>,
    note_id: &str,
    line: &PlanLine,
) -> DbResult<DailyPlanItem> {
    let mut item = DailyPlanItem {
        key: item_key(line),
        kind: line.kind,
        reference: line.reference.clone(),
        title: line.label.clone(),
        state: PlanItemState::Text,
        marker: line.marker.clone(),
        task_id: None,
        account_id: None,
        goal: None,
        contact_id: None,
        carry_count: 0,
        start: line.start,
        end: line.end,
    };
    let Some(reference) = &line.reference else {
        return Ok(item);
    };
    match line.kind {
        PlanItemKind::Task => {
            let row = c
                .query_row(
                    "SELECT t.title,t.status,t.account_id,g.id,g.title FROM tasks t LEFT JOIN goals g ON g.id=t.goal_id WHERE t.id=?1",
                    [&reference.id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)),
                )
                .optional()?;
            match row {
                Some((title, status, account, goal_id, goal_title)) => {
                    item.title = title;
                    item.state = task_state(&status);
                    item.task_id = Some(reference.id.clone());
                    item.account_id = Some(account);
                    item.goal = goal_id
                        .zip(goal_title)
                        .map(|(id, title)| PlanGoal { id, title });
                }
                None => item.state = PlanItemState::Unavailable,
            }
        }
        PlanItemKind::NoteTask => {
            let owner = reference.note.clone().unwrap_or_default();
            let row = c
                .query_row(
                    "SELECT t.title,t.checked FROM note_tasks t JOIN notes n ON n.id=t.note_id WHERE t.note_id=?1 AND t.marker_id=?2 AND n.trashed=0",
                    params![owner, reference.id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
                )
                .optional()?;
            match row {
                Some((title, checked)) => {
                    item.title = title;
                    item.state = if checked {
                        PlanItemState::Done
                    } else {
                        PlanItemState::Open
                    };
                    item.task_id = Some(format!("note:{owner}:{}", reference.id));
                }
                None => item.state = PlanItemState::Unavailable,
            }
        }
        PlanItemKind::NewTask => {
            item.state = if line.checked {
                PlanItemState::Done
            } else {
                PlanItemState::Open
            };
            item.task_id = line
                .marker_id
                .as_ref()
                .map(|marker| format!("note:{note_id}:{marker}"));
        }
        PlanItemKind::Contact => {
            let contact = contact_for(c, reference)?;
            let marked: Option<String> = match &contact {
                Some(id) => c
                    .query_row(
                        "SELECT kit_last_touch_at FROM contacts WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten(),
                None => None,
            };
            // Mail the user sent to any of the contact's addresses, from any
            // account, or to the linked address when no contact owns it.
            let emailed: Option<String> = c.query_row(
                "SELECT MAX(sent_at) FROM contact_interactions WHERE direction='sent'
                   AND email IN (SELECT email FROM contact_addresses WHERE contact_id=?1 UNION SELECT ?2)",
                params![contact, normalize_contact_address(&reference.id)],
                |r| r.get(0),
            )?;
            // Marked contacted, or emailed, on or after the plan's day.
            item.state = if at_or_after(marked.as_deref(), day_start)
                || at_or_after(emailed.as_deref(), day_start)
            {
                PlanItemState::Done
            } else {
                PlanItemState::Open
            };
            item.contact_id = contact;
        }
        PlanItemKind::Text => {}
    }
    Ok(item)
}

/// Checks a link before it goes into a plan: the task, note task, or contact
/// must be on this device, and a note task's marker must be saved in its note
/// so the link keeps naming the same item.
pub(super) fn check_link(c: &Connection, reference: &NoteReference) -> DbResult<()> {
    if !reference.validate() {
        return Err(DatabaseError::invalid("Invalid plan item"));
    }
    let ok = match (reference.kind.as_str(), &reference.note) {
        ("task", Some(owner)) => {
            let body: Option<String> = c
                .query_row(
                    "SELECT body FROM notes WHERE id=?1 AND trashed=0",
                    [owner],
                    |r| r.get(0),
                )
                .optional()?;
            match body {
                Some(body)
                    if body.contains(&format!("<!-- threestrands-task:{} -->", reference.id)) =>
                {
                    true
                }
                Some(_) => {
                    return Err(DatabaseError::invalid(
                        "Open that note and save it before planning its task",
                    ))
                }
                None => false,
            }
        }
        ("task", None) => c.query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1)",
            [&reference.id],
            |r| r.get(0),
        )?,
        ("contact", None) => true,
        _ => {
            return Err(DatabaseError::invalid(
                "A plan can link only tasks and contacts",
            ))
        }
    };
    if ok {
        Ok(())
    } else {
        Err(DatabaseError::invalid("That item isn't on this device"))
    }
}

const MESSAGES: DailyInsertMessages = DailyInsertMessages {
    changed: "That day's note changed. Open it before changing the plan.",
    conflict: "Resolve that day's note conflict before changing the plan.",
    fence: "That day's note has a code block that never closes. Open it before changing the plan.",
};

impl Database {
    /// The plan in `date`'s daily note, with each item's current state, for a
    /// day starting at UTC midnight; see [`Self::daily_plan_from`].
    pub fn daily_plan(&self, date: &str) -> DbResult<DailyPlan> {
        self.daily_plan_from(date, None)
    }

    /// The plan in `date`'s daily note, with each item's current state.
    /// `day_start` is the instant the user's local day begins, so a check-in
    /// counts mail and Mark Contacted from local midnight.
    pub fn daily_plan_from(&self, date: &str, day_start: Option<&str>) -> DbResult<DailyPlan> {
        if !crate::notes::is_daily_date(date) {
            return Err(DatabaseError::invalid("Invalid date"));
        }
        let day_start = plan_day_start(date, day_start)?;
        let note_id = crate::notes::daily_note_id(date);
        self.with_connection(|c| {
            let note: Option<(String, String)> = c
                .query_row(
                    "SELECT body,updated_at FROM notes WHERE id=?1 AND trashed=0",
                    [&note_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let mut plan = DailyPlan {
                date: date.into(),
                note_id: note_id.clone(),
                revision: None,
                items: Vec::new(),
                not_today: Vec::new(),
            };
            let Some((body, revision)) = note else {
                return Ok(plan);
            };
            plan.revision = Some(revision);
            if let Some(section) =
                daily_plan::parse_plan(&note_id, &body).map_err(DatabaseError::invalid)?
            {
                for line in &section.items {
                    plan.items.push(resolve(c, day_start, &note_id, line)?);
                }
                for line in &section.not_today {
                    plan.not_today.push(resolve(c, day_start, &note_id, line)?);
                }
            }
            super::plan_review::fill_carry_counts(
                c,
                date,
                plan.items.iter_mut().chain(plan.not_today.iter_mut()),
            )?;
            Ok(plan)
        })
    }

    /// Adds items to `date`'s plan after the existing ones, writing the
    /// section (and the note) when needed. A link already in the plan is left
    /// as it is and reported in `already`, so repeating an add changes nothing.
    pub fn add_to_daily_plan(
        &self,
        date: &str,
        expected: Option<&str>,
        additions: &[PlanAddition],
    ) -> DbResult<PlanSave> {
        if additions.is_empty() || additions.len() > MAX_PLAN_ADDITIONS {
            return Err(DatabaseError::invalid(format!(
                "Add between 1 and {MAX_PLAN_ADDITIONS} plan items at once"
            )));
        }
        let note_id = crate::notes::daily_note_id(date);
        self.with_transaction(|tx| {
            let start = begin_daily_insert(tx, date, expected, &MESSAGES)?;
            let section = daily_plan::parse_plan(&note_id, &start.previous_body)
                .map_err(DatabaseError::invalid)?;
            let mut present: Vec<String> = section
                .iter()
                .flat_map(|s| s.items.iter().map(item_key))
                .collect();
            let (mut lines, mut changed, mut already) = (Vec::new(), Vec::new(), Vec::new());
            for addition in additions {
                if let PlanAddition::Link { reference, .. } = addition {
                    check_link(tx, reference)?;
                    let key = reference.target().map_err(DatabaseError::invalid)?;
                    if present.contains(&key) {
                        already.push(key);
                        continue;
                    }
                    present.push(key.clone());
                    changed.push(key);
                }
                lines.push(daily_plan::plan_line(addition, None).map_err(DatabaseError::invalid)?);
            }
            if lines.is_empty() {
                let note = start.note.ok_or(DatabaseError::NotFound("Note"))?;
                return Ok(PlanSave {
                    note,
                    changed,
                    already,
                    previous_body: start.previous_body,
                });
            }
            let body = daily_plan::insert_plan_lines(&note_id, &start.previous_body, &lines, false)
                .map_err(|_| DatabaseError::invalid(MESSAGES.fence))?;
            let saved = super::notes::save_daily_in(tx, date, &body)?
                .ok_or(DatabaseError::NotFound("Note"))?;
            // New tasks are keyed by the markers saving gave them.
            if let Some(section) =
                daily_plan::parse_plan(&note_id, &saved.body).map_err(DatabaseError::invalid)?
            {
                for line in section
                    .items
                    .iter()
                    .filter(|line| line.kind == PlanItemKind::NewTask)
                {
                    let key = item_key(line);
                    if !present.contains(&key) {
                        present.push(key.clone());
                        changed.push(key);
                    }
                }
            }
            Ok(PlanSave {
                note: saved,
                changed,
                already,
                previous_body: start.previous_body,
            })
        })
    }

    /// Removes the item with `key` from `date`'s plan, with its nested lines.
    /// This takes it off the day's plan; it doesn't mark it dropped.
    pub fn remove_from_daily_plan(
        &self,
        date: &str,
        expected: Option<&str>,
        key: &str,
    ) -> DbResult<PlanSave> {
        let note_id = crate::notes::daily_note_id(date);
        self.with_transaction(|tx| {
            let start = begin_daily_insert(tx, date, expected, &MESSAGES)?;
            let section = daily_plan::parse_plan(&note_id, &start.previous_body)
                .map_err(DatabaseError::invalid)?
                .ok_or_else(|| DatabaseError::invalid("That day has no plan"))?;
            let line = section
                .items
                .iter()
                .find(|line| item_key(line) == key)
                .ok_or_else(|| DatabaseError::invalid("That item is no longer in the plan"))?;
            let body = daily_plan::remove_plan_line(&start.previous_body, line.start, line.end);
            let saved = super::notes::save_daily_in(tx, date, &body)?
                .ok_or(DatabaseError::NotFound("Note"))?;
            Ok(PlanSave {
                note: saved,
                changed: vec![key.to_string()],
                already: Vec::new(),
                previous_body: start.previous_body,
            })
        })
    }
}

#[cfg(test)]
#[path = "tests/daily_plan.rs"]
mod tests;
