use std::collections::BTreeSet;
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde_bytes::ByteBuf;
use threestrands_sync_envelope::limits::MAX_EARLIER_EPOCH_KEYS;
use threestrands_sync_envelope::{
    compute_cid, decode_signed_enrollment_grant, decode_signed_key_rotation, encode_signed_enrollment_grant,
    encode_signed_key_rotation, generate_recovery_seed, protocol_marker_cid, seal_to_x25519, sign_enrollment_grant,
    try_open_sealed_box, verify_enrollment_grant, DeviceId as EnvelopeDeviceId, EnrollmentGrant, RequestId, RosterEntry,
    X25519PublicKey, PROTOCOL_MARKER,
};
use threestrands_sync_protocol::EntityType;
use threestrands_sync_transport::fake::FakeTransport;
use threestrands_sync_transport::{Cid as TransportCid, SyncTransport};

use super::crypto::*;
use super::join_codes::{find_invitation, verify_fetched_invitation, MAX_JOIN_CODE_HOURS, MIN_JOIN_CODE_HOURS};
use super::service::*;
use super::test_support::{local_keys_for, test_identity, FakeEpochKeyStore};
use super::*;
use crate::db::Database;
use crate::replicated_sync::{
    encode_id, pull_from_transports, push_local_state, random_id, x25519_public_bytes, DeviceIdentity, LocalKeys, SPACE_ID,
};

fn fake_transports(name: &str) -> Vec<Arc<dyn SyncTransport>> {
    let transport: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new(name));
    vec![transport]
}

#[test]
fn enrollment_status_serializes_all_variant_fields_in_camel_case() {
    let cases = [
        (EnrollmentStatus::NotStarted, serde_json::json!({ "state": "notStarted" })),
        (
            EnrollmentStatus::AwaitingGrant {
                request_id: "request-1".to_string(),
                fingerprint: "AAAA-BBBB".to_string(),
                created_at: "2026-09-22T19:40:01Z".to_string(),
            },
            serde_json::json!({
                "state": "awaitingGrant",
                "requestId": "request-1",
                "fingerprint": "AAAA-BBBB",
                "createdAt": "2026-09-22T19:40:01Z",
            }),
        ),
        (
            EnrollmentStatus::AwaitingConfirmation {
                request_id: "request-2".to_string(),
                fingerprint: "CCCC-DDDD".to_string(),
                approver_fingerprint: "EEEE-FFFF".to_string(),
            },
            serde_json::json!({
                "state": "awaitingConfirmation",
                "requestId": "request-2",
                "fingerprint": "CCCC-DDDD",
                "approverFingerprint": "EEEE-FFFF",
            }),
        ),
        (
            EnrollmentStatus::Rejected { request_id: "request-3".to_string(), fingerprint: "AAAA-CCCC".to_string() },
            serde_json::json!({ "state": "rejected", "requestId": "request-3", "fingerprint": "AAAA-CCCC" }),
        ),
        (
            EnrollmentStatus::Enrolled { device_count: 2, awaiting_admission_from: None, history_handoff_warning: None },
            serde_json::json!({ "state": "enrolled", "deviceCount": 2, "awaitingAdmissionFrom": null }),
        ),
        (
            EnrollmentStatus::Enrolled { device_count: 1, awaiting_admission_from: Some("Work laptop".to_string()), history_handoff_warning: None },
            serde_json::json!({ "state": "enrolled", "deviceCount": 1, "awaitingAdmissionFrom": "Work laptop" }),
        ),
        (
            EnrollmentStatus::Enrolled {
                device_count: 1,
                awaiting_admission_from: None,
                history_handoff_warning: Some(HistoryHandoffWarning { used: 820, limit: MAX_EARLIER_EPOCH_KEYS }),
            },
            serde_json::json!({
                "state": "enrolled",
                "deviceCount": 1,
                "awaitingAdmissionFrom": null,
                "historyHandoffWarning": { "used": 820, "limit": MAX_EARLIER_EPOCH_KEYS }
            }),
        ),
    ];

    for (status, expected) in cases {
        assert_eq!(serde_json::to_value(status).unwrap(), expected);
    }
}

#[test]
fn beta_toggle_defaults_off_and_persists() {
    let database = Database::open_memory();
    assert!(!database.beta_features_enabled().unwrap());
    database.set_beta_features_enabled(true).unwrap();
    assert!(database.beta_features_enabled().unwrap());
    database.set_beta_features_enabled(false).unwrap();
    assert!(!database.beta_features_enabled().unwrap());
}

#[tokio::test]
async fn enrollment_status_progresses_from_not_started_to_enrolled_after_genesis() {
    let database = Database::open_memory();
    assert!(matches!(database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));

    let identity = test_identity(&database);
    let epoch_keys = FakeEpochKeyStore::default();
    let transports = fake_transports("genesis");
    let phrase = begin_genesis(&database, &identity, &epoch_keys, &transports, false).await.unwrap();
    assert_eq!(phrase.split_whitespace().count(), 24);

    match database.enrollment_status().unwrap() {
        EnrollmentStatus::Enrolled { device_count, .. } => assert_eq!(device_count, 1),
        other => panic!("expected Enrolled, got {other:?}"),
    }
}

#[tokio::test]
async fn enrollment_status_warns_as_history_handoff_capacity_approaches_its_limit() {
    let database = Database::open_memory();
    let identity = test_identity(&database);
    let epoch_keys = FakeEpochKeyStore::default();
    begin_genesis(&database, &identity, &epoch_keys, &fake_transports("history-limit"), false)
        .await
        .unwrap();

    {
        let connection = database.test_connection().unwrap();
        for epoch in 1..=820 {
            connection
                .execute(
                    "INSERT INTO sync_epoch_history(key_epoch, activated_at) VALUES (?1, 'now')",
                    [epoch],
                )
                .unwrap();
        }
        connection.execute("UPDATE sync_spaces SET active_epoch=819", []).unwrap();
    }
    assert!(matches!(
        database.enrollment_status().unwrap(),
        EnrollmentStatus::Enrolled { history_handoff_warning: None, .. }
    ));

    database
        .test_connection()
        .unwrap()
        .execute("UPDATE sync_spaces SET active_epoch=820", [])
        .unwrap();
    match database.enrollment_status().unwrap() {
        EnrollmentStatus::Enrolled { history_handoff_warning: Some(warning), .. } => {
            assert_eq!(warning.used, 820);
            assert_eq!(warning.limit, MAX_EARLIER_EPOCH_KEYS);
        }
        other => panic!("expected history handoff warning, got {other:?}"),
    }
}

#[tokio::test]
async fn cross_device_sync_counts_as_enrolled_only_while_the_beta_is_on() {
    let database = Database::open_memory();
    let identity = test_identity(&database);
    let epoch_keys = FakeEpochKeyStore::default();
    begin_genesis(&database, &identity, &epoch_keys, &fake_transports("genesis"), false).await.unwrap();

    database.set_beta_features_enabled(true).unwrap();
    assert!(database.cross_device_sync_enrolled().unwrap());
    // Turning the beta off returns account removal to local-only
    // semantics even though the enrollment itself is kept.
    database.set_beta_features_enabled(false).unwrap();
    assert!(!database.cross_device_sync_enrolled().unwrap());
}

#[tokio::test]
async fn two_device_peer_enrollment_lets_the_new_device_push_and_the_first_pull_it() {
    assert_peer_enrollment_round_trips(fake_transports("shared")).await;
}

#[tokio::test]
async fn another_existing_device_clears_a_request_after_a_trusted_peer_approves_it() {
    let transports = fake_transports("shared");
    let database_a = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

    let database_b = Database::open_memory();
    let identity_b = test_identity(&database_b);
    let epoch_keys_b = FakeEpochKeyStore::default();
    join_with_recovery_phrase(&database_b, &identity_b, &epoch_keys_b, &phrase, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

    let database_c = Database::open_memory();
    let identity_c = test_identity(&database_c);
    publish_enrollment_request(&database_c, &identity_c, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    let pending_a = database_a.pending_incoming_enrollment_requests().unwrap();
    let pending_b = database_b.pending_incoming_enrollment_requests().unwrap();
    assert_eq!(pending_a.len(), 1);
    assert_eq!(pending_b.len(), 1);

    let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending_a[0].request_id, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    assert!(database_b.pending_incoming_enrollment_requests().unwrap().is_empty());
}

#[tokio::test]
async fn rejecting_a_request_resolves_it_for_the_group_and_the_joining_device() {
    let transports = fake_transports("shared");
    let database_a = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

    let database_b = Database::open_memory();
    let identity_b = test_identity(&database_b);
    let epoch_keys_b = FakeEpochKeyStore::default();
    join_with_recovery_phrase(&database_b, &identity_b, &epoch_keys_b, &phrase, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

    let database_c = Database::open_memory();
    let identity_c = test_identity(&database_c);
    publish_enrollment_request(&database_c, &identity_c, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    let request_id = database_a.pending_incoming_enrollment_requests().unwrap()[0].request_id.clone();

    reject_enrollment_request(&database_a, &identity_a, &request_id, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    run_enrollment_sweep(&database_c, &identity_c, &FakeEpochKeyStore::default(), &transports).await.unwrap();

    assert!(database_b.pending_incoming_enrollment_requests().unwrap().is_empty());
    assert!(matches!(database_c.enrollment_status().unwrap(), EnrollmentStatus::Rejected { .. }));
}

#[tokio::test]
async fn a_published_approval_takes_precedence_over_a_concurrent_rejection() {
    let transports = fake_transports("shared");
    let database_a = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

    let database_b = Database::open_memory();
    let identity_b = test_identity(&database_b);
    let epoch_keys_b = FakeEpochKeyStore::default();
    join_with_recovery_phrase(&database_b, &identity_b, &epoch_keys_b, &phrase, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

    let database_c = Database::open_memory();
    let identity_c = test_identity(&database_c);
    let epoch_keys_c = FakeEpochKeyStore::default();
    publish_enrollment_request(&database_c, &identity_c, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    let request_id = database_a.pending_incoming_enrollment_requests().unwrap()[0].request_id.clone();

    let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    approve_enrollment_request(&database_a, &identity_a, &keys_a, &request_id, &transports).await.unwrap();
    reject_enrollment_request(&database_b, &identity_b, &request_id, &transports).await.unwrap();
    run_enrollment_sweep(&database_c, &identity_c, &epoch_keys_c, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();

    assert!(matches!(database_c.enrollment_status().unwrap(), EnrollmentStatus::AwaitingConfirmation { .. }));
    assert!(database_b.pending_incoming_enrollment_requests().unwrap().is_empty());
}

/// Two S3 connector instances (one per device) pointed at the same
/// bucket, over a real SigV4-verifying fake S3 server.
async fn shared_s3_transports(server: &crate::s3_transport::fake_server::FakeS3Server) -> (Vec<Arc<dyn SyncTransport>>, Vec<Arc<dyn SyncTransport>>) {
    use crate::s3_transport::{fake_server::FakeS3Server, S3Transport};
    let open = |id: &str| -> Vec<Arc<dyn SyncTransport>> {
        vec![Arc::new(S3Transport::new(id, &server.config("group"), &FakeS3Server::credentials()).unwrap())]
    };
    (open("s3-device-a"), open("s3-device-b"))
}

#[tokio::test]
async fn peer_enrollment_round_trips_over_an_s3_connector() {
    let server = crate::s3_transport::fake_server::FakeS3Server::spawn().await;
    let (transports, _) = shared_s3_transports(&server).await;
    assert_peer_enrollment_round_trips(transports).await;
    assert!(!server.state().objects.is_empty());
}

#[tokio::test]
async fn recovery_phrase_join_round_trips_over_s3_connectors() {
    let server = crate::s3_transport::fake_server::FakeS3Server::spawn().await;
    let (transports_a, transports_c) = shared_s3_transports(&server).await;
    let database_a = Database::open_memory();
    let database_c = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let identity_c = test_identity(&database_c);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let epoch_keys_c = FakeEpochKeyStore::default();

    let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports_a, false).await.unwrap();
    assert_eq!(inspect_sync_space(&transports_c).await, SyncSpacePresence::Existing);
    join_with_recovery_phrase(&database_c, &identity_c, &epoch_keys_c, &phrase, &transports_c).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports_a).await.unwrap();
    assert!(database_a.known_device_roster().unwrap().iter().any(|(id, _)| *id == identity_c.device_id));

    database_c
        .record_replicated_write(EntityType::Snippet, "c-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("c-1", "From C over S3"))
        .unwrap();
    let keys_c = local_keys_for(&database_c, &identity_c, &epoch_keys_c);
    push_local_state(&database_c, &keys_c, &transports_c).await.unwrap();
    let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    let outcome = pull_from_transports(&database_a, &keys_a, &transports_a).await.unwrap();
    assert_eq!(outcome.merged_states, 1);
    let name: String = database_a.test_connection().unwrap().query_row("SELECT name FROM snippets WHERE id='c-1'", [], |row| row.get(0)).unwrap();
    assert_eq!(name, "From C over S3");
}

#[tokio::test]
async fn an_s3_connection_test_finds_a_group_another_device_created() {
    use crate::s3_transport::fake_server::FakeS3Server;
    let server = FakeS3Server::spawn().await;
    let (transports_a, _) = shared_s3_transports(&server).await;
    let database_a = Database::open_memory();
    let identity_a = test_identity(&database_a);
    begin_genesis(&database_a, &identity_a, &FakeEpochKeyStore::default(), &transports_a, false).await.unwrap();

    let engine = crate::replicated_sync::ReplicatedSync::new(Arc::new(Database::open_memory()));
    let found = engine.probe_s3(&server.config("group"), &FakeS3Server::credentials()).await.unwrap();
    assert_eq!(found.space_presence, Some(SyncSpacePresence::Existing));
    let elsewhere = engine.probe_s3(&server.config("other-prefix"), &FakeS3Server::credentials()).await.unwrap();
    assert_eq!(elsewhere.space_presence, Some(SyncSpacePresence::None));
}

async fn assert_peer_enrollment_round_trips(transports: Vec<Arc<dyn SyncTransport>>) {
    let database_a = Database::open_memory();
    let database_b = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let identity_b = test_identity(&database_b);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let epoch_keys_b = FakeEpochKeyStore::default();

    // A creates the space.
    begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

    // B requests to join.
    let fingerprint_b = publish_enrollment_request(&database_b, &identity_b, &transports).await.unwrap();
    assert_eq!(fingerprint_b.split('-').count(), 4);

    // A's sweep discovers the request.
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
    let pending = database_a.pending_incoming_enrollment_requests().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].fingerprint, fingerprint_b);

    // A approves it, publishing a grant.
    let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending[0].request_id, &transports).await.unwrap();

    // B's sweep discovers and stages the grant, matching the fingerprint
    // it already displayed when it made the request.
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    match database_b.enrollment_status().unwrap() {
        EnrollmentStatus::AwaitingConfirmation { fingerprint, approver_fingerprint, .. } => {
            assert_eq!(fingerprint, fingerprint_b);
            assert!(!approver_fingerprint.is_empty());
        }
        other => panic!("expected AwaitingConfirmation, got {other:?}"),
    }

    // B confirms (the human compared fingerprints across two screens).
    let outgoing_request_id: String = database_b
        .test_connection()
        .unwrap()
        .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
        .unwrap();
    confirm_and_import_grant(&database_b, &identity_b, &epoch_keys_b, &outgoing_request_id).await.unwrap();
    match database_b.enrollment_status().unwrap() {
        EnrollmentStatus::Enrolled { device_count, .. } => assert_eq!(device_count, 2),
        other => panic!("expected Enrolled, got {other:?}"),
    }

    // Prove it actually works end to end: B writes a snippet, pushes,
    // and A — who now trusts B — pulls and materializes it.
    database_b
        .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
        .unwrap();
    let keys_b = local_keys_for(&database_b, &identity_b, &epoch_keys_b);
    push_local_state(&database_b, &keys_b, &transports).await.unwrap();

    let keys_a_after = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    let outcome = pull_from_transports(&database_a, &keys_a_after, &transports).await.unwrap();
    assert_eq!(outcome.merged_states, 1);
    let snippet_name: String = database_a.test_connection().unwrap().query_row("SELECT name FROM snippets WHERE id='b-1'", [], |row| row.get(0)).unwrap();
    assert_eq!(snippet_name, "From B");
}

#[tokio::test]
async fn revoking_a_device_stops_its_future_head_from_being_trusted() {
    let database_a = Database::open_memory();
    let database_b = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let identity_b = test_identity(&database_b);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let epoch_keys_b = FakeEpochKeyStore::default();
    let transports = fake_transports("shared");

    begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();
    publish_enrollment_request(&database_b, &identity_b, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
    let pending = database_a.pending_incoming_enrollment_requests().unwrap();
    let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending[0].request_id, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    let outgoing_request_id: String = database_b
        .test_connection()
        .unwrap()
        .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
        .unwrap();
    confirm_and_import_grant(&database_b, &identity_b, &epoch_keys_b, &outgoing_request_id).await.unwrap();

    // B is enrolled and trusted by A.
    assert_eq!(database_a.known_device_roster().unwrap().len(), 2);

    // A revokes B.
    let keys_a_2 = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    rotate_epoch(&database_a, &identity_a, &keys_a_2, &epoch_keys_a, &transports, Some(&encode_id(identity_b.device_id.as_bytes())))
        .await
        .unwrap();

    // A's active roster now excludes B.
    let roster_after = database_a.known_device_roster().unwrap();
    assert_eq!(roster_after.len(), 1);
    assert!(!roster_after.iter().any(|(id, _)| *id == identity_b.device_id));

    // B is still able to decrypt everything it already had, but a
    // fresh push from B after revocation is never pulled by A: B's
    // device id is no longer in A's locator set at all.
    database_b
        .record_replicated_write(EntityType::Snippet, "after-revoke", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("after-revoke", "Should not sync"))
        .unwrap();
    let keys_b_after = local_keys_for(&database_b, &identity_b, &epoch_keys_b);
    push_local_state(&database_b, &keys_b_after, &transports).await.unwrap();
    let keys_a_after_revoke = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    let outcome = pull_from_transports(&database_a, &keys_a_after_revoke, &transports).await.unwrap();
    assert_eq!(outcome.merged_states, 0);
    let exists: Option<String> = database_a
        .test_connection()
        .unwrap()
        .query_row("SELECT name FROM snippets WHERE id='after-revoke'", [], |row| row.get(0))
        .optional()
        .unwrap();
    assert_eq!(exists, None);
}

#[tokio::test]
async fn recovery_phrase_bootstraps_a_third_device_and_announces_it_to_the_first() {
    let database_a = Database::open_memory();
    let database_c = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let identity_c = test_identity(&database_c);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let epoch_keys_c = FakeEpochKeyStore::default();
    let transports = fake_transports("shared");

    let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

    join_with_recovery_phrase(&database_c, &identity_c, &epoch_keys_c, &phrase, &transports).await.unwrap();
    match database_c.enrollment_status().unwrap() {
        // C now trusts both the recovered roster (A) and itself.
        EnrollmentStatus::Enrolled { device_count, .. } => assert_eq!(device_count, 2),
        other => panic!("expected Enrolled, got {other:?}"),
    }

    // A's sweep discovers C's recovery-signed self-announcement and
    // trusts it without any staged confirmation step.
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
    let roster = database_a.known_device_roster().unwrap();
    assert_eq!(roster.len(), 2);
    assert!(roster.iter().any(|(id, _)| *id == identity_c.device_id));
}

/// One simulated device with its own database, identity, and keychain.
struct Member {
    database: Database,
    identity: DeviceIdentity,
    epoch_keys: FakeEpochKeyStore,
}

impl Member {
    fn new() -> Self {
        let database = Database::open_memory();
        let identity = test_identity(&database);
        Self { database, identity, epoch_keys: FakeEpochKeyStore::default() }
    }

    fn keys(&self) -> LocalKeys {
        local_keys_for(&self.database, &self.identity, &self.epoch_keys)
    }

    fn active_epoch(&self) -> u32 {
        self.database
            .test_connection()
            .unwrap()
            .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
            .unwrap()
    }

    fn write_snippet(&self, id: &str) {
        self.database
            .record_replicated_write(EntityType::Snippet, id, &fields(&["id", "name", "body", "createdAt"]), &snippet_payload(id, id))
            .unwrap();
    }

    async fn push(&self, transports: &[Arc<dyn SyncTransport>]) {
        push_local_state(&self.database, &self.keys(), transports).await.unwrap();
    }

    async fn pull(&self, transports: &[Arc<dyn SyncTransport>]) -> crate::replicated_sync::PullOutcome {
        pull_from_transports(&self.database, &self.keys(), transports).await.unwrap()
    }

    async fn sweep(&self, transports: &[Arc<dyn SyncTransport>]) {
        run_enrollment_sweep(&self.database, &self.identity, &self.epoch_keys, transports).await.unwrap();
    }

    async fn rotate(&self, transports: &[Arc<dyn SyncTransport>]) {
        rotate_epoch(&self.database, &self.identity, &self.keys(), &self.epoch_keys, transports, None).await.unwrap();
    }

    fn snippet(&self, id: &str) -> Option<String> {
        self.database
            .test_connection()
            .unwrap()
            .query_row("SELECT name FROM snippets WHERE id=?1", params![id], |row| row.get(0))
            .optional()
            .unwrap()
    }
}

/// A founding device that wrote under epoch 0, rotated twice, and wrote
/// under epoch 2.
async fn founder_with_history_across_rotations(transports: &[Arc<dyn SyncTransport>]) -> Member {
    let a = Member::new();
    begin_genesis(&a.database, &a.identity, &a.epoch_keys, transports, false).await.unwrap();
    a.write_snippet("epoch-0");
    a.push(transports).await;
    a.rotate(transports).await;
    a.rotate(transports).await;
    assert_eq!(a.active_epoch(), 2);
    a.write_snippet("epoch-2");
    a.push(transports).await;
    a
}

#[tokio::test]
async fn a_device_approved_after_rotations_reads_history_from_every_epoch() {
    let transports = fake_transports("shared");
    let a = founder_with_history_across_rotations(&transports).await;

    let b = Member::new();
    publish_enrollment_request(&b.database, &b.identity, &transports).await.unwrap();
    a.sweep(&transports).await;
    let pending = a.database.pending_incoming_enrollment_requests().unwrap();
    approve_enrollment_request(&a.database, &a.identity, &a.keys(), &pending[0].request_id, &transports).await.unwrap();
    b.sweep(&transports).await;
    let request_id: String = b
        .database
        .test_connection()
        .unwrap()
        .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
        .unwrap();
    confirm_and_import_grant(&b.database, &b.identity, &b.epoch_keys, &request_id).await.unwrap();

    assert_eq!(b.active_epoch(), 2);
    for epoch in 0..=2 {
        assert_eq!(b.epoch_keys.get(epoch), a.epoch_keys.get(epoch), "epoch {epoch}");
    }
    let outcome = b.pull(&transports).await;
    assert_eq!(outcome.failed_transports, 0);
    assert_eq!(outcome.merged_states, 1);
    assert_eq!(b.snippet("epoch-0").as_deref(), Some("epoch-0"));
    assert_eq!(b.snippet("epoch-2").as_deref(), Some("epoch-2"));
}

#[tokio::test]
async fn a_grant_whose_earlier_keys_are_not_sealed_to_this_device_is_refused() {
    let transports = fake_transports("shared");
    let a = founder_with_history_across_rotations(&transports).await;
    let b = Member::new();
    publish_enrollment_request(&b.database, &b.identity, &transports).await.unwrap();
    a.sweep(&transports).await;
    let pending = a.database.pending_incoming_enrollment_requests().unwrap();
    approve_enrollment_request(&a.database, &a.identity, &a.keys(), &pending[0].request_id, &transports).await.unwrap();
    b.sweep(&transports).await;

    // Swap one earlier key for a box sealed to someone else, re-signed
    // by the approver so only the sealing is wrong.
    let (request_id, grant_cbor): (String, Vec<u8>) = b
        .database
        .test_connection()
        .unwrap()
        .query_row(
            "SELECT request_id, pending_grant_cbor FROM replicated_sync_enrollment_requests WHERE direction='outgoing'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let mut grant = decode_signed_enrollment_grant(&grant_cbor).unwrap().grant;
    grant.earlier_epoch_keys[0].sealed_key = ByteBuf::from(seal_to_x25519(&[9u8; 32], &[1u8; 32]));
    let resigned = encode_signed_enrollment_grant(&sign_enrollment_grant(&a.identity.signing_key, grant).unwrap()).unwrap();
    b.database
        .test_connection()
        .unwrap()
        .execute("UPDATE replicated_sync_enrollment_requests SET pending_grant_cbor=?1 WHERE request_id=?2", params![resigned, request_id])
        .unwrap();

    assert!(confirm_and_import_grant(&b.database, &b.identity, &b.epoch_keys, &request_id).await.is_err());
    assert!(b.epoch_keys.get(2).is_none());
    assert!(!matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { .. }));
}

#[tokio::test]
async fn a_recovery_phrase_join_after_rotations_adopts_the_newest_epoch_and_reads_every_one() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    a.write_snippet("epoch-0");
    a.push(&transports).await;
    a.rotate(&transports).await;
    a.rotate(&transports).await;
    a.write_snippet("epoch-2");
    a.push(&transports).await;

    let c = Member::new();
    join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
    assert_eq!(c.active_epoch(), 2);
    for epoch in 0..=2 {
        assert_eq!(c.epoch_keys.get(epoch), a.epoch_keys.get(epoch), "epoch {epoch}");
    }
    let outcome = c.pull(&transports).await;
    assert_eq!(outcome.failed_transports, 0);
    assert_eq!(c.snippet("epoch-0").as_deref(), Some("epoch-0"));
    assert_eq!(c.snippet("epoch-2").as_deref(), Some("epoch-2"));
}

#[tokio::test]
async fn a_recovery_join_waits_until_every_epochs_rotation_has_arrived() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    a.rotate(&transports).await;
    a.rotate(&transports).await;
    a.write_snippet("epoch-2");
    a.push(&transports).await;

    let mut rotations: std::collections::BTreeMap<u32, (String, Vec<u8>)> = std::collections::BTreeMap::new();
    let page = transports[0].scan(None).await.unwrap().unwrap();
    for locator in page.objects {
        let bytes = transports[0].get_object(&locator.cid).await.unwrap();
        if let Ok(signed) = decode_signed_key_rotation(&bytes) {
            rotations.insert(signed.rotation.key_epoch, (locator.cid.0, bytes));
        }
    }
    assert_eq!(rotations.keys().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
    let hide = |epoch: u32| {
        let transport = transports[0].clone();
        let cid = rotations[&epoch].0.clone();
        async move { transport.delete_object(&TransportCid(cid)).await.unwrap() }
    };
    let restore = |epoch: u32| {
        let transport = transports[0].clone();
        let (cid, bytes) = rotations[&epoch].clone();
        async move { transport.put_object(&TransportCid(cid), &bytes).await.unwrap() }
    };

    // A gap in the middle.
    hide(1).await;
    let c = Member::new();
    let refused = join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await;
    assert!(matches!(refused.unwrap_err(), EnrollmentError::RecoveryIncomplete));
    restore(1).await;

    // The newest rotation missing, while A's head already says epoch 2.
    hide(2).await;
    let refused = join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await;
    assert!(matches!(refused.unwrap_err(), EnrollmentError::RecoveryIncomplete));
    assert!(c.epoch_keys.get(0).is_none(), "a refused join stores no keys");
    restore(2).await;

    join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
    assert_eq!(c.active_epoch(), 2);
    c.pull(&transports).await;
    assert_eq!(c.snippet("epoch-2").as_deref(), Some("epoch-2"));
}

#[tokio::test]
async fn a_member_offline_across_a_rotation_catches_up_on_events_from_before_it() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    let c = Member::new();
    join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
    a.sweep(&transports).await;

    // While C is away, A writes, rotates, and writes again.
    a.write_snippet("before-rotation");
    a.push(&transports).await;
    a.rotate(&transports).await;
    a.write_snippet("after-rotation");
    a.push(&transports).await;

    c.sweep(&transports).await;
    assert_eq!(c.active_epoch(), 1);
    let outcome = c.pull(&transports).await;
    assert_eq!(outcome.failed_transports, 0);
    assert_eq!(outcome.merged_states, 1);
    assert_eq!(c.snippet("before-rotation").as_deref(), Some("before-rotation"));
    assert_eq!(c.snippet("after-rotation").as_deref(), Some("after-rotation"));
}

#[tokio::test]
async fn a_member_a_rotation_was_never_sealed_to_catches_up_from_a_peer() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    let c = Member::new();
    join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();

    // A rotates before hearing that C joined, so the new key isn't
    // sealed to C, and writes under it.
    a.rotate(&transports).await;
    a.write_snippet("after-rotation");
    a.push(&transports).await;
    c.sweep(&transports).await;
    assert_eq!(c.active_epoch(), 0);
    assert!(c.epoch_keys.get(1).is_none());
    c.push(&transports).await;
    assert_eq!(c.pull(&transports).await.failed_transports, 1, "C can't open A's snapshot yet");

    // A learns of C, sees C's head still on epoch 0, and shares its keys.
    a.sweep(&transports).await;
    a.pull(&transports).await;
    assert_eq!(share_keys_with_lagging_peers(&a.database, &a.keys(), &transports).await.unwrap(), 1);
    assert_eq!(share_keys_with_lagging_peers(&a.database, &a.keys(), &transports).await.unwrap(), 0, "once per epoch");

    c.sweep(&transports).await;
    assert_eq!(c.active_epoch(), 1);
    assert_eq!(c.epoch_keys.get(1), a.epoch_keys.get(1));
    let outcome = c.pull(&transports).await;
    assert_eq!((outcome.merged_states, outcome.failed_transports), (1, 0));
    assert_eq!(c.snippet("after-rotation").as_deref(), Some("after-rotation"));
}

#[tokio::test]
async fn concurrent_rotations_to_the_same_epoch_converge_on_a_fresh_epoch_without_the_revoked_device() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    let b = Member::new();
    join_with_recovery_phrase(&b.database, &b.identity, &b.epoch_keys, &phrase, &transports).await.unwrap();
    let x = Member::new();
    join_with_recovery_phrase(&x.database, &x.identity, &x.epoch_keys, &phrase, &transports).await.unwrap();
    for member in [&a, &b, &x] {
        member.sweep(&transports).await;
    }
    let start = a.active_epoch();
    assert_eq!(b.active_epoch(), start);

    // A revokes X while B, not having seen that yet, rotates too: both
    // create the same epoch number under different keys, and B's key is
    // sealed to X.
    let x_hex = encode_id(x.identity.device_id.as_bytes());
    rotate_epoch(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &transports, Some(&x_hex)).await.unwrap();
    b.rotate(&transports).await;
    let collided = start + 1;
    let a_key = a.epoch_keys.get(collided).unwrap();
    let b_key = b.epoch_keys.get(collided).unwrap();
    assert_ne!(a_key, b_key);

    let a_resolves = run_enrollment_sweep(&a.database, &a.identity, &a.epoch_keys, &transports).await.unwrap();
    let b_resolves = run_enrollment_sweep(&b.database, &b.identity, &b.epoch_keys, &transports).await.unwrap();
    assert!(a_resolves != b_resolves, "exactly one initiator resolves the collision");
    // Neither device swaps its key for the other's.
    assert_eq!(a.epoch_keys.get(collided), Some(a_key));
    assert_eq!(b.epoch_keys.get(collided), Some(b_key));
    // A sweep without a rotation in between still asks to resolve it.
    let resolver = if a_resolves { &a } else { &b };
    assert!(run_enrollment_sweep(&resolver.database, &resolver.identity, &resolver.epoch_keys, &transports).await.unwrap());

    resolver.rotate(&transports).await;
    assert!(!run_enrollment_sweep(&resolver.database, &resolver.identity, &resolver.epoch_keys, &transports).await.unwrap());
    for member in [&a, &b, &x] {
        member.sweep(&transports).await;
    }
    let fresh = collided + 1;
    assert_eq!(a.active_epoch(), fresh);
    assert_eq!(b.active_epoch(), fresh);
    assert!(a.epoch_keys.get(fresh).is_some());
    assert_eq!(a.epoch_keys.get(fresh), b.epoch_keys.get(fresh));
    assert!(x.epoch_keys.get(fresh).is_none(), "the revoked device never receives the fresh key");

    // Data written under the fresh epoch reaches the other device.
    a.write_snippet("after-collision");
    a.push(&transports).await;
    b.pull(&transports).await;
    assert_eq!(b.snippet("after-collision").as_deref(), Some("after-collision"));
}

#[tokio::test]
async fn rotating_from_stale_keys_is_refused_without_touching_the_newer_epoch() {
    let transports = fake_transports("shared");
    let a = Member::new();
    begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    let stale = a.keys();
    a.rotate(&transports).await;
    let current = a.active_epoch();
    let current_key = a.epoch_keys.get(current);

    let error = rotate_epoch(&a.database, &a.identity, &stale, &a.epoch_keys, &transports, None).await.unwrap_err();
    assert!(matches!(error, EnrollmentError::EpochChanged), "{error}");
    assert_eq!(a.active_epoch(), current);
    assert_eq!(a.epoch_keys.get(current), current_key);
}

/// Callers branch on the failure category, never on message text: a
/// malformed id, a request that is gone, a recovery phrase that doesn't
/// parse, and a keychain refusal each arrive as their own variant.
#[tokio::test]
async fn enrollment_failures_keep_their_category() {
    let transports = fake_transports("shared");
    let a = Member::new();
    begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();

    let error = rotate_epoch(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &transports, Some("not-hex")).await.unwrap_err();
    assert!(matches!(error, EnrollmentError::MalformedId(_)), "{error}");

    let error = reject_enrollment_request(&a.database, &a.identity, &encode_id(&random_id()), &transports).await.unwrap_err();
    assert!(matches!(error, EnrollmentError::RequestNotPending), "{error}");
    assert_eq!(String::from(error), "This request is no longer pending.");

    let b = Member::new();
    let error = join_with_recovery_phrase(&b.database, &b.identity, &b.epoch_keys, "not a phrase", &transports).await.unwrap_err();
    assert!(matches!(error, EnrollmentError::InvalidRecoveryPhrase(_)), "{error}");

    struct RefusingKeychain;
    impl EpochKeyStore for RefusingKeychain {
        fn store(&self, _: u32, _: &[u8; 32]) -> EnrollmentResult<()> {
            Err(EnrollmentError::Keychain("keychain locked".to_string()))
        }
        fn load(&self, _: u32) -> EnrollmentResult<Option<[u8; 32]>> {
            Ok(None)
        }
    }
    let c = Member::new();
    let error = begin_genesis(&c.database, &c.identity, &RefusingKeychain, &fake_transports("other"), false).await.unwrap_err();
    assert!(matches!(error, EnrollmentError::Keychain(_)), "{error}");
}

/// Settings shows these sentences verbatim, and the shared constants are
/// still what each refusal displays.
#[test]
fn refusals_display_the_sentences_settings_shows() {
    assert_eq!(EnrollmentError::LegacySpace.to_string(), LEGACY_SPACE_REFUSAL);
    assert_eq!(EnrollmentError::ExistingSpace.to_string(), EXISTING_SPACE_REFUSAL);
    assert_eq!(EnrollmentError::RecoveryIncomplete.to_string(), RECOVERY_INCOMPLETE);
    assert_eq!(EnrollmentError::InvitationNotFound.to_string(), join_codes::INVITATION_NOT_FOUND);
    assert_eq!(EnrollmentError::CredentialsRejected.to_string(), join_codes::CREDENTIALS_REJECTED);
    assert_eq!(EnrollmentError::StorageUnreachable.to_string(), join_codes::STORAGE_UNREACHABLE);
    assert_eq!(
        EnrollmentError::HistoryLimitReached.to_string(),
        format!(
            "This sync group has changed its keys more than {MAX_EARLIER_EPOCH_KEYS} times, so it can't hand its history to a new device. Create a new sync group instead."
        )
    );
}

#[test]
fn a_peer_key_never_replaces_a_different_key_held_for_the_same_epoch() {
    let store = FakeEpochKeyStore::default();
    assert!(adopt_epoch_key(&store, 3, &[1; 32]).unwrap());
    assert!(adopt_epoch_key(&store, 3, &[1; 32]).unwrap());
    assert!(!adopt_epoch_key(&store, 3, &[2; 32]).unwrap());
    assert_eq!(store.get(3), Some([1; 32]));
}

#[tokio::test]
async fn a_stale_roster_cannot_reactivate_a_revoked_device() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    let b = Member::new();
    join_with_recovery_phrase(&b.database, &b.identity, &b.epoch_keys, &phrase, &transports).await.unwrap();
    let c = Member::new();
    join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
    a.sweep(&transports).await;
    c.sweep(&transports).await;
    let b_hex = encode_id(b.identity.device_id.as_bytes());
    let status_on_c = || -> String {
        c.database
            .test_connection()
            .unwrap()
            .query_row("SELECT status FROM sync_devices WHERE device_id=?1", params![b_hex], |row| row.get(0))
            .unwrap()
    };
    // A roster snapshot taken while B was still active.
    let stale_roster = a.database.full_roster_snapshot().unwrap();
    assert!(stale_roster.iter().any(|entry| entry.device_id == b.identity.device_id && entry.status == "active"));

    rotate_epoch(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &transports, Some(&b_hex)).await.unwrap();
    c.sweep(&transports).await;
    assert_eq!(status_on_c(), "revoked");

    // A lagging key share from A, still carrying the pre-revocation
    // roster, reaches C after the revocation.
    let keys = a.keys();
    let c_x25519 = x25519_public_bytes(&c.identity.x25519_secret);
    let (recovery_ed25519, recovery_x25519) = a.database.recovery_public_keys().unwrap().unwrap();
    let lagging = sign_enrollment_grant(
        &a.identity.signing_key,
        EnrollmentGrant {
            request_id: RequestId::from_bytes(random_id()),
            approver_device_id: a.identity.device_id,
            signed_by_recovery: false,
            key_epoch: a.active_epoch(),
            sealed_epoch_key: ByteBuf::from(seal_to_x25519(&c_x25519, &a.epoch_keys.get(a.active_epoch()).unwrap())),
            roster: stale_roster,
            recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
            recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
            created_at_ms: now_ms(),
            earlier_epoch_keys: seal_earlier_epoch_keys(&keys, &c_x25519).unwrap(),
        },
    )
    .unwrap();
    assert!(matches!(
        apply_key_share(&c.database, &c.identity, &c.epoch_keys, &lagging, "lagging-share").unwrap(),
        KeyShare::Applied
    ));
    assert_eq!(status_on_c(), "revoked");
    assert!(!c.database.full_roster_snapshot().unwrap().iter().any(|entry| entry.device_id == b.identity.device_id && entry.status == "active"));
}

#[tokio::test]
async fn a_key_share_from_a_revoked_or_unknown_member_is_not_applied() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    let c = Member::new();
    join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
    a.sweep(&transports).await;
    a.rotate(&transports).await;
    // A share signed by a stranger, sealed to C, carrying a made-up key.
    let stranger = threestrands_sync_envelope::SigningKey::generate(&mut threestrands_sync_envelope::os_rng());
    let c_x25519 = x25519_public_bytes(&c.identity.x25519_secret);
    let (recovery_ed25519, recovery_x25519) = a.database.recovery_public_keys().unwrap().unwrap();
    let forged = sign_enrollment_grant(
        &stranger,
        EnrollmentGrant {
            request_id: RequestId::from_bytes(random_id()),
            approver_device_id: threestrands_sync_envelope::DeviceId::from_bytes([0x5a; 16]),
            signed_by_recovery: false,
            key_epoch: 7,
            sealed_epoch_key: ByteBuf::from(seal_to_x25519(&c_x25519, &[0x42; 32])),
            roster: vec![],
            recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
            recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
            created_at_ms: now_ms(),
            earlier_epoch_keys: vec![],
        },
    )
    .unwrap();
    assert!(matches!(
        apply_key_share(&c.database, &c.identity, &c.epoch_keys, &forged, "forged-unknown").unwrap(),
        KeyShare::UnknownSender
    ));
    publish_to_all(&transports, &encode_signed_enrollment_grant(&forged).unwrap()).await;
    c.sweep(&transports).await;
    assert!(c.epoch_keys.get(7).is_none());
    assert_eq!(c.active_epoch(), 1, "the real rotation still applies");

    // A share that claims to come from A, a trusted active member, but is
    // signed by a different key fails signature verification and is
    // deliberately consumed without applying anything.
    let a_status: String = c
        .database
        .test_connection()
        .unwrap()
        .query_row(
            "SELECT status FROM sync_devices WHERE device_id=?1",
            params![encode_id(a.identity.device_id.as_bytes())],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(a_status, "active");
    let impersonating = sign_enrollment_grant(
        &stranger,
        EnrollmentGrant {
            request_id: RequestId::from_bytes(random_id()),
            approver_device_id: a.identity.device_id,
            signed_by_recovery: false,
            key_epoch: 8,
            sealed_epoch_key: ByteBuf::from(seal_to_x25519(&c_x25519, &[0x44; 32])),
            roster: vec![],
            recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
            recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
            created_at_ms: now_ms(),
            earlier_epoch_keys: vec![],
        },
    )
    .unwrap();
    assert!(verify_enrollment_grant(&a.identity.signing_key.verifying_key(), &impersonating).is_err());
    assert!(matches!(
        apply_key_share(&c.database, &c.identity, &c.epoch_keys, &impersonating, "forged-impersonating").unwrap(),
        KeyShare::Applied
    ));
    assert!(c.epoch_keys.get(8).is_none());
    assert_eq!(c.active_epoch(), 1);
    publish_to_all(&transports, &encode_signed_enrollment_grant(&impersonating).unwrap()).await;
    c.sweep(&transports).await;
    assert!(c.epoch_keys.get(8).is_none());
    assert_eq!(c.active_epoch(), 1);

    // A share from a member C has since revoked is ignored too.
    let shared_before_revocation = {
        let keys = a.keys();
        let recipient = c_x25519;
        sign_enrollment_grant(
            &a.identity.signing_key,
            EnrollmentGrant {
                request_id: RequestId::from_bytes(random_id()),
                approver_device_id: a.identity.device_id,
                signed_by_recovery: false,
                key_epoch: 9,
                sealed_epoch_key: ByteBuf::from(seal_to_x25519(&recipient, &[0x43; 32])),
                roster: vec![],
                recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
                recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
                created_at_ms: now_ms(),
                earlier_epoch_keys: seal_earlier_epoch_keys(&keys, &recipient).unwrap(),
            },
        )
        .unwrap()
    };
    c.database.revoke_device(a.identity.device_id.as_bytes()).unwrap();
    publish_to_all(&transports, &encode_signed_enrollment_grant(&shared_before_revocation).unwrap()).await;
    c.sweep(&transports).await;
    assert!(c.epoch_keys.get(9).is_none());
}

#[tokio::test]
async fn meeting_an_older_rotation_after_a_newer_one_never_moves_the_active_epoch_back() {
    let transports = fake_transports("shared");
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    let c = Member::new();
    join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
    a.sweep(&transports).await;
    a.rotate(&transports).await;
    a.rotate(&transports).await;

    let rotation_cids: Vec<String> = {
        let connection = a.database.test_connection().unwrap();
        let mut statement = connection.prepare("SELECT cid FROM sync_control_objects_seen WHERE object_kind='key_rotation'").unwrap();
        let rows = statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<Vec<String>, _>>().unwrap();
        rows
    };
    let mut rotations = Vec::new();
    for cid in rotation_cids {
        let bytes = transports[0].get_object(&TransportCid(cid.clone())).await.unwrap();
        rotations.push((decode_signed_key_rotation(&bytes).unwrap(), cid));
    }
    rotations.sort_by_key(|(signed, _)| std::cmp::Reverse(signed.rotation.key_epoch));
    for (signed, cid) in rotations {
        assert!(matches!(
            apply_incoming_rotation(&c.database, &c.identity, &c.epoch_keys, signed, &cid).unwrap(),
            RotationOutcome::Done
        ));
    }
    assert_eq!(c.active_epoch(), 2);
    assert_eq!(c.epoch_keys.get(1), a.epoch_keys.get(1));
    assert_eq!(c.epoch_keys.get(2), a.epoch_keys.get(2));
}

/// A group as an earlier build left it on `transports`: a genesis
/// rotation with no protocol marker. Returns its recovery phrase.
async fn legacy_group(transports: &[Arc<dyn SyncTransport>]) -> String {
    let a = Member::new();
    let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, transports, false).await.unwrap();
    for transport in transports {
        transport.delete_object(&TransportCid(protocol_marker_cid())).await.unwrap();
    }
    phrase
}

#[tokio::test]
async fn genesis_stores_the_protocol_marker_where_every_connector_gets_it() {
    let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(FakeTransport::new("one")), Arc::new(FakeTransport::new("two"))];
    let a = Member::new();
    begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
    for transport in &transports {
        assert_eq!(transport.get_object(&TransportCid(protocol_marker_cid())).await.unwrap(), PROTOCOL_MARKER);
    }
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Existing);

    // A connector added later receives it through repair.
    let later = fake_transports("later");
    a.database.enqueue_repair_deliveries(&[later[0].instance_id()]).unwrap();
    a.push(&later).await;
    assert_eq!(later[0].get_object(&TransportCid(protocol_marker_cid())).await.unwrap(), PROTOCOL_MARKER);
}

#[tokio::test]
async fn a_group_from_an_earlier_build_is_reported_as_legacy_and_never_joined_or_replaced() {
    let transports = fake_transports("shared");
    let phrase = legacy_group(&transports).await;
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Legacy);
    let objects_before = transports[0].scan(None).await.unwrap().unwrap().objects.len();

    let b = Member::new();
    for allow_existing_space in [false, true] {
        let refused = begin_genesis(&b.database, &b.identity, &b.epoch_keys, &transports, allow_existing_space).await;
        assert!(matches!(refused.unwrap_err(), EnrollmentError::LegacySpace));
    }
    let refused = join_with_recovery_phrase(&b.database, &b.identity, &b.epoch_keys, &phrase, &transports).await;
    assert!(matches!(refused.unwrap_err(), EnrollmentError::LegacySpace));
    let refused = publish_enrollment_request(&b.database, &b.identity, &transports).await;
    assert!(matches!(refused.unwrap_err(), EnrollmentError::LegacySpace));
    // Nothing was set up or published on the way to refusing.
    assert_eq!(transports[0].scan(None).await.unwrap().unwrap().objects.len(), objects_before);
    assert!(b.epoch_keys.get(0).is_none());
    let recorded: i64 = b
        .database
        .test_connection()
        .unwrap()
        .query_row("SELECT (SELECT COUNT(*) FROM sync_epoch_history) + (SELECT COUNT(*) FROM replicated_sync_enrollment_requests)", [], |row| row.get(0))
        .unwrap();
    assert_eq!(recorded, 0);
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Legacy);
}

#[tokio::test]
async fn an_s3_group_from_an_earlier_build_is_reported_as_legacy() {
    use crate::s3_transport::fake_server::FakeS3Server;
    let server = FakeS3Server::spawn().await;
    let (transports, _) = shared_s3_transports(&server).await;
    legacy_group(&transports).await;
    let engine = crate::replicated_sync::ReplicatedSync::new(Arc::new(Database::open_memory()));
    let found = engine.probe_s3(&server.config("group"), &FakeS3Server::credentials()).await.unwrap();
    assert_eq!(found.space_presence, Some(SyncSpacePresence::Legacy));
}

#[tokio::test]
async fn inspecting_reports_no_space_until_genesis_publishes_one() {
    let database = Database::open_memory();
    let identity = test_identity(&database);
    let epoch_keys = FakeEpochKeyStore::default();
    let transports = fake_transports("shared");

    assert_eq!(inspect_sync_space(&[]).await, SyncSpacePresence::Unknown);
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);

    begin_genesis(&database, &identity, &epoch_keys, &transports, false).await.unwrap();
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Existing);
}

#[tokio::test]
async fn a_pending_join_request_or_unrelated_object_is_not_a_space() {
    let database = Database::open_memory();
    let identity = test_identity(&database);
    let transports = fake_transports("shared");

    publish_enrollment_request(&database, &identity, &transports).await.unwrap();
    publish_to_all(&transports, b"not a control object").await;
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);
}

#[tokio::test]
async fn a_rotation_that_fails_its_initiator_signature_is_not_a_space() {
    let database = Database::open_memory();
    let identity = test_identity(&database);
    let epoch_keys = FakeEpochKeyStore::default();
    let genesis_transports = fake_transports("genesis");
    begin_genesis(&database, &identity, &epoch_keys, &genesis_transports, false).await.unwrap();

    let page = genesis_transports[0].scan(None).await.unwrap().unwrap();
    let mut rotation = None;
    for locator in &page.objects {
        let bytes = genesis_transports[0].get_object(&locator.cid).await.unwrap();
        if let Ok(decoded) = decode_signed_key_rotation(&bytes) {
            rotation = Some(decoded);
        }
    }
    let mut tampered = rotation.expect("genesis publishes a rotation");
    tampered.rotation.created_at_ms += 1;

    let transports = fake_transports("tampered");
    publish_to_all(&transports, &encode_signed_key_rotation(&tampered).unwrap()).await;
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);
}

#[tokio::test]
async fn an_incomplete_scan_is_unknown_rather_than_empty() {
    let fake = Arc::new(FakeTransport::new("flaky"));
    fake.inject_transient_outage(1);
    let transports: Vec<Arc<dyn SyncTransport>> = vec![fake];
    assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Unknown);
}

#[tokio::test]
async fn genesis_refuses_an_existing_space_without_side_effects_unless_allowed() {
    let database_a = Database::open_memory();
    let database_b = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let identity_b = test_identity(&database_b);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let epoch_keys_b = FakeEpochKeyStore::default();
    let transports = fake_transports("shared");

    begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

    database_b.set_beta_features_enabled(false).unwrap();
    let refused = begin_genesis(&database_b, &identity_b, &epoch_keys_b, &transports, false).await;
    let refused = refused.unwrap_err();
    assert!(matches!(refused, EnrollmentError::ExistingSpace));
    assert_eq!(refused.to_string(), EXISTING_SPACE_REFUSAL, "the refusal Settings shows is unchanged");
    assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
    assert!(!database_b.beta_features_enabled().unwrap());
    assert_eq!(epoch_keys_b.get(0), None);

    let phrase = begin_genesis(&database_b, &identity_b, &epoch_keys_b, &transports, true).await.unwrap();
    assert_eq!(phrase.split_whitespace().count(), 24);
    assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { device_count: 1, .. }));
}

#[tokio::test]
async fn genesis_and_recovery_join_refuse_devices_already_in_a_sync_group() {
    let existing = Member::new();
    let existing_transports = fake_transports("existing-group");
    begin_genesis(
        &existing.database,
        &existing.identity,
        &existing.epoch_keys,
        &existing_transports,
        false,
    )
    .await
    .unwrap();
    existing.database.set_beta_features_enabled(false).unwrap();
    existing
        .database
        .record_replicated_write(
            EntityType::Snippet,
            "old-group-snippet",
            &BTreeSet::from(["id".to_string(), "name".to_string(), "body".to_string()]),
            &serde_json::json!({ "id": "old-group-snippet", "name": "Private note", "body": "Keep in this group" }),
        )
        .unwrap();

    let new_group = Member::new();
    let new_group_transports = fake_transports("new-group");
    let new_group_phrase = begin_genesis(
        &new_group.database,
        &new_group.identity,
        &new_group.epoch_keys,
        &new_group_transports,
        false,
    )
    .await
    .unwrap();
    let new_group_objects_before = new_group_transports[0].scan(None).await.unwrap().unwrap().objects.len();
    let sync_values_before: i64 = existing
        .database
        .test_connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_values", [], |row| row.get(0))
        .unwrap();
    let sync_context_before: i64 = existing
        .database
        .test_connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_context", [], |row| row.get(0))
        .unwrap();
    let active_epoch_before: i64 = existing
        .database
        .test_connection()
        .unwrap()
        .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
        .unwrap();
    let sync_local_state_before: (bool, i64, i64, Option<i64>) = existing
        .database
        .test_connection()
        .unwrap()
        .query_row(
            "SELECT dirty, generation, state_sequence, sealed_epoch FROM sync_local_state WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let recovery_keys_before: (Option<Vec<u8>>, Option<Vec<u8>>) = existing
        .database
        .test_connection()
        .unwrap()
        .query_row(
            "SELECT recovery_public_key, recovery_x25519_public FROM sync_spaces WHERE id=?1",
            params![SPACE_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let roster_before: Vec<String> = existing
        .database
        .test_connection()
        .unwrap()
        .prepare("SELECT device_id FROM sync_devices ORDER BY device_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let epoch_key_before = existing.epoch_keys.get(0).unwrap();

    let refused = begin_genesis(
        &existing.database,
        &existing.identity,
        &existing.epoch_keys,
        &new_group_transports,
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(refused, EnrollmentError::AlreadyEnrolled), "{refused}");
    let refused = join_with_recovery_phrase(
        &existing.database,
        &existing.identity,
        &existing.epoch_keys,
        &new_group_phrase,
        &new_group_transports,
    )
    .await
    .unwrap_err();
    assert!(matches!(refused, EnrollmentError::AlreadyEnrolled), "{refused}");

    assert!(matches!(existing.database.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { .. }));
    assert!(!existing.database.beta_features_enabled().unwrap());
    assert_eq!(existing.epoch_keys.get(0), Some(epoch_key_before));
    assert_eq!(existing.database.resolve_field_winner(EntityType::Snippet, "old-group-snippet", "name").unwrap(), Some(serde_json::json!("Private note")));
    assert_eq!(
        new_group_transports[0].scan(None).await.unwrap().unwrap().objects.len(),
        new_group_objects_before
    );
    let connection = existing.database.test_connection().unwrap();
    let sync_values_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM sync_values", [], |row| row.get(0))
        .unwrap();
    let sync_context_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM sync_context", [], |row| row.get(0))
        .unwrap();
    let active_epoch_after: i64 = connection
        .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
        .unwrap();
    let sync_local_state_after: (bool, i64, i64, Option<i64>) = connection
        .query_row(
            "SELECT dirty, generation, state_sequence, sealed_epoch FROM sync_local_state WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let recovery_keys_after: (Option<Vec<u8>>, Option<Vec<u8>>) = connection
        .query_row(
            "SELECT recovery_public_key, recovery_x25519_public FROM sync_spaces WHERE id=?1",
            params![SPACE_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let roster_after: Vec<String> = connection
        .prepare("SELECT device_id FROM sync_devices ORDER BY device_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(sync_values_after, sync_values_before);
    assert_eq!(sync_context_after, sync_context_before);
    assert_eq!(active_epoch_after, active_epoch_before);
    assert_eq!(sync_local_state_after, sync_local_state_before);
    assert_eq!(recovery_keys_after, recovery_keys_before);
    assert_eq!(roster_after, roster_before);
}

/// A and B enrolled into one space through peer approval.
async fn two_enrolled_devices() -> (Database, Database, DeviceIdentity, DeviceIdentity, Vec<Arc<dyn SyncTransport>>, String) {
    let database_a = Database::open_memory();
    let database_b = Database::open_memory();
    let identity_a = test_identity(&database_a);
    let identity_b = test_identity(&database_b);
    let epoch_keys_a = FakeEpochKeyStore::default();
    let epoch_keys_b = FakeEpochKeyStore::default();
    let transports = fake_transports("shared");
    let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();
    publish_enrollment_request(&database_b, &identity_b, &transports).await.unwrap();
    run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
    let pending = database_a.pending_incoming_enrollment_requests().unwrap();
    assert_eq!(pending[0].device_id.as_deref(), Some(encode_id(identity_b.device_id.as_bytes()).as_str()));
    let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
    approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending[0].request_id, &transports).await.unwrap();
    run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
    let outgoing: String = database_b
        .test_connection()
        .unwrap()
        .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
        .unwrap();
    confirm_and_import_grant(&database_b, &identity_b, &epoch_keys_b, &outgoing).await.unwrap();
    (database_a, database_b, identity_a, identity_b, transports, phrase)
}

#[tokio::test]
async fn the_roster_marks_this_device_by_its_flag_and_lists_it_first() {
    let (database_a, database_b, identity_a, identity_b, _, _) = two_enrolled_devices().await;
    for (database, identity) in [(&database_a, &identity_a), (&database_b, &identity_b)] {
        let roster = database.device_roster().unwrap();
        assert_eq!(roster.len(), 2);
        assert!(roster[0].is_self);
        assert_eq!(roster[0].device_id, encode_id(identity.device_id.as_bytes()));
        assert!(!roster[1].is_self);
    }
}

#[tokio::test]
async fn the_roster_reports_shared_labels_and_each_devices_latest_change() {
    let (database_a, _, identity_a, identity_b, _, _) = two_enrolled_devices().await;
    let peer = encode_id(identity_b.device_id.as_bytes());
    let own = encode_id(identity_a.device_id.as_bytes());
    database_a.set_device_label(&peer, "  Work laptop  ").unwrap();
    database_a
        .test_connection()
        .unwrap()
        .execute(
            "INSERT INTO sync_remote_states(device_id, state_sequence, merged_at) VALUES (?1, 2, '2026-09-21T10:00:00+00:00')",
            params![peer],
        )
        .unwrap();

    let roster = database_a.device_roster().unwrap();
    let peer_entry = roster.iter().find(|entry| entry.device_id == peer).unwrap();
    assert_eq!(peer_entry.label.as_deref(), Some("Work laptop"));
    assert_eq!(peer_entry.last_change_at.as_deref(), Some("2026-09-21T10:00:00+00:00"));
    let own_entry = roster.iter().find(|entry| entry.device_id == own).unwrap();
    assert!(own_entry.label.as_deref().is_some_and(|label| !label.is_empty()));
    assert_eq!(own_entry.last_change_at, None);

    database_a.set_device_label(&peer, "   ").unwrap();
    assert_eq!(database_a.device_roster().unwrap().iter().find(|entry| entry.device_id == peer).unwrap().label, None);
}

#[test]
fn device_labels_accept_up_to_the_limit_and_reject_beyond_it() {
    let database = Database::open_memory();
    let at_limit = "x".repeat(MAX_DEVICE_LABEL_CHARS);
    database.set_device_label("d1", &"x".repeat(MAX_DEVICE_LABEL_CHARS - 1)).unwrap();
    database.set_device_label("d1", &at_limit).unwrap();
    assert!(database.set_device_label("d1", &"x".repeat(MAX_DEVICE_LABEL_CHARS + 1)).is_err());
    let stored: String = database.test_connection().unwrap().query_row("SELECT label FROM sync_device_labels", [], |row| row.get(0)).unwrap();
    assert_eq!(stored, at_limit);
}

#[tokio::test]
async fn leaving_resets_membership_but_keeps_local_data_locations_and_the_beta() {
    let (_, database_b, _, identity_b, transports, phrase) = two_enrolled_devices().await;
    database_b
        .test_connection()
        .unwrap()
        .execute_batch(
            "INSERT INTO snippets(id,name,body,created_at) VALUES ('s1','Kept','body','2026-09-22T00:00:00Z');
             INSERT INTO sync_transports(instance_id,kind,enabled) VALUES ('folder-1','folder',1);",
        )
        .unwrap();
    database_b.set_device_label(&encode_id(identity_b.device_id.as_bytes()), "Old name").unwrap();

    assert_eq!(database_b.leave_sync_space().unwrap(), 0);

    assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
    assert!(database_b.device_roster().unwrap().is_empty());
    assert!(database_b.recovery_public_keys().unwrap().is_none());
    assert!(database_b.beta_features_enabled().unwrap());
    let count = |sql: &str| -> i64 { database_b.test_connection().unwrap().query_row(sql, [], |row| row.get(0)).unwrap() };
    for table in ["sync_values", "sync_context", "sync_remote_states", "sync_local_state", "sync_objects"] {
        assert_eq!(count(&format!("SELECT COUNT(*) FROM {table}")), 0, "{table} should be empty");
    }
    assert_eq!(count("SELECT COUNT(*) FROM sync_epoch_history"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM replicated_sync_enrollment_requests"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM sync_device_labels"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM snippets WHERE id='s1'"), 1);
    assert_eq!(count("SELECT COUNT(*) FROM sync_transports WHERE instance_id='folder-1'"), 1);

    // A fresh identity rejoins the same space cleanly.
    let rejoined = test_identity(&database_b);
    assert_ne!(rejoined.device_id, identity_b.device_id);
    let epoch_keys = FakeEpochKeyStore::default();
    join_with_recovery_phrase(&database_b, &rejoined, &epoch_keys, &phrase, &transports).await.unwrap();
    assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { .. }));
}

#[tokio::test]
async fn leaving_reports_the_highest_epoch_to_forget() {
    let (database_a, ..) = two_enrolled_devices().await;
    database_a.record_epoch_activation(1, "cid-1").unwrap();
    database_a.record_epoch_activation(2, "cid-2").unwrap();
    database_a.set_active_epoch(1).unwrap();
    assert_eq!(database_a.leave_sync_space().unwrap(), 2);
}

#[test]
fn wrong_recovery_phrase_never_opens_the_genesis_rotation() {
    let seed_a = generate_recovery_seed();
    let seed_b = generate_recovery_seed();
    let secret_a = threestrands_sync_envelope::recovery_x25519_secret(&seed_a);
    let public_a = X25519PublicKey::from(&secret_a).to_bytes();
    let sealed = seal_to_x25519(&public_a, b"epoch key");
    assert!(try_open_sealed_box(&seed_b, &sealed).is_none());
    assert_eq!(try_open_sealed_box(&seed_a, &sealed), None); // seed bytes alone are not the derived secret
}

fn fields(names: &[&str]) -> std::collections::BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn snippet_payload(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "body": "body",
        "createdAt": "2026-01-01T00:00:00Z",
    })
}

/// Join codes end to end, over real sync folders (and S3 through the
/// fake server), with an explicit clock.
mod join_code_flows {
    use super::*;
    use crate::replicated_sync::build_configured_transports;
    use threestrands_sync_envelope::{
        decode_join_code, encode_join_code, encode_signed_invitation, encode_signed_invitation_redemption,
        invite_ed25519_signing_key, invite_x25519_secret, sign_invitation, sign_invitation_redemption,
        Invitation, InvitationRedemption, SigningKey,
    };

    struct SharedFolder {
        path: std::path::PathBuf,
    }

    impl SharedFolder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("threestrands-join-code-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn choice(&self, index: usize) -> JoinFolderChoice {
            JoinFolderChoice { connector_index: index, path: self.path.to_string_lossy().into_owned() }
        }
    }

    impl Drop for SharedFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct Device {
        database: Database,
        identity: DeviceIdentity,
        epoch_keys: FakeEpochKeyStore,
    }

    impl Device {
        fn new() -> Self {
            let database = Database::open_memory();
            let identity = test_identity(&database);
            Self { database, identity, epoch_keys: FakeEpochKeyStore::default() }
        }

        fn keys(&self) -> LocalKeys {
            local_keys_for(&self.database, &self.identity, &self.epoch_keys)
        }

        async fn transports(&self) -> Vec<Arc<dyn SyncTransport>> {
            build_configured_transports(&self.database).await
        }

        async fn sweep(&self) {
            run_enrollment_sweep(&self.database, &self.identity, &self.epoch_keys, &self.transports().await).await.unwrap();
        }

        async fn process(&self, now: i64) -> bool {
            process_join_codes(&self.database, &self.identity, &self.keys(), &self.epoch_keys, &self.transports().await, now)
                .await
                .unwrap()
        }

        fn active_epoch(&self) -> u32 {
            self.database
                .test_connection()
                .unwrap()
                .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
                .unwrap()
        }

        fn trusts(&self, other: &Device) -> bool {
            self.database.known_device_roster().unwrap().iter().any(|(id, _)| *id == other.identity.device_id)
        }

        fn awaiting_admission(&self) -> Option<String> {
            match self.database.enrollment_status().unwrap() {
                EnrollmentStatus::Enrolled { awaiting_admission_from, .. } => awaiting_admission_from,
                other => panic!("expected Enrolled, got {other:?}"),
            }
        }

        fn device_id_hex(&self) -> String {
            encode_id(self.identity.device_id.as_bytes())
        }
    }

    fn choice(instance_id: &str, include_credentials: bool) -> JoinCodeConnectorChoice {
        JoinCodeConnectorChoice { instance_id: instance_id.to_string(), include_credentials }
    }

    /// A (genesis) and C (joined by recovery phrase) sharing `folder`.
    async fn group(folder: &SharedFolder) -> (Device, Device) {
        let a = Device::new();
        a.database.add_folder_transport("folder-a", &folder.path).unwrap();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();
        let c = Device::new();
        c.database.add_folder_transport("folder-c", &folder.path).unwrap();
        join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &c.transports().await).await.unwrap();
        a.sweep().await;
        (a, c)
    }

    async fn create(a: &Device, now: i64) -> String {
        create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice("folder-a", true)], 24, now)
            .await
            .unwrap()
    }

    async fn join(device: &Device, code: &str, folder: &SharedFolder, now: i64) -> EnrollmentResult<()> {
        join_with_code(&device.database, &device.identity, &device.epoch_keys, code, &[folder.choice(0)], vec![], now).await
    }

    fn snippet_name(database: &Database, id: &str) -> Option<String> {
        database.test_connection().unwrap().query_row("SELECT name FROM snippets WHERE id=?1", params![id], |row| row.get(0)).optional().unwrap()
    }

    #[tokio::test]
    async fn a_join_code_adds_a_device_that_every_member_then_trusts() {
        let folder = SharedFolder::new();
        let (a, c) = group(&folder).await;
        a.database
            .record_replicated_write(EntityType::Snippet, "a-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("a-1", "Before B"))
            .unwrap();
        push_local_state(&a.database, &a.keys(), &a.transports().await).await.unwrap();

        let now = now_ms();
        let code = create(&a, now).await;
        assert!(code.starts_with("TSJOIN1-"));
        let epoch_before = a.active_epoch();

        let b = Device::new();
        join(&b, &code, &folder, now + 1_000).await.unwrap();
        // B's connector is saved, it can read at once, and it's waiting.
        let rows = b.database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "folder");
        assert!(b.awaiting_admission().is_some());
        pull_from_transports(&b.database, &b.keys(), &b.transports().await).await.unwrap();
        assert_eq!(snippet_name(&b.database, "a-1").as_deref(), Some("Before B"));
        assert!(!c.trusts(&b));

        // A admits B by rotating; C and B apply that rotation.
        a.sweep().await;
        assert!(a.process(now + 2_000).await);
        assert_eq!(a.active_epoch(), epoch_before + 1);
        assert!(a.trusts(&b));
        c.sweep().await;
        b.sweep().await;
        assert!(c.trusts(&b));
        assert_eq!(b.awaiting_admission(), None);
        assert_eq!(b.active_epoch(), epoch_before + 1);

        // Everyone can see how B joined.
        let notices = c.database.join_code_notices().unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, "joined");
        assert_eq!(notices[0].device_id, b.device_id_hex());
        assert_eq!(notices[0].inviter_device_id.as_deref(), Some(a.device_id_hex().as_str()));
        c.database.dismiss_join_code_notice(&notices[0].redemption_cid).unwrap();
        assert!(c.database.join_code_notices().unwrap().is_empty());
        let roster = c.database.device_roster().unwrap();
        assert!(roster.iter().find(|entry| entry.device_id == b.device_id_hex()).unwrap().joined_with_join_code);
        assert!(!roster.iter().find(|entry| entry.device_id == a.device_id_hex()).unwrap().joined_with_join_code);
        assert!(b.database.device_roster().unwrap().iter().find(|entry| entry.is_self).unwrap().joined_with_join_code);
        let codes = a.database.outstanding_join_codes().unwrap();
        assert_eq!(codes[0].status, "redeemed");
        assert_eq!(codes[0].redeemed_by_device_id.as_deref(), Some(b.device_id_hex().as_str()));

        // The redeemed invitation stays for members that sync later,
        // then goes once the code would have expired — without another
        // rotation.
        let invitation_cid = TransportCid(decode_join_code(&code).unwrap().invitation_cid);
        assert!(a.transports().await[0].get_object(&invitation_cid).await.is_ok());
        let epoch = a.active_epoch();
        assert!(!a.process(now + 25 * 3_600_000).await);
        assert_eq!(a.active_epoch(), epoch);
        assert!(a.transports().await[0].get_object(&invitation_cid).await.is_err());

        // B's writes now reach A and C.
        b.database
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        push_local_state(&b.database, &b.keys(), &b.transports().await).await.unwrap();
        pull_from_transports(&a.database, &a.keys(), &a.transports().await).await.unwrap();
        pull_from_transports(&c.database, &c.keys(), &c.transports().await).await.unwrap();
        assert_eq!(snippet_name(&a.database, "b-1").as_deref(), Some("From B"));
        assert_eq!(snippet_name(&c.database, "b-1").as_deref(), Some("From B"));
    }

    #[tokio::test]
    async fn a_device_joining_by_code_after_a_rotation_reads_history_from_every_epoch() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        a.database
            .record_replicated_write(EntityType::Snippet, "old", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("old", "Old"))
            .unwrap();
        push_local_state(&a.database, &a.keys(), &a.transports().await).await.unwrap();

        // A first join code admits B, which rotates the epoch.
        let now = now_ms();
        let b = Device::new();
        join(&b, &create(&a, now).await, &folder, now + 1_000).await.unwrap();
        a.sweep().await;
        assert!(a.process(now + 2_000).await);
        let rotated = a.active_epoch();
        a.database
            .record_replicated_write(EntityType::Snippet, "new", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("new", "New"))
            .unwrap();
        push_local_state(&a.database, &a.keys(), &a.transports().await).await.unwrap();

        // D joins with a second code, under the rotated epoch.
        let d = Device::new();
        join(&d, &create(&a, now + 3_000).await, &folder, now + 4_000).await.unwrap();
        assert_eq!(d.active_epoch(), rotated);
        let outcome = pull_from_transports(&d.database, &d.keys(), &d.transports().await).await.unwrap();
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(snippet_name(&d.database, "old").as_deref(), Some("Old"));
        assert_eq!(snippet_name(&d.database, "new").as_deref(), Some("New"));
    }

    #[tokio::test]
    async fn a_join_code_for_a_group_from_an_earlier_build_is_refused() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        a.transports().await[0].delete_object(&TransportCid(protocol_marker_cid())).await.unwrap();
        assert_eq!(inspect_sync_space(&a.transports().await).await, SyncSpacePresence::Legacy);

        let d = Device::new();
        assert!(matches!(join(&d, &code, &folder, now + 1_000).await.unwrap_err(), EnrollmentError::LegacySpace));
        assert!(d.database.configured_transports().unwrap().is_empty());
        assert!(matches!(d.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
    }

    /// Objects are scanned in content-address order, so a redemption can
    /// come before the invitation it names. One sweep must still record
    /// it, not leave it for the next cycle.
    #[tokio::test]
    async fn one_sweep_records_a_redemption_scanned_before_its_invitation() {
        let folder = SharedFolder::new();
        let (a, c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        let b = Device::new();
        join(&b, &code, &folder, now + 1_000).await.unwrap();
        let invitation_cid = decode_join_code(&code).unwrap().invitation_cid;
        let redemption_cid: String = b
            .database
            .test_connection()
            .unwrap()
            .query_row("SELECT redemption_cid FROM replicated_sync_invitations WHERE direction='incoming'", [], |row| row.get(0))
            .unwrap();

        // Just these two objects, invitation first, scanned in reverse.
        let shared = a.transports().await;
        let reordered = threestrands_sync_transport::fake::FakeTransport::new("reordered");
        for cid in [&invitation_cid, &redemption_cid] {
            let bytes = shared[0].get_object(&TransportCid(cid.clone())).await.unwrap();
            reordered.put_object(&TransportCid(cid.clone()), &bytes).await.unwrap();
        }
        reordered.enable_scan_reordering();
        let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(reordered)];

        run_enrollment_sweep(&c.database, &c.identity, &c.epoch_keys, &transports).await.unwrap();
        let state: String = c
            .database
            .test_connection()
            .unwrap()
            .query_row(
                "SELECT state FROM replicated_sync_invitation_redemptions WHERE redemption_cid=?1",
                params![redemption_cid],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "observed");
        assert!(c.database.seen_control_object(&redemption_cid).unwrap());
    }

    #[tokio::test]
    async fn a_code_admits_only_its_first_redemption() {
        let folder = SharedFolder::new();
        let (a, c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        let b = Device::new();
        let d = Device::new();
        join(&b, &code, &folder, now + 1_000).await.unwrap();
        join(&d, &code, &folder, now + 1_500).await.unwrap();

        a.sweep().await;
        assert!(a.process(now + 2_000).await);
        c.sweep().await;
        d.sweep().await;
        assert!(a.trusts(&b) && c.trusts(&b));
        assert!(!a.trusts(&d) && !c.trusts(&d));
        assert!(d.awaiting_admission().is_some());

        let codes = a.database.outstanding_join_codes().unwrap();
        assert_eq!(codes[0].rejected_attempts, 1);
        let notices = a.database.join_code_notices().unwrap();
        assert!(notices.iter().any(|notice| notice.kind == "rejectedAttempt" && notice.device_id == d.device_id_hex()));
        // D never shows as "joined" anywhere.
        assert!(!c.database.join_code_notices().unwrap().iter().any(|notice| notice.device_id == d.device_id_hex()));
    }

    #[tokio::test]
    async fn a_redemption_arriving_after_expiry_is_rejected_and_the_old_key_stops_working() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        let parsed = decode_join_code(&code).unwrap();
        let b = Device::new();
        join(&b, &code, &folder, now + 1_000).await.unwrap();

        a.sweep().await;
        let after_expiry = now + 25 * 3_600_000;
        assert!(a.process(after_expiry).await);
        assert!(!a.trusts(&b));
        let codes = a.database.outstanding_join_codes().unwrap();
        assert_eq!(codes[0].status, "expired");
        assert_eq!(codes[0].rejected_attempts, 1);

        // The invitation object is gone, and the new epoch's stanzas
        // don't open with the invite secret.
        let transports = a.transports().await;
        assert!(transports[0].get_object(&TransportCid(parsed.invitation_cid.clone())).await.is_err());
        let invite_secret = invite_x25519_secret(&parsed.invite_secret()).to_bytes();
        let mut rotations_checked = 0;
        let page = transports[0].scan(None).await.unwrap().unwrap();
        for locator in page.objects {
            let Ok(bytes) = transports[0].get_object(&locator.cid).await else { continue };
            let Ok(rotation) = decode_signed_key_rotation(&bytes) else { continue };
            if rotation.rotation.key_epoch == a.active_epoch() {
                rotations_checked += 1;
                assert!(rotation.rotation.sealed_stanzas.iter().all(|stanza| try_open_sealed_box(&invite_secret, stanza).is_none()));
            }
        }
        assert_eq!(rotations_checked, 1);
    }

    #[tokio::test]
    async fn an_unused_code_expires_with_a_rotation_and_its_invitation_is_deleted() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        let cid = decode_join_code(&code).unwrap().invitation_cid;
        assert!(!a.process(now + 3_600_000).await, "still open within its lifetime");
        let epoch = a.active_epoch();
        assert!(a.process(now + 25 * 3_600_000).await);
        assert_eq!(a.active_epoch(), epoch + 1);
        assert_eq!(a.database.outstanding_join_codes().unwrap()[0].status, "expired");
        assert!(a.transports().await[0].get_object(&TransportCid(cid)).await.is_err());
        assert!(!a.process(now + 26 * 3_600_000).await, "an expired code rotates only once");
    }

    #[tokio::test]
    async fn cancelling_a_code_rotates_and_refuses_later_joins() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        let cid = decode_join_code(&code).unwrap().invitation_cid;
        let epoch = a.active_epoch();

        cancel_join_code(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &a.transports().await, &cid).await.unwrap();
        assert_eq!(a.active_epoch(), epoch + 1);
        assert_eq!(a.database.outstanding_join_codes().unwrap()[0].status, "cancelled");
        assert!(cancel_join_code(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &a.transports().await, &cid).await.is_err());

        let b = Device::new();
        let error = join(&b, &code, &folder, now + 1_000).await.unwrap_err();
        assert!(matches!(error, EnrollmentError::InvitationNotFound), "{error}");
        assert!(b.database.configured_transports().unwrap().is_empty());
        assert!(matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
    }

    #[tokio::test]
    async fn a_redemption_that_raced_a_cancellation_is_rejected() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        let b = Device::new();
        join(&b, &code, &folder, now + 1_000).await.unwrap();
        let cid = decode_join_code(&code).unwrap().invitation_cid;
        cancel_join_code(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &a.transports().await, &cid).await.unwrap();
        a.sweep().await;
        assert!(!a.process(now + 2_000).await);
        assert!(!a.trusts(&b));
        assert_eq!(a.database.outstanding_join_codes().unwrap()[0].rejected_attempts, 1);
    }

    #[tokio::test]
    async fn a_code_pointing_at_the_wrong_object_or_with_the_wrong_secret_saves_nothing() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let code = decode_join_code(&create(&a, now).await).unwrap();

        let mut missing = code.clone();
        missing.invitation_cid = compute_cid(b"no such invitation");
        let b = Device::new();
        let error = join(&b, &encode_join_code(&missing).unwrap(), &folder, now).await.unwrap_err();
        assert!(matches!(error, EnrollmentError::InvitationNotFound), "{error}");

        let mut wrong_secret = code.clone();
        wrong_secret.invite_secret = ByteBuf::from(vec![9u8; 32]);
        let error = join(&b, &encode_join_code(&wrong_secret).unwrap(), &folder, now).await.unwrap_err();
        assert!(matches!(error, EnrollmentError::InvitationDamaged), "{error}");

        assert!(b.database.configured_transports().unwrap().is_empty());
        assert!(matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
        assert!(verify_fetched_invitation(b"not the invitation", &code).is_err());
    }

    #[tokio::test]
    async fn a_planted_look_alike_invitation_is_never_used() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let code = decode_join_code(&create(&a, now).await).unwrap();
        let secret = code.invite_secret();

        // Someone with write access to the folder publishes an
        // invitation with the same (public) invite keys but their own
        // roster and epoch key.
        let mallory_key = SigningKey::generate(&mut threestrands_sync_envelope::os_rng());
        let mallory_id = EnvelopeDeviceId::from_bytes([0xee; 16]);
        let invite_x25519_public = X25519PublicKey::from(&invite_x25519_secret(&secret)).to_bytes();
        let planted = sign_invitation(
            &mallory_key,
            Invitation {
                inviter_device_id: mallory_id,
                invite_ed25519_public: ByteBuf::from(invite_ed25519_signing_key(&secret).verifying_key().to_bytes().to_vec()),
                invite_x25519_public: ByteBuf::from(invite_x25519_public.to_vec()),
                key_epoch: 0,
                sealed_epoch_key: ByteBuf::from(seal_to_x25519(&invite_x25519_public, &[0x42; 32])),
                roster: vec![RosterEntry {
                    device_id: mallory_id,
                    ed25519_public: ByteBuf::from(mallory_key.verifying_key().to_bytes().to_vec()),
                    x25519_public: ByteBuf::from(vec![1u8; 32]),
                    status: "active".to_string(),
                }],
                recovery_ed25519_public: ByteBuf::from(vec![2u8; 32]),
                recovery_x25519_public: ByteBuf::from(vec![3u8; 32]),
                created_at_ms: now,
                expires_at_ms: now + 3_600_000,
                earlier_epoch_keys: vec![],
            },
        )
        .unwrap();
        let planted_bytes = encode_signed_invitation(&planted).unwrap();
        publish_to_all(&a.transports().await, &planted_bytes).await;
        assert!(verify_fetched_invitation(&planted_bytes, &code).is_err());

        let b = Device::new();
        join(&b, &encode_join_code(&code).unwrap(), &folder, now).await.unwrap();
        assert!(b.trusts(&a));
        assert!(!b.database.known_device_roster().unwrap().iter().any(|(id, _)| *id == mallory_id));
    }

    #[tokio::test]
    async fn a_redemption_without_the_code_or_reusing_a_device_id_is_never_admitted() {
        let folder = SharedFolder::new();
        let (a, c) = group(&folder).await;
        let now = now_ms();
        let code = decode_join_code(&create(&a, now).await).unwrap();
        let transports = a.transports().await;
        let intruder = SigningKey::generate(&mut threestrands_sync_envelope::os_rng());

        // Signed with an invite key that isn't the code's.
        let forged = sign_invitation_redemption(
            &invite_ed25519_signing_key(&[7u8; 32]),
            &intruder,
            InvitationRedemption {
                invitation_cid: code.invitation_cid.clone(),
                device_id: EnvelopeDeviceId::from_bytes([0xab; 16]),
                ed25519_public: ByteBuf::from(intruder.verifying_key().to_bytes().to_vec()),
                x25519_public: ByteBuf::from(vec![4u8; 32]),
                device_name: "Intruder".to_string(),
                created_at_ms: now,
            },
        )
        .unwrap();
        publish_to_all(&transports, &encode_signed_invitation_redemption(&forged).unwrap()).await;

        // Holding the code, but claiming C's device id to replace C's keys.
        let hijack = sign_invitation_redemption(
            &invite_ed25519_signing_key(&code.invite_secret()),
            &intruder,
            InvitationRedemption {
                invitation_cid: code.invitation_cid.clone(),
                device_id: c.identity.device_id,
                ed25519_public: ByteBuf::from(intruder.verifying_key().to_bytes().to_vec()),
                x25519_public: ByteBuf::from(vec![5u8; 32]),
                device_name: "Not C".to_string(),
                created_at_ms: now + 1,
            },
        )
        .unwrap();
        publish_to_all(&transports, &encode_signed_invitation_redemption(&hijack).unwrap()).await;

        a.sweep().await;
        assert!(!a.process(now + 2_000).await);
        let c_key = a.database.known_device_roster().unwrap().into_iter().find(|(id, _)| *id == c.identity.device_id).unwrap().1;
        assert_eq!(c_key, c.identity.verifying_key);
        let codes = a.database.outstanding_join_codes().unwrap();
        assert_eq!(codes[0].status, "open");
        assert_eq!(codes[0].rejected_attempts, 1, "only the code-holding attempt is recorded");
    }

    #[tokio::test]
    async fn an_invitation_alone_is_not_evidence_of_a_group() {
        let transports = fake_transports("empty");
        let key = SigningKey::generate(&mut threestrands_sync_envelope::os_rng());
        let id = EnvelopeDeviceId::from_bytes([1; 16]);
        let invitation = sign_invitation(
            &key,
            Invitation {
                inviter_device_id: id,
                invite_ed25519_public: ByteBuf::from(vec![1u8; 32]),
                invite_x25519_public: ByteBuf::from(vec![2u8; 32]),
                key_epoch: 0,
                sealed_epoch_key: ByteBuf::from(vec![3u8; 72]),
                roster: vec![RosterEntry {
                    device_id: id,
                    ed25519_public: ByteBuf::from(key.verifying_key().to_bytes().to_vec()),
                    x25519_public: ByteBuf::from(vec![4u8; 32]),
                    status: "active".to_string(),
                }],
                recovery_ed25519_public: ByteBuf::from(vec![5u8; 32]),
                recovery_x25519_public: ByteBuf::from(vec![6u8; 32]),
                created_at_ms: 1,
                expires_at_ms: 2,
                earlier_epoch_keys: vec![],
            },
        )
        .unwrap();
        publish_to_all(&transports, &encode_signed_invitation(&invitation).unwrap()).await;
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);
    }

    #[tokio::test]
    async fn joining_is_refused_for_an_enrolled_device_or_an_expired_code() {
        let folder = SharedFolder::new();
        let (a, c) = group(&folder).await;
        let now = now_ms();
        let code = create(&a, now).await;
        let error = join(&c, &code, &folder, now).await.unwrap_err();
        assert!(matches!(error, EnrollmentError::AlreadyEnrolled), "{error}");

        let b = Device::new();
        let error = join(&b, &code, &folder, now + 25 * 3_600_000).await.unwrap_err();
        assert!(matches!(error, EnrollmentError::JoinCodeExpired), "{error}");
        assert!(b.database.configured_transports().unwrap().is_empty());

        let error = join_with_code(&b.database, &b.identity, &b.epoch_keys, &code, &[], vec![], now).await.unwrap_err();
        assert!(matches!(&error, EnrollmentError::Invalid(message) if message.contains("Choose this device's copy")), "{error}");
    }

    #[tokio::test]
    async fn leaving_the_group_forgets_join_codes() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        create(&a, now_ms()).await;
        let b = Device::new();
        join(&b, &create(&a, now_ms()).await, &folder, now_ms()).await.unwrap();
        a.sweep().await;
        a.database.leave_sync_space().unwrap();
        b.database.leave_sync_space().unwrap();
        for database in [&a.database, &b.database] {
            let count: i64 = database
                .test_connection()
                .unwrap()
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM replicated_sync_invitations) + (SELECT COUNT(*) FROM replicated_sync_invitation_redemptions)",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[tokio::test]
    async fn creating_a_code_validates_lifetime_and_connectors() {
        let folder = SharedFolder::new();
        let (a, _c) = group(&folder).await;
        let now = now_ms();
        let transports = a.transports().await;
        let create_for = |hours: u32, choices: Vec<JoinCodeConnectorChoice>| {
            let transports = transports.clone();
            let a = &a;
            async move { create_join_code(&a.database, &a.identity, &a.keys(), &transports, &choices, hours, now).await }
        };
        for (hours, ok) in [
            (MIN_JOIN_CODE_HOURS - 1, false),
            (MIN_JOIN_CODE_HOURS, true),
            (MIN_JOIN_CODE_HOURS + 1, true),
            (MAX_JOIN_CODE_HOURS - 1, true),
            (MAX_JOIN_CODE_HOURS, true),
            (MAX_JOIN_CODE_HOURS + 1, false),
        ] {
            assert_eq!(create_for(hours, vec![choice("folder-a", true)]).await.is_ok(), ok, "{hours} hours");
        }
        assert!(create_for(24, vec![]).await.is_err());
        assert!(create_for(24, vec![choice("missing", true)]).await.is_err());
        let too_many = vec![choice("folder-a", true); threestrands_sync_envelope::limits::MAX_JOIN_CONNECTORS + 1];
        let before = a.database.outstanding_join_codes().unwrap().len();
        assert!(create_for(24, too_many).await.is_err());
        assert_eq!(a.database.outstanding_join_codes().unwrap().len(), before, "nothing published for a code that can't encode");
    }

    /// Every text or blob value in every table, for "this secret is
    /// stored nowhere in SQLite" assertions.
    fn database_contains(database: &Database, needle: &[u8]) -> bool {
        let connection = database.test_connection().unwrap();
        let tables: Vec<String> = connection
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for table in tables {
            let mut statement = connection.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for index in 0..columns {
                    let bytes: &[u8] = match row.get_ref(index).unwrap() {
                        rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => bytes,
                        _ => continue,
                    };
                    if bytes.windows(needle.len()).any(|window| window == needle) {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[tokio::test]
    async fn a_join_code_over_s3_carries_everything_and_secrets_stay_out_of_sqlite() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let a = Device::new();
        let a_connector = format!("s3-{}", uuid::Uuid::new_v4());
        a.database.add_s3_transport(&a_connector, &server.config("group"), &FakeS3Server::credentials()).unwrap();
        begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();

        let now = now_ms();
        let code = create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice(&a_connector, true)], 1, now)
            .await
            .unwrap();
        let secret = decode_join_code(&code).unwrap().invite_secret();
        assert!(!database_contains(&a.database, code.as_bytes()));
        assert!(!database_contains(&a.database, &secret));
        assert!(!database_contains(&a.database, crate::s3_transport::fake_server::SECRET_KEY.as_bytes()));

        let preview = preview_join_code(&code, now).unwrap();
        assert_eq!(preview.connectors.len(), 1);
        assert!(preview.connectors[0].supported && preview.connectors[0].credentials_included);
        assert!(!preview.connectors[0].needs_credentials && !preview.connectors[0].needs_folder);

        // No input beyond the code.
        let b = Device::new();
        join_with_code(&b.database, &b.identity, &b.epoch_keys, &code, &[], vec![], now).await.unwrap();
        let rows = b.database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "s3");
        assert!(!database_contains(&b.database, crate::s3_transport::fake_server::SECRET_KEY.as_bytes()));
        assert!(!database_contains(&b.database, &secret));
        assert_eq!(
            crate::sync_connectors::TransportSecrets::load("s3", &rows[0].instance_id).unwrap(),
            Some(crate::sync_connectors::TransportSecrets::S3(FakeS3Server::credentials()))
        );

        a.sweep().await;
        assert!(a.process(now + 1_000).await);
        b.sweep().await;
        assert_eq!(b.awaiting_admission(), None);
    }

    #[tokio::test]
    async fn a_code_without_credentials_asks_the_joiner_for_them() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let a = Device::new();
        let a_connector = format!("s3-{}", uuid::Uuid::new_v4());
        a.database.add_s3_transport(&a_connector, &server.config("group"), &FakeS3Server::credentials()).unwrap();
        begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();
        let now = now_ms();
        let code = create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice(&a_connector, false)], 1, now)
            .await
            .unwrap();
        assert!(decode_join_code(&code).unwrap().connectors[0].secrets_json.is_none());
        assert!(preview_join_code(&code, now).unwrap().connectors[0].needs_credentials);

        let b = Device::new();
        let error = join_with_code(&b.database, &b.identity, &b.epoch_keys, &code, &[], vec![], now).await.unwrap_err();
        assert!(matches!(&error, EnrollmentError::Connector(message) if message.contains("access key")), "{error}");
        let credentials: crate::sync_connectors::ConnectorCredentials = serde_json::from_value(serde_json::json!({
            "kind": "s3",
            "accessKeyId": crate::s3_transport::fake_server::ACCESS_KEY,
            "secretAccessKey": crate::s3_transport::fake_server::SECRET_KEY,
        }))
        .unwrap();
        join_with_code(
            &b.database,
            &b.identity,
            &b.epoch_keys,
            &code,
            &[],
            vec![JoinCredentialsChoice { connector_index: 0, credentials }],
            now,
        )
        .await
        .unwrap();
        assert!(b.awaiting_admission().is_some());
    }

    #[test]
    fn preview_describes_each_connector_without_side_effects() {
        let code = threestrands_sync_envelope::JoinCode {
            version: 1,
            invite_secret: ByteBuf::from(vec![1u8; 32]),
            invitation_cid: compute_cid(b"invitation"),
            inviter_name: "Laptop".to_string(),
            expires_at_ms: 2_000_000_000_000,
            connectors: vec![
                threestrands_sync_envelope::JoinConnector {
                    kind: "folder".to_string(),
                    config_json: r#"{"folderName":"Dropbox Sync","label":"Home"}"#.to_string(),
                    secrets_json: None,
                },
                threestrands_sync_envelope::JoinConnector {
                    kind: "ipfs_rpc".to_string(),
                    config_json: serde_json::json!({"baseUrl": crate::ipfs_transport::FILEBASE_RPC_URL}).to_string(),
                    secrets_json: None,
                },
                threestrands_sync_envelope::JoinConnector {
                    kind: "carrier-pigeon".to_string(),
                    config_json: "{}".to_string(),
                    secrets_json: None,
                },
            ],
        };
        let text = encode_join_code(&code).unwrap();
        let preview = preview_join_code(&text, 1_000).unwrap();
        assert_eq!(preview.inviter_name, "Laptop");
        assert!(!preview.expired);
        assert!(preview_join_code(&text, 3_000_000_000_000).unwrap().expired);
        let folder = &preview.connectors[0];
        assert!(folder.supported && folder.needs_folder);
        assert_eq!(folder.folder_name.as_deref(), Some("Dropbox Sync"));
        assert_eq!(folder.label.as_deref(), Some("Home"));
        let ipfs = &preview.connectors[1];
        assert!(ipfs.supported && !ipfs.needs_credentials && !ipfs.needs_folder);
        assert_eq!(ipfs.location, crate::ipfs_transport::FILEBASE_RPC_URL);
        assert!(!preview.connectors[2].supported);

        assert!(matches!(
            preview_join_code("hello", 0).unwrap_err(),
            EnrollmentError::JoinCode(threestrands_sync_envelope::JoinCodeError::NotAJoinCode)
        ));
        assert!(matches!(
            preview_join_code("TSJOIN9-abc", 0).unwrap_err(),
            EnrollmentError::JoinCode(threestrands_sync_envelope::JoinCodeError::NewerVersion)
        ));
    }

    #[test]
    fn connectors_of_unknown_kinds_are_skipped_and_all_unknown_is_an_error() {
        let base = threestrands_sync_envelope::JoinCode {
            version: 1,
            invite_secret: ByteBuf::from(vec![1u8; 32]),
            invitation_cid: compute_cid(b"invitation"),
            inviter_name: String::new(),
            expires_at_ms: 1,
            connectors: vec![threestrands_sync_envelope::JoinConnector {
                kind: "carrier-pigeon".to_string(),
                config_json: "{}".to_string(),
                secrets_json: None,
            }],
        };
        let error = join_codes::prepare_join_connectors(&base, &[], vec![]).unwrap_err();
        assert!(matches!(&error, EnrollmentError::Connector(message) if message.contains("Update this app")), "{error}");
        let mut mixed = base;
        mixed.connectors.push(threestrands_sync_envelope::JoinConnector {
            kind: "ipfs_rpc".to_string(),
            config_json: serde_json::json!({"baseUrl": crate::ipfs_transport::FILEBASE_RPC_URL}).to_string(),
            secrets_json: Some(r#"{"token":"t"}"#.to_string()),
        });
        let prepared = join_codes::prepare_join_connectors(&mixed, &[], vec![]).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].0.kind(), "ipfs_rpc");
    }

    #[tokio::test]
    async fn find_invitation_says_why_the_invitation_could_not_be_fetched() {
        let code = threestrands_sync_envelope::JoinCode {
            version: 1,
            invite_secret: ByteBuf::from(vec![1u8; 32]),
            invitation_cid: compute_cid(b"invitation"),
            inviter_name: String::new(),
            expires_at_ms: 1,
            connectors: vec![],
        };
        let rejecting = threestrands_sync_transport::fake::FakeTransport::new("rejecting");
        rejecting.set_authentication_failure(true);
        let offline = threestrands_sync_transport::fake::FakeTransport::new("offline");
        offline.inject_transient_outage(10);
        let empty = threestrands_sync_transport::fake::FakeTransport::new("empty");
        let as_transports = |list: Vec<threestrands_sync_transport::fake::FakeTransport>| -> Vec<Arc<dyn SyncTransport>> {
            list.into_iter().map(|transport| Arc::new(transport) as Arc<dyn SyncTransport>).collect()
        };

        let error = find_invitation(&code, &as_transports(vec![rejecting])).await.err().unwrap();
        assert!(matches!(error, EnrollmentError::CredentialsRejected), "{error}");
        let error = find_invitation(&code, &as_transports(vec![offline])).await.err().unwrap();
        assert!(matches!(error, EnrollmentError::StorageUnreachable), "{error}");

        // Rejected credentials outrank an outage or an empty connector.
        let rejecting = threestrands_sync_transport::fake::FakeTransport::new("rejecting");
        rejecting.set_authentication_failure(true);
        let offline = threestrands_sync_transport::fake::FakeTransport::new("offline");
        offline.inject_transient_outage(10);
        let error = find_invitation(&code, &as_transports(vec![empty, offline, rejecting])).await.err().unwrap();
        assert!(matches!(error, EnrollmentError::CredentialsRejected), "{error}");

        // An object that doesn't match the code outranks everything.
        let wrong = threestrands_sync_transport::fake::FakeTransport::new("wrong");
        wrong.put_object(&TransportCid(code.invitation_cid.clone()), b"not the invitation").await.unwrap();
        let rejecting = threestrands_sync_transport::fake::FakeTransport::new("rejecting");
        rejecting.set_authentication_failure(true);
        let error = find_invitation(&code, &as_transports(vec![rejecting, wrong])).await.err().unwrap();
        assert!(matches!(error, EnrollmentError::InvitationDamaged), "{error}");
    }

    #[tokio::test]
    async fn joining_with_rejected_s3_credentials_says_so_and_saves_nothing() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let a = Device::new();
        let a_connector = format!("s3-{}", uuid::Uuid::new_v4());
        a.database.add_s3_transport(&a_connector, &server.config("group"), &FakeS3Server::credentials()).unwrap();
        begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();
        let now = now_ms();
        let code = create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice(&a_connector, false)], 1, now)
            .await
            .unwrap();

        let b = Device::new();
        let wrong: crate::sync_connectors::ConnectorCredentials = serde_json::from_value(serde_json::json!({
            "kind": "s3",
            "accessKeyId": crate::s3_transport::fake_server::ACCESS_KEY,
            "secretAccessKey": "a-revoked-secret",
        }))
        .unwrap();
        let error = join_with_code(
            &b.database,
            &b.identity,
            &b.epoch_keys,
            &code,
            &[],
            vec![JoinCredentialsChoice { connector_index: 0, credentials: wrong }],
            now,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, EnrollmentError::CredentialsRejected), "{error}");
        assert!(b.database.configured_transports().unwrap().is_empty());
        assert!(matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
    }

    #[tokio::test]
    async fn find_invitation_reports_a_missing_object() {
        let code = threestrands_sync_envelope::JoinCode {
            version: 1,
            invite_secret: ByteBuf::from(vec![1u8; 32]),
            invitation_cid: compute_cid(b"invitation"),
            inviter_name: String::new(),
            expires_at_ms: 1,
            connectors: vec![],
        };
        let error = find_invitation(&code, &fake_transports("empty")).await.err().unwrap();
        assert!(matches!(error, EnrollmentError::InvitationNotFound), "{error}");
    }
}
