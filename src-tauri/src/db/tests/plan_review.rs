use super::*;
use crate::db::test_support::database;

const DAY: &str = "2026-11-16";
const ME: &str = "me@example.com";

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

fn plan(db: &Database, date: &str, body: &str) {
    db.save_daily_note(date, body).unwrap().unwrap();
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
