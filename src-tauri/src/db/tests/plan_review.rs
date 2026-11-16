use super::*;
use crate::db::test_support::database;

const DAY: &str = "2026-11-16";
const ME: &str = "me@example.com";

fn now() -> DateTime<Utc> {
    "2026-11-16T12:00:00Z".parse().unwrap()
}

fn task_ref(id: &str) -> NoteReference {
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

fn href(id: &str) -> String {
    task_ref(id).href().unwrap()
}

fn key(id: &str) -> String {
    task_ref(id).target().unwrap()
}

fn task(db: &Database, title: &str) -> String {
    db.adopt_account(ME).unwrap();
    db.create_task(
        &serde_json::from_value(
            serde_json::json!({"accountId": ME, "title": title, "kind": "action"}),
        )
        .unwrap(),
    )
    .unwrap()
    .id
}

fn complete(db: &Database, id: &str, at: &str) {
    db.connection()
        .unwrap()
        .execute(
            "UPDATE tasks SET status='completed', completed_at=?2 WHERE id=?1",
            [id, at],
        )
        .unwrap();
}

fn plan(db: &Database, date: &str, body: &str) {
    db.save_daily_note(date, body).unwrap().unwrap();
}

fn titles(items: &[ReviewItem]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|entry| (entry.item.title.clone(), entry.planned_on.clone()))
        .collect()
}

#[test]
fn history_counts_carried_plan_days_once_each_within_the_range() {
    let db = database();
    let a = task(&db, "Contract");
    let b = task(&db, "Deck");
    plan(
        &db,
        "2026-11-13",
        &format!("## Plan\n\n- [A]({})\n", href(&a)),
    );
    plan(
        &db,
        "2026-11-14",
        &format!(
            "## Plan\n\n- [A]({}) <!-- threestrands-plan:carried 2026-11-13 -->\n- [A again]({})\n",
            href(&a),
            href(&a)
        ),
    );
    plan(
        &db,
        "2026-11-15",
        &format!(
            "## Plan\n\n- [A]({}) <!-- threestrands-plan:carried 2026-11-14 -->\n\n### Not today\n\n- [B]({}) <!-- threestrands-plan:dropped -->\n",
            href(&a),
            href(&b)
        ),
    );
    // A link outside the plan section isn't a plan item.
    plan(&db, DAY, &format!("Talked about [A]({})\n", href(&a)));

    let history = db
        .plan_history(&[key(&a), key(&b), key(&a)], "2026-11-13", DAY)
        .unwrap();
    assert_eq!(
        history,
        [
            PlanHistory {
                target: key(&a),
                carry_count: 2,
                planned_days: vec![
                    "2026-11-13".into(),
                    "2026-11-14".into(),
                    "2026-11-15".into()
                ],
            },
            PlanHistory {
                target: key(&b),
                carry_count: 0,
                planned_days: vec![],
            },
        ]
    );
    // The range bounds what counts.
    let late = db.plan_history(&[key(&a)], "2026-11-15", DAY).unwrap();
    assert_eq!(late[0].carry_count, 1);
    assert_eq!(late[0].planned_days, ["2026-11-15"]);

    // The day's plan reads its items' carry counts up to its date.
    let day = db.daily_plan("2026-11-15").unwrap();
    assert_eq!(day.items[0].carry_count, 2);
    assert_eq!(day.not_today[0].carry_count, 0);
    assert_eq!(db.daily_plan("2026-11-13").unwrap().items[0].carry_count, 0);
}

#[test]
fn history_ranges_and_target_counts_are_bounded() {
    let db = database();
    let a = task(&db, "Contract");
    plan(
        &db,
        "2026-08-19",
        &format!(
            "## Plan\n\n- [A]({}) <!-- threestrands-plan:carried 2026-08-18 -->\n",
            href(&a)
        ),
    );
    plan(
        &db,
        "2026-08-18",
        &format!(
            "## Plan\n\n- [A]({}) <!-- threestrands-plan:carried 2026-08-17 -->\n",
            href(&a)
        ),
    );
    // 89 days apart: 90 plan days, the most a range may cover.
    let exact = db.plan_history(&[key(&a)], "2026-08-19", DAY).unwrap();
    assert_eq!(exact[0].carry_count, 1);
    assert!(db.plan_history(&[key(&a)], "2026-08-20", DAY).is_ok());
    assert!(db.plan_history(&[key(&a)], "2026-08-18", DAY).is_err());
    // daily_plan counts over the same 90 days ending on its date, so the
    // plan of the 18th falls outside today's window.
    plan(
        &db,
        DAY,
        &format!(
            "## Plan\n\n- [A]({}) <!-- threestrands-plan:carried 2026-11-15 -->\n",
            href(&a)
        ),
    );
    assert_eq!(db.daily_plan(DAY).unwrap().items[0].carry_count, 2);
    // An old note edited by hand changes its history.
    plan(&db, "2026-08-19", "Nothing planned after all\n");
    assert_eq!(db.daily_plan(DAY).unwrap().items[0].carry_count, 1);
    assert!(db.plan_history(&[], DAY, "2026-11-15").is_err());
    assert!(db.plan_history(&[], "2026-11-16", DAY).unwrap().is_empty());
    assert!(db.plan_history(&[], "not a day", DAY).is_err());

    let at_limit: Vec<String> = (0..MAX_PLAN_HISTORY_TARGETS)
        .map(|n| key(&format!("t{n}")))
        .collect();
    assert!(db.plan_history(&at_limit, DAY, DAY).is_ok());
    let over: Vec<String> = (0..=MAX_PLAN_HISTORY_TARGETS)
        .map(|n| key(&format!("t{n}")))
        .collect();
    assert!(db.plan_history(&over, DAY, DAY).is_err());
}

#[test]
fn history_finds_round_tasks_in_their_own_plan_and_links_to_them_later() {
    let db = database();
    plan(&db, "2026-11-14", "## Plan\n\n- ( ) Draft memo\n");
    let memo = db.daily_plan("2026-11-14").unwrap().items[0].clone();
    let reference = memo.reference.clone().unwrap();
    plan(
        &db,
        "2026-11-15",
        &format!(
            "## Plan\n\n- [Draft memo]({}) <!-- threestrands-plan:carried 2026-11-14 -->\n",
            reference.href().unwrap()
        ),
    );
    let history = db
        .plan_history(std::slice::from_ref(&memo.key), "2026-11-01", DAY)
        .unwrap();
    assert_eq!(history[0].planned_days, ["2026-11-14", "2026-11-15"]);
    assert_eq!(history[0].carry_count, 1);
}

#[test]
fn review_reads_the_most_recent_plan_within_seven_days() {
    let db = database();
    let a = task(&db, "Exactly seven");
    let b = task(&db, "Eight days ago");
    plan(
        &db,
        "2026-11-08",
        &format!("## Plan\n\n- [B]({})\n", href(&b)),
    );
    plan(
        &db,
        "2026-11-09",
        &format!("## Plan\n\n- [A]({})\n", href(&a)),
    );
    let review = db.plan_review_at(DAY, now()).unwrap();
    assert_eq!(review.reviewed.as_deref(), Some("2026-11-09"));
    assert_eq!(
        titles(&review.didnt_get_to),
        [("Exactly seven".into(), "2026-11-09".into())]
    );

    // Below the boundary: a plan six days back is the one reviewed.
    plan(&db, "2026-11-10", "## Plan\n\n- Just text\n");
    assert_eq!(
        db.plan_review_at(DAY, now()).unwrap().reviewed.as_deref(),
        Some("2026-11-10")
    );
    // Above it: nothing to review.
    let empty = db.plan_review_at("2026-11-18", now()).unwrap();
    assert!(empty.reviewed.is_none() && empty.didnt_get_to.is_empty() && empty.done.is_empty());
    assert!(empty.meetings.is_none() && empty.also_done.is_empty());
    // Today's own plan isn't reviewed.
    plan(&db, "2026-11-17", "## Plan\n\n- Today\n");
    assert_eq!(
        db.plan_review_at("2026-11-17", now())
            .unwrap()
            .reviewed
            .as_deref(),
        Some("2026-11-10")
    );
    assert!(db.plan_review_at("someday", now()).is_err());
}

#[test]
fn review_lists_done_items_and_tasks_done_that_day_outside_the_plan() {
    let db = database();
    let planned = task(&db, "Planned and done");
    let open = task(&db, "Planned, still open");
    let unplanned = task(&db, "Done without a plan");
    let other_day = task(&db, "Done the day before");
    complete(&db, &planned, "2026-11-15T10:00:00Z");
    complete(&db, &unplanned, "2026-11-15T16:00:00Z");
    complete(&db, &other_day, "2026-11-14T23:59:59Z");
    let note = db.create_note("Projects", "- ( ) Note chore\n").unwrap();
    db.connection()
        .unwrap()
        .execute(
            "UPDATE note_tasks SET checked=1, completed_at='2026-11-15T09:00:00Z' WHERE note_id=?1",
            [&note.id],
        )
        .unwrap();
    plan(
        &db,
        "2026-11-15",
        &format!(
            "## Plan\n\n- [P]({})\n- [O]({})\n- (x) Typed and done\n",
            href(&planned),
            href(&open)
        ),
    );

    let review = db.plan_review_at(DAY, now()).unwrap();
    let done: Vec<_> = review.done.iter().map(|item| item.title.as_str()).collect();
    assert_eq!(done, ["Planned and done", "Typed and done"]);
    let also: Vec<_> = review
        .also_done
        .iter()
        .map(|task| task.title.as_str())
        .collect();
    assert_eq!(also, ["Note chore", "Done without a plan"]);
    assert!(review.also_done[0]
        .task_id
        .starts_with(&format!("note:{}:", note.id)));
    assert_eq!(review.also_done[0].note_title.as_deref(), Some("Projects"));
    assert_eq!(review.also_done[1].key, key(&unplanned));
    assert_eq!(
        titles(&review.didnt_get_to),
        [("Planned, still open".into(), "2026-11-15".into())]
    );
}

#[test]
fn review_keeps_unsettled_items_and_due_deferrals_in_order() {
    let db = database();
    let ids: Vec<String> = [
        "Old unsettled",
        "Carried later",
        "Dropped later",
        "Yesterday open",
        "Deferred due",
        "Deferred future",
        "Deferred long ago",
        "Settled by its own shutdown",
        "Twice",
    ]
    .iter()
    .map(|title| task(&db, title))
    .collect();
    let line = |n: usize| format!("[{n}]({})", href(&ids[n]));
    // Deferred three weeks ago until yesterday: older than the lookback.
    plan(
        &db,
        "2026-10-26",
        &format!(
            "## Plan\n\n- Text\n\n### Not today\n\n- {} <!-- threestrands-plan:deferred 2026-11-15 -->\n",
            line(6)
        ),
    );
    plan(
        &db,
        "2026-11-11",
        &format!(
            "## Plan\n\n- {}\n- {}\n- {}\n- {}\n",
            line(0),
            line(1),
            line(2),
            line(8)
        ),
    );
    plan(
        &db,
        "2026-11-13",
        &format!(
            "## Plan\n\n- {} <!-- threestrands-plan:carried 2026-11-11 -->\n- {}\n\n### Not today\n\n- {} <!-- threestrands-plan:dropped -->\n- {} <!-- threestrands-plan:deferred 2026-11-16 -->\n- {} <!-- threestrands-plan:deferred 2026-11-17 -->\n",
            line(1), line(8), line(2), line(4), line(5)
        ),
    );
    plan(
        &db,
        "2026-11-15",
        &format!(
            "## Plan\n\n- {}\n- {}\n\n### Not today\n\n- {} <!-- threestrands-plan:deferred 2026-11-20 -->\n",
            line(3), line(7), line(7)
        ),
    );
    // Today's plan settles what it mentions.
    plan(
        &db,
        DAY,
        &format!(
            "## Plan\n\n- {} <!-- threestrands-plan:carried 2026-11-13 -->\n",
            line(1)
        ),
    );

    let review = db.plan_review_at(DAY, now()).unwrap();
    assert_eq!(
        titles(&review.didnt_get_to),
        [
            ("Yesterday open".to_string(), "2026-11-15".to_string()),
            ("Twice".into(), "2026-11-13".into()),
            ("Old unsettled".into(), "2026-11-11".into()),
            ("Deferred long ago".into(), "2026-10-26".into()),
            ("Deferred due".into(), "2026-11-13".into()),
        ]
    );
    assert_eq!(
        review.didnt_get_to[4].item.marker,
        Some(PlanMarker::Deferred("2026-11-16".into()))
    );
    // A task finished since leaves the list.
    complete(&db, &ids[8], "2026-11-15T10:00:00Z");
    let after = db.plan_review_at(DAY, now()).unwrap();
    assert!(!titles(&after.didnt_get_to)
        .iter()
        .any(|(title, _)| title == "Twice"));
    // So does a finished deferral; a missing task stays, to be dropped.
    complete(&db, &ids[4], "2026-11-14T10:00:00Z");
    db.connection()
        .unwrap()
        .execute("DELETE FROM tasks WHERE id=?1", [&ids[6]])
        .unwrap();
    let later = db.plan_review_at(DAY, now()).unwrap();
    let rest: Vec<_> = later
        .didnt_get_to
        .iter()
        .map(|entry| (entry.item.title.clone(), entry.item.state))
        .collect();
    assert!(!rest.iter().any(|(title, _)| title == "Deferred due"));
    assert!(rest
        .iter()
        .any(|(_, state)| *state == PlanItemState::Unavailable));
    // A deferral re-decided in a later plan is settled.
    plan(
        &db,
        "2026-11-14",
        &format!(
            "## Plan\n\n- Text\n\n### Not today\n\n- {} <!-- threestrands-plan:dropped -->\n",
            line(6)
        ),
    );
    let settled = db.plan_review_at(DAY, now()).unwrap();
    assert!(!settled
        .didnt_get_to
        .iter()
        .any(|entry| entry.planned_on == "2026-10-26"));
}

#[test]
fn review_carries_counts_on_unsettled_items() {
    let db = database();
    let a = task(&db, "Contract");
    for (date, from) in [
        ("2026-11-13", "2026-11-12"),
        ("2026-11-14", "2026-11-13"),
        ("2026-11-15", "2026-11-14"),
    ] {
        plan(
            &db,
            date,
            &format!(
                "## Plan\n\n- [A]({}) <!-- threestrands-plan:carried {from} -->\n",
                href(&a)
            ),
        );
    }
    let review = db.plan_review_at(DAY, now()).unwrap();
    assert_eq!(review.didnt_get_to.len(), 1);
    assert_eq!(review.didnt_get_to[0].item.carry_count, 3);
}

fn event(id: &str, start: &str, end: &str, extra: serde_json::Value) -> ScheduleEvent {
    let mut value = serde_json::json!({
        "id": id, "accountId": ME, "calendarId": "primary", "title": id,
        "start": start, "end": end, "allDay": false,
        "attendees": [], "attendeeResponses": {}, "canRespond": false, "canEdit": true
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

#[test]
fn review_lists_attended_meetings_from_the_calendar_cache_or_says_it_cannot() {
    let db = database();
    plan(&db, "2026-11-15", "## Plan\n\n- Something\n");
    db.adopt_calendar_account(ME).unwrap();
    // Nothing cached for that day: unknown, not empty.
    assert!(db.plan_review_at(DAY, now()).unwrap().meetings.is_none());

    let events = [
        event(
            "standup",
            "2026-11-15T09:00:00Z",
            "2026-11-15T09:15:00Z",
            serde_json::json!({}),
        ),
        event(
            "declined",
            "2026-11-15T10:00:00Z",
            "2026-11-15T11:00:00Z",
            serde_json::json!({"responseStatus": "declined"}),
        ),
        event(
            "holiday",
            "2026-11-15",
            "2026-11-16",
            serde_json::json!({"allDay": true}),
        ),
        event(
            "other-day",
            "2026-11-14T09:00:00Z",
            "2026-11-14T10:00:00Z",
            serde_json::json!({}),
        ),
        event(
            "copy",
            "2026-11-15T13:00:00Z",
            "2026-11-15T13:15:00Z",
            serde_json::json!({"uid": "u1"}),
        ),
        event(
            "copy2",
            "2026-11-15T13:00:00Z",
            "2026-11-15T13:15:00Z",
            serde_json::json!({"uid": "u1", "calendarId": "team"}),
        ),
    ];
    db.cache_note_events(ME, &[], "", "", &events).unwrap();
    let meetings = db.plan_review_at(DAY, now()).unwrap().meetings.unwrap();
    let ids: Vec<_> = meetings.iter().map(|event| event.id.as_str()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert_eq!(ids[0], "standup");
    assert!(ids[1].starts_with("copy"));

    // Reviewing a day whose meetings haven't all ended lists the ended ones.
    let early: DateTime<Utc> = "2026-11-15T09:10:00Z".parse().unwrap();
    plan(&db, "2026-11-14", "## Plan\n\n- Earlier\n");
    let review = db.plan_review_at("2026-11-15", early).unwrap();
    assert_eq!(review.reviewed.as_deref(), Some("2026-11-14"));
    let ids: Vec<_> = review
        .meetings
        .unwrap()
        .iter()
        .map(|e| e.id.clone())
        .collect();
    assert_eq!(ids, ["other-day"]);
    // Cached, with nothing ended yet: known and empty.
    let mid_meeting: DateTime<Utc> = "2026-11-15T09:05:00Z".parse().unwrap();
    let ongoing = db.plan_review_at(DAY, mid_meeting).unwrap();
    assert_eq!(ongoing.meetings.map(|m| m.len()), Some(0));
}
