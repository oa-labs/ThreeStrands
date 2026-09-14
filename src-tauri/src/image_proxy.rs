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
// Generous for a marketing-email header image or a profile photo, small
// enough to bound memory use per request.
const MAX_BYTES: usize = 10 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 200;

#[derive(Clone, Default)]
pub(crate) struct ImageCache(Arc<tokio::sync::Mutex<ImageCacheInner>>);

#[derive(Default)]
struct ImageCacheInner {
    entries: HashMap<String, String>,
    // Insertion order, so a full cache evicts the oldest entry rather than
    // an arbitrary one (a bare LRU would be nicer but isn't worth a new
    // dependency for a bound whose only job is capping memory use).
    order: VecDeque<String>,
}

impl ImageCache {
    async fn get(&self, url: &str) -> Option<String> {
        self.0.lock().await.entries.get(url).cloned()
    }

    async fn insert(&self, url: String, data_uri: String) {
        let mut inner = self.0.lock().await;
        if !inner.entries.contains_key(&url) && inner.entries.len() >= MAX_CACHE_ENTRIES {
            if let Some(oldest) = inner.order.pop_front() {
                inner.entries.remove(&oldest);
            }
        }
        inner.order.push_back(url.clone());
        inner.entries.insert(url, data_uri);
    }
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

    let client = reqwest::Client::builder()
        .dns_resolver(net_safety::dns_resolver())
        .redirect(Policy::limited(5))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Unable to prepare image request: {error}"))?;

    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| format!("Image request failed: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("Image endpoint returned HTTP {}", response.status()));
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
    if !content_type.starts_with("image/") {
        return Err(format!("Refusing non-image content-type: {content_type}"));
    }

    if let Some(claimed_len) = response.content_length() {
        if claimed_len as usize > MAX_BYTES {
            return Err("Image exceeds the maximum allowed size".to_string());
        }
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Image download failed: {error}"))?;
    if bytes.len() > MAX_BYTES {
        return Err("Image exceeds the maximum allowed size".to_string());
    }

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

    #[tokio::test]
    async fn caches_by_url_and_evicts_oldest_entry_past_the_cap() {
        let cache = ImageCache::default();
        for index in 0..MAX_CACHE_ENTRIES {
            cache
                .insert(format!("https://example.com/{index}.png"), "data:x".into())
                .await;
        }
        assert!(cache.get("https://example.com/0.png").await.is_some());

        cache
            .insert("https://example.com/overflow.png".into(), "data:x".into())
            .await;
        assert!(
            cache.get("https://example.com/0.png").await.is_none(),
            "oldest entry should be evicted once the cache is full"
        );
        assert!(cache.get("https://example.com/overflow.png").await.is_some());
    }
}

#[cfg(test)]
mod live_smoke_test {
    use super::*;

    #[tokio::test]
    #[ignore = "hits the real network; run manually with `cargo test -- --ignored`"]
    async fn fetches_a_real_remote_image_end_to_end() {
        let cache = ImageCache::default();
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
