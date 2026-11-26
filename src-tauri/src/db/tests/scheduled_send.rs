use super::*;
use crate::correspondence::OutboxItem;
use std::path::Path;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
fn db() -> Database {
    let db = Database::open_memory();
    db.adopt_account("me@example.com").unwrap();
    db.set_compose_identity("me@example.com").unwrap();
    db
}
fn queue(db: &Database) -> OutboxItem {
    let mut draft = db.create_draft("new", None, "me@example.com", &[]).unwrap();
    draft.to = "to@example.com".into();
    draft.bcc = "hidden@example.com".into();
    draft.subject = "Summary only".into();
    draft.body = "secret body".into();
    let d = db.save_draft(draft).unwrap();
    db.schedule_draft(
        &d.id,
        d.revision,
        &Selection {
            local_time: "2030-01-01T09:00".into(),
            time_zone: "UTC".into(),
            offset_seconds: None,
        },
        OWNER,
        &[crate::provider::SendingIdentity {
            address: "me@example.com".into(),
            display_name: None,
            ..Default::default()
        }],
        None,
        Path::new("/unused"),
        1_800_000_000_000,
    )
    .unwrap()
}
fn group_setup(db: &Database, key: u8) {
    db.set_beta_features_enabled(true).unwrap();
    db.with_transaction(|tx| {
        crate::db::replicated_sync::ensure_space_and_device(tx)?;
        tx.execute(
            "UPDATE sync_spaces SET recovery_public_key=?1,active_epoch=1",
            [vec![key; 32]],
        )?;
        Ok(())
    })
    .unwrap();
}
#[test]
fn snapshots_replicate_only_metadata_and_never_create_sendable_rows() {
    let owner = db();
    group_setup(&owner, 7);
    let item = queue(&owner);
    let identity = owner
        .with_transaction(crate::db::replicated_sync::ensure_space_and_device)
        .unwrap();
    let (snapshot, _) = owner
        .take_local_snapshot(
            threestrands_sync_envelope::DeviceId::from_bytes(identity),
            1,
            item.deadline - 1,
        )
        .unwrap();
    let bytes = serde_json::to_string(&snapshot).unwrap();
    for secret in [
        "secret body",
        "hidden@example.com",
        "to@example.com",
        "raw",
        "attachments",
    ] {
        assert!(!bytes.contains(secret), "leaked {secret}");
    }
    let peer = db();
    let state = crate::db::sync_state::snapshot_to_state(&snapshot).unwrap();
    peer.merge_replica_state(&state).unwrap();
    let report = &item.schedule.as_ref().unwrap().report;
    peer.materialize_entity(
        EntityType::ScheduledSendSummary,
        &report.entity_id(),
        Some(&serde_json::json!({"report":report})),
        false,
    )
    .unwrap();
    assert!(peer.outbox().unwrap().is_empty());
    assert!(peer.drafts().unwrap().is_empty());
    assert_eq!(peer.scheduled_summaries().unwrap().len(), 1);
    assert!(peer
        .materialize_entity(
            EntityType::ScheduledSendSummary,
            "different-id",
            Some(&serde_json::json!({"report":report})),
            false
        )
        .is_err());
    owner
        .manage_schedule(
            &item.id,
            1,
            Some(&Selection {
                local_time: "2030-01-02T09:00".into(),
                time_zone: "UTC".into(),
                offset_seconds: None,
            }),
            false,
            OWNER,
            item.deadline - 1,
        )
        .unwrap();
    let (newer, _) = owner
        .take_local_snapshot(
            threestrands_sync_envelope::DeviceId::from_bytes(identity),
            2,
            item.deadline - 1,
        )
        .unwrap();
    peer.merge_replica_state(&crate::db::sync_state::snapshot_to_state(&newer).unwrap())
        .unwrap();
    peer.merge_replica_state(&state).unwrap();
    let value = peer
        .resolve_field_winner(
            EntityType::ScheduledSendSummary,
            &report.entity_id(),
            "report",
        )
        .unwrap()
        .unwrap();
    assert!(value["reportRevision"].as_i64().unwrap() > 1);
    assert!(peer.outbox().unwrap().is_empty());
}
#[test]
fn disabled_sync_backlog_and_group_changes_do_not_republish_old_jobs() {
    let owner = db();
    group_setup(&owner, 7);
    let item = queue(&owner);
    let entity = item.schedule.as_ref().unwrap().report.entity_id();
    owner.set_beta_features_enabled(false).unwrap();
    owner
        .overdue_schedules_owned(item.deadline + 60_001, false, OWNER)
        .unwrap();
    assert_eq!(
        owner
            .resolve_field_winner(EntityType::ScheduledSendSummary, &entity, "report")
            .unwrap()
            .unwrap()["state"],
        "scheduled"
    );
    owner.set_beta_features_enabled(true).unwrap();
    owner
        .refresh_schedule_reports(OWNER, item.deadline + 60_002)
        .unwrap();
    assert_eq!(
        owner
            .resolve_field_winner(EntityType::ScheduledSendSummary, &entity, "report")
            .unwrap()
            .unwrap()["state"],
        "overdue"
    );
    owner.leave_sync_space().unwrap();
    group_setup(&owner, 8);
    owner
        .refresh_schedule_reports(OWNER, item.deadline + 60_003)
        .unwrap();
    assert!(owner
        .resolve_field_winner(EntityType::ScheduledSendSummary, &entity, "report")
        .unwrap()
        .is_none());
    assert_eq!(
        owner.outbox().unwrap()[0]
            .schedule
            .as_ref()
            .unwrap()
            .report
            .owner_installation_id,
        OWNER
    );
}
#[test]
fn restart_preserves_future_time_and_never_releases_elapsed_schedules() {
    let db = db();
    let item = queue(&db);
    crate::schema::migrate(&mut db.connection().unwrap()).unwrap();
    assert_eq!(db.outbox().unwrap()[0].deadline, item.deadline);
    db.overdue_schedules(item.deadline + 1, true).unwrap();
    assert_eq!(db.outbox().unwrap()[0].state, "overdue");
    assert!(!db.pending_undo().unwrap());
}

#[test]
fn switching_a_connection_cancels_only_owned_local_schedules_even_if_sync_is_paused() {
    let database = db();
    group_setup(&database, 7);
    let item = queue(&database);
    let report = &item.schedule.as_ref().unwrap().report;
    let mut peer_report = report.clone();
    peer_report.owner_installation_id = "00000000-0000-4000-8000-000000000002".into();
    database
        .upsert_scheduled_summary(
            &peer_report.entity_id(),
            &serde_json::to_string(&peer_report).unwrap(),
        )
        .unwrap();
    database.set_beta_features_enabled(false).unwrap();

    database
        .with_transaction(|tx| {
            cancel_schedules_for_connection_switch(tx, "another@example.com", OWNER)?;
            cancel_schedules_for_connection_switch(
                tx,
                "me@example.com",
                &peer_report.owner_installation_id,
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(database.outbox().unwrap()[0].state, "scheduled");

    database
        .with_transaction(|tx| cancel_schedules_for_connection_switch(tx, "me@example.com", OWNER))
        .unwrap();
    assert_eq!(database.outbox().unwrap()[0].state, "canceled");
    let replica = database.load_replica_state().unwrap();
    let (_, values) = replica
        .fields()
        .iter()
        .find(|(key, _)| {
            key.entity_type == EntityType::ScheduledSendSummary
                && key.field == "report"
                && key.entity_id == report.entity_id()
        })
        .unwrap();
    assert_eq!(values[0].value.as_ref().unwrap()["state"], "canceled");
    assert_eq!(
        database.scheduled_summaries().unwrap()[0]["state"],
        "scheduled"
    );
}

// A message that leaves finishes the task it was linked to, once, in the
// transaction that makes the row `sent`.
mod linked_tasks {
    use super::*;
    use crate::models::CreateTaskRequest;

    const AT: i64 = 1_900_000_000_000;

    fn task(db: &Database, kind: &str, repeat: Option<i64>) -> crate::models::ThreadTask {
        db.create_task(&CreateTaskRequest {
            account_id: "me@example.com".into(),
            thread_id: None,
            source_message_id: None,
            subject_snapshot: None,
            title: "Send the contract".into(),
            notes: None,
            kind: kind.into(),
            due_kind: if repeat.is_some() { "date" } else { "none" }.into(),
            due_value: repeat.map(|_| "2026-12-01".into()),
            time_zone: None,
            repeat_interval_days: repeat,
            evidence_text: None,
            goal_id: None,
        })
        .unwrap()
    }
    fn get(db: &Database, id: &str) -> crate::models::ThreadTask {
        db.list_tasks(None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
    }
    fn draft_linked(
        db: &Database,
        completes: Option<&str>,
        follow_up: Option<&str>,
    ) -> crate::correspondence::Draft {
        let mut draft = db.create_draft("new", None, "me@example.com", &[]).unwrap();
        draft.to = "to@example.com".into();
        draft.subject = "The contract".into();
        draft.completes_task_id = completes.map(Into::into);
        draft.follow_up_task_id = follow_up.map(Into::into);
        db.save_draft(draft).unwrap()
    }
    /// An immediate send past its undo window, ready to deliver.
    fn immediate(db: &Database, completes: Option<&str>, follow_up: Option<&str>) -> String {
        let draft = draft_linked(db, completes, follow_up);
        let id = format!("send-{}", draft.id);
        let revision = draft.revision;
        db.insert_queued_send(
            &OutboxItem {
                id: id.clone(),
                draft,
                state: "ready".into(),
                deadline: 0,
                error: None,
                provider_id: None,
                last_attempt_at: None,
                archive_on_send: false,
                schedule: None,
                sent_copy: None,
            },
            revision,
            b"raw",
        )
        .unwrap();
        id
    }
    fn deliver(db: &Database, id: &str) {
        assert_eq!(db.claim_due_send(id, AT, 0, None, "").unwrap(), 1);
        db.record_send_accepted(id, "provider-1", None, "", AT)
            .unwrap();
    }
    fn scheduled(db: &Database, completes: Option<&str>, follow_up: Option<&str>) -> OutboxItem {
        let draft = draft_linked(db, completes, follow_up);
        db.schedule_draft(
            &draft.id,
            draft.revision,
            &Selection {
                local_time: "2030-01-01T09:00".into(),
                time_zone: "UTC".into(),
                offset_seconds: None,
            },
            OWNER,
            &[crate::provider::SendingIdentity {
                address: "me@example.com".into(),
                display_name: None,
                ..Default::default()
            }],
            None,
            Path::new("/unused"),
            1_800_000_000_000,
        )
        .unwrap()
    }
    fn deliver_scheduled(db: &Database, item: &OutboxItem) {
        let revision = item.schedule.as_ref().unwrap().report.report_revision;
        assert_eq!(
            db.claim_due_send(&item.id, item.deadline, 0, Some(revision), OWNER)
                .unwrap(),
            1
        );
        db.record_send_accepted(&item.id, "provider-2", None, OWNER, item.deadline)
            .unwrap();
    }

    #[test]
    fn an_immediate_send_completes_the_linked_task_with_a_record() {
        let db = db();
        let one_off = task(&db, "action", None);
        db.place_task_on_board(&one_off.id, "open", std::slice::from_ref(&one_off.id))
            .unwrap();
        let id = immediate(&db, Some(&one_off.id), None);
        assert_eq!(get(&db, &one_off.id).status, "open", "queued is not sent");
        deliver(&db, &id);
        let done = get(&db, &one_off.id);
        assert_eq!(done.status, "completed");
        assert_eq!(done.board_rank, None, "Done leaves the board column");
        assert_eq!(done.completion_source.as_deref(), Some("user"));
        assert_eq!(done.thread_id, None);
        assert_eq!(
            done.completed_by_email,
            Some(crate::models::CompletedByEmail {
                account_id: "me@example.com".into(),
                subject: "The contract".into(),
                sent_at: AT,
                message_id: Some("provider-1".into()),
                thread_id: None,
                recipients: vec!["to@example.com".into()],
            })
        );
    }

    #[test]
    fn a_scheduled_send_completes_the_linked_task_when_it_leaves() {
        let db = db();
        let one_off = task(&db, "action", None);
        let item = scheduled(&db, Some(&one_off.id), None);
        assert_eq!(get(&db, &one_off.id).status, "open");
        deliver_scheduled(&db, &item);
        let done = get(&db, &one_off.id);
        assert_eq!(done.status, "completed");
        assert_eq!(
            done.completed_by_email.unwrap().message_id.as_deref(),
            Some("provider-2")
        );
    }

    #[test]
    fn a_repeating_follow_up_advances_instead_of_completing() {
        let db = db();
        let repeating = task(&db, "follow_up", Some(7));
        let id = immediate(&db, Some(&repeating.id), None);
        deliver(&db, &id);
        let next = get(&db, &repeating.id);
        assert_eq!(next.status, "open");
        assert_eq!(next.due_value.as_deref(), Some("2026-12-08"));
        assert_eq!(next.completed_by_email, None);
    }

    #[test]
    fn a_follow_up_link_records_once_for_immediate_and_scheduled_sends() {
        let db = db();
        let repeating = task(&db, "follow_up", Some(7));
        let id = immediate(&db, None, Some(&repeating.id));
        deliver(&db, &id);
        assert_eq!(
            get(&db, &repeating.id).due_value.as_deref(),
            Some("2026-12-08")
        );
        // The row is already sent: accepting it again changes nothing.
        db.record_send_accepted(&id, "provider-1", None, "", AT)
            .unwrap();
        assert_eq!(
            get(&db, &repeating.id).due_value.as_deref(),
            Some("2026-12-08")
        );

        let item = scheduled(&db, None, Some(&repeating.id));
        deliver_scheduled(&db, &item);
        assert_eq!(
            get(&db, &repeating.id).due_value.as_deref(),
            Some("2026-12-15")
        );
        db.record_send_accepted(&item.id, "provider-2", None, OWNER, item.deadline)
            .unwrap();
        assert_eq!(
            get(&db, &repeating.id).due_value.as_deref(),
            Some("2026-12-15")
        );
    }

    #[test]
    fn linking_one_repeating_task_both_ways_advances_it_once() {
        let db = db();
        let repeating = task(&db, "follow_up", Some(7));
        let id = immediate(&db, Some(&repeating.id), Some(&repeating.id));
        deliver(&db, &id);
        assert_eq!(
            get(&db, &repeating.id).due_value.as_deref(),
            Some("2026-12-08")
        );
    }

    #[test]
    fn an_uncertain_send_found_in_sent_mail_finishes_the_task_once() {
        let db = db();
        let one_off = task(&db, "action", None);
        let id = immediate(&db, Some(&one_off.id), None);
        assert_eq!(db.claim_due_send(&id, AT, 0, None, "").unwrap(), 1);
        db.record_send_failed(&id, false, "timed out", "", AT)
            .unwrap();
        assert_eq!(
            get(&db, &one_off.id).status,
            "open",
            "an unknown outcome finishes nothing"
        );
        db.mark_uncertain_send_sent(&id, "found-1", "", AT + 5)
            .unwrap();
        let done = get(&db, &one_off.id);
        assert_eq!(done.status, "completed");
        assert_eq!(
            done.completed_by_email
                .as_ref()
                .unwrap()
                .message_id
                .as_deref(),
            Some("found-1")
        );
        assert_eq!(done.completed_by_email.unwrap().sent_at, AT + 5);
    }

    #[test]
    fn a_failed_or_undone_send_changes_nothing() {
        let db = db();
        let failing = task(&db, "action", None);
        let failed = immediate(&db, Some(&failing.id), None);
        assert_eq!(db.claim_due_send(&failed, AT, 0, None, "").unwrap(), 1);
        db.record_send_failed(&failed, true, "rejected", "", AT)
            .unwrap();
        assert_eq!(get(&db, &failing.id).status, "open");

        let undone_task = task(&db, "action", None);
        let undone = immediate(&db, Some(&undone_task.id), None);
        db.cancel_send(&undone, false).unwrap();
        assert_eq!(get(&db, &undone_task.id).status, "open");

        let cancelled_task = task(&db, "action", None);
        let item = scheduled(&db, Some(&cancelled_task.id), None);
        db.cancel_send(&item.id, false).unwrap();
        assert_eq!(get(&db, &cancelled_task.id).status, "open");
    }

    #[test]
    fn a_finished_or_missing_task_is_left_alone_and_the_send_still_succeeds() {
        let db = db();
        let finished = task(&db, "action", None);
        db.set_task_status(&finished.id, "cancelled", "user")
            .unwrap();
        let before = get(&db, &finished.id);
        let id = immediate(&db, Some(&finished.id), None);
        deliver(&db, &id);
        assert_eq!(db.outbox().unwrap()[0].state, "sent");
        let after = get(&db, &finished.id);
        assert_eq!(
            (after.status.as_str(), after.updated_at.as_str()),
            ("cancelled", before.updated_at.as_str())
        );
        assert_eq!(after.completed_by_email, None);

        let missing = immediate(&db, Some("no-such-task"), Some("also-missing"));
        deliver(&db, &missing);
        assert!(db.outbox().unwrap().iter().all(|item| item.state == "sent"));
    }

    #[test]
    fn reopening_a_task_clears_the_record() {
        let db = db();
        let one_off = task(&db, "action", None);
        let id = immediate(&db, Some(&one_off.id), None);
        deliver(&db, &id);
        assert!(get(&db, &one_off.id).completed_by_email.is_some());
        let reopened = db.set_task_status(&one_off.id, "open", "user").unwrap();
        assert_eq!(reopened.completed_by_email, None);
        assert_eq!(get(&db, &one_off.id).completed_by_email, None);
    }

    #[test]
    fn the_changed_fields_replicate_when_sync_is_on() {
        let db = db();
        group_setup(&db, 9);
        let one_off = task(&db, "action", None);
        let id = immediate(&db, Some(&one_off.id), None);
        deliver(&db, &id);
        let replica = db.load_replica_state().unwrap();
        let recorded = |field: &str| {
            replica.fields().iter().any(|(key, _)| {
                key.entity_type == EntityType::Task
                    && key.entity_id == one_off.id
                    && key.field == field
            })
        };
        assert!(recorded("completedByEmail") && recorded("status") && recorded("completionSource"));
    }
}
