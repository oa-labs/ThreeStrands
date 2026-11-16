//! What earlier plans say: plan history (carry counts and planned days) and
//! the review of the last plan (Done and Didn't get to). Both read plan
//! sections from daily notes on this device and store nothing: plans are
//! found through the existing note link index (`note_entity_links`, by
//! target) and the daily notes' ids (`daily:YYYY-MM-DD`, a primary-key range),
//! so only the bounded set of daily notes that can matter is parsed.
use super::daily_plan::{item_key, resolve, DailyPlanItem, PlanItemState};
use super::{Database, DatabaseError, DbResult};
use crate::daily_plan::{self, PlanItemKind, PlanMarker, PlanSection};
use crate::models::ScheduleEvent;
use crate::note_entities::NoteReference;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{params, params_from_iter, Connection};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Plan days a carry count and planned days look back over, including the
/// last day.
pub const PLAN_HISTORY_DAYS: i64 = 90;
/// Days before the plan date the review looks for earlier plans.
pub const PLAN_LOOKBACK_DAYS: i64 = 7;
/// Most targets one history request may name.
pub const MAX_PLAN_HISTORY_TARGETS: usize = 200;
/// Most unplanned completed tasks the review lists.
pub const MAX_ALSO_DONE: usize = 50;
/// Most cached events the review reads for the reviewed day.
const MAX_REVIEW_EVENTS: usize = 200;
const DEFERRED_MARKER: &str = "<!-- threestrands-plan:deferred ";

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

/// An item the review offers to carry, defer, or drop.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    #[serde(flatten)]
    pub item: DailyPlanItem,
    /// The plan it was planned in, or deferred from.
    pub planned_on: String,
}

/// A task finished on the reviewed day that wasn't in its plan.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedTask {
    /// The plan item key linking it would produce.
    pub key: String,
    /// The task's id as Tasks lists it (`note:<note>:<marker>` for a note task).
    pub task_id: String,
    pub title: String,
    pub account_id: Option<String>,
    /// The note a note task is in, and its title.
    pub note_id: Option<String>,
    pub note_title: Option<String>,
    pub completed_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReview {
    pub date: String,
    /// The date of the most recent earlier plan within `PLAN_LOOKBACK_DAYS`.
    pub reviewed: Option<String>,
    /// That plan's finished items.
    pub done: Vec<DailyPlanItem>,
    /// Tasks completed on that day outside its plan, oldest first.
    pub also_done: Vec<CompletedTask>,
    /// Timed meetings of that day that have ended and weren't declined, from
    /// the calendar cache. `None` when there is no reviewed plan or this
    /// device has no cached events for that day: unknown, not empty.
    pub meetings: Option<Vec<ScheduleEvent>>,
    /// The reviewed plan's unfinished, unsettled items, then older plans'
    /// (newest plan first), then deferrals due on or before `date`.
    pub didnt_get_to: Vec<ReviewItem>,
}

struct DayPlan {
    date: String,
    note_id: String,
    section: PlanSection,
}

impl DayPlan {
    /// Whether the plan names `key` as a plan item or a Not today item.
    fn mentions(&self, key: &str) -> bool {
        self.section
            .items
            .iter()
            .chain(&self.section.not_today)
            .any(|line| line.kind != PlanItemKind::Text && item_key(line) == key)
    }
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
    Some(DayPlan {
        date,
        note_id,
        section,
    })
}

/// The plans in daily notes dated `from..=to`, oldest first, optionally only
/// notes whose text contains `containing`. Daily note ids sort by date, so
/// this is a primary-key range read.
fn plans_between(
    c: &Connection,
    from: &str,
    to: &str,
    containing: Option<&str>,
) -> DbResult<Vec<DayPlan>> {
    let mut q = c.prepare(
        "SELECT id,daily_date,body FROM notes WHERE id BETWEEN ?1 AND ?2 AND daily_date IS NOT NULL
         AND trashed=0 AND (?3 IS NULL OR instr(body,?3)>0) ORDER BY id",
    )?;
    let rows = q
        .query_map(
            params![
                crate::notes::daily_note_id(from),
                crate::notes::daily_note_id(to),
                containing
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, date, body)| day_plan(id, date, &body))
        .collect())
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

/// Local midnight at the start of `day` and of the day after, as instants.
fn day_bounds<Z: TimeZone>(zone: &Z, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let midnight = |day: NaiveDate| {
        zone.from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
            .earliest()
            .map(|at| at.with_timezone(&Utc))
    };
    Some((midnight(day)?, midnight(day.succ_opt()?)?))
}

fn also_done(
    c: &Connection,
    (start, end): (DateTime<Utc>, DateTime<Utc>),
    planned: &HashSet<String>,
) -> DbResult<Vec<CompletedTask>> {
    let (start, end) = (start.to_rfc3339(), end.to_rfc3339());
    let limit = (MAX_ALSO_DONE + planned.len()) as i64;
    let mut done = Vec::new();
    let mut tasks = c.prepare(
        "SELECT id,title,account_id,completed_at FROM tasks WHERE status='completed'
         AND julianday(completed_at)>=julianday(?1) AND julianday(completed_at)<julianday(?2)
         ORDER BY julianday(completed_at),id LIMIT ?3",
    )?;
    for row in tasks.query_map(params![start, end, limit], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (id, title, account, completed_at) = row?;
        let key = task_target(&id)?;
        done.push(CompletedTask {
            key,
            task_id: id,
            title,
            account_id: Some(account),
            note_id: None,
            note_title: None,
            completed_at,
        });
    }
    // A note task's completion is when this device first saw it checked.
    let mut notes = c.prepare(
        "SELECT t.note_id,t.marker_id,t.title,n.title,t.completed_at FROM note_tasks t
         JOIN notes n ON n.id=t.note_id WHERE n.trashed=0 AND t.checked=1
         AND julianday(t.completed_at)>=julianday(?1) AND julianday(t.completed_at)<julianday(?2)
         ORDER BY julianday(t.completed_at),t.note_id,t.marker_id LIMIT ?3",
    )?;
    for row in notes.query_map(params![start, end, limit], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })? {
        let (note, marker, title, note_title, completed_at) = row?;
        let key = note_task_target(&note, &marker)?;
        done.push(CompletedTask {
            key,
            task_id: format!("note:{note}:{marker}"),
            title,
            account_id: None,
            note_id: Some(note),
            note_title: Some(note_title),
            completed_at,
        });
    }
    done.retain(|task| !planned.contains(&task.key));
    done.sort_by(|a, b| {
        let at = |t: &CompletedTask| DateTime::parse_from_rfc3339(&t.completed_at).ok();
        at(a).cmp(&at(b)).then_with(|| a.task_id.cmp(&b.task_id))
    });
    done.truncate(MAX_ALSO_DONE);
    Ok(done)
}

/// Ended, attended, timed meetings of the day from the calendar cache, or
/// `None` when nothing of that day is cached for a connected calendar.
fn attended<Z: TimeZone>(
    c: &Connection,
    (start, end): (DateTime<Utc>, DateTime<Utc>),
    now: &DateTime<Z>,
) -> DbResult<Option<Vec<ScheduleEvent>>> {
    let mut q = c.prepare(
        "SELECT body FROM note_event_cache WHERE julianday(start)<julianday(?2) AND julianday(end)>julianday(?1)
         AND account IN (SELECT email FROM calendar_accounts WHERE status='connected')
         ORDER BY julianday(start),account,calendar,id LIMIT ?3",
    )?;
    let bodies = q
        .query_map(
            params![
                start.to_rfc3339(),
                end.to_rfc3339(),
                MAX_REVIEW_EVENTS as i64
            ],
            |r| r.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if bodies.is_empty() {
        return Ok(None);
    }
    let now = now.with_timezone(&Utc);
    let mut seen = HashSet::new();
    let mut meetings = Vec::new();
    for body in bodies {
        let Ok(event) = serde_json::from_str::<ScheduleEvent>(&body) else {
            continue;
        };
        let ended = DateTime::parse_from_rfc3339(&event.end).is_ok_and(|end| end <= now);
        let declined = event.response_status.as_deref() == Some("declined");
        let location = event.event_type.as_deref() == Some("workingLocation");
        // A copy of one meeting on a second calendar is still one meeting.
        let identity = (
            event.uid.clone().unwrap_or_else(|| event.id.clone()),
            event.start.clone(),
        );
        if !event.all_day && ended && !declined && !location && seen.insert(identity) {
            meetings.push(event);
        }
    }
    Ok(Some(meetings))
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

/// Whether a resolved item is still to do, or can only be dropped.
fn unfinished(item: &DailyPlanItem) -> bool {
    matches!(
        item.state,
        PlanItemState::Open | PlanItemState::InProgress | PlanItemState::Unavailable
    )
}

impl Database {
    /// Carry counts and planned days for each of `targets` (plan item keys)
    /// from plans dated `from..=to`, a range of at most `PLAN_HISTORY_DAYS`.
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

    /// The review of the plans before `date`, in this device's time zone.
    pub fn plan_review(&self, date: &str) -> DbResult<PlanReview> {
        self.plan_review_at(date, chrono::Local::now())
    }

    /// The review of the plans before `date` as of `now`, whose zone sets
    /// which instants fall on which day.
    pub fn plan_review_at<Z: TimeZone>(
        &self,
        date: &str,
        now: DateTime<Z>,
    ) -> DbResult<PlanReview> {
        let day = parse_day(date)?;
        let window_start = day_text(day - Duration::days(PLAN_LOOKBACK_DAYS));
        let yesterday = day_text(day - Duration::days(1));
        self.with_connection(|c| {
            // Every plan from the window's first day through `date`.
            let plans = plans_between(c, &window_start, date, None)?;
            // A later plan, or the plan's own Not today decisions, settle an item.
            let settled = |key: &str, plan: &DayPlan| {
                plan.section
                    .not_today
                    .iter()
                    .any(|line| line.kind != PlanItemKind::Text && item_key(line) == key)
                    || plans
                        .iter()
                        .any(|later| later.date > plan.date && later.mentions(key))
            };
            let earlier: Vec<&DayPlan> = plans
                .iter()
                .rev()
                .filter(|plan| plan.date.as_str() < date && !plan.section.items.is_empty())
                .collect();
            let reviewed = earlier.first().map(|plan| plan.date.clone());
            let mut review = PlanReview {
                date: date.into(),
                reviewed: reviewed.clone(),
                done: Vec::new(),
                also_done: Vec::new(),
                meetings: None,
                didnt_get_to: Vec::new(),
            };
            let mut listed = HashSet::new();
            for plan in &earlier {
                let is_reviewed = Some(&plan.date) == reviewed.as_ref();
                for line in &plan.section.items {
                    if line.kind == PlanItemKind::Text {
                        continue;
                    }
                    let item = resolve(c, &plan.date, &plan.note_id, line)?;
                    if is_reviewed && item.state == PlanItemState::Done {
                        review.done.push(item);
                        continue;
                    }
                    if !unfinished(&item)
                        || settled(&item.key, plan)
                        || !listed.insert(item.key.clone())
                    {
                        continue;
                    }
                    review.didnt_get_to.push(ReviewItem {
                        item,
                        planned_on: plan.date.clone(),
                    });
                }
            }

            // Deferrals due by `date`, from plans as old as the history window.
            let mut due = Vec::new();
            let deferring = plans_between(
                c,
                &day_text(history_start(day)),
                &yesterday,
                Some(DEFERRED_MARKER),
            )?;
            for plan in &deferring {
                for line in &plan.section.not_today {
                    let Some(PlanMarker::Deferred(until)) = &line.marker else {
                        continue;
                    };
                    if line.kind == PlanItemKind::Text || until.as_str() > date {
                        continue;
                    }
                    let key = item_key(line);
                    if listed.contains(&key) {
                        continue;
                    }
                    // Plans after the deferring one, up to `date`.
                    let later_settles = if plan.date >= window_start {
                        plans
                            .iter()
                            .any(|later| later.date > plan.date && later.mentions(&key))
                    } else {
                        let after = day_text(parse_day(&plan.date)? + Duration::days(1));
                        plans_naming(c, std::slice::from_ref(&key), &after, date)?
                            .iter()
                            .any(|later| later.mentions(&key))
                    };
                    if later_settles {
                        continue;
                    }
                    let item = resolve(c, &plan.date, &plan.note_id, line)?;
                    if unfinished(&item) && listed.insert(key) {
                        due.push((until.clone(), plan.date.clone(), item));
                    }
                }
            }
            due.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            review.didnt_get_to.extend(
                due.into_iter()
                    .map(|(_, planned_on, item)| ReviewItem { item, planned_on }),
            );

            let mut counted: Vec<&mut DailyPlanItem> = review
                .done
                .iter_mut()
                .chain(review.didnt_get_to.iter_mut().map(|entry| &mut entry.item))
                .collect();
            fill_carry_counts(c, date, counted.iter_mut().map(|item| &mut **item))?;

            if let Some(reviewed) = &reviewed {
                let reviewed_day = parse_day(reviewed)?;
                if let Some(bounds) = day_bounds(&now.timezone(), reviewed_day) {
                    let planned: HashSet<String> =
                        earlier[0].section.items.iter().map(item_key).collect();
                    review.also_done = also_done(c, bounds, &planned)?;
                    review.meetings = attended(c, bounds, &now)?;
                }
            }
            Ok(review)
        })
    }
}

#[cfg(test)]
#[path = "tests/plan_review.rs"]
mod tests;
