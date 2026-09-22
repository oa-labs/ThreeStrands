use threestrands_sync_transport::conformance;
use threestrands_sync_transport::fake::FakeTransport;
use threestrands_sync_transport::SyncTransport;

#[tokio::test]
async fn the_fake_transport_passes_its_own_conformance_suite() {
    let a = FakeTransport::new("a");
    let b = FakeTransport::new("b");
    conformance::run_all(&a, &b).await;
}

#[tokio::test]
async fn scan_capability_is_reported_correctly() {
    let transport = FakeTransport::new("a");
    assert!(transport.capabilities().enumeration);
    assert!(transport.capabilities().incremental_cursor);
}
