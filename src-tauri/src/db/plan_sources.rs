//! Candidates for a day's plan, one source and one page at a time. Each
//! source reuses an existing local query (tasks, starred conversations, open
//! promises, Keep in Touch, goals, note tasks) and never asks a server.
//! A candidate's `key` is the plan item key adding it produces, so callers can
//! mark candidates already in the plan.
use super::daily_plan::{item_key, PlanGoal};
use super::plan_review::{note_task_target, task_key, PLAN_HISTORY_DAYS};
use super::tasks::{select_sql, task_from_row};
use super::{Database, DatabaseError, DbResult};
use crate::daily_plan::{self, PlanAddition};
use crate::models::ThreadTask;
use crate::note_entities::{normalize_contact_address, NoteReference};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Candidates in one page of a source.
pub const PLAN_SOURCE_PAGE: usize = 50;
/// Most open tasks Suggested considers before ordering them.
const MAX_SUGGESTED_TASKS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanSource {
    Suggested,
    Tasks,
    StarredMail,
    Promises,
    KeepInTouch,
    Goals,
    Notes,
}

/// Why a candidate is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanCandidateReason {
    Overdue,
    /// Due on the plan's date.
    DueToday,
    InProgress,
    /// Waiting for a reply past its wait date.
    Waiting,
    /// An open task with nothing more pressing.
    Open,
    Promise,
    KeepInTouch,
    Starred,
    /// A current goal itself, to add a task for it.
    Goal,
    GoalTask,
    NoteTask,
}

/// What adding a candidate to the plan takes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PlanCandidateAction {
    /// Add this item as it is.
    Link { addition: PlanAddition },
    /// Make a follow-up task from the promise first (Make a Task), then link it.
    #[serde(rename_all = "camelCase")]
    MakeTask {
        thread_id: String,
        message_id: String,
    },
    /// Open the task form prefilled from the conversation; saving creates the
    /// task to link. Cancel writes nothing.
    #[serde(rename_all = "camelCase")]
    TaskForm {
        thread_id: String,
        account_id: String,
    },
    /// Open the task form with the goal set (New Task for Goal).
    #[serde(rename_all = "camelCase")]
    NewTaskForGoal { goal_id: String, account_id: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCandidate {
    /// The plan item key adding it produces. For a starred conversation
    /// without a task or a promise, the conversation's reference target,
    /// since the task doesn't exist yet; for a goal, `goal:<id>`.
    pub key: String,
    pub title: String,
    pub reason: PlanCandidateReason,
    /// Context: a note's title, a conversation's subject or sender, a
    /// promise's recipients.
    pub detail: Option<String>,
    pub account_id: Option<String>,
    /// A task's due value, or when a check-in falls due.
    pub due: Option<String>,
    /// The task's id as Tasks lists it (`note:<note>:<marker>` for a note task).
    pub task_id: Option<String>,
    pub thread_id: Option<String>,
    pub contact_id: Option<String>,
    pub goal: Option<PlanGoal>,
    /// For a goal's task: dates in the goal's period (and the history
    /// window) on which it was planned.
    pub planned_days: Vec<String>,
    /// For a goal: its open tasks.
    pub open_tasks: Option<usize>,
    /// Already a plan item on the plan's date.
    pub planned: bool,
    pub action: PlanCandidateAction,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSourcePage {
    pub candidates: Vec<PlanCandidate>,
    /// The cursor for the next page, when there is one.
    pub next: Option<String>,
}

fn task_reference(id: &str) -> NoteReference {
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
}

fn link(reference: NoteReference, label: &str) -> PlanCandidateAction {
    PlanCandidateAction::Link {
        addition: PlanAddition::Link {
            reference,
            label: label.into(),
        },
    }
}

fn candidate(
    key: String,
    title: String,
    reason: PlanCandidateReason,
    action: PlanCandidateAction,
) -> PlanCandidate {
    PlanCandidate {
        key,
        title,
        reason,
        detail: None,
        account_id: None,
        due: None,
        task_id: None,
        thread_id: None,
        contact_id: None,
        goal: None,
        planned_days: Vec::new(),
        open_tasks: None,
        planned: false,
        action,
    }
}

/// How a task's due date stands against the plan's `date`: overdue when it
/// falls on an earlier day or its time has passed, due on that day otherwise.
fn due_reason<Z: TimeZone>(
    task: &ThreadTask,
    date: NaiveDate,
    now: &DateTime<Z>,
) -> Option<PlanCandidateReason> {
    let value = task.due_value.as_deref()?;
    let (day, passed) = match task.due_kind.as_str() {
        "date" => (NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?, false),
        "datetime" => {
            let at = DateTime::parse_from_rfc3339(value).ok()?;
            (
                at.with_timezone(&now.timezone()).date_naive(),
                at.with_timezone(&Utc) < now.with_timezone(&Utc),
            )
        }
        _ => return None,
    };
    if day < date || passed {
        Some(PlanCandidateReason::Overdue)
    } else if day == date {
        Some(PlanCandidateReason::DueToday)
    } else {
        None
    }
}

fn task_candidate(
    task: &ThreadTask,
    reason: PlanCandidateReason,
    goals: &HashMap<String, String>,
) -> DbResult<PlanCandidate> {
    let mut out = candidate(
        task_key(&task.id)?,
        task.title.clone(),
        reason,
        link(task_reference(&task.id), &task.title),
    );
    out.account_id = Some(task.account_id.clone());
    out.due = task.due_value.clone();
    out.task_id = Some(task.id.clone());
    out.thread_id = task.thread_id.clone();
    out.goal = task.goal_id.as_ref().and_then(|id| {
        goals.get(id).map(|title| PlanGoal {
            id: id.clone(),
            title: title.clone(),
        })
    });
    Ok(out)
}

fn goal_titles(c: &Connection) -> DbResult<HashMap<String, String>> {
    let mut q = c.prepare("SELECT id,title FROM goals")?;
    let rows = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Open and in-progress tasks in display order: dated first by due time,
/// then the rest, oldest first.
const TASK_ORDER: &str =
    "ORDER BY CASE WHEN due_kind='none' OR due_value IS NULL THEN 1 ELSE 0 END,
    julianday(due_value), created_at, id";

/// The period of `horizon` containing `date`, as goals name periods.
fn period_for(horizon: &str, date: NaiveDate) -> String {
    match horizon {
        "year" => format!("{}", date.year()),
        "half" => format!("{}-H{}", date.year(), if date.month() <= 6 { 1 } else { 2 }),
        _ => format!("{}-Q{}", date.year(), (date.month() - 1) / 3 + 1),
    }
}

/// The first day of the period of `horizon` containing `date`.
fn period_start(horizon: &str, date: NaiveDate) -> NaiveDate {
    let month = match horizon {
        "year" => 1,
        "half" => {
            if date.month() <= 6 {
                1
            } else {
                7
            }
        }
        _ => (date.month() - 1) / 3 * 3 + 1,
    };
    NaiveDate::from_ymd_opt(date.year(), month, 1).unwrap_or(date)
}

fn offset_of(cursor: Option<&str>) -> DbResult<usize> {
    match cursor {
        None => Ok(0),
        Some(text) => text
            .parse::<usize>()
            .map_err(|_| DatabaseError::invalid("Invalid page cursor")),
    }
}

/// One page of `all`, with the cursor after it.
fn page_of(mut all: Vec<PlanCandidate>, offset: usize) -> PlanSourcePage {
    let more = all.len() > offset + PLAN_SOURCE_PAGE;
    let candidates: Vec<PlanCandidate> = all
        .drain(offset.min(all.len())..)
        .take(PLAN_SOURCE_PAGE)
        .collect();
    PlanSourcePage {
        next: more.then(|| (offset + candidates.len()).to_string()),
        candidates,
    }
}

impl Database {
    /// One page of plan candidates from `source` for the plan of `date`, in
    /// this device's time zone. `account` scopes the mail sources (Tasks,
    /// Starred Mail, promises, Goals, and Suggested's tasks and promises).
    pub fn plan_sources(
        &self,
        date: &str,
        source: PlanSource,
        account: Option<&str>,
        cursor: Option<&str>,
    ) -> DbResult<PlanSourcePage> {
        self.plan_sources_at(date, source, account, cursor, chrono::Local::now())
    }

    pub fn plan_sources_at<Z: TimeZone>(
        &self,
        date: &str,
        source: PlanSource,
        account: Option<&str>,
        cursor: Option<&str>,
        now: DateTime<Z>,
    ) -> DbResult<PlanSourcePage> {
        if !crate::notes::is_daily_date(date) {
            return Err(DatabaseError::invalid("Invalid date"));
        }
        let day = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| DatabaseError::invalid("Invalid date"))?;
        let offset = offset_of(cursor)?;
        let mut page = match source {
            PlanSource::Suggested => {
                let mut all = self.suggested_tasks(day, account, &now)?;
                all.extend(self.promise_candidates(account, &now)?);
                all.extend(self.keep_in_touch_candidates(day, &now)?);
                page_of(all, offset)
            }
            PlanSource::Tasks => self.task_candidates(day, account, offset, &now)?,
            PlanSource::StarredMail => self.starred_candidates(account, offset)?,
            PlanSource::Promises => page_of(self.promise_candidates(account, &now)?, offset),
            PlanSource::KeepInTouch => page_of(self.keep_in_touch_candidates(day, &now)?, offset),
            PlanSource::Goals => page_of(self.goal_candidates(day, account)?, offset),
            PlanSource::Notes => self.note_candidates(date, offset)?,
        };
        let planned = self.planned_keys(date)?;
        for candidate in &mut page.candidates {
            candidate.planned = planned.contains(&candidate.key);
        }
        Ok(page)
    }

    fn planned_keys(&self, date: &str) -> DbResult<HashSet<String>> {
        let note_id = crate::notes::daily_note_id(date);
        self.with_connection(|c| {
            let body: Option<String> = c
                .query_row(
                    "SELECT body FROM notes WHERE id=?1 AND trashed=0",
                    [&note_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(body
                .and_then(|body| daily_plan::parse_plan(&note_id, &body).ok().flatten())
                .map(|section| section.items.iter().map(item_key).collect())
                .unwrap_or_default())
        })
    }

    fn suggested_tasks<Z: TimeZone>(
        &self,
        day: NaiveDate,
        account: Option<&str>,
        now: &DateTime<Z>,
    ) -> DbResult<Vec<PlanCandidate>> {
        self.with_connection(|c| {
            let goals = goal_titles(c)?;
            // Dated tasks up to a day past the plan's date (zones differ by
            // under a day), and every task in progress; ranked below.
            let mut q = c.prepare(&format!(
                "{} WHERE status IN ('open','in_progress') AND (?1 IS NULL OR account_id=?1)
                 AND (status='in_progress' OR (due_kind!='none' AND julianday(due_value)<julianday(?2,'+2 days')))
                 {TASK_ORDER} LIMIT ?3",
                select_sql()
            ))?;
            let tasks = q
                .query_map(
                    params![account, day.to_string(), MAX_SUGGESTED_TASKS as i64],
                    task_from_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let mut ranked = Vec::new();
            for task in &tasks {
                let due = due_reason(task, day, now);
                let reason = if task.kind == "waiting_for" && due.is_some() {
                    PlanCandidateReason::Waiting
                } else if let Some(due) = due {
                    due
                } else if task.status == "in_progress" {
                    PlanCandidateReason::InProgress
                } else {
                    continue;
                };
                ranked.push(task_candidate(task, reason, &goals)?);
            }
            // Stable: within a reason, display order is kept.
            ranked.sort_by_key(|candidate| candidate.reason);
            Ok(ranked)
        })
    }

    fn task_candidates<Z: TimeZone>(
        &self,
        day: NaiveDate,
        account: Option<&str>,
        offset: usize,
        now: &DateTime<Z>,
    ) -> DbResult<PlanSourcePage> {
        self.with_connection(|c| {
            let goals = goal_titles(c)?;
            let mut q = c.prepare(&format!(
                "{} WHERE status IN ('open','in_progress') AND (?1 IS NULL OR account_id=?1)
                 {TASK_ORDER} LIMIT ?2 OFFSET ?3",
                select_sql()
            ))?;
            let tasks = q
                .query_map(
                    params![account, (PLAN_SOURCE_PAGE + 1) as i64, offset as i64],
                    task_from_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let more = tasks.len() > PLAN_SOURCE_PAGE;
            let mut candidates = Vec::new();
            for task in tasks.iter().take(PLAN_SOURCE_PAGE) {
                let reason =
                    due_reason(task, day, now).unwrap_or(if task.status == "in_progress" {
                        PlanCandidateReason::InProgress
                    } else {
                        PlanCandidateReason::Open
                    });
                candidates.push(task_candidate(task, reason, &goals)?);
            }
            Ok(PlanSourcePage {
                next: more.then(|| (offset + PLAN_SOURCE_PAGE).to_string()),
                candidates,
            })
        })
    }

    fn starred_candidates(&self, account: Option<&str>, offset: usize) -> DbResult<PlanSourcePage> {
        let page = self.list_starred_page(account, offset, PLAN_SOURCE_PAGE)?;
        self.with_connection(|c| {
            let goals = goal_titles(c)?;
            let mut open = c.prepare(&format!(
                "{} WHERE thread_id=?1 AND status IN ('open','in_progress') ORDER BY created_at,id LIMIT 1",
                select_sql()
            ))?;
            let mut candidates = Vec::new();
            for thread in &page.threads {
                let sender = thread.participants.first().cloned();
                // A conversation with an open task offers that task instead.
                let task = open
                    .query_row([&thread.id], task_from_row)
                    .optional()?;
                let mut out = match task {
                    Some(task) => {
                        let mut out = task_candidate(&task, PlanCandidateReason::Starred, &goals)?;
                        out.detail = Some(thread.subject.clone());
                        out
                    }
                    None => {
                        let reference = NoteReference {
                            kind: "conversation".into(),
                            account: Some(thread.account_id.clone()),
                            ..task_reference(&thread.provider_thread_id)
                        };
                        let mut out = candidate(
                            reference.target().map_err(DatabaseError::invalid)?,
                            thread.subject.clone(),
                            PlanCandidateReason::Starred,
                            PlanCandidateAction::TaskForm {
                                thread_id: thread.id.clone(),
                                account_id: thread.account_id.clone(),
                            },
                        );
                        out.detail = sender;
                        out
                    }
                };
                out.account_id = Some(thread.account_id.clone());
                out.thread_id = Some(thread.id.clone());
                candidates.push(out);
            }
            Ok(PlanSourcePage {
                next: page
                    .has_more
                    .then(|| (offset + page.threads.len()).to_string()),
                candidates,
            })
        })
    }

    fn promise_candidates<Z: TimeZone>(
        &self,
        account: Option<&str>,
        now: &DateTime<Z>,
    ) -> DbResult<Vec<PlanCandidate>> {
        let since =
            (now.with_timezone(&Utc) - Duration::days(super::PROMISE_LOOKBACK_DAYS)).to_rfc3339();
        let mut out = Vec::new();
        for promise in self.list_open_promises(account, &since)? {
            let reference = NoteReference {
                kind: "conversation".into(),
                account: Some(promise.thread.account_id.clone()),
                ..task_reference(&promise.thread.provider_thread_id)
            };
            let mut item = candidate(
                reference.target().map_err(DatabaseError::invalid)?,
                promise.excerpt.clone(),
                PlanCandidateReason::Promise,
                PlanCandidateAction::MakeTask {
                    thread_id: promise.thread.id.clone(),
                    message_id: promise.message_id.clone(),
                },
            );
            item.detail = Some(promise.recipients.join(", "));
            item.account_id = Some(promise.thread.account_id.clone());
            item.thread_id = Some(promise.thread.id.clone());
            out.push(item);
        }
        Ok(out)
    }

    /// Check-ins due on or before the plan's date, soonest first.
    fn keep_in_touch_candidates<Z: TimeZone>(
        &self,
        day: NaiveDate,
        now: &DateTime<Z>,
    ) -> DbResult<Vec<PlanCandidate>> {
        let mut out = Vec::new();
        for profile in self.list_keep_in_touch()? {
            let Some(due) = profile.keep_in_touch_due_at.as_deref() else {
                continue;
            };
            let Ok(at) = DateTime::parse_from_rfc3339(due) else {
                continue;
            };
            let Some(address) = profile.addresses.first() else {
                continue;
            };
            if at.with_timezone(&now.timezone()).date_naive() > day {
                continue;
            }
            let reference = NoteReference {
                kind: "contact".into(),
                contact: Some(profile.id.clone()),
                ..task_reference(&normalize_contact_address(address))
            };
            let title = profile
                .display_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| address.clone());
            let mut item = candidate(
                reference.target().map_err(DatabaseError::invalid)?,
                title.clone(),
                PlanCandidateReason::KeepInTouch,
                link(reference, &title),
            );
            item.due = Some(due.to_string());
            item.contact_id = Some(profile.id.clone());
            out.push(item);
        }
        Ok(out)
    }

    /// Active goals of the periods containing the plan's date, shortest
    /// horizon first, each followed by its open tasks with their planned days.
    fn goal_candidates(
        &self,
        day: NaiveDate,
        account: Option<&str>,
    ) -> DbResult<Vec<PlanCandidate>> {
        self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT id,account_id,title,horizon,period FROM goals WHERE status='active'
                 AND (?1 IS NULL OR account_id=?1)
                 ORDER BY CASE horizon WHEN 'quarter' THEN 0 WHEN 'half' THEN 1 ELSE 2 END, created_at, id",
            )?;
            let goals = q
                .query_map(params![account], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut tasks_of = c.prepare(&format!(
                "{} WHERE goal_id=?1 AND status IN ('open','in_progress') {TASK_ORDER}",
                select_sql()
            ))?;
            let mut out = Vec::new();
            let mut starts = HashMap::new();
            for (id, account_id, title, horizon, period) in goals {
                if period != period_for(&horizon, day) {
                    continue;
                }
                let tasks = tasks_of
                    .query_map([&id], task_from_row)?
                    .collect::<Result<Vec<_>, _>>()?;
                let goal = PlanGoal {
                    id: id.clone(),
                    title: title.clone(),
                };
                let mut head = candidate(
                    format!("goal:{id}"),
                    title.clone(),
                    PlanCandidateReason::Goal,
                    PlanCandidateAction::NewTaskForGoal {
                        goal_id: id.clone(),
                        account_id: account_id.clone(),
                    },
                );
                head.detail = Some(period.clone());
                head.account_id = Some(account_id.clone());
                head.goal = Some(goal.clone());
                head.open_tasks = Some(tasks.len());
                out.push(head);
                for task in &tasks {
                    let mut item = candidate(
                        task_key(&task.id)?,
                        task.title.clone(),
                        PlanCandidateReason::GoalTask,
                        link(task_reference(&task.id), &task.title),
                    );
                    item.account_id = Some(task.account_id.clone());
                    item.due = task.due_value.clone();
                    item.task_id = Some(task.id.clone());
                    item.thread_id = task.thread_id.clone();
                    item.goal = Some(goal.clone());
                    starts.insert(item.key.clone(), period_start(&horizon, day));
                    out.push(item);
                }
            }
            // Planned days within each goal's period and the history window.
            let targets: Vec<String> = starts.keys().cloned().collect();
            let from = day - Duration::days(PLAN_HISTORY_DAYS - 1);
            for chunk in targets.chunks(super::plan_review::MAX_PLAN_HISTORY_TARGETS) {
                let history = super::plan_review::history_for(c, chunk, from, day)?;
                for item in out.iter_mut().filter(|item| chunk.contains(&item.key)) {
                    let start = starts[&item.key].to_string();
                    if let Some(entry) = history.get(&item.key) {
                        let mut days: Vec<String> = entry
                            .planned_days
                            .iter()
                            .filter(|planned| **planned >= start)
                            .cloned()
                            .collect();
                        days.sort();
                        item.planned_days = days;
                    }
                }
            }
            Ok(out)
        })
    }

    /// Open round tasks in notes other than the plan's own, newest note first.
    fn note_candidates(&self, date: &str, offset: usize) -> DbResult<PlanSourcePage> {
        let own = crate::notes::daily_note_id(date);
        self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT t.note_id,t.marker_id,t.title,n.title FROM note_tasks t JOIN notes n ON n.id=t.note_id
                 WHERE n.trashed=0 AND t.checked=0 AND t.note_id!=?1
                 ORDER BY n.updated_at DESC, t.note_id, t.rowid LIMIT ?2 OFFSET ?3",
            )?;
            let rows = q
                .query_map(
                    params![own, (PLAN_SOURCE_PAGE + 1) as i64, offset as i64],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let more = rows.len() > PLAN_SOURCE_PAGE;
            let mut candidates = Vec::new();
            for (note, marker, title, note_title) in rows.into_iter().take(PLAN_SOURCE_PAGE) {
                let reference = NoteReference {
                    note: Some(note.clone()),
                    ..task_reference(&marker)
                };
                let mut item = candidate(
                    note_task_target(&note, &marker)?,
                    title.clone(),
                    PlanCandidateReason::NoteTask,
                    link(reference, &title),
                );
                item.detail = Some(note_title);
                item.task_id = Some(format!("note:{note}:{marker}"));
                candidates.push(item);
            }
            Ok(PlanSourcePage {
                next: more.then(|| (offset + PLAN_SOURCE_PAGE).to_string()),
                candidates,
            })
        })
    }
}

#[cfg(test)]
#[path = "tests/plan_sources.rs"]
mod tests;
