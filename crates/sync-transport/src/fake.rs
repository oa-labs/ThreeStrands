//! A deterministic in-memory [`SyncTransport`] with fault injection, used
//! to build and test the replicator before any real adapter exists, and to
//! exercise the conformance suite's own tests against a known-good
//! baseline.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use threestrands_sync_envelope::SignedDeviceHead;

use crate::{
    Cid, HeadLocator, ObjectLocator, ScanPage, SyncTransport, TransportCapabilities, TransportError,
    TransportHealth, TransportInstanceId,
};

#[derive(Default)]
struct Faults {
    authentication_failure: bool,
    transient_outages_remaining: usize,
    quota_after_puts: Option<usize>,
    corrupt_gets_remaining: usize,
    delayed_visibility: HashMap<Cid, usize>,
    reorder_scan: bool,
    duplicate_scan_entry: bool,
    page_size: usize,
}

#[derive(Default)]
struct State {
    objects: HashMap<Cid, Vec<u8>>,
    /// Insertion order, for deterministic (absent fault injection) scans.
    scan_order: Vec<Cid>,
    heads: HashMap<threestrands_sync_envelope::DeviceId, SignedDeviceHead>,
    puts_accepted: usize,
    /// Bumped by [`FakeTransport::reset_remote_store`]; a cursor minted
    /// before a reset is no longer recognized, matching a real provider
    /// wiping its store out from under a stored incremental cursor.
    generation: u64,
}

/// An in-memory, fault-injectable [`SyncTransport`]. Every fault-injection
/// method takes `&self` (interior mutability) so a test can configure
/// faults on a transport it has already handed to code under test.
pub struct FakeTransport {
    instance_id: TransportInstanceId,
    state: Mutex<State>,
    faults: Mutex<Faults>,
}

impl FakeTransport {
    pub fn new(instance_id: impl Into<String>) -> Self {
        Self {
            instance_id: TransportInstanceId(instance_id.into()),
            state: Mutex::new(State {
                puts_accepted: 0,
                ..State::default()
            }),
            faults: Mutex::new(Faults {
                page_size: usize::MAX,
                ..Faults::default()
            }),
        }
    }

    /// The next `count` calls to any method fail with `Transient`.
    pub fn inject_transient_outage(&self, count: usize) {
        self.faults.lock().unwrap().transient_outages_remaining = count;
    }

    /// Every call fails with `Authentication` until this is called again
    /// with `false` — simulating a revoked or expired credential.
    pub fn set_authentication_failure(&self, failing: bool) {
        self.faults.lock().unwrap().authentication_failure = failing;
    }

    /// `put_object` succeeds for the first `after` accepted puts, then
    /// fails with `Quota` from then on.
    pub fn inject_quota_after(&self, after: usize) {
        self.faults.lock().unwrap().quota_after_puts = Some(after);
    }

    /// The next `count` calls to `get_object` return bit-flipped bytes.
    pub fn inject_corruption_on_get(&self, count: usize) {
        self.faults.lock().unwrap().corrupt_gets_remaining = count;
    }

    /// `cid` is excluded from `get_object` (returns `NotFound`) and from
    /// `scan` results for the next `calls` attempts against it, then
    /// becomes visible — simulating a cloud-folder client that has
    /// accepted an upload but not yet hydrated it everywhere.
    pub fn inject_delayed_visibility(&self, cid: &Cid, calls: usize) {
        self.faults
            .lock()
            .unwrap()
            .delayed_visibility
            .insert(cid.clone(), calls);
    }

    /// `scan` returns its objects in reverse insertion order.
    pub fn enable_scan_reordering(&self) {
        self.faults.lock().unwrap().reorder_scan = true;
    }

    /// `scan` includes a duplicate entry for the first object, on top of
    /// whatever `put_object` at-least-once duplication a caller injects
    /// itself by calling `put_object` twice.
    pub fn enable_duplicate_scan_entry(&self) {
        self.faults.lock().unwrap().duplicate_scan_entry = true;
    }

    /// Caps how many objects one `scan` page returns, forcing pagination.
    pub fn set_page_size(&self, size: usize) {
        self.faults.lock().unwrap().page_size = size.max(1);
    }

    /// Wipes every stored object and head and invalidates every
    /// outstanding scan cursor — simulating the remote store being reset
    /// (the folder deleted and recreated, a bucket emptied, ...).
    pub fn reset_remote_store(&self) {
        let mut state = self.state.lock().unwrap();
        state.objects.clear();
        state.scan_order.clear();
        state.heads.clear();
        state.generation += 1;
    }

    /// How many objects this instance currently holds. Test-only
    /// introspection; not part of `SyncTransport`.
    pub fn object_count(&self) -> usize {
        self.state.lock().unwrap().objects.len()
    }

    fn take_fault(&self) -> Result<(), TransportError> {
        let mut faults = self.faults.lock().unwrap();
        if faults.authentication_failure {
            return Err(TransportError::Authentication(
                "the configured credential was rejected".to_string(),
            ));
        }
        if faults.transient_outages_remaining > 0 {
            faults.transient_outages_remaining -= 1;
            return Err(TransportError::Transient("injected outage".to_string()));
        }
        Ok(())
    }
}

#[async_trait]
impl SyncTransport for FakeTransport {
    fn instance_id(&self) -> TransportInstanceId {
        self.instance_id.clone()
    }

    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            enumeration: true,
            incremental_cursor: true,
            head_discovery: true,
        }
    }

    async fn put_object(&self, cid: &Cid, bytes: &[u8]) -> Result<ObjectLocator, TransportError> {
        self.take_fault()?;
        if let Some(after) = self.faults.lock().unwrap().quota_after_puts {
            if self.state.lock().unwrap().puts_accepted >= after {
                return Err(TransportError::Quota("storage quota exceeded".to_string()));
            }
        }
        let mut state = self.state.lock().unwrap();
        let already_present = state.objects.contains_key(cid);
        state.objects.insert(cid.clone(), bytes.to_vec());
        if !already_present {
            state.scan_order.push(cid.clone());
        }
        state.puts_accepted += 1;
        Ok(ObjectLocator {
            cid: cid.clone(),
            remote_id: Some(cid.0.clone()),
        })
    }

    async fn get_object(&self, cid: &Cid) -> Result<Vec<u8>, TransportError> {
        self.take_fault()?;
        if let Some(remaining) = self.faults.lock().unwrap().delayed_visibility.get_mut(cid) {
            if *remaining > 0 {
                *remaining -= 1;
                return Err(TransportError::NotFound);
            }
        }
        let bytes = {
            let state = self.state.lock().unwrap();
            state.objects.get(cid).cloned().ok_or(TransportError::NotFound)?
        };
        let mut faults = self.faults.lock().unwrap();
        if faults.corrupt_gets_remaining > 0 {
            faults.corrupt_gets_remaining -= 1;
            let mut corrupted = bytes;
            if let Some(byte) = corrupted.first_mut() {
                *byte ^= 0xFF;
            } else {
                corrupted.push(0xFF);
            }
            return Ok(corrupted);
        }
        Ok(bytes)
    }

    async fn publish_head(&self, head: &SignedDeviceHead) -> Result<HeadLocator, TransportError> {
        self.take_fault()?;
        let mut state = self.state.lock().unwrap();
        let device_id = head.head.device_id;
        state.heads.insert(device_id, head.clone());
        Ok(HeadLocator {
            device_id,
            remote_id: None,
        })
    }

    async fn resolve_heads(&self, known: &[HeadLocator]) -> Result<Vec<SignedDeviceHead>, TransportError> {
        self.take_fault()?;
        let state = self.state.lock().unwrap();
        Ok(known
            .iter()
            .filter_map(|locator| state.heads.get(&locator.device_id).cloned())
            .collect())
    }

    async fn scan(&self, cursor: Option<&str>) -> Result<Option<ScanPage>, TransportError> {
        self.take_fault()?;
        let (generation, offset) = match cursor {
            None => (self.state.lock().unwrap().generation, 0),
            Some(cursor) => match parse_cursor(cursor) {
                Some(parsed) => parsed,
                None => return Ok(None),
            },
        };

        let mut listing: Vec<Cid> = {
            let state = self.state.lock().unwrap();
            if state.generation != generation {
                // A cursor minted before a reset is stale: fall back to
                // head-based discovery rather than silently resuming into
                // a different epoch of the store.
                return Ok(None);
            }
            state.scan_order.clone()
        };

        let faults = self.faults.lock().unwrap();
        if faults.reorder_scan {
            listing.reverse();
        }
        if faults.duplicate_scan_entry {
            if let Some(first) = listing.first().cloned() {
                listing.push(first);
            }
        }
        let page_size = faults.page_size;
        drop(faults);

        let visible: Vec<Cid> = {
            let state = self.state.lock().unwrap();
            listing
                .into_iter()
                .filter(|cid| state.objects.contains_key(cid))
                .collect()
        };

        let end = (offset + page_size).min(visible.len());
        let page: Vec<Cid> = visible.get(offset..end).unwrap_or_default().to_vec();
        let next_cursor = if end < visible.len() {
            Some(format!("{generation}:{end}"))
        } else {
            None
        };
        Ok(Some(ScanPage {
            objects: page
                .into_iter()
                .map(|cid| ObjectLocator {
                    remote_id: Some(cid.0.clone()),
                    cid,
                })
                .collect(),
            next_cursor,
        }))
    }

    async fn delete_object(&self, cid: &Cid) -> Result<(), TransportError> {
        self.take_fault()?;
        let mut state = self.state.lock().unwrap();
        state.objects.remove(cid);
        Ok(())
    }

    async fn health(&self) -> Result<TransportHealth, TransportError> {
        let faults = self.faults.lock().unwrap();
        if faults.authentication_failure {
            return Ok(TransportHealth::Unavailable(
                "the configured credential was rejected".to_string(),
            ));
        }
        if faults.transient_outages_remaining > 0 {
            return Ok(TransportHealth::Degraded("recent transient failures".to_string()));
        }
        Ok(TransportHealth::Healthy)
    }
}

fn parse_cursor(cursor: &str) -> Option<(u64, usize)> {
    let (generation, offset) = cursor.split_once(':')?;
    Some((generation.parse().ok()?, offset.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(label: &str) -> Cid {
        Cid::for_bytes(label.as_bytes())
    }

    #[tokio::test]
    async fn transient_outage_fails_the_next_n_calls_then_recovers() {
        let transport = FakeTransport::new("a");
        transport.inject_transient_outage(2);
        assert_eq!(
            transport.put_object(&cid("x"), b"x").await,
            Err(TransportError::Transient("injected outage".to_string()))
        );
        assert_eq!(
            transport.get_object(&cid("x")).await,
            Err(TransportError::Transient("injected outage".to_string()))
        );
        // The third call is not an outage.
        transport.put_object(&cid("x"), b"x").await.unwrap();
    }

    #[tokio::test]
    async fn authentication_failure_blocks_every_call_until_cleared() {
        let transport = FakeTransport::new("a");
        transport.set_authentication_failure(true);
        assert!(matches!(
            transport.put_object(&cid("x"), b"x").await,
            Err(TransportError::Authentication(_))
        ));
        transport.set_authentication_failure(false);
        transport.put_object(&cid("x"), b"x").await.unwrap();
    }

    #[tokio::test]
    async fn quota_is_enforced_after_the_configured_put_count() {
        let transport = FakeTransport::new("a");
        transport.inject_quota_after(1);
        transport.put_object(&cid("x"), b"x").await.unwrap();
        assert_eq!(
            transport.put_object(&cid("y"), b"y").await,
            Err(TransportError::Quota("storage quota exceeded".to_string()))
        );
    }

    #[tokio::test]
    async fn corrupted_bytes_are_returned_for_the_configured_number_of_gets() {
        let transport = FakeTransport::new("a");
        transport.put_object(&cid("x"), b"hello").await.unwrap();
        transport.inject_corruption_on_get(1);
        let corrupted = transport.get_object(&cid("x")).await.unwrap();
        assert_ne!(corrupted, b"hello");
        let clean = transport.get_object(&cid("x")).await.unwrap();
        assert_eq!(clean, b"hello");
    }

    #[tokio::test]
    async fn delayed_visibility_hides_an_object_for_the_configured_number_of_attempts() {
        let transport = FakeTransport::new("a");
        let target = cid("x");
        transport.put_object(&target, b"hello").await.unwrap();
        transport.inject_delayed_visibility(&target, 2);
        assert_eq!(transport.get_object(&target).await, Err(TransportError::NotFound));
        assert_eq!(transport.get_object(&target).await, Err(TransportError::NotFound));
        assert_eq!(transport.get_object(&target).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn pagination_is_crash_safe_across_repeated_resumes_from_a_stored_cursor() {
        let transport = FakeTransport::new("a");
        transport.set_page_size(2);
        let mut expected = Vec::new();
        for index in 0..5 {
            let object_cid = cid(&format!("object-{index}"));
            transport.put_object(&object_cid, b"x").await.unwrap();
            expected.push(object_cid);
        }

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            // Re-fetch the page from the same cursor twice, as if the
            // caller crashed after receiving a page but before persisting
            // that it had processed it: resuming must not skip anything,
            // and re-processing an already-seen page is expected to be
            // handled by the caller's own dedup (idempotent by CID).
            let page = transport.scan(cursor.as_deref()).await.unwrap().unwrap();
            let replay = transport.scan(cursor.as_deref()).await.unwrap().unwrap();
            assert_eq!(page, replay);

            seen.extend(page.objects.into_iter().map(|object| object.cid));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        seen.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(seen, expected_sorted);
    }

    #[tokio::test]
    async fn a_reset_store_invalidates_outstanding_cursors() {
        let transport = FakeTransport::new("a");
        transport.set_page_size(1);
        transport.put_object(&cid("x"), b"x").await.unwrap();
        transport.put_object(&cid("y"), b"y").await.unwrap();
        let first_page = transport.scan(None).await.unwrap().unwrap();
        let cursor = first_page.next_cursor.expect("a second page remains");

        transport.reset_remote_store();
        assert_eq!(transport.scan(Some(&cursor)).await.unwrap(), None);
        // A fresh scan from scratch still works after the reset.
        assert!(transport.scan(None).await.unwrap().is_some());
        assert_eq!(transport.object_count(), 0);
    }

    #[tokio::test]
    async fn reordered_scan_results_still_contain_every_object() {
        let transport = FakeTransport::new("a");
        transport.put_object(&cid("x"), b"x").await.unwrap();
        transport.put_object(&cid("y"), b"y").await.unwrap();
        transport.enable_scan_reordering();
        let page = transport.scan(None).await.unwrap().unwrap();
        let mut cids: Vec<_> = page.objects.into_iter().map(|object| object.cid).collect();
        cids.sort();
        assert_eq!(cids, vec![cid("x"), cid("y")]);
    }

    #[tokio::test]
    async fn duplicate_scan_entries_do_not_hide_the_object_once_deleted() {
        let transport = FakeTransport::new("a");
        transport.put_object(&cid("x"), b"x").await.unwrap();
        transport.enable_duplicate_scan_entry();
        let page = transport.scan(None).await.unwrap().unwrap();
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[0].cid, page.objects[1].cid);

        // A duplicated scan listing entry for an object that has since
        // been deleted must not resurrect it in the page.
        transport.delete_object(&cid("x")).await.unwrap();
        let page = transport.scan(None).await.unwrap().unwrap();
        assert!(page.objects.is_empty());
    }

    #[tokio::test]
    async fn health_reflects_injected_faults() {
        let transport = FakeTransport::new("a");
        assert_eq!(transport.health().await.unwrap(), TransportHealth::Healthy);
        transport.inject_transient_outage(1);
        assert_eq!(
            transport.health().await.unwrap(),
            TransportHealth::Degraded("recent transient failures".to_string())
        );
        transport.set_authentication_failure(true);
        assert!(matches!(
            transport.health().await.unwrap(),
            TransportHealth::Unavailable(_)
        ));
    }
}
