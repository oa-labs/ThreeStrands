use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use threestrands_sync_envelope::SignedDeviceHead;
use threestrands_sync_transport::conformance;
use threestrands_sync_transport::fake::FakeTransport;
use threestrands_sync_transport::{
    Cid, HeadLocator, ObjectLocator, ScanPage, SyncTransport, TransportCapabilities, TransportError, TransportHealth,
    TransportInstanceId,
};

/// Delegates every call to an inner transport while recording what `scan`
/// returned, so a test can observe how a shared conformance check actually
/// walked the enumeration (how many pages, which cursors, which objects).
/// Optionally hides enumeration, standing in for an adapter that reports
/// no scan support and answers `scan` the way the trait contract says it
/// must: `Ok(None)`.
struct ObservedTransport<T> {
    inner: T,
    hide_enumeration: bool,
    scans: Mutex<Vec<Option<ScanPage>>>,
}

impl<T> ObservedTransport<T> {
    fn new(inner: T) -> Self {
        Self { inner, hide_enumeration: false, scans: Mutex::new(Vec::new()) }
    }

    fn without_enumeration(inner: T) -> Self {
        Self { hide_enumeration: true, ..Self::new(inner) }
    }

    fn scans(&self) -> Vec<Option<ScanPage>> {
        self.scans.lock().unwrap().clone()
    }
}

#[async_trait]
impl<T: SyncTransport> SyncTransport for ObservedTransport<T> {
    fn instance_id(&self) -> TransportInstanceId {
        self.inner.instance_id()
    }

    fn capabilities(&self) -> TransportCapabilities {
        let mut capabilities = self.inner.capabilities();
        if self.hide_enumeration {
            capabilities.enumeration = false;
            capabilities.incremental_cursor = false;
        }
        capabilities
    }

    async fn put_object(&self, cid: &Cid, bytes: &[u8]) -> Result<ObjectLocator, TransportError> {
        self.inner.put_object(cid, bytes).await
    }

    async fn get_object(&self, cid: &Cid) -> Result<Vec<u8>, TransportError> {
        self.inner.get_object(cid).await
    }

    async fn publish_head(&self, head: &SignedDeviceHead) -> Result<HeadLocator, TransportError> {
        self.inner.publish_head(head).await
    }

    async fn resolve_heads(&self, known: &[HeadLocator]) -> Result<Vec<SignedDeviceHead>, TransportError> {
        self.inner.resolve_heads(known).await
    }

    async fn scan(&self, cursor: Option<&str>) -> Result<Option<ScanPage>, TransportError> {
        let result = if self.hide_enumeration { Ok(None) } else { self.inner.scan(cursor).await };
        if let Ok(page) = &result {
            self.scans.lock().unwrap().push(page.clone());
        }
        result
    }

    async fn delete_object(&self, cid: &Cid) -> Result<(), TransportError> {
        self.inner.delete_object(cid).await
    }

    async fn health(&self) -> Result<TransportHealth, TransportError> {
        self.inner.health().await
    }
}

fn fake_with_page_size(instance: &str, page_size: usize) -> FakeTransport {
    let transport = FakeTransport::new(instance);
    transport.set_page_size(page_size);
    transport
}

#[tokio::test]
async fn the_fake_transport_passes_its_own_conformance_suite() {
    let a = FakeTransport::new("a");
    let b = FakeTransport::new("b");
    conformance::run_all(&a, &b).await;
}

/// The whole suite must also pass when the transport under test paginates
/// its enumeration, including the degenerate one-object-per-page case.
#[tokio::test]
async fn the_fake_transport_passes_its_conformance_suite_with_small_scan_pages() {
    for page_size in [1, 2, 3] {
        let a = fake_with_page_size("a", page_size);
        let b = fake_with_page_size("b", page_size);
        conformance::run_all(&a, &b).await;
    }
}

/// Runs the shared `scan_enumerates_every_put_object_across_pages` check
/// against a fake whose page size is smaller than the number of objects the
/// check puts, and observes that the check really walked several pages
/// (following a non-`None` cursor) and saw every object exactly once.
#[tokio::test]
async fn scan_enumerates_every_put_object_across_pages() {
    const PAGE_SIZE: usize = 2;
    let transport = ObservedTransport::new(fake_with_page_size("a", PAGE_SIZE));
    conformance::scan_enumerates_every_put_object_across_pages(&transport).await;

    let pages: Vec<ScanPage> = transport.scans().into_iter().map(|page| page.expect("the fake enumerates")).collect();
    assert!(pages.len() > 1, "the check must fetch more than one page, fetched {}", pages.len());
    assert!(
        pages[..pages.len() - 1].iter().all(|page| page.next_cursor.is_some()),
        "every page but the last must hand back a cursor"
    );
    assert_eq!(pages.last().unwrap().next_cursor, None, "the walk ends on a page with no cursor");
    assert!(pages.iter().all(|page| page.objects.len() <= PAGE_SIZE), "no page may exceed the page size");

    let mut counts: HashMap<Cid, usize> = HashMap::new();
    for object in pages.iter().flat_map(|page| &page.objects) {
        *counts.entry(object.cid.clone()).or_default() += 1;
    }
    let expected: Vec<Cid> =
        (0..7).map(|index| Cid::for_bytes(format!("conformance: scan object {index}").as_bytes())).collect();
    assert_eq!(counts.len(), expected.len(), "only the put objects are enumerated: {counts:?}");
    for cid in &expected {
        assert_eq!(counts.get(cid), Some(&1), "{cid} must be enumerated exactly once across all pages");
    }
    assert!(pages.len() >= expected.len().div_ceil(PAGE_SIZE));
}

/// Checks that what a transport reports in `capabilities()` matches how its
/// `scan` actually behaves. The trait's contract for "no enumeration" is
/// `Ok(None)` (not an error), so that is what a non-enumerating transport
/// must return; an enumerating one must return a page listing what was put,
/// and an incremental cursor must resume without repeating earlier objects.
async fn assert_scan_matches_reported_capabilities(transport: &dyn SyncTransport) {
    let mut put: Vec<Cid> = Vec::new();
    for index in 0..3 {
        let bytes = format!("capability check object {index}").into_bytes();
        let cid = Cid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();
        put.push(cid);
    }

    let capabilities = transport.capabilities();
    let first = transport.scan(None).await.expect("scan must not error on a healthy transport");
    if !capabilities.enumeration {
        assert_eq!(first, None, "a transport reporting no enumeration must answer scan with Ok(None)");
        return;
    }

    let mut seen: Vec<Cid> = Vec::new();
    let mut page = first.expect("a transport reporting enumeration must return a scan page");
    loop {
        for object in &page.objects {
            assert!(!seen.contains(&object.cid), "resuming a cursor must not repeat {}", object.cid);
            seen.push(object.cid.clone());
        }
        let Some(cursor) = page.next_cursor.take() else { break };
        assert!(capabilities.incremental_cursor, "a cursor was returned without incremental_cursor support");
        page = transport.scan(Some(&cursor)).await.unwrap().expect("a fresh cursor must still be recognized");
    }
    for cid in &put {
        assert!(seen.contains(cid), "an enumerating transport must list {cid}");
    }
}

#[tokio::test]
async fn scan_capability_matches_observed_scan_behavior() {
    let fake = FakeTransport::new("a");
    assert!(fake.capabilities().enumeration);
    assert!(fake.capabilities().incremental_cursor);
    assert_scan_matches_reported_capabilities(&fake).await;

    // The same check with pagination forced, so the cursor path is taken.
    let paged = ObservedTransport::new(fake_with_page_size("b", 1));
    assert_scan_matches_reported_capabilities(&paged).await;
    assert!(paged.scans().iter().filter(|page| page.as_ref().is_some_and(|p| p.next_cursor.is_some())).count() >= 2);

    // A transport that reports no enumeration answers scan with Ok(None),
    // and the shared enumeration check skips it rather than failing.
    let hidden = ObservedTransport::without_enumeration(FakeTransport::new("c"));
    assert!(!hidden.capabilities().enumeration);
    assert_scan_matches_reported_capabilities(&hidden).await;
    conformance::scan_enumerates_every_put_object_across_pages(&hidden).await;
    assert_eq!(hidden.scans().len(), 1, "the shared check must not scan a non-enumerating transport");
}
