//! What earlier plans say: plan history (carry counts and planned days), read
//! from plan sections in daily notes on this device; nothing is stored. Plans are
//! found through the existing note link index (`note_entity_links`, by
//! target) and the daily notes' ids (`daily:YYYY-MM-DD`, a primary-key range),
//! so only the bounded set of daily notes that can matter is parsed.
use super::daily_plan::{item_key, DailyPlanItem};
use super::{Database, DatabaseError, DbResult};
use crate::daily_plan::{self, PlanItemKind, PlanMarker, PlanSection};
use crate::note_entities::NoteReference;
use chrono::{Duration, NaiveDate};
use rusqlite::{params_from_iter, Connection};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};

/// Plan days a carry count and planned days look back over, including the
/// last day.
pub const PLAN_HISTORY_DAYS: i64 = 90;
/// Most targets one history request may name.
pub const MAX_PLAN_HISTORY_TARGETS: usize = 200;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanHistory {
    pub target: String,
    /// Plan days in the range on which the target was a plan item carried
    /// from an earlier day.
    pub carry_count: u32,
    /// Dates, oldest first, on which the target was a plan item.
    pub planned_days: Vec<String>,
}

struct DayPlan {
    date: String,
    section: PlanSection,
}

fn parse_day(value: &str) -> DbResult<NaiveDate> {
    if !crate::notes::is_daily_date(value) {
        return Err(DatabaseError::invalid("Invalid date"));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| DatabaseError::invalid("Invalid date"))
}

fn day_text(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

fn day_plan(note_id: String, date: String, body: &str) -> Option<DayPlan> {
    // A note the parser can't read has no plan to count; Notes shows why.
    let section = daily_plan::parse_plan(&note_id, body).ok().flatten()?;
    Some(DayPlan { date, section })
}

/// The plans in daily notes dated `from..=to` that may name one of
/// `targets`: notes linking a target (from the link index), and the daily
/// note a round task typed while planning belongs to. Oldest first.
fn plans_naming(
    c: &Connection,
    targets: &[String],
    from: &str,
    to: &str,
) -> DbResult<Vec<DayPlan>> {
    if targets.is_empty() || from > to {
        return Ok(Vec::new());
    }
    let (low, high) = (
        crate::notes::daily_note_id(from),
        crate::notes::daily_note_id(to),
    );
    let mut ids = BTreeSet::new();
    let marks = vec!["?"; targets.len()].join(",");
    let mut q = c.prepare(&format!(
        "SELECT DISTINCT note_id FROM note_entity_links WHERE target IN ({marks})
         AND note_id BETWEEN ?{} AND ?{}",
        targets.len() + 1,
        targets.len() + 2
    ))?;
    let values = targets.iter().cloned().chain([low.clone(), high.clone()]);
    for id in q.query_map(params_from_iter(values), |r| r.get::<_, String>(0))? {
        ids.insert(id?);
    }
    for target in targets {
        if let Some(owner) = NoteReference::parse(target).and_then(|r| r.note) {
            if owner >= low && owner <= high {
                ids.insert(owner);
            }
        }
    }
    let mut plans = Vec::new();
    let mut note = c.prepare(
        "SELECT daily_date,body FROM notes WHERE id=?1 AND trashed=0 AND daily_date IS NOT NULL",
    )?;
    for id in ids {
        let mut rows = note.query([&id])?;
        if let Some(row) = rows.next()? {
            let (date, body): (String, String) = (row.get(0)?, row.get(1)?);
            plans.extend(day_plan(id, date, &body));
        }
    }
    Ok(plans)
}

/// Carry counts and planned days for `targets` from plans dated `from..=to`.
fn history_in(
    c: &Connection,
    targets: &[String],
    from: &str,
    to: &str,
) -> DbResult<HashMap<String, PlanHistory>> {
    let mut out: HashMap<String, PlanHistory> = targets
        .iter()
        .map(|target| {
            (
                target.clone(),
                PlanHistory {
                    target: target.clone(),
                    carry_count: 0,
                    planned_days: Vec::new(),
                },
            )
        })
        .collect();
    for plan in plans_naming(c, targets, from, to)? {
        // One plan day counts once per target, however often it is named.
        let mut seen: HashMap<String, bool> = HashMap::new();
        for line in plan
            .section
            .items
            .iter()
            .filter(|line| line.kind != PlanItemKind::Text)
        {
            let key = item_key(line);
            if out.contains_key(&key) {
                let carried = matches!(line.marker, Some(PlanMarker::Carried(_)));
                *seen.entry(key).or_default() |= carried;
            }
        }
        for (key, carried) in seen {
            let history = out.get_mut(&key).expect("only wanted targets are seen");
            history.planned_days.push(plan.date.clone());
            if carried {
                history.carry_count += 1;
            }
        }
    }
    Ok(out)
}

/// `history_in` over dates.
pub(super) fn history_for(
    c: &Connection,
    targets: &[String],
    from: NaiveDate,
    to: NaiveDate,
) -> DbResult<HashMap<String, PlanHistory>> {
    history_in(c, targets, &day_text(from), &day_text(to))
}

/// The first day of the history window that ends on `date`.
fn history_start(date: NaiveDate) -> NaiveDate {
    date - Duration::days(PLAN_HISTORY_DAYS - 1)
}

/// Sets each tracked item's carry count over the history window ending on `date`.
pub(super) fn fill_carry_counts<'a>(
    c: &Connection,
    date: &str,
    items: impl Iterator<Item = &'a mut DailyPlanItem>,
) -> DbResult<()> {
    let day = parse_day(date)?;
    let mut items: Vec<&mut DailyPlanItem> = items
        .filter(|item| item.kind != PlanItemKind::Text)
        .collect();
    let targets: Vec<String> = items
        .iter()
        .map(|item| item.key.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let history = history_in(c, &targets, &day_text(history_start(day)), date)?;
    for item in &mut items {
        item.carry_count = history.get(&item.key).map_or(0, |h| h.carry_count);
    }
    Ok(())
}

fn task_target(id: &str) -> DbResult<String> {
    NoteReference {
        version: 1,
        kind: "task".into(),
        id: id.into(),
        account: None,
        calendar: None,
        note: None,
        contact: None,
        series: None,
    }
    .target()
    .map_err(DatabaseError::invalid)
}

pub(super) fn note_task_target(note: &str, marker: &str) -> DbResult<String> {
    NoteReference {
        version: 1,
        kind: "task".into(),
        id: marker.into(),
        account: None,
        calendar: None,
        note: Some(note.into()),
        contact: None,
        series: None,
    }
    .target()
    .map_err(DatabaseError::invalid)
}

pub(super) fn task_key(id: &str) -> DbResult<String> {
    task_target(id)
}

impl Database {
    /// Carry counts and planned days for each of `targets` (plan item keys)
    /// from plans dated `from..=to`, a range of at most `PLAN_HISTORY_DAYS`.
    /// Only tests ask for it directly; Today reads carry counts and source
    /// history through `fill_carry_counts` and `history_for`.
    #[cfg(test)]
    pub fn plan_history(
        &self,
        targets: &[String],
        from: &str,
        to: &str,
    ) -> DbResult<Vec<PlanHistory>> {
        if targets.len() > MAX_PLAN_HISTORY_TARGETS {
            return Err(DatabaseError::invalid(format!(
                "Ask for at most {MAX_PLAN_HISTORY_TARGETS} plan items' history at once"
            )));
        }
        let (start, end) = (parse_day(from)?, parse_day(to)?);
        if end < start || (end - start).num_days() >= PLAN_HISTORY_DAYS {
            return Err(DatabaseError::invalid(format!(
                "Plan history covers 1 to {PLAN_HISTORY_DAYS} days"
            )));
        }
        let mut unique = Vec::new();
        for target in targets {
            if !unique.contains(target) {
                unique.push(target.clone());
            }
        }
        self.with_connection(|c| {
            let mut history = history_in(c, &unique, from, to)?;
            Ok(unique
                .iter()
                .filter_map(|target| history.remove(target))
                .map(|mut entry| {
                    entry.planned_days.sort();
                    entry
                })
                .collect())
        })
    }
}

#[cfg(test)]
#[path = "tests/plan_review.rs"]
mod tests;
