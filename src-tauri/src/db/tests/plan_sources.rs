use super::*;
use crate::db::test_support::{clear_seed_threads, database, message};
use crate::mime::NormalizedMessage;

const DAY: &str = "2026-11-16";
const ME: &str = "me@example.com";
const OTHER: &str = "other@example.com";

fn now() -> DateTime<Utc> {
    "2026-11-16T12:00:00Z".parse().unwrap()
}

fn task(db: &Database, account: &str, title: &str, extra: serde_json::Value) -> ThreadTask {
    db.adopt_account(account).unwrap();
    let mut value = serde_json::json!({"accountId": account, "title": title, "kind": "action"});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    db.create_task(&serde_json::from_value(value).unwrap())
        .unwrap()
}

fn page(
    db: &Database,
    source: PlanSource,
    account: Option<&str>,
    cursor: Option<&str>,
) -> PlanSourcePage {
    db.plan_sources_at(DAY, source, account, cursor, now())
        .unwrap()
}

fn titles(page: &PlanSourcePage) -> Vec<(&str, PlanCandidateReason)> {
    page.candidates
        .iter()
        .map(|candidate| (candidate.title.as_str(), candidate.reason))
        .collect()
}

/// Adding a candidate's own addition gives a plan item with its key.
fn add(db: &Database, candidate: &PlanCandidate) -> String {
    let PlanCandidateAction::Link { addition } = &candidate.action else {
        panic!("{} isn't added directly", candidate.title);
    };
    let expected = db.get_daily_note(DAY).unwrap().map(|note| note.updated_at);
    db.add_to_daily_plan(DAY, expected.as_deref(), std::slice::from_ref(addition))
        .unwrap();
    db.daily_plan(DAY)
        .unwrap()
        .items
        .last()
        .unwrap()
        .key
        .clone()
}

#[test]
fn suggested_offers_pressing_tasks_then_promises_then_due_check_ins() {
    let db = database();
    clear_seed_threads(&db);
    let date = |value: &str| serde_json::json!({"dueKind": "date", "dueValue": value});
    let at = |value: &str| serde_json::json!({"dueKind": "datetime", "dueValue": value});
    task(&db, ME, "Upcoming", date("2026-11-18"));
    task(&db, ME, "Undated", serde_json::json!({}));
    task(&db, ME, "Due this afternoon", at("2026-11-16T15:00:00Z"));
    task(&db, ME, "Due this morning", at("2026-11-16T09:00:00Z"));
    task(&db, ME, "Overdue", date("2026-11-14"));
    task(&db, ME, "Due today", date(DAY));
    let started = task(&db, ME, "Started", serde_json::json!({}));
    db.set_task_status(&started.id, "in_progress", "user")
        .unwrap();
    let mut waiting = date("2026-11-15");
    waiting["kind"] = "waiting_for".into();
    task(&db, ME, "Waiting on Jill", waiting);
    task(&db, OTHER, "Other account overdue", date("2026-11-01"));

    let mut sent: NormalizedMessage = message(
        "p1",
        "promise",
        "2026-11-15T10:00:00Z",
        "Sure. I'll send the figures over tomorrow.",
    );
    sent.from = format!("Me <{ME}>");
    sent.to = vec!["Jill <jill@example.org>".into()];
    sent.labels = vec!["SENT".into()];
    db.upsert_thread(ME, &[sent]).unwrap();

    let contact = |name: &str, address: &str| {
        db.save_contact_profile(&crate::models::SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some(name.into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec![address.into()],
            time_zone: None,
        })
        .unwrap()
    };
    let ann = contact("Ann", "Ann@Example.org");
    let later = contact("Bo", "bo@example.org");
    let connection = db.connection().unwrap();
    // Due yesterday; Bo falls due after the plan's date.
    connection
        .execute(
            "UPDATE contacts SET kit_interval_days=7, kit_started_at='2026-11-08T09:00:00Z' WHERE id=?1",
            [&ann.id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE contacts SET kit_interval_days=7, kit_started_at='2026-11-12T09:00:00Z' WHERE id=?1",
            [&later.id],
        )
        .unwrap();
    drop(connection);

    let suggested = page(&db, PlanSource::Suggested, Some(ME), None);
    use PlanCandidateReason::*;
    assert_eq!(
        titles(&suggested),
        [
            ("Overdue", Overdue),
            ("Due this morning", Overdue),
            ("Due today", DueToday),
            ("Due this afternoon", DueToday),
            ("Started", InProgress),
            ("Waiting on Jill", Waiting),
            ("I'll send the figures over tomorrow", Promise),
            ("Ann", KeepInTouch),
        ]
    );
    assert!(suggested.next.is_none());
    let promise = &suggested.candidates[6];
    assert!(matches!(
        &promise.action,
        PlanCandidateAction::MakeTask { message_id, .. } if message_id == "p1"
    ));
    // Every account's tasks without a filter.
    let all = page(&db, PlanSource::Suggested, None, None);
    assert_eq!(all.candidates[0].title, "Other account overdue");

    // A candidate's key is the key of the plan item adding it makes.
    let check_in = suggested.candidates.last().unwrap().clone();
    assert_eq!(add(&db, &check_in), check_in.key);
    let overdue = suggested.candidates[0].clone();
    assert_eq!(add(&db, &overdue), overdue.key);
    let again = page(&db, PlanSource::Suggested, Some(ME), None);
    assert!(again.candidates[0].planned && again.candidates.last().unwrap().planned);
    assert!(!again.candidates[1].planned);
}

#[test]
fn tasks_page_at_fifty_in_display_order_by_account() {
    let db = database();
    for n in 0..PLAN_SOURCE_PAGE {
        task(&db, ME, &format!("Task {n:02}"), serde_json::json!({}));
    }
    task(&db, OTHER, "Elsewhere", serde_json::json!({}));
    let dated = task(
        &db,
        ME,
        "Dated",
        serde_json::json!({"dueKind": "date", "dueValue": "2026-12-01"}),
    );
    // Exactly a page: no next page.
    let mine = page(&db, PlanSource::Tasks, Some(OTHER), None);
    assert_eq!(titles(&mine), [("Elsewhere", PlanCandidateReason::Open)]);
    assert!(mine.next.is_none());
    db.set_task_status(&dated.id, "completed", "user").unwrap();
    let exact = page(&db, PlanSource::Tasks, Some(ME), None);
    assert_eq!(exact.candidates.len(), PLAN_SOURCE_PAGE);
    assert!(exact.next.is_none());
    // One over: a second page with the rest.
    db.set_task_status(&dated.id, "open", "user").unwrap();
    let first = page(&db, PlanSource::Tasks, Some(ME), None);
    assert_eq!(first.candidates.len(), PLAN_SOURCE_PAGE);
    assert_eq!(first.candidates[0].title, "Dated", "dated tasks come first");
    assert_eq!(first.candidates[1].title, "Task 00");
    assert_eq!(first.next.as_deref(), Some("50"));
    let second = page(&db, PlanSource::Tasks, Some(ME), first.next.as_deref());
    assert_eq!(titles(&second), [("Task 49", PlanCandidateReason::Open)]);
    assert!(second.next.is_none());
    // One under a page.
    db.set_task_status(&dated.id, "cancelled", "user").unwrap();
    let id = &first.candidates[1].task_id.clone().unwrap();
    db.set_task_status(id, "completed", "user").unwrap();
    let under = page(&db, PlanSource::Tasks, Some(ME), None);
    assert_eq!(under.candidates.len(), PLAN_SOURCE_PAGE - 1);
    assert!(under.next.is_none());
    assert!(db
        .plan_sources_at(DAY, PlanSource::Tasks, None, Some("x"), now())
        .is_err());
    assert!(db
        .plan_sources_at("tomorrow", PlanSource::Tasks, None, None, now())
        .is_err());
}

#[test]
fn starred_mail_offers_a_conversations_open_task_or_the_task_form() {
    let db = database();
    clear_seed_threads(&db);
    db.adopt_account(ME).unwrap();
    db.adopt_account(OTHER).unwrap();
    let starred = |id: &str, thread: &str, date: &str| {
        let mut message = message(id, thread, date, "Body");
        message.subject = format!("About {thread}");
        message.from = "Jill <jill@example.org>".into();
        message.labels = vec!["INBOX".into(), "STARRED".into()];
        message
    };
    db.upsert_thread(ME, &[starred("a", "budget", "2026-11-14T09:00:00Z")])
        .unwrap();
    db.upsert_thread(ME, &[starred("b", "hiring", "2026-11-15T09:00:00Z")])
        .unwrap();
    db.upsert_thread(OTHER, &[starred("c", "elsewhere", "2026-11-15T10:00:00Z")])
        .unwrap();
    let mut plain = message("d", "plain", "2026-11-15T11:00:00Z", "Body");
    plain.labels = vec!["INBOX".into()];
    db.upsert_thread(ME, &[plain]).unwrap();
    let budget = db
        .list_starred_page(Some(ME), 0, 10)
        .unwrap()
        .threads
        .into_iter()
        .find(|thread| thread.provider_thread_id == "budget")
        .unwrap();
    task(
        &db,
        ME,
        "Approve the budget",
        serde_json::json!({"threadId": budget.id, "subjectSnapshot": "About budget"}),
    );

    let mine = page(&db, PlanSource::StarredMail, Some(ME), None);
    assert_eq!(
        titles(&mine),
        [
            ("About hiring", PlanCandidateReason::Starred),
            ("Approve the budget", PlanCandidateReason::Starred)
        ]
    );
    assert!(matches!(
        &mine.candidates[0].action,
        PlanCandidateAction::TaskForm { account_id, .. } if account_id == ME
    ));
    assert_eq!(mine.candidates[1].detail.as_deref(), Some("About budget"));
    assert!(matches!(
        mine.candidates[1].action,
        PlanCandidateAction::Link { .. }
    ));
    assert_eq!(
        page(&db, PlanSource::StarredMail, None, None)
            .candidates
            .len(),
        3
    );
    // Listing never unstars.
    assert_eq!(
        db.list_starred_page(Some(ME), 0, 10).unwrap().threads.len(),
        2
    );
}

#[test]
fn completing_a_starred_messages_work_keeps_the_star_and_allows_work_from_a_later_message() {
    let db = database();
    clear_seed_threads(&db);
    db.adopt_account(ME).unwrap();
    let mut original = message(
        "starred-work",
        "follow-up",
        "2026-11-14T09:00:00Z",
        "Please review",
    );
    original.labels = vec!["INBOX".into(), "STARRED".into()];
    db.upsert_thread(ME, std::slice::from_ref(&original))
        .unwrap();
    let thread_id = page(&db, PlanSource::StarredMail, Some(ME), None).candidates[0]
        .thread_id
        .clone()
        .unwrap();
    let message_id = db.message_ids_for_thread(&thread_id).unwrap().remove(0);
    let work = task(
        &db,
        ME,
        "Review the proposal",
        serde_json::json!({"threadId": thread_id, "sourceMessageId": message_id, "subjectSnapshot": "Proposal follow-up"}),
    );
    db.set_task_status(&work.id, "completed", "user").unwrap();
    assert!(page(&db, PlanSource::StarredMail, Some(ME), None)
        .candidates
        .is_empty());
    let starred = db.list_starred_page(Some(ME), 0, 10).unwrap();
    assert_eq!(starred.threads.len(), 1);
    assert!(!starred.threads[0].archived);
    assert_eq!(
        db.message_ids_for_thread(&thread_id).unwrap(),
        vec![message_id]
    );

    // Reopening the task offers that existing work, without creating another task.
    db.set_task_status(&work.id, "open", "user").unwrap();
    assert_eq!(
        page(&db, PlanSource::StarredMail, Some(ME), None).candidates[0]
            .task_id
            .as_deref(),
        Some(work.id.as_str())
    );
    db.set_task_status(&work.id, "cancelled", "user").unwrap();
    assert!(matches!(
        page(&db, PlanSource::StarredMail, Some(ME), None).candidates[0].action,
        PlanCandidateAction::TaskForm { .. }
    ));
    db.set_task_status(&work.id, "completed", "user").unwrap();
    let mut later = message(
        "new-work",
        "follow-up",
        "2026-11-15T09:00:00Z",
        "One more question",
    );
    later.labels = vec!["INBOX".into()];
    db.upsert_thread(ME, &[original, later]).unwrap();
    assert!(matches!(
        page(&db, PlanSource::StarredMail, Some(ME), None).candidates[0].action,
        PlanCandidateAction::TaskForm { .. }
    ));
}

#[test]
fn goals_list_current_goals_with_open_tasks_and_planned_days_in_the_period() {
    let db = database();
    db.adopt_account(ME).unwrap();
    let goal = |title: &str, horizon: &str, period: &str| {
        db.create_goal(&crate::models::CreateGoalRequest {
            account_id: ME.into(),
            title: title.into(),
            notes: None,
            horizon: horizon.into(),
            period: period.into(),
            parent_goal_id: None,
        })
        .unwrap()
    };
    let quarter = goal("Ship v2", "quarter", "2026-Q4");
    goal("Last quarter", "quarter", "2026-Q3");
    let year = goal("Grow", "year", "2026");
    let shipped = task(
        &db,
        ME,
        "Write notes",
        serde_json::json!({"goalId": quarter.id}),
    );
    task(
        &db,
        ME,
        "Done one",
        serde_json::json!({"goalId": quarter.id}),
    );
    db.set_task_status(
        &db.list_tasks(Some(ME), None)
            .unwrap()
            .into_iter()
            .find(|t| t.title == "Done one")
            .unwrap()
            .id,
        "completed",
        "user",
    )
    .unwrap();
    let href = NoteReference {
        version: 1,
        kind: "task".into(),
        id: shipped.id.clone(),
        account: None,
        calendar: None,
        note: None,
        contact: None,
        series: None,
    }
    .href()
    .unwrap();
    // Planned in the previous quarter and twice in this one.
    for date in ["2026-09-30", "2026-10-01", "2026-11-10"] {
        db.save_daily_note(date, &format!("## Plan\n\n- [Write notes]({href})\n"))
            .unwrap();
    }

    let goals = page(&db, PlanSource::Goals, Some(ME), None);
    use PlanCandidateReason::*;
    assert_eq!(
        titles(&goals),
        [("Ship v2", Goal), ("Write notes", GoalTask), ("Grow", Goal)]
    );
    assert_eq!(goals.candidates[0].open_tasks, Some(1));
    assert_eq!(
        goals.candidates[1].planned_days,
        ["2026-10-01", "2026-11-10"]
    );
    assert_eq!(goals.candidates[1].goal.as_ref().unwrap().id, quarter.id);
    assert!(matches!(
        &goals.candidates[2].action,
        PlanCandidateAction::NewTaskForGoal { goal_id, .. } if *goal_id == year.id
    ));
    assert!(page(&db, PlanSource::Goals, Some(OTHER), None)
        .candidates
        .is_empty());
}

#[test]
fn notes_offer_open_round_tasks_including_the_current_day_newest_first() {
    let db = database();
    db.create_note("Older", "- ( ) Old chore\n- (x) Finished\n")
        .unwrap();
    db.connection()
        .unwrap()
        .execute(
            "UPDATE notes SET updated_at='2026-01-01T00:00:00Z' WHERE title='Older'",
            [],
        )
        .unwrap();
    db.create_note("Newer", "- ( ) New chore\n- ( ) Second chore\n")
        .unwrap();
    db.save_daily_note(DAY, "## Plan\n\n- ( ) In the plan itself\n\n## Meetings\n\n- ( ) Prep the discussion\n- [ ] Local checklist\n")
        .unwrap();

    let notes = page(&db, PlanSource::Notes, None, None);
    let shown: Vec<_> = notes
        .candidates
        .iter()
        .map(|c| (c.title.as_str(), c.detail.as_deref().unwrap()))
        .collect();
    assert_eq!(
        shown,
        [
            ("In the plan itself", DAY),
            ("Prep the discussion", DAY),
            ("New chore", "Newer"),
            ("Second chore", "Newer"),
            ("Old chore", "Older")
        ]
    );
    let chore = notes
        .candidates
        .iter()
        .find(|candidate| candidate.title == "New chore")
        .unwrap()
        .clone();
    assert!(chore.task_id.as_deref().unwrap().starts_with("note:"));
    assert_eq!(add(&db, &chore), chore.key);
    assert!(
        page(&db, PlanSource::Notes, None, None)
            .candidates
            .iter()
            .find(|candidate| candidate.title == "New chore")
            .unwrap()
            .planned
    );

    // Paging past a full page.
    let body: String = (0..PLAN_SOURCE_PAGE)
        .map(|n| format!("- ( ) Chore {n}\n"))
        .collect();
    db.create_note("Many", &body).unwrap();
    let first = page(&db, PlanSource::Notes, None, None);
    assert_eq!(first.candidates.len(), PLAN_SOURCE_PAGE);
    let second = page(&db, PlanSource::Notes, None, first.next.as_deref());
    assert_eq!(second.candidates.len(), 5);
    assert!(second.next.is_none());
}
