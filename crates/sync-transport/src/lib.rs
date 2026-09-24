//! `sync-transport`: the provider-neutral asynchronous interface every
//! ThreeStrands sync transport (a local folder, an IPFS-compatible RPC
//! endpoint, a future relay) implements, plus a deterministic in-memory
//! fake with fault injection and the conformance suite every adapter must
//! pass.
//!
//! Core rules (see the plan's "Transport abstraction" section):
//!
//! - CIDs and signed per-device heads are the portable discovery model.
//!   Enumeration ([`SyncTransport::scan`]) and incremental cursors are
//!   optional, provider-efficiency-only capabilities, never the only way to
//!   reconstruct history.
//! - `put_object` is at-least-once and idempotent by CID.
//! - A provider deletion is not a logical sync deletion and never inverts
//!   anything already merged.
//! - This crate never branches on a named provider; adapters live outside
//!   it and are held only as `dyn SyncTransport` by callers.

mod error;
pub mod fake;
pub mod types;

pub use error::TransportError;
pub use types::{Cid, HeadLocator, ObjectLocator, ScanPage, TransportCapabilities, TransportHealth, TransportInstanceId};

use async_trait::async_trait;
use threestrands_sync_envelope::SignedDeviceHead;

#[async_trait]
pub trait SyncTransport: Send + Sync {
    fn instance_id(&self) -> TransportInstanceId;
    fn capabilities(&self) -> TransportCapabilities;

    /// Stores `bytes` under `cid`. At-least-once and idempotent: putting
    /// the same CID twice, even concurrently, is not an error.
    async fn put_object(&self, cid: &Cid, bytes: &[u8]) -> Result<ObjectLocator, TransportError>;

    /// Fetches the exact bytes previously stored under `cid`.
    /// `TransportError::NotFound` if this instance has never accepted it
    /// (or no longer has it — a provider deletion is not a logical
    /// deletion, but it can still make a `get` legitimately fail).
    async fn get_object(&self, cid: &Cid) -> Result<Vec<u8>, TransportError>;

    /// Publishes this device's current signed head, replacing whatever
    /// this transport instance previously held for that device.
    async fn publish_head(&self, head: &SignedDeviceHead) -> Result<HeadLocator, TransportError>;

    /// Resolves the current signed head for each device named in `known`,
    /// using any `remote_id` hint present as an optimization only. Devices
    /// with no published head on this instance are omitted from the
    /// result, not an error.
    async fn resolve_heads(&self, known: &[HeadLocator]) -> Result<Vec<SignedDeviceHead>, TransportError>;

    /// One page of this transport's optional enumeration, or `Ok(None)` if
    /// this instance does not support scanning
    /// ([`TransportCapabilities::enumeration`] is `false`) or a previously
    /// valid cursor is no longer recognized (for example after the remote
    /// store was reset) and the caller must fall back to head-based
    /// discovery.
    async fn scan(&self, cursor: Option<&str>) -> Result<Option<ScanPage>, TransportError>;

    /// Removes an object from this transport instance. A *provider*
    /// deletion, not a logical sync deletion: it never inverts an
    /// anything already merged, and other replicas are unaffected.
    async fn delete_object(&self, cid: &Cid) -> Result<(), TransportError>;

    /// This instance's current health, for the replicator's aggregation
    /// and the Settings UI. Distinct from a hard error: a degraded or
    /// unavailable transport is a normal, expected state to report, not a
    /// bug.
    async fn health(&self) -> Result<TransportHealth, TransportError>;
}

pub mod conformance;
