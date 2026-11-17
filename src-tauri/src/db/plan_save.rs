//! Plan Today's save: every change the routine made to a day's plan, written
//! to that day's note in one version-checked transaction. Writes stay item
//! level (`daily_plan::arrange_plan_items` and `insert_plan_lines`), so text
//! the user wrote in the section is never rewritten. Each save carries an
//! operation id with a local receipt in the same transaction: a retry after a
//! lost response returns the original result, and Undo restores the body
//! that save replaced only while the note is still as the save left it.
use super::daily_plan::{item_key, MAX_PLAN_ADDITIONS};
use super::note_entities::{begin_daily_insert, DailyInsertMessages};
use super::{Database, DatabaseError, DbResult};
use crate::daily_plan::{self, Arranged, PlanAddition, PlanItemKind, PlanMarker};
use crate::models::Note;
use crate::note_entities::NoteReference;
use chrono::{Duration, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

/// Days a plan save's receipt is kept for retries and Undo.
pub const PLAN_RECEIPT_DAYS: i64 = 7;
/// Most receipts kept on this device; the oldest go first.
pub const MAX_PLAN_RECEIPTS: i64 = 200;
/// Longest operation id accepted.
const MAX_OPERATION_ID: usize = 100;

/// One place in the saved plan, in order.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum PlanEntry {
    /// An item already in the plan, by its key (`DailyPlanItem::key`).
    Existing { key: String },
    /// A new item; `carried` names the plan date it was carried from.
    Add {
        addition: PlanAddition,
        #[serde(default)]
        carried: Option<String>,
    },
}

/// A decision about unfinished work from an earlier plan, recorded under
/// `### Not today`: deferred to a later date, or dropped.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotTodayEntry {
    pub reference: NoteReference,
    pub label: String,
    pub decision: PlanMarker,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanChanges {
    /// The plan's items in their new order. Existing items not named here
    /// and not in `remove` stay where they are.
    #[serde(default)]
    pub items: Vec<PlanEntry>,
    /// Keys of existing items to take off the plan (not dropped).
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default)]
    pub not_today: Vec<NotTodayEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSaveResult {
    pub operation_id: String,
    pub note: Note,
    /// Keys of the items this save added to the plan.
    pub added: Vec<String>,
    /// Keys of the items it took off.
    pub removed: Vec<String>,
    /// Keys recorded under Not today.
    pub not_today: Vec<String>,
    /// Keys of links asked for that were already in the plan, left as they were.
    pub already: Vec<String>,
    /// Whether the body changed at all.
    pub changed: bool,
}

const MESSAGES: DailyInsertMessages = DailyInsertMessages {
    changed: "That day's note changed. Open it before saving the plan.",
    conflict: "Resolve that day's note conflict before saving the plan.",
    fence: "That day's note has a code block that never closes. Open it before saving the plan.",
};

fn checked_date(value: &str) -> DbResult<&str> {
    if crate::notes::is_daily_date(value) {
        Ok(value)
    } else {
        Err(DatabaseError::invalid("Invalid date"))
    }
}

fn link_key(addition: &PlanAddition) -> DbResult<Option<String>> {
    match addition {
        PlanAddition::Link { reference, .. } => {
            reference.target().map(Some).map_err(DatabaseError::invalid)
        }
        PlanAddition::NewTask { .. } => Ok(None),
    }
}

fn prune(tx: &Transaction<'_>) -> DbResult<()> {
    let cutoff = (Utc::now() - Duration::days(PLAN_RECEIPT_DAYS)).to_rfc3339();
    tx.execute(
        "DELETE FROM plan_save_receipts WHERE created_at < ?1",
        [cutoff],
    )?;
    tx.execute(
        "DELETE FROM plan_save_receipts WHERE operation_id NOT IN (SELECT operation_id FROM plan_save_receipts ORDER BY created_at DESC LIMIT ?1)",
        [MAX_PLAN_RECEIPTS],
    )?;
    Ok(())
}

fn previous_receipt(
    tx: &Transaction<'_>,
    operation_id: &str,
) -> DbResult<Option<(String, String, String, Option<String>)>> {
    Ok(tx
        .query_row(
            "SELECT date,saved_revision,result_json,undone_at FROM plan_save_receipts WHERE operation_id=?1",
            [operation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?)
}

impl Database {
    /// Saves Plan Today's changes to `date`'s plan in one guarded
    /// transaction: items added (carried ones marked with the date they came
    /// from), reordered, or taken off, and Not today decisions. `expected` is
    /// the note's version the routine read (`None` without a note). Repeating
    /// an `operation_id` returns the first result without writing again.
    pub fn save_daily_plan(
        &self,
        date: &str,
        expected: Option<&str>,
        operation_id: &str,
        changes: &PlanChanges,
    ) -> DbResult<PlanSaveResult> {
        checked_date(date)?;
        if operation_id.trim().is_empty() || operation_id.len() > MAX_OPERATION_ID {
            return Err(DatabaseError::invalid("Invalid operation id"));
        }
        let adds = changes
            .items
            .iter()
            .filter(|entry| matches!(entry, PlanEntry::Add { .. }))
            .count();
        if adds + changes.not_today.len() > MAX_PLAN_ADDITIONS {
            return Err(DatabaseError::invalid(format!(
                "Save at most {MAX_PLAN_ADDITIONS} new plan lines at once"
            )));
        }
        let note_id = crate::notes::daily_note_id(date);
        self.with_transaction(|tx| {
            if let Some((saved_date, _, result, _)) = previous_receipt(tx, operation_id)? {
                if saved_date != date {
                    return Err(DatabaseError::invalid(
                        "That operation saved another day's plan",
                    ));
                }
                return serde_json::from_str(&result)
                    .map_err(|error| DatabaseError::invalid(error.to_string()));
            }
            let start = begin_daily_insert(tx, date, expected, &MESSAGES)?;
            if start.parsed.unclosed_fence {
                return Err(DatabaseError::invalid(MESSAGES.fence));
            }
            let body = &start.previous_body;
            let section =
                daily_plan::parse_plan(&note_id, body).map_err(DatabaseError::invalid)?;
            let existing: Vec<String> = section
                .iter()
                .flat_map(|s| s.items.iter().map(item_key))
                .collect();
            let index_of = |key: &str| -> DbResult<usize> {
                existing
                    .iter()
                    .position(|candidate| candidate == key)
                    .ok_or_else(|| DatabaseError::invalid("That item is no longer in the plan"))
            };
            let mut remove = Vec::new();
            for key in &changes.remove {
                remove.push(index_of(key)?);
            }
            let removed: Vec<String> = changes.remove.clone();
            // Links present after the save, so an add never repeats one.
            let mut present: Vec<String> = existing
                .iter()
                .filter(|key| !removed.contains(key))
                .cloned()
                .collect();
            let (mut order, mut added, mut already) = (Vec::new(), Vec::new(), Vec::new());
            for entry in &changes.items {
                match entry {
                    PlanEntry::Existing { key } => order.push(Arranged::Existing(index_of(key)?)),
                    PlanEntry::Add { addition, carried } => {
                        let marker = match carried {
                            Some(from) if checked_date(from)? < date => {
                                Some(PlanMarker::Carried(from.clone()))
                            }
                            Some(_) => {
                                return Err(DatabaseError::invalid(
                                    "An item can be carried only from an earlier plan",
                                ))
                            }
                            None => None,
                        };
                        if let PlanAddition::Link { reference, .. } = addition {
                            super::daily_plan::check_link(tx, reference)?;
                        }
                        if let Some(key) = link_key(addition)? {
                            if present.contains(&key) {
                                already.push(key);
                                continue;
                            }
                            present.push(key.clone());
                            added.push(key);
                        }
                        order.push(Arranged::New(
                            daily_plan::plan_line(addition, marker.as_ref())
                                .map_err(DatabaseError::invalid)?,
                        ));
                    }
                }
            }
            let mut next = daily_plan::arrange_plan_items(
                &note_id,
                body,
                section.as_ref(),
                &order,
                &remove,
            )
            .map_err(DatabaseError::invalid)?;

            // Not today: one line per decision, unless the same decision is already there.
            let recorded: Vec<(String, Option<PlanMarker>)> = section
                .iter()
                .flat_map(|s| s.not_today.iter())
                .map(|line| (item_key(line), line.marker.clone()))
                .collect();
            let (mut not_today_lines, mut not_today) = (Vec::new(), Vec::new());
            for entry in &changes.not_today {
                if !entry.reference.validate()
                    || !matches!(entry.reference.kind.as_str(), "task" | "contact")
                {
                    return Err(DatabaseError::invalid("Invalid plan item"));
                }
                match &entry.decision {
                    PlanMarker::Deferred(until) if checked_date(until)? > date => {}
                    PlanMarker::Dropped => {}
                    PlanMarker::Deferred(_) => {
                        return Err(DatabaseError::invalid(
                            "Defer to a date after the plan's day",
                        ))
                    }
                    PlanMarker::Carried(_) => {
                        return Err(DatabaseError::invalid(
                            "Carried items belong in the plan, not under Not today",
                        ))
                    }
                }
                let key = entry
                    .reference
                    .target()
                    .map_err(DatabaseError::invalid)?;
                if recorded.contains(&(key.clone(), Some(entry.decision.clone())))
                    || not_today.contains(&key)
                {
                    continue;
                }
                let addition = PlanAddition::Link {
                    reference: entry.reference.clone(),
                    label: entry.label.clone(),
                };
                not_today_lines.push(
                    daily_plan::plan_line(&addition, Some(&entry.decision))
                        .map_err(DatabaseError::invalid)?,
                );
                not_today.push(key);
            }
            if !not_today_lines.is_empty() {
                next = daily_plan::insert_plan_lines(&note_id, &next, &not_today_lines, true)
                    .map_err(|_| DatabaseError::invalid(MESSAGES.fence))?;
            }

            let changed = next != *body;
            let note = if changed {
                super::notes::save_daily_in(tx, date, &next)?
                    .ok_or(DatabaseError::NotFound("Note"))?
            } else {
                start
                    .note
                    .ok_or_else(|| DatabaseError::invalid("Nothing to save"))?
            };
            // New tasks are keyed by the markers saving gave them.
            if let Some(saved) =
                daily_plan::parse_plan(&note_id, &note.body).map_err(DatabaseError::invalid)?
            {
                for line in saved
                    .items
                    .iter()
                    .filter(|line| line.kind == PlanItemKind::NewTask)
                {
                    let key = item_key(line);
                    if !existing.contains(&key) && !added.contains(&key) {
                        added.push(key);
                    }
                }
            }
            let result = PlanSaveResult {
                operation_id: operation_id.to_string(),
                note,
                added,
                removed,
                not_today,
                already,
                changed,
            };
            prune(tx)?;
            tx.execute(
                "INSERT INTO plan_save_receipts(operation_id,date,saved_revision,result_json,created_at) VALUES (?1,?2,?3,?4,?5)",
                params![
                    operation_id,
                    date,
                    result.note.updated_at,
                    serde_json::to_string(&result)
                        .map_err(|error| DatabaseError::invalid(error.to_string()))?,
                    Utc::now().to_rfc3339(),
                ],
            )?;
            // The body the save replaced, kept only with its receipt for Undo.
            tx.execute(
                "UPDATE plan_save_receipts SET result_json=json_set(result_json,'$.previousBody',?2) WHERE operation_id=?1",
                params![operation_id, body],
            )?;
            Ok(result)
        })
    }

    /// Undoes the plan save `operation_id`: restores the body it replaced,
    /// while the note is still exactly as that save left it. Otherwise it
    /// refuses, so later writing is never overwritten. Undoing twice does
    /// nothing more.
    pub fn undo_daily_plan_save(&self, date: &str, operation_id: &str) -> DbResult<Note> {
        checked_date(date)?;
        self.with_transaction(|tx| {
            let Some((saved_date, saved_revision, result, undone)) =
                previous_receipt(tx, operation_id)?
            else {
                return Err(DatabaseError::invalid(
                    "That plan save can no longer be undone",
                ));
            };
            if saved_date != date {
                return Err(DatabaseError::invalid(
                    "That operation saved another day's plan",
                ));
            }
            if undone.is_some() {
                return super::notes::get(tx, &crate::notes::daily_note_id(date))?
                    .ok_or(DatabaseError::NotFound("Note"));
            }
            let start = begin_daily_insert(
                tx,
                date,
                Some(&saved_revision),
                &DailyInsertMessages {
                    changed:
                        "That day's note changed after the plan was saved. Open Note to change it.",
                    ..MESSAGES
                },
            )?;
            let value: serde_json::Value = serde_json::from_str(&result)
                .map_err(|error| DatabaseError::invalid(error.to_string()))?;
            let previous = value
                .get("previousBody")
                .and_then(|body| body.as_str())
                .unwrap_or_default()
                .to_string();
            let note = if previous == start.previous_body {
                start.note.ok_or(DatabaseError::NotFound("Note"))?
            } else {
                super::notes::save_daily_in(tx, date, &previous)?
                    .ok_or(DatabaseError::NotFound("Note"))?
            };
            tx.execute(
                "UPDATE plan_save_receipts SET undone_at=?2 WHERE operation_id=?1",
                params![operation_id, Utc::now().to_rfc3339()],
            )?;
            Ok(note)
        })
    }
}

#[cfg(test)]
#[path = "tests/plan_save.rs"]
mod tests;
