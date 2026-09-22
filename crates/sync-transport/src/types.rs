use std::fmt;

use threestrands_sync_envelope::DeviceId;

/// A content identifier, wrapping the CIDv1 string [`threestrands_sync_envelope::compute_cid`]
/// produces. Newtyped so a transport signature can't accidentally accept an
/// arbitrary string where a content address is required.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cid(pub String);

impl Cid {
    pub fn for_bytes(bytes: &[u8]) -> Self {
        Cid(threestrands_sync_envelope::compute_cid(bytes))
    }
}

impl fmt::Display for Cid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Cid {
    fn from(value: String) -> Self {
        Cid(value)
    }
}

impl AsRef<str> for Cid {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Identifies one configured transport instance (e.g. one folder, one RPC
/// endpoint), stable across restarts. Distinct from the transport *kind*.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransportInstanceId(pub String);

impl fmt::Display for TransportInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Optional capabilities a transport instance may or may not have. None of
/// these are required: a transport with all `false` is still fully usable
/// through `put_object`/`get_object` alone, just not accelerable by
/// scanning and not independently bootstrappable for a new device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportCapabilities {
    /// Whether [`crate::SyncTransport::scan`] can enumerate this
    /// transport's objects at all.
    pub enumeration: bool,
    /// Whether a `scan` cursor can be resumed incrementally rather than
    /// always restarting from the beginning.
    pub incremental_cursor: bool,
    /// Whether `publish_head`/`resolve_heads` are backed by a real,
    /// independently enumerable discovery index on this instance (a local
    /// folder's `heads/` directory; an RPC endpoint's MFS namespace).
    /// `false` distinguishes a `storage-only` endpoint — still a valid
    /// write/read replica — from `storage-and-discovery`: a storage-only
    /// endpoint cannot, by itself, bootstrap a new device, and the UI must
    /// say so.
    pub head_discovery: bool,
}

/// Where one object landed after a successful `put_object`, or where it can
/// be found for a `resolve_heads` hint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectLocator {
    pub cid: Cid,
    /// An opaque, transport-specific hint (a file path, an object key, a
    /// row id) that can speed up a later fetch. Never required for
    /// correctness: every object must also be reachable by CID alone.
    pub remote_id: Option<String>,
}

/// A hint for resolving one device's head: which device, and optionally
/// where its head object was last found on this transport instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadLocator {
    pub device_id: DeviceId,
    pub remote_id: Option<String>,
}

/// One page of a transport's optional enumeration.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ScanPage {
    pub objects: Vec<ObjectLocator>,
    /// Opaque, scoped to this transport instance. `None` means this was the
    /// last page.
    pub next_cursor: Option<String>,
}

/// A transport instance's current health, for the replicator's health
/// aggregation and Settings UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportHealth {
    Healthy,
    Degraded(String),
    Unavailable(String),
}
