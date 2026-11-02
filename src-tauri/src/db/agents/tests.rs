use super::*;
use crate::{db::test_support::TempDbPath, models::MailProviderKind};
fn setup() -> Database {
    let db = Database::open_memory();
    seed(&db);
    db
}
fn seed(db: &Database) {
    db.adopt_mail_account("you@example.com", MailProviderKind::Gmail)
        .unwrap();
    db.set_agent_runner_enabled(true, 0).unwrap();
}
fn create(db: &Database) -> AgentDetail {
    db.create_agent_task(
        &CreateAgentTask {
            account_id: "you@example.com".into(),
            objective: "Evaluate a fixture invitation".into(),
            source_thread_id: None,
            linked_task_id: None,
            workflow: WORKFLOW.into(),
        },
        0,
    )
    .unwrap()
}
fn step(db: &Database, now: i64) -> Lease {
    let lease = db.claim_agent_run("worker", now).unwrap().unwrap();
    assert!(db
        .finish_agent_step(&lease, fixture_step(lease.run.checkpoint), now)
        .unwrap());
    lease
}
#[test]
fn required_question_stops_dispatch_until_answered_once() {
    let db = setup();
    let d = create(&db);
    step(&db, 1);
    let q = db
        .get_agent_task(&d.task.id)
        .unwrap()
        .run
        .question_id
        .unwrap();
    step(&db, 2);
    let waiting = db.get_agent_task(&d.task.id).unwrap();
    assert_eq!(waiting.run.state, RunState::WaitingForInput);
    assert_eq!(waiting.run.dispatches, 2);
    for time in [3, 100, 10000] {
        assert!(db.claim_agent_run("worker", time).unwrap().is_none());
    }
    assert!(db
        .answer_agent_question(&d.task.id, "wrong", "Sales", 3)
        .is_err());
    let answered = db
        .answer_agent_question(&d.task.id, &q, "Sales", 3)
        .unwrap();
    assert_eq!(answered.run.state, RunState::Queued);
    let duplicate = db
        .answer_agent_question(&d.task.id, &q, "Sales", 3)
        .unwrap();
    assert_eq!(answered.turns.len(), duplicate.turns.len());
    assert!(db
        .answer_agent_question(&d.task.id, &q, "Different", 4)
        .is_err());
    step(&db, 4);
    let done = db.get_agent_task(&d.task.id).unwrap();
    assert_eq!(done.run.state, RunState::Completed);
    assert_eq!(done.artifacts.len(), 1);
    assert!(done.artifacts[0].not_researched);
    assert!(db.claim_agent_run("worker", 5).unwrap().is_none());
}
#[test]
fn early_answer_during_independent_work_does_not_invalidate_or_block_it() {
    let db = setup();
    let d = create(&db);
    step(&db, 1);
    let lease = db.claim_agent_run("worker", 2).unwrap().unwrap();
    db.answer_agent_question(
        &d.task.id,
        lease.run.question_id.as_ref().unwrap(),
        "Sales",
        2,
    )
    .unwrap();
    assert!(db
        .finish_agent_step(&lease, StepResult::EvidenceCollected, 2)
        .unwrap());
    assert_eq!(
        db.get_agent_task(&d.task.id).unwrap().run.state,
        RunState::Queued
    );
    step(&db, 3);
}
#[test]
fn restart_replays_only_uncommitted_steps_after_exact_lease_expiry() {
    let path = TempDbPath::new();
    let id;
    let stale;
    {
        let db = Database::open(&path.path).unwrap();
        seed(&db);
        id = create(&db).task.id;
        step(&db, 1);
        stale = db.claim_agent_run("old", 2).unwrap().unwrap();
    }
    let db = Database::open(&path.path).unwrap();
    assert!(db
        .claim_agent_run("new", 2 + LEASE_SECONDS - 1)
        .unwrap()
        .is_none());
    let recovered = db
        .claim_agent_run("new", 2 + LEASE_SECONDS)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.run.checkpoint, 1);
    assert_eq!(recovered.run.dispatches, 3);
    assert!(!db
        .finish_agent_step(&stale, StepResult::EvidenceCollected, 2 + LEASE_SECONDS)
        .unwrap());
    assert!(db
        .finish_agent_step(&recovered, StepResult::EvidenceCollected, 2 + LEASE_SECONDS)
        .unwrap());
    let d = db.get_agent_task(&id).unwrap();
    assert_eq!(
        d.turns.iter().filter(|t| t.question_id.is_some()).count(),
        1
    );
    let count: i64 = db
        .connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM agent_steps WHERE status='ambiguous'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
#[test]
fn cancellation_deletion_pause_and_disable_reject_late_commits() {
    for action in ["cancel", "delete", "pause", "disable"] {
        let db = setup();
        let d = create(&db);
        let lease = db.claim_agent_run("worker", 1).unwrap().unwrap();
        match action {
            "delete" => db.delete_agent_task(&d.task.id).unwrap(),
            "disable" => db.set_agent_runner_enabled(false, 2).unwrap(),
            _ => {
                db.control_agent_task(&d.task.id, action, 2).unwrap();
            }
        }
        assert!(!db
            .finish_agent_step(&lease, StepResult::Prepared, 3)
            .unwrap());
        if action == "delete" {
            assert!(db.get_agent_task(&d.task.id).is_err());
        } else {
            assert_eq!(db.get_agent_task(&d.task.id).unwrap().turns.len(), 1);
        }
        if action == "disable" {
            db.set_agent_runner_enabled(true, 4).unwrap();
            assert!(db.claim_agent_run("worker", 4).unwrap().is_none());
            db.control_agent_task(&d.task.id, "resume", 4).unwrap();
            assert!(db.claim_agent_run("worker", 4).unwrap().is_some());
        }
    }
}
#[test]
fn retries_are_bounded_backoff_is_durable_and_explicit_retry_preserves_total_allowance() {
    let db = setup();
    let d = create(&db);
    for (time, next) in [(0, 2), (2, 6), (6, 14)] {
        let lease = db.claim_agent_run("worker", time).unwrap().unwrap();
        db.finish_agent_step(&lease, StepResult::TransientFailure, time)
            .unwrap();
        assert!(db.claim_agent_run("worker", next - 1).unwrap().is_none());
    }
    let failed = db.get_agent_task(&d.task.id).unwrap();
    assert_eq!(failed.run.state, RunState::Failed);
    assert_eq!(failed.run.dispatches, 3);
    db.control_agent_task(&d.task.id, "retry", 15).unwrap();
    let lease = db.claim_agent_run("worker", 15).unwrap().unwrap();
    assert_eq!(lease.run.dispatches, 4);
    assert_eq!(lease.run.checkpoint, 0);
    assert!(db
        .finish_agent_step(&lease, StepResult::Prepared, 15)
        .unwrap());
}
#[test]
fn global_concurrency_and_one_lease_per_job_are_enforced_in_database() {
    let db = setup();
    for _ in 0..3 {
        create(&db);
    }
    let a = db.claim_agent_run("a", 0).unwrap().unwrap();
    let b = db.claim_agent_run("b", 0).unwrap().unwrap();
    assert_ne!(a.run.task_id, b.run.task_id);
    assert!(db.claim_agent_run("c", 0).unwrap().is_none());
    db.finish_agent_step(&a, StepResult::Prepared, 1).unwrap();
    assert!(db.claim_agent_run("c", 1).unwrap().is_some());
}
#[test]
fn budget_boundaries_do_not_silently_reset_on_resume() {
    for (used, can_dispatch) in [
        (MAX_DISPATCHES - 1, true),
        (MAX_DISPATCHES, false),
        (MAX_DISPATCHES + 1, false),
    ] {
        let db = setup();
        let d = create(&db);
        db.with_connection(|c| {
            let mut r = run(c, &d.task.id)?;
            r.dispatches = used;
            save_run(c, &r)
        })
        .unwrap();
        assert_eq!(
            db.claim_agent_run("worker", 0).unwrap().is_some(),
            can_dispatch
        );
        if !can_dispatch {
            assert!(db.control_agent_task(&d.task.id, "resume", 0).is_err());
        }
    }
    for (used, can_dispatch) in [
        (MAX_ACTIVE_SECONDS - 1, true),
        (MAX_ACTIVE_SECONDS, false),
        (MAX_ACTIVE_SECONDS + 1, false),
    ] {
        let db = setup();
        let d = create(&db);
        db.with_connection(|c| {
            let mut r = run(c, &d.task.id)?;
            r.active_seconds = used;
            save_run(c, &r)
        })
        .unwrap();
        assert_eq!(
            db.claim_agent_run("worker", 0).unwrap().is_some(),
            can_dispatch
        );
    }
}
#[test]
fn text_validation_and_account_links_are_checked_before_insertion() {
    let db = setup();
    let request = CreateAgentTask {
        account_id: "you@example.com".into(),
        objective: "x".into(),
        source_thread_id: None,
        linked_task_id: None,
        workflow: WORKFLOW.into(),
    };
    for n in [MAX_OBJECTIVE - 1, MAX_OBJECTIVE] {
        db.create_agent_task(
            &CreateAgentTask {
                objective: "x".repeat(n),
                ..request.clone()
            },
            0,
        )
        .unwrap();
    }
    assert!(db
        .create_agent_task(
            &CreateAgentTask {
                objective: "x".repeat(MAX_OBJECTIVE + 1),
                ..request.clone()
            },
            0
        )
        .is_err());
    for patch in [
        CreateAgentTask {
            account_id: "other@example.com".into(),
            ..request.clone()
        },
        CreateAgentTask {
            workflow: "research_live".into(),
            ..request.clone()
        },
        CreateAgentTask {
            source_thread_id: Some("missing".into()),
            ..request.clone()
        },
        CreateAgentTask {
            linked_task_id: Some("missing".into()),
            ..request
        },
    ] {
        assert!(db.create_agent_task(&patch, 0).is_err());
    }
}
#[test]
fn account_removal_cascades_agent_data_and_invalidates_workers() {
    let db = setup();
    let d = create(&db);
    let lease = db.claim_agent_run("worker", 0).unwrap().unwrap();
    db.remove_account("you@example.com").unwrap();
    assert!(db.get_agent_task(&d.task.id).is_err());
    assert!(!db
        .finish_agent_step(&lease, StepResult::Prepared, 1)
        .unwrap());
    let count: i64 = db
        .connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM agent_steps", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
#[test]
fn v94_upgrade_preserves_accounts_and_adds_disabled_local_runner() {
    let path = TempDbPath::new();
    {
        let db = Database::open(&path.path).unwrap();
        db.adopt_mail_account("you@example.com", MailProviderKind::Gmail)
            .unwrap();
        db.connection().unwrap().execute_batch("DROP TABLE agent_artifacts; DROP TABLE agent_steps; DROP TABLE agent_turns; DROP TABLE agent_runs; DROP TABLE agent_tasks; PRAGMA user_version=94;").unwrap();
    }
    let db = Database::open(&path.path).unwrap();
    assert_eq!(db.list_accounts().unwrap().len(), 1);
    assert!(!db.agent_runner_enabled().unwrap());
    assert!(db.list_agent_tasks(None, true).unwrap().is_empty());
}

#[test]
fn task_links_stay_local_and_research_never_completes_personal_tasks() {
    let db = setup();
    db.adopt_mail_account("other@example.com", MailProviderKind::Gmail)
        .unwrap();
    let request:crate::models::CreateTaskRequest=serde_json::from_value(serde_json::json!({"accountId":"you@example.com","title":"Decide whether to attend","kind":"action"})).unwrap();
    let personal = db.create_task(&request).unwrap();
    let own = CreateAgentTask {
        account_id: "you@example.com".into(),
        objective: "Fixture".into(),
        source_thread_id: None,
        linked_task_id: Some(personal.id.clone()),
        workflow: WORKFLOW.into(),
    };
    assert!(db
        .create_agent_task(
            &CreateAgentTask {
                account_id: "other@example.com".into(),
                ..own.clone()
            },
            0
        )
        .is_err());
    let d = db.create_agent_task(&own, 0).unwrap();
    step(&db, 1);
    step(&db, 2);
    let q = db
        .get_agent_task(&d.task.id)
        .unwrap()
        .run
        .question_id
        .unwrap();
    db.answer_agent_question(&d.task.id, &q, "Sales", 3)
        .unwrap();
    step(&db, 4);
    assert_eq!(
        db.list_tasks(Some("you@example.com"), None).unwrap()[0].status,
        "open"
    );
    db.connection()
        .unwrap()
        .execute("DELETE FROM tasks WHERE id=?1", [personal.id])
        .unwrap();
    assert!(db
        .get_agent_task(&d.task.id)
        .unwrap()
        .task
        .linked_task_id
        .is_none());
    assert_eq!(
        db.list_agent_tasks(Some("other@example.com"), true)
            .unwrap()
            .len(),
        0
    );
    db.control_agent_task(&d.task.id, "archive", 4).unwrap();
    assert!(db.list_agent_tasks(None, false).unwrap().is_empty());
    assert_eq!(db.list_agent_tasks(None, true).unwrap().len(), 1);
    assert!(
        !db.control_agent_task(&d.task.id, "mark_read", 4)
            .unwrap()
            .task
            .unread
    );
    db.disconnect_account_locally("you@example.com").unwrap();
    assert!(db.get_agent_task(&d.task.id).is_err());
}
#[test]
fn invalid_publication_rolls_back_and_a_committed_step_cannot_be_replayed() {
    let db = setup();
    let d = create(&db);
    let lease = db.claim_agent_run("worker", 0).unwrap().unwrap();
    assert!(db
        .finish_agent_step(&lease, StepResult::Publish, 0)
        .is_err());
    assert!(db.get_agent_task(&d.task.id).unwrap().artifacts.is_empty());
    assert!(db
        .finish_agent_step(&lease, StepResult::Prepared, 0)
        .unwrap());
    assert!(!db
        .finish_agent_step(&lease, StepResult::Prepared, 0)
        .unwrap());
    let q = db
        .get_agent_task(&d.task.id)
        .unwrap()
        .run
        .question_id
        .unwrap();
    assert!(db
        .answer_agent_question(&d.task.id, &q, &"x".repeat(MAX_ANSWER + 1), 0)
        .is_err());
    db.answer_agent_question(&d.task.id, &q, &"x".repeat(MAX_ANSWER), 0)
        .unwrap();
    assert_eq!(
        db.get_agent_task(&d.task.id)
            .unwrap()
            .turns
            .iter()
            .filter(|t| t.answer_to.is_some())
            .count(),
        1
    );
}

#[test]
fn mismatched_run_or_checkpoint_cannot_commit_even_with_a_matching_generation() {
    let db = setup();
    let d = create(&db);
    let lease = db.claim_agent_run("worker", 0).unwrap().unwrap();
    let mut wrong_run = lease.clone();
    wrong_run.run.id = Uuid::new_v4().to_string();
    assert!(!db
        .finish_agent_step(&wrong_run, StepResult::Prepared, 0)
        .unwrap());
    let mut wrong_checkpoint = lease.clone();
    wrong_checkpoint.run.checkpoint += 1;
    assert!(!db
        .finish_agent_step(&wrong_checkpoint, StepResult::Prepared, 0)
        .unwrap());
    assert_eq!(db.get_agent_task(&d.task.id).unwrap().turns.len(), 1);
    assert!(db
        .finish_agent_step(&lease, StepResult::Prepared, 0)
        .unwrap());
}

#[test]
fn a_lease_reserves_only_remaining_active_time_and_cannot_publish_at_its_deadline() {
    let db = setup();
    let d = create(&db);
    db.with_connection(|c| {
        let mut r = run(c, &d.task.id)?;
        r.active_seconds = MAX_ACTIVE_SECONDS - 1;
        save_run(c, &r)
    })
    .unwrap();
    let lease = db.claim_agent_run("worker", 0).unwrap().unwrap();
    assert_eq!(lease.run.lease_until, Some(1));
    assert!(!db
        .finish_agent_step(&lease, StepResult::Prepared, 1)
        .unwrap());
    assert!(db
        .claim_agent_run("worker", LEASE_SECONDS + 1)
        .unwrap()
        .is_none());
    let d = db.get_agent_task(&d.task.id).unwrap();
    assert_eq!(d.run.active_seconds, MAX_ACTIVE_SECONDS);
    assert_eq!(d.run.state, RunState::Paused);
    assert_eq!(d.run.reason.as_deref(), Some("budget_exhausted"));
    assert_eq!(d.turns.len(), 1);
}

#[test]
fn discussion_preserves_report_and_explicit_revision_adds_an_immutable_version() {
    let db = setup();
    let d = create(&db);
    step(&db, 1);
    step(&db, 2);
    let waiting = db.get_agent_task(&d.task.id).unwrap();
    db.answer_agent_question(
        &d.task.id,
        waiting.run.question_id.as_ref().unwrap(),
        "Sales",
        3,
    )
    .unwrap();
    step(&db, 4);
    let before = db.get_agent_task(&d.task.id).unwrap();
    let first = before.artifacts[0].body.clone();
    let chat = db
        .interact_agent_task(&d.task.id, "Explain", "discuss", 5)
        .unwrap();
    assert_eq!(chat.artifacts[0].body, first);
    assert_eq!(chat.artifacts.len(), 1);
    let queued = db
        .interact_agent_task(&d.task.id, "Focus on customers", "revise", 6)
        .unwrap();
    assert_eq!(queued.run.dispatches, before.run.dispatches);
    assert_eq!(queued.task.revision, 2);
    assert_eq!(queued.artifacts[0].body, first);
    step(&db, 7);
    let done = db.get_agent_task(&d.task.id).unwrap();
    assert_eq!(done.artifacts.len(), 2);
    assert_eq!(done.artifacts[0].body, first);
    assert_eq!(done.artifacts[1].version, 2);
    assert!(done.artifacts[1].body.contains("Focus on customers"));
    let stale = db
        .mark_agent_task_read(
            &d.task.id,
            before.run.generation,
            before.turns.len(),
            before.artifacts.len(),
        )
        .unwrap();
    assert!(stale.task.unread);
    let read = db
        .mark_agent_task_read(
            &d.task.id,
            done.run.generation,
            done.turns.len(),
            done.artifacts.len(),
        )
        .unwrap();
    assert!(!read.task.unread);
}

#[test]
fn linking_validates_account_and_disabled_revisions_leave_existing_artifacts_intact() {
    let db = setup();
    let d = create(&db);
    step(&db, 1);
    step(&db, 2);
    let q = db
        .get_agent_task(&d.task.id)
        .unwrap()
        .run
        .question_id
        .unwrap();
    db.answer_agent_question(&d.task.id, &q, "Sales", 3)
        .unwrap();
    step(&db, 4);
    assert!(db.link_agent_task(&d.task.id, Some("missing")).is_err());
    db.set_agent_runner_enabled(false, 5).unwrap();
    assert!(db
        .interact_agent_task(&d.task.id, "Revise", "revise", 6)
        .is_err());
    let done = db
        .interact_agent_task(&d.task.id, "Explain saved result", "discuss", 6)
        .unwrap();
    assert_eq!(done.artifacts.len(), 1);
    assert_eq!(done.task.revision, 1);
}

#[test]
fn chat_is_bounded_and_link_updates_do_not_mutate_personal_tasks() {
    let db = setup();
    let d = create(&db);
    step(&db, 1);
    step(&db, 2);
    let q = db
        .get_agent_task(&d.task.id)
        .unwrap()
        .run
        .question_id
        .unwrap();
    db.answer_agent_question(&d.task.id, &q, "Sales", 3)
        .unwrap();
    step(&db, 4);
    db.adopt_mail_account("other@example.com", crate::models::MailProviderKind::Gmail)
        .unwrap();
    let make = |account: &str| {
        db.create_task(
            &serde_json::from_value(
                serde_json::json!({"accountId":account,"title":"Decision","kind":"action"}),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let own = make("you@example.com");
    let other = make("other@example.com");
    assert!(db.link_agent_task(&d.task.id, Some(&other.id)).is_err());
    assert_eq!(
        db.link_agent_task(&d.task.id, Some(&own.id))
            .unwrap()
            .task
            .linked_task_id
            .as_deref(),
        Some(own.id.as_str())
    );
    assert!(db
        .link_agent_task(&d.task.id, None)
        .unwrap()
        .task
        .linked_task_id
        .is_none());
    assert_eq!(
        db.list_tasks(Some("you@example.com"), None).unwrap()[0].status,
        "open"
    );
    let initial = db.get_agent_task(&d.task.id).unwrap();
    for _ in 0..(MAX_TURNS - initial.turns.len() as i64) / 2 {
        db.interact_agent_task(&d.task.id, "Explain", "discuss", 5)
            .unwrap();
    }
    let final_state = db.get_agent_task(&d.task.id).unwrap();
    assert_eq!(final_state.turns.len() as i64, MAX_TURNS);
    assert!(db
        .interact_agent_task(&d.task.id, "Explain", "discuss", 5)
        .is_err());
    assert_eq!(
        db.get_agent_task(&d.task.id).unwrap().turns.len(),
        final_state.turns.len()
    );
    assert_eq!(final_state.run.dispatches, initial.run.dispatches);
}
