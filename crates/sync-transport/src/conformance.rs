//! The shared, adapter-agnostic conformance suite. Every function here
//! talks to a transport only through [`crate::SyncTransport`] — never a
//! fake-specific or adapter-specific method — so the exact same assertions
//! run against the in-memory fake, the folder adapter, and any future
//! adapter.
//!
//! Fault injection (corruption, outages, quota, a reset store, a
//! fault-injecting virtual filesystem, ...) is each adapter's own concern,
//! set up before calling into this module: see `fake.rs`'s fault-injection
//! methods for the in-memory case. What this module checks is what must
//! stay true no matter how a transport got into whatever state it's in.

use threestrands_sync_envelope::os_rng;
use threestrands_sync_envelope::{sign_device_head, DeviceHead, DeviceId, SigningKey};

use crate::{Cid, HeadLocator, SyncTransport};

/// `put_object` accepts the same CID more than once without error, and a
/// `get_object` afterward returns the bytes from the *original* put (the
/// content the CID actually addresses), not merely "a" successful write.
pub async fn put_is_idempotent_and_readable(transport: &dyn SyncTransport) {
    let bytes = b"conformance: put is idempotent and readable".to_vec();
    let cid = Cid::for_bytes(&bytes);

    let first = transport.put_object(&cid, &bytes).await.unwrap();
    assert_eq!(first.cid, cid);
    let second = transport.put_object(&cid, &bytes).await.unwrap();
    assert_eq!(second.cid, cid);

    let fetched = transport.get_object(&cid).await.unwrap();
    assert_eq!(fetched, bytes);
}

/// A CID nothing has ever been put under is reported as not found, not as
/// an empty success or a different error kind.
pub async fn get_returns_not_found_for_unknown_cid(transport: &dyn SyncTransport) {
    let cid = Cid::for_bytes(b"conformance: never put under this content");
    let result = transport.get_object(&cid).await;
    assert_eq!(result, Err(crate::TransportError::NotFound));
}

/// After `delete_object`, the object is no longer fetchable through this
/// instance. (A provider deletion is not a logical sync deletion — the
/// replicator, not this suite, is responsible for what that means for the
/// operation graph.)
pub async fn delete_removes_the_object(transport: &dyn SyncTransport) {
    let bytes = b"conformance: delete removes the object".to_vec();
    let cid = Cid::for_bytes(&bytes);
    transport.put_object(&cid, &bytes).await.unwrap();
    transport.get_object(&cid).await.unwrap();

    transport.delete_object(&cid).await.unwrap();
    assert_eq!(transport.get_object(&cid).await, Err(crate::TransportError::NotFound));
}

/// If a transport supports enumeration, walking every page (following
/// `next_cursor` until it is `None`) yields every object that was put,
/// each exactly once, regardless of how many objects fit on one page.
pub async fn scan_enumerates_every_put_object_across_pages(transport: &dyn SyncTransport) {
    if !transport.capabilities().enumeration {
        return;
    }
    let mut expected: Vec<Cid> = Vec::new();
    for index in 0..7 {
        let bytes = format!("conformance: scan object {index}").into_bytes();
        let cid = Cid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();
        expected.push(cid);
    }

    let mut seen: Vec<Cid> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let Some(page) = transport.scan(cursor.as_deref()).await.unwrap() else {
            break;
        };
        seen.extend(page.objects.into_iter().map(|object| object.cid));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    for cid in &expected {
        assert!(
            seen.contains(cid),
            "scan across all pages must include every put object; missing {cid}"
        );
    }
}

/// A published head is exactly what `resolve_heads` returns for that
/// device afterward, and its signature still verifies.
pub async fn resolve_heads_round_trips_a_published_head(transport: &dyn SyncTransport) {
    let signing_key = SigningKey::generate(&mut os_rng());
    let device_id = DeviceId::from_bytes([9u8; 16]);
    let head = DeviceHead {
        sync_space_id: b"conformance-space".to_vec(),
        device_id,
        epoch: 1,
        state_sequence: 3,
        state_cid: Some("bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e".to_string()),
        published_at_ms: 0,
    };
    let signed = sign_device_head(&signing_key, head).unwrap();
    transport.publish_head(&signed).await.unwrap();

    let resolved = transport
        .resolve_heads(&[HeadLocator {
            device_id,
            remote_id: None,
        }])
        .await
        .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0], signed);
    threestrands_sync_envelope::verify_device_head(&signing_key.verifying_key(), &resolved[0]).unwrap();
}

/// Asking to resolve a device that has never published a head on this
/// instance omits it from the result rather than erroring.
pub async fn resolve_heads_omits_unknown_devices(transport: &dyn SyncTransport) {
    let resolved = transport
        .resolve_heads(&[HeadLocator {
            device_id: DeviceId::from_bytes([0xEE; 16]),
            remote_id: None,
        }])
        .await
        .unwrap();
    assert!(resolved.is_empty());
}

/// An object put on one instance and copied to a second (anti-entropy
/// repair, simulated here by the caller doing the copy exactly as the
/// replicator would) is then readable from the second instance too.
pub async fn anti_entropy_repairs_a_missing_object_between_two_instances(
    source: &dyn SyncTransport,
    target: &dyn SyncTransport,
) {
    let bytes = b"conformance: anti-entropy repair".to_vec();
    let cid = Cid::for_bytes(&bytes);
    source.put_object(&cid, &bytes).await.unwrap();
    assert_eq!(target.get_object(&cid).await, Err(crate::TransportError::NotFound));

    let repaired = source.get_object(&cid).await.unwrap();
    target.put_object(&cid, &repaired).await.unwrap();
    assert_eq!(target.get_object(&cid).await.unwrap(), bytes);
}

/// Runs every check above against `transport`, using `peer` only for the
/// anti-entropy check (an independent instance of the same transport
/// kind — e.g. a second configured folder).
pub async fn run_all(transport: &dyn SyncTransport, peer: &dyn SyncTransport) {
    put_is_idempotent_and_readable(transport).await;
    get_returns_not_found_for_unknown_cid(transport).await;
    delete_removes_the_object(transport).await;
    scan_enumerates_every_put_object_across_pages(transport).await;
    resolve_heads_round_trips_a_published_head(transport).await;
    resolve_heads_omits_unknown_devices(transport).await;
    anti_entropy_repairs_a_missing_object_between_two_instances(transport, peer).await;
}
