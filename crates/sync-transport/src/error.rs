/// Typed transport failures. Every adapter maps its own provider errors
/// (HTTP status, filesystem error, RPC fault, ...) into exactly one of
/// these, so the replicator's retry/backoff and health logic never branches
/// on provider-specific error shapes.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// Worth retrying with backoff: a timeout, a dropped connection, a
    /// 5xx-class response, or similar.
    #[error("transient transport failure: {0}")]
    Transient(String),
    /// The configured credential is missing, expired, or rejected. Retrying
    /// the same request will not help without re-authorization.
    #[error("transport authentication failed: {0}")]
    Authentication(String),
    /// A rate limit or storage quota was hit. Retryable, but only after the
    /// caller backs off longer than a plain transient failure.
    #[error("transport quota exceeded: {0}")]
    Quota(String),
    /// Bytes were returned but do not match what was requested (wrong CID,
    /// truncated, or otherwise corrupt). Never treat as the requested
    /// content; never retry the identical request without suspicion.
    #[error("transport object corrupted: {0}")]
    Corruption(String),
    /// Not found. Distinct from `Transient`: `get_object`/`resolve_heads`
    /// callers should not busy-retry a plain absence.
    #[error("object not found")]
    NotFound,
    /// Any other definite, non-retryable failure (invalid configuration,
    /// unsupported operation, a provider capability the instance lacks).
    #[error("transport failure: {0}")]
    Permanent(String),
}

impl TransportError {
    /// Whether the replicator should schedule a backoff retry rather than
    /// surface this as a settled failure.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient(_) | Self::Quota(_))
    }
}
