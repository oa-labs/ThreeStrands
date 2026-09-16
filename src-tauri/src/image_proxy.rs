//! Fetches remote images referenced by message HTML on the sender's behalf,
//! instead of letting the sandboxed message iframe (`SafeMessage.tsx`) talk
//! to arbitrary hosts directly.
//!
//! This closes two gaps a same-scheme-only `img-src` policy leaves open:
//! senders never see the reader's real IP/User-Agent (this app's client
//! makes the request, not the reader's actual browser engine), and a
//! malicious `src` can't be used to probe internal/private-network
//! addresses reachable from the user's machine — see [`crate::net_safety`].
//! It also lets the iframe's CSP drop `img-src https:` down to `data:`
//! only, since the iframe itself never makes a network request for an
//! image.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::redirect::Policy;
use url::Url;

use crate::net_safety::{self, is_disallowed_host};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 4;

#[derive(Clone)]
pub(crate) struct ImageCache {
    inner: Arc<tokio::sync::Mutex<ImageCacheInner>>,
    // reqwest clients own their connection pool. Keeping one here lets
    // images from the same host reuse DNS results and established TLS
    // connections while every request still passes through the guarded
    // resolver and redirect policy configured below.
    client: reqwest::Client,
    request_slots: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct ImageCacheInner {
    entries: HashMap<String, String>,
    // Insertion order, so a full cache evicts the oldest entry rather than
    // an arbitrary one (a bare LRU would be nicer but isn't worth a new
    // dependency for a bound whose only job is capping memory use).
    order: VecDeque<String>,
    total_bytes: usize,
}

impl ImageCache {
    pub(crate) fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .dns_resolver(net_safety::dns_resolver())
            .redirect(Policy::limited(5))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| format!("Unable to prepare image requests: {error}"))?;
        Ok(Self {
            inner: Arc::new(tokio::sync::Mutex::new(ImageCacheInner::default())),
            client,
            request_slots: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_REQUESTS)),
        })
    }

    async fn get(&self, url: &str) -> Option<String> {
        self.inner.lock().await.entries.get(url).cloned()
    }

    async fn insert(&self, url: String, data_uri: String) {
        let mut inner = self.inner.lock().await;
        inner.insert_with_limit(url, data_uri, MAX_CACHE_BYTES);
    }
}

impl ImageCacheInner {
    fn insert_with_limit(&mut self, url: String, data_uri: String, max_bytes: usize) {
        let entry_bytes = url.len().saturating_add(data_uri.len());
        if entry_bytes > max_bytes {
            return;
        }
        if let Some(previous) = self.entries.remove(&url) {
            self.total_bytes = self
                .total_bytes
                .saturating_sub(url.len().saturating_add(previous.len()));
            self.order.retain(|key| key != &url);
        }
        while self.total_bytes.saturating_add(entry_bytes) > max_bytes {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(removed) = self.entries.remove(&oldest) {
                    self.total_bytes = self
                        .total_bytes
                        .saturating_sub(oldest.len().saturating_add(removed.len()));
                }
            } else {
                break;
            }
        }
        self.order.push_back(url.clone());
        self.entries.insert(url, data_uri);
        self.total_bytes += entry_bytes;
    }
}

fn append_bounded(bytes: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if chunk.len() > crate::image_format::MAX_RASTER_BYTES.saturating_sub(bytes.len()) {
        return Err("Image exceeds the maximum allowed size".to_string());
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

/// Same shape as `SafeMessage.tsx`'s own `safeImageSrc`: only plain http(s)
/// URLs are eligible. `data:` URIs never reach this far since the frontend
/// only proxies URLs it marked as blocked, and it never blocks `data:`.
fn validate_public_image_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "Invalid image URL".to_string())?;
    let scheme = url.scheme();
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Image URL must be a plain http(s) URL".to_string());
    }
    if let Some(host) = url.host_str() {
        if is_disallowed_host(host) {
            return Err("Image URL points to a local or private host".to_string());
        }
    }
    Ok(url)
}

pub(crate) async fn fetch(url: &str, cache: &ImageCache) -> Result<String, String> {
    if let Some(cached) = cache.get(url).await {
        return Ok(cached);
    }

    let parsed = validate_public_image_url(url)?;
    let _request_slot = cache
        .request_slots
        .acquire()
        .await
        .map_err(|_| "Image request limiter is unavailable".to_string())?;

    let mut response = cache
        .client
        .get(parsed)
        .send()
        .await
        .map_err(|error| format!("Image request failed: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Image endpoint returned HTTP {}",
            response.status()
        ));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !crate::image_format::is_supported_raster_mime(&content_type) {
        return Err(format!(
            "Refusing unsupported image content-type: {content_type}"
        ));
    }

    if let Some(claimed_len) = response.content_length() {
        if claimed_len > crate::image_format::MAX_RASTER_BYTES as u64 {
            return Err("Image exceeds the maximum allowed size".to_string());
        }
    }

    // Never call Response::bytes(): a chunked response without Content-Length
    // could otherwise be completely buffered before the limit is checked.
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(0)
            .min(crate::image_format::MAX_RASTER_BYTES as u64) as usize,
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Image download failed: {error}"))?
    {
        append_bounded(&mut bytes, &chunk)?;
    }

    crate::image_format::validate_raster(&bytes)?;

    let data_uri = format!("data:{content_type};base64,{}", STANDARD.encode(&bytes));
    cache.insert(url.to_string(), data_uri.clone()).await;
    Ok(data_uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_schemes_and_private_hosts() {
        assert!(validate_public_image_url("data:image/png;base64,AAAA").is_err());
        assert!(validate_public_image_url("javascript:alert(1)").is_err());
        assert!(validate_public_image_url("file:///etc/passwd").is_err());
        assert!(validate_public_image_url("https://127.0.0.1/x.png").is_err());
        assert!(validate_public_image_url("http://169.254.169.254/x.png").is_err());
        assert!(validate_public_image_url("https://user:pass@example.com/x.png").is_err());
        assert!(validate_public_image_url("https://example.com/x.png").is_ok());
        assert!(validate_public_image_url("http://example.com/x.png").is_ok());
    }

    #[test]
    fn streams_through_the_byte_limit_and_rejects_the_next_byte() {
        let mut bytes = vec![0; crate::image_format::MAX_RASTER_BYTES - 1];
        assert!(append_bounded(&mut bytes, &[0]).is_ok());
        assert_eq!(bytes.len(), crate::image_format::MAX_RASTER_BYTES);
        assert!(append_bounded(&mut bytes, &[0]).is_err());
        assert_eq!(
            bytes.len(),
            crate::image_format::MAX_RASTER_BYTES,
            "an oversized chunk must not be appended"
        );
    }

    #[test]
    fn cache_evicts_by_combined_entry_weight() {
        let mut cache = ImageCacheInner::default();
        cache.insert_with_limit("a".into(), "123456789".into(), 20);
        cache.insert_with_limit("b".into(), "123456789".into(), 20);
        assert!(cache.entries.contains_key("a"));
        assert!(cache.entries.contains_key("b"));
        assert_eq!(cache.total_bytes, 20);

        cache.insert_with_limit("c".into(), "x".into(), 20);
        assert!(
            !cache.entries.contains_key("a"),
            "oldest entries are evicted to make room"
        );
        assert!(cache.entries.contains_key("b"));
        assert!(cache.entries.contains_key("c"));
        assert_eq!(cache.total_bytes, 12);

        cache.insert_with_limit("too-large".into(), "123456789012".into(), 20);
        assert!(!cache.entries.contains_key("too-large"));
    }
}

#[cfg(test)]
mod live_smoke_test {
    use super::*;

    #[tokio::test]
    #[ignore = "hits the real network; run manually with `cargo test -- --ignored`"]
    async fn fetches_a_real_remote_image_end_to_end() {
        let cache = ImageCache::new().unwrap();
        let data_uri = fetch(
            "https://userimg-assets.customeriomail.com/images/client-env-141356/01M260RE1ZYJ8R9E1FY2BM0Q5A.png",
            &cache,
        )
        .await
        .expect("real image fetch should succeed");
        assert!(data_uri.starts_with("data:image/png;base64,"));
        assert!(data_uri.len() > 1000);
        // Second call should hit the cache, not the network.
        let cached = fetch(
            "https://userimg-assets.customeriomail.com/images/client-env-141356/01M260RE1ZYJ8R9E1FY2BM0Q5A.png",
            &cache,
        )
        .await
        .expect("cached fetch should succeed");
        assert_eq!(cached, data_uri);
    }
}
