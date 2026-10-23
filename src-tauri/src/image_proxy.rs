//! Fetches remote images referenced by message HTML on the sender's behalf,
//! instead of letting the sandboxed message iframe (`SafeMessage.tsx`) talk
//! to arbitrary hosts directly.
//!
//! This closes two gaps a same-scheme-only `img-src` policy leaves open:
//! senders don't receive the reader's browser context, cookies, referrer, or
//! own User-Agent — the proxy sends a fixed one — (though the image host still
//! sees the device's public network egress IP), and a malicious `src` can't be used to probe
//! internal/private-network addresses reachable from the user's machine —
//! see [`crate::net_safety`].
//! It also lets the iframe's CSP drop `img-src https:` down to `data:`
//! only, since the iframe itself never makes a network request for an
//! image.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::redirect::Policy;
use url::Url;

use crate::net_safety::{self, is_disallowed_url_host};

#[cfg(target_os = "macos")]
mod urlsession;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 4;
// CDN bot filters (Akamai, for one) answer 403 to image requests that don't
// look like a browser's: a non-browser User-Agent, or a browser one missing
// the Accept, Accept-Language, Accept-Encoding, and Sec-Fetch-* headers a
// real <img> load sends. The proxy therefore sends one fixed, browser-like
// header set. It is a constant, never derived from the reader's webview,
// OS, locale, or app version, so every reader's requests look identical and
// none of the reader's own browser context reaches the sender.
const IMAGE_PROXY_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const IMAGE_PROXY_HEADERS: [(&str, &str); 5] = [
    (
        "accept",
        "image/webp,image/avif,image/png,image/jpeg,image/gif,image/*;q=0.8",
    ),
    ("accept-language", "en-US,en;q=0.9"),
    ("sec-fetch-dest", "image"),
    ("sec-fetch-mode", "no-cors"),
    ("sec-fetch-site", "cross-site"),
];

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
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in IMAGE_PROXY_HEADERS {
            headers.insert(name, reqwest::header::HeaderValue::from_static(value));
        }
        let client = crate::http_client::builder()
            .dns_resolver(net_safety::dns_resolver())
            .redirect(Policy::limited(5))
            .user_agent(IMAGE_PROXY_USER_AGENT)
            .default_headers(headers)
            // Bot filters also expect Accept-Encoding. Turning decoding on
            // makes reqwest send it and decode the body, so the streamed
            // byte limit below applies to the decoded image.
            .gzip(true)
            .brotli(true)
            .deflate(true)
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
    if is_disallowed_url_host(&url) {
        return Err("Image URL points to a local or private host".to_string());
    }
    Ok(url)
}

pub(crate) async fn fetch(url: &str, cache: &ImageCache) -> Result<String, String> {
    if let Some(cached) = cache.get(url).await {
        return Ok(cached);
    }

    let parsed = validate_public_image_url(url)?;
    let data_uri = download(parsed, cache).await?;
    cache.insert(url.to_string(), data_uri.clone()).await;
    Ok(data_uri)
}

/// Downloads an already-validated URL and returns it as a raster data URI.
async fn download(parsed: Url, cache: &ImageCache) -> Result<String, String> {
    let _request_slot = cache
        .request_slots
        .acquire()
        .await
        .map_err(|_| "Image request limiter is unavailable".to_string())?;

    let mut response = cache
        .client
        .get(parsed.clone())
        .send()
        .await
        .map_err(|error| format!("Image request failed: {error}"))?;

    let content_type = media_type(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or(""),
    );

    if !response.status().is_success() {
        #[cfg(target_os = "macos")]
        if should_retry_with_platform_stack(&parsed, response.status(), &content_type) {
            drop(response);
            return download_with_platform_stack(parsed).await;
        }
        return Err(format!(
            "Image endpoint returned HTTP {}",
            response.status()
        ));
    }

    check_declared_type(&content_type)?;

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

    raster_data_uri(&content_type, &bytes)
}

/// A `Content-Type` header's media type, lowercased, without parameters.
fn media_type(header: &str) -> String {
    header
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Some CDNs label real images as generic binary; those are identified from
/// their bytes in [`raster_data_uri`]. Every other non-raster type is
/// refused before the body is read.
fn check_declared_type(content_type: &str) -> Result<(), String> {
    if !crate::image_format::is_supported_raster_mime(content_type)
        && !crate::image_format::is_generic_binary_mime(content_type)
    {
        return Err(format!(
            "Refusing unsupported image content-type: {content_type}"
        ));
    }
    Ok(())
}

fn raster_data_uri(content_type: &str, bytes: &[u8]) -> Result<String, String> {
    let mime_type = if crate::image_format::is_supported_raster_mime(content_type) {
        content_type
    } else {
        crate::image_format::sniff_raster_mime(bytes).ok_or_else(|| {
            format!("Refusing unrecognized image data labelled as content-type: {content_type}")
        })?
    };
    crate::image_format::validate_raster(bytes)?;

    Ok(format!(
        "data:{mime_type};base64,{}",
        STANDARD.encode(bytes)
    ))
}

/// A bot filter that judges the TLS handshake answers the reqwest client
/// with a 403 page instead of the image, whatever its headers. Only that
/// shape earns a second attempt through the system network stack: other
/// failures, and a 403 that is itself an image, are final. HTTPS only, since
/// App Transport Security blocks plain-HTTP URLSession loads in the app.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn should_retry_with_platform_stack(
    url: &Url,
    status: reqwest::StatusCode,
    content_type: &str,
) -> bool {
    status == reqwest::StatusCode::FORBIDDEN
        && url.scheme() == "https"
        && !crate::image_format::is_supported_raster_mime(content_type)
        && !crate::image_format::is_generic_binary_mime(content_type)
}

/// Every resolved address must be public: URLSession picks among them
/// itself, so a single private answer could be the one it connects to.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn addresses_are_public(addresses: &[std::net::IpAddr]) -> bool {
    !addresses.is_empty()
        && addresses
            .iter()
            .all(|address| !net_safety::is_private_ip(*address))
}

/// The proxy's public-host rules for the URLSession fallback, applied to
/// the first URL and to every redirect target. Blocks on DNS, which
/// [`urlsession`] allows for.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn check_platform_fallback_target(url: &Url) -> Result<(), String> {
    let url = validate_public_image_url(url.as_str())?;
    if url.scheme() != "https" {
        return Err("Image URL must use HTTPS".to_string());
    }
    if let Some(url::Host::Domain(domain)) = url.host() {
        use std::net::ToSocketAddrs;
        let addresses: Vec<std::net::IpAddr> = (domain, url.port_or_known_default().unwrap_or(443))
            .to_socket_addrs()
            .map_err(|error| format!("Image request failed: {error}"))?
            .map(|address| address.ip())
            .collect();
        if !addresses_are_public(&addresses) {
            return Err("Image URL points to a local or private host".to_string());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn download_with_platform_stack(parsed: Url) -> Result<String, String> {
    const POLICY: urlsession::PeerPolicy = urlsession::PeerPolicy {
        check_target: check_platform_fallback_target,
        allows_peer: |address| !net_safety::is_private_ip(address),
    };
    let response = urlsession::fetch(
        &parsed,
        POLICY,
        IMAGE_PROXY_USER_AGENT,
        &IMAGE_PROXY_HEADERS,
        REQUEST_TIMEOUT,
        crate::image_format::MAX_RASTER_BYTES,
    )
    .await?;
    if !(200..300).contains(&response.status) {
        return Err(format!("Image endpoint returned HTTP {}", response.status));
    }
    let content_type = media_type(&response.content_type);
    check_declared_type(&content_type)?;
    raster_data_uri(&content_type, &response.bytes)
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
    fn rejects_private_ipv6_literal_hosts() {
        // The URL parser keeps IPv6 hosts bracketed, and the HTTP client
        // connects to IP literals without consulting the DNS resolver, so
        // this check is the only defense for these hosts.
        for url in [
            "http://[::1]/x.png",
            "https://[::]/x.png",
            "http://[fd00::1]/x.png",
            "http://[fe80::1]/x.png",
            "http://[::ffff:127.0.0.1]/x.png",
            "http://[::ffff:169.254.169.254]/x.png",
        ] {
            assert_eq!(
                validate_public_image_url(url).unwrap_err(),
                "Image URL points to a local or private host",
                "{url} must be rejected"
            );
        }
        assert!(validate_public_image_url("http://[2606:4700:4700::1111]/x.png").is_ok());
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

    /// Serves one canned response on a loopback port and returns the
    /// lowercased request it received. The guarded resolver isn't consulted
    /// for IP literals and `validate_public_image_url` is bypassed here, so
    /// the shared client can talk to the listener directly.
    fn serve_once(response: Vec<u8>) -> (u16, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            stream.write_all(&response).unwrap();
            String::from_utf8(request).unwrap().to_ascii_lowercase()
        });
        (port, server)
    }

    #[tokio::test]
    async fn requests_carry_a_fixed_browser_like_header_set() {
        let (port, server) =
            serve_once(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec());

        let cache = ImageCache::new().unwrap();
        cache
            .client
            .get(format!("http://127.0.0.1:{port}/icon.png"))
            .send()
            .await
            .unwrap();
        let request = server.join().unwrap();
        assert!(
            request.contains(&format!(
                "\r\nuser-agent: {}\r\n",
                IMAGE_PROXY_USER_AGENT.to_ascii_lowercase()
            )),
            "{request}"
        );
        for (name, value) in IMAGE_PROXY_HEADERS {
            assert!(
                request.contains(&format!("\r\n{name}: {}\r\n", value.to_ascii_lowercase())),
                "missing {name}: {request}"
            );
        }
        let accept_encoding = request
            .lines()
            .find_map(|line| line.strip_prefix("accept-encoding: "))
            .unwrap_or_else(|| panic!("missing accept-encoding: {request}"));
        for encoding in ["gzip", "br", "deflate"] {
            assert!(accept_encoding.contains(encoding), "{request}");
        }
        assert!(!request.contains("cookie:"), "{request}");
        assert!(!request.contains("referer:"), "{request}");
        assert!(!request.contains("origin:"), "{request}");
    }

    #[test]
    fn proxy_headers_are_constants_not_reader_context() {
        // The header set must be identical for every reader: a fixed
        // browser identity, never the reader's real OS, app version, or
        // locale. Guard against anyone splicing runtime values in.
        assert!(!IMAGE_PROXY_USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
        assert!(!IMAGE_PROXY_USER_AGENT.contains("ThreeStrands"));
        assert!(IMAGE_PROXY_USER_AGENT.starts_with("Mozilla/5.0 "));
    }

    #[tokio::test]
    async fn decodes_compressed_image_responses() {
        // gzip of b"decoded image bytes"
        const GZIPPED: [u8; 39] = [
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0x4b, 0x49, 0x4d, 0xce,
            0x4f, 0x49, 0x4d, 0x51, 0xc8, 0xcc, 0x4d, 0x4c, 0x4f, 0x55, 0x48, 0xaa, 0x2c, 0x49,
            0x2d, 0x06, 0x00, 0x5a, 0x72, 0x61, 0xed, 0x13, 0x00, 0x00, 0x00,
        ];
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Encoding: gzip\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            GZIPPED.len()
        )
        .into_bytes();
        response.extend_from_slice(&GZIPPED);
        let (port, server) = serve_once(response);

        let cache = ImageCache::new().unwrap();
        let body = cache
            .client
            .get(format!("http://127.0.0.1:{port}/icon.png"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(&body[..], b"decoded image bytes");
    }

    /// A 2×3 PNG header: enough for the signature sniff and dimension check.
    const TINY_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\
        \x00\x00\x00\x02\x00\x00\x00\x03\x08\x06\x00\x00\x00\x00\x00\x00\x00";
    /// A JFIF APP0 segment followed by a 3×2 baseline SOF0 frame header.
    const TINY_JPEG: &[u8] =
        b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\
        \xff\xc0\x00\x11\x08\x00\x02\x00\x03\x03\x01\x11\x00\x02\x11\x01\x03\x11\x01\xff\xd9";

    /// A JFIF APP0 segment followed by a baseline SOF0 frame header.
    fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = TINY_JPEG.to_vec();
        bytes[25..27].copy_from_slice(&height.to_be_bytes());
        bytes[27..29].copy_from_slice(&width.to_be_bytes());
        bytes
    }

    async fn download_served(content_type: Option<&str>, body: &[u8]) -> Result<String, String> {
        let header = content_type
            .map(|value| format!("Content-Type: {value}\r\n"))
            .unwrap_or_default();
        let mut response = format!(
            "HTTP/1.1 200 OK\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        let (port, server) = serve_once(response);
        let cache = ImageCache::new().unwrap();
        let url = Url::parse(&format!("http://127.0.0.1:{port}/image")).unwrap();
        let result = download(url, &cache).await;
        server.join().unwrap();
        result
    }

    #[tokio::test]
    async fn identifies_rasters_served_with_a_generic_binary_type() {
        let png = download_served(Some("application/octet-stream"), TINY_PNG)
            .await
            .unwrap();
        assert!(png.starts_with("data:image/png;base64,"), "{png}");

        let jpeg = download_served(None, TINY_JPEG).await.unwrap();
        assert!(jpeg.starts_with("data:image/jpeg;base64,"), "{jpeg}");
    }

    #[tokio::test]
    async fn admits_full_size_phone_photos_and_refuses_larger_rasters() {
        let photo = download_served(Some("image/jpeg"), &jpeg_header(5_712, 4_284))
            .await
            .unwrap();
        assert!(photo.starts_with("data:image/jpeg;base64,"), "{photo}");

        let error = download_served(Some("image/jpeg"), &jpeg_header(6_000, 6_000))
            .await
            .unwrap_err();
        assert_eq!(error, "Image dimensions exceed the maximum allowed size");
    }

    #[tokio::test]
    async fn keeps_a_declared_raster_type() {
        let png = download_served(Some("image/png"), TINY_PNG).await.unwrap();
        assert!(png.starts_with("data:image/png;base64,"), "{png}");
    }

    #[tokio::test]
    async fn refuses_generic_binary_that_is_not_a_supported_raster() {
        for body in [
            &b"<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(1)\"/>"[..],
            b"<!DOCTYPE html><script>alert(1)</script>",
        ] {
            let error = download_served(Some("application/octet-stream"), body)
                .await
                .unwrap_err();
            assert!(
                error.starts_with("Refusing unrecognized image data"),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn never_sniffs_an_explicitly_non_raster_type() {
        for content_type in ["image/svg+xml", "text/html"] {
            let error = download_served(Some(content_type), TINY_PNG)
                .await
                .unwrap_err();
            assert!(
                error.starts_with("Refusing unsupported image content-type"),
                "{content_type}: {error}"
            );
        }
    }

    #[test]
    fn retries_only_a_non_image_403_over_https() {
        let https = Url::parse("https://example.com/a.png").unwrap();
        let http = Url::parse("http://example.com/a.png").unwrap();
        let forbidden = reqwest::StatusCode::FORBIDDEN;
        assert!(should_retry_with_platform_stack(
            &https,
            forbidden,
            "text/html"
        ));
        assert!(!should_retry_with_platform_stack(
            &http,
            forbidden,
            "text/html"
        ));
        assert!(!should_retry_with_platform_stack(
            &https,
            forbidden,
            "image/png"
        ));
        assert!(!should_retry_with_platform_stack(
            &https,
            forbidden,
            "application/octet-stream"
        ));
        // No type at all may still be an image, so it is treated like one.
        assert!(!should_retry_with_platform_stack(&https, forbidden, ""));
        for status in [
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::NOT_FOUND,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert!(
                !should_retry_with_platform_stack(&https, status, "text/html"),
                "{status}"
            );
        }
    }

    #[test]
    fn platform_fallback_requires_every_resolved_address_to_be_public() {
        let parse = |values: &[&str]| -> Vec<std::net::IpAddr> {
            values.iter().map(|value| value.parse().unwrap()).collect()
        };
        assert!(addresses_are_public(&parse(&[
            "93.184.216.34",
            "2606:4700::1111"
        ])));
        assert!(!addresses_are_public(&parse(&[
            "93.184.216.34",
            "10.0.0.1"
        ])));
        assert!(!addresses_are_public(&parse(&["::ffff:127.0.0.1"])));
        assert!(!addresses_are_public(&[]));
    }

    #[test]
    fn platform_fallback_targets_are_public_https_urls() {
        for url in [
            "http://93.184.216.34/a.png",
            "https://127.0.0.1/a.png",
            "https://169.254.169.254/a.png",
            "https://[::1]/a.png",
            "https://localhost/a.png",
            "https://printer.local/a.png",
            "https://user:pass@93.184.216.34/a.png",
        ] {
            assert!(
                check_platform_fallback_target(&Url::parse(url).unwrap()).is_err(),
                "{url} must be refused"
            );
        }
        assert!(check_platform_fallback_target(
            &Url::parse("https://93.184.216.34/a.png").unwrap()
        )
        .is_ok());
    }

    #[tokio::test]
    async fn a_plain_http_403_page_is_final() {
        let body = b"<html>challenge</html>";
        let mut response = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        let (port, server) = serve_once(response);
        let cache = ImageCache::new().unwrap();
        let url = Url::parse(&format!("http://127.0.0.1:{port}/image")).unwrap();
        let error = download(url, &cache).await.unwrap_err();
        server.join().unwrap();
        assert_eq!(error, "Image endpoint returned HTTP 403 Forbidden");
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
