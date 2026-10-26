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
        crate::replicated_sync::ensure_space_and_device(tx)?;
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
        .with_transaction(crate::replicated_sync::ensure_space_and_device)
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
