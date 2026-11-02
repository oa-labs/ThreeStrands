//! Isolated HTTPS boundary: no proxies, cookies, redirects, compression or mail consent.
use super::{
    policy::*,
    research::{self, Source},
};
use serde_json::{json, Value};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::{Host, Url};
#[derive(Clone, Debug)]
pub(crate) enum Failure {
    Pause(&'static str),
    Fail(&'static str),
    Retry(u64),
}
impl Failure {
    pub fn step(self) -> super::StepResult {
        match self {
            Self::Pause(s) => super::StepResult::Pause(s.into()),
            Self::Fail(s) => super::StepResult::Fail(s.into()),
            Self::Retry(d) => super::StepResult::Retry(d),
        }
    }
}
pub(crate) type Result<T> = std::result::Result<T, Failure>;
fn invalid() -> Failure {
    Failure::Fail("response_invalid")
}
pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let n = u32::from(v);
            ![
                (0, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 3),
            ]
            .iter()
            .any(|(base, bits)| n >> (32 - bits) == base >> (32 - bits))
        }
        IpAddr::V6(v) => {
            let n = u128::from(v);
            n >> 125 == 1
                && ![
                    (0x2001u128 << 112, 23),
                    (0x20010db8u128 << 96, 32),
                    (0x2002u128 << 112, 16),
                    (0x3fffu128 << 112, 20),
                ]
                .iter()
                .any(|(base, bits)| n >> (128 - bits) == base >> (128 - bits))
        }
    }
}
pub(crate) fn validate_url(value: &str) -> Result<Url> {
    if value.len() > MAX_URL {
        return Err(Failure::Fail("unsafe_url"));
    }
    let u = Url::parse(value).map_err(|_| Failure::Fail("unsafe_url"))?;
    let safe = match u.host() {
        Some(Host::Domain(h)) => {
            h.contains('.')
                && !h.ends_with('.')
                && !h.ends_with(".local")
                && !h.ends_with(".localhost")
                && h != "localhost"
        }
        Some(Host::Ipv4(a)) => public_ip(a.into()),
        Some(Host::Ipv6(a)) => public_ip(a.into()),
        None => false,
    };
    let path = percent_encoding::percent_decode_str(u.path())
        .decode_utf8()
        .map_err(|_| Failure::Fail("unsafe_url"))?
        .to_lowercase();
    let action = path.split('/').any(|p| {
        [
            "unsubscribe",
            "subscribe",
            "logout",
            "login",
            "signin",
            "signout",
            "confirm",
            "delete",
            "remove",
            "activate",
            "click",
            "track",
            "tracking",
            "action",
        ]
        .contains(&p)
    });
    if u.scheme() != "https"
        || u.port_or_known_default() != Some(443)
        || !u.username().is_empty()
        || u.password().is_some()
        || !safe
        || u.query().is_some()
        || action
    {
        return Err(Failure::Fail("unsafe_url"));
    }
    let mut u = u;
    u.set_fragment(None);
    Ok(u)
}
fn client() -> reqwest::ClientBuilder {
    crate::http_client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(CONNECT_SECONDS))
        .timeout(Duration::from_secs(REQUEST_SECONDS - REQUEST_GRACE_SECONDS))
}
pub(crate) fn outcome(status: u16, retry_after: Option<&str>, attempt: i64) -> Result<()> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    if [401, 403].contains(&status) {
        return Err(Failure::Pause("credentials_or_access"));
    }
    if status == 402 {
        return Err(Failure::Pause("provider_credits_required"));
    }
    if ![408, 429, 500, 502, 503, 504, 529].contains(&status) {
        return Err(Failure::Fail("provider_rejected"));
    }
    if attempt > MAX_RETRIES {
        return Err(Failure::Fail("retry_exhausted"));
    }
    let delay = (RETRY_BASE_SECONDS as u64) * 2u64.pow((attempt - 1).max(0) as u32);
    let delay = if let Some(value) = retry_after {
        if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
            return Err(Failure::Pause("retry_review"));
        }
        delay.max(
            value
                .parse::<u64>()
                .map_err(|_| Failure::Pause("retry_review"))?,
        )
    } else {
        delay
    };
    if delay > MAX_RETRY_DELAY {
        Err(Failure::Pause("retry_review"))
    } else {
        Err(Failure::Retry(delay))
    }
}
async fn bytes(mut response: reqwest::Response, attempt: i64) -> Result<Vec<u8>> {
    outcome(
        response.status().as_u16(),
        response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        attempt,
    )?;
    if response
        .headers()
        .get("content-encoding")
        .is_some_and(|v| v != "identity")
        || response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
    {
        return Err(invalid());
    }
    let mut data = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Failure::Retry(RETRY_BASE_SECONDS as u64))?
    {
        if data.len() + chunk.len() > MAX_RESPONSE {
            return Err(invalid());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}
pub(crate) async fn provider(
    path: &str,
    key: &str,
    body: Option<&Value>,
    attempt: i64,
) -> Result<Value> {
    // Callers cannot choose a host; key identity and endpoint are inseparable.
    let (url, model) = match path {
        "model" => ("https://api.anthropic.com/v1/messages", true),
        "count" => ("https://api.anthropic.com/v1/messages/count_tokens", true),
        "verify" => (
            "https://api.anthropic.com/v1/models/claude-sonnet-4-6",
            true,
        ),
        "search" => ("https://api.exa.ai/search", false),
        _ => return Err(invalid()),
    };
    let c = client().build().map_err(|_| invalid())?;
    let mut req = if let Some(body) = body {
        c.post(url).json(body)
    } else {
        c.get(url)
    };
    req = req.header("x-api-key", key);
    if model {
        req = req.header("anthropic-version", "2023-06-01");
    }
    let response = req
        .send()
        .await
        .map_err(|_| Failure::Retry(RETRY_BASE_SECONDS as u64))?;
    serde_json::from_slice(&bytes(response, attempt).await?).map_err(|_| invalid())
}
pub(crate) fn search_request(query: &str) -> Value {
    json!({"query":query,"type":"auto","numResults":SEARCH_RESULTS,"contents":{"highlights":true}})
}
pub(crate) fn parse_search(v: &Value, now: i64) -> Result<(Vec<Source>, Option<f64>)> {
    let rows = v["results"].as_array().ok_or_else(invalid)?;
    if rows.len() > SEARCH_RESULTS {
        return Err(invalid());
    }
    let mut out = vec![];
    for row in rows {
        let url = validate_url(row["url"].as_str().ok_or_else(invalid)?)?;
        let title = row["title"].as_str().ok_or_else(invalid)?;
        research::text(title, MAX_TITLE).map_err(|_| invalid())?;
        let excerpts = match row.get("highlights") {
            None => vec![],
            Some(v) => v
                .as_array()
                .ok_or_else(invalid)?
                .iter()
                .map(|v| v.as_str().ok_or_else(invalid))
                .collect::<Result<Vec<_>>>()?,
        };
        let excerpt = excerpts.join("\n");
        if excerpt.chars().count() > MAX_EXTRACTED {
            return Err(invalid());
        }
        out.push(Source {
            id: uuid::Uuid::new_v4().to_string(),
            url: url.to_string(),
            title: title.into(),
            excerpt,
            kind: "search_snippet".into(),
            retrieved_at: now,
        });
    }
    let cost = v["costDollars"]["total"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0);
    Ok((out, cost))
}
fn extract(data: &[u8], content_type: &str) -> Result<String> {
    let parts: Vec<_> = content_type.split(';').map(str::trim).collect();
    if !["text/html", "text/plain"].contains(&parts[0].to_ascii_lowercase().as_str()) {
        return Err(Failure::Fail("unsupported_page_type"));
    }
    let mut ascii = false;
    for p in &parts[1..] {
        let (name, value) = p.split_once('=').ok_or_else(invalid)?;
        if name.trim().eq_ignore_ascii_case("charset") {
            match value.trim().trim_matches('"').to_ascii_lowercase().as_str() {
                "utf-8" => {}
                "us-ascii" => ascii = true,
                _ => return Err(Failure::Fail("unsupported_page_encoding")),
            }
        }
    }
    if ascii && !data.is_ascii() {
        return Err(invalid());
    }
    let text = std::str::from_utf8(data).map_err(|_| invalid())?;
    let text = if parts[0].eq_ignore_ascii_case("text/html") {
        crate::html_plain_text::html_to_text(text)
    } else {
        text.into()
    };
    if text.chars().count() > MAX_EXTRACTED {
        return Err(Failure::Fail("page_too_large"));
    }
    Ok(text)
}
fn validate_addresses(addresses: &[SocketAddr]) -> Result<()> {
    if addresses.is_empty()
        || addresses.len() > MAX_DNS_ANSWERS
        || addresses.iter().any(|a| !public_ip(a.ip()))
    {
        Err(Failure::Fail("unsafe_dns"))
    } else {
        Ok(())
    }
}
async fn pinned_request(
    url: &Url,
    addresses: &[SocketAddr],
    builder: reqwest::ClientBuilder,
) -> Result<reqwest::Response> {
    let host = url.host_str().ok_or_else(invalid)?.trim_matches(['[', ']']);
    let c = builder
        .resolve_to_addrs(host, addresses)
        .build()
        .map_err(|_| invalid())?;
    let response = c
        .get(url.clone())
        .header("accept", "text/html, text/plain")
        .send()
        .await
        .map_err(|_| Failure::Retry(RETRY_BASE_SECONDS as u64))?;
    // The caller validated all pinned addresses before any bytes could be sent.
    if !response
        .remote_addr()
        .is_some_and(|a| addresses.contains(&a))
    {
        return Err(Failure::Fail("unsafe_peer"));
    }
    Ok(response)
}
pub(crate) async fn page(
    source: &Source,
    attempt: i64,
    guard: impl Fn() -> bool,
) -> Result<Source> {
    let mut url = validate_url(&source.url)?;
    for hop in 0..=MAX_REDIRECTS {
        let host = url.host_str().ok_or_else(invalid)?.trim_matches(['[', ']']);
        let addresses: Vec<SocketAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![SocketAddr::new(ip, 443)],
            Err(_) => tokio::time::timeout(
                Duration::from_secs(CONNECT_SECONDS),
                tokio::net::lookup_host((host, 443)),
            )
            .await
            .map_err(|_| Failure::Retry(RETRY_BASE_SECONDS as u64))?
            .map_err(|_| Failure::Retry(RETRY_BASE_SECONDS as u64))?
            .take(MAX_DNS_ANSWERS + 1)
            .collect(),
        };
        validate_addresses(&addresses)?;
        // Pin ALL vetted answers. The connector cannot resolve the hostname again;
        // HTTPS still verifies its original hostname and SNI. Each hop has a fresh client.
        if !guard() {
            return Err(Failure::Pause("interrupted"));
        }
        let response = pinned_request(&url, &addresses, client()).await?;
        if response.status().is_redirection() {
            if hop == MAX_REDIRECTS
                || ![301, 302, 303, 307, 308].contains(&response.status().as_u16())
            {
                return Err(Failure::Fail("redirect_limit"));
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(invalid)?;
            url = validate_url(url.join(location).map_err(|_| invalid())?.as_str())?;
            continue;
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(invalid)?
            .to_string();
        let data = bytes(response, attempt).await?;
        let text = tokio::task::spawn_blocking(move || extract(&data, &content_type))
            .await
            .map_err(|_| invalid())??;
        let mut s = source.clone();
        s.id = uuid::Uuid::new_v4().to_string();
        s.url = url.to_string();
        s.excerpt = text;
        s.kind = "page".into();
        s.retrieved_at = chrono::Utc::now().timestamp();
        return Ok(s);
    }
    Err(invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_frozen_search_and_network_fixtures_obey_native_policy() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../docs/agent-integrations/fixtures/search.json"
        ))
        .unwrap();
        assert_eq!(
            search_request("Example Summit official speakers"),
            fixture["request"]["body"]
        );
        let (sources, cost) = parse_search(&fixture["response"], 1).unwrap();
        assert_eq!(sources[0].excerpt, fixture["normalized"][0]["excerpt"]);
        assert_eq!(cost, Some(0.007));
        for value in [
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://[::ffff:8.8.8.8]/",
            "https://[2001:db8::1]/",
            "http://example.com/",
            "https://example.com:444/",
            "https://user:pass@example.com/",
            "https://example.com/?token=private",
            "https://example.com/unsubscribe/x",
            "https://example.com/%63onfirm/x",
            "https://example.com./",
        ] {
            assert!(validate_url(value).is_err(), "{value}");
        }
        for value in [
            "https://example.com/speakers",
            "https://example.net/podcast",
            "https://[2606:4700:4700::1111]/",
        ] {
            assert!(validate_url(value).is_ok(), "{value}");
        }
        assert!(validate_addresses(&[
            "8.8.8.8:443".parse().unwrap(),
            "10.0.0.1:443".parse().unwrap()
        ])
        .is_err());
        assert!(validate_addresses(&["8.8.8.8:443".parse().unwrap()]).is_ok());
        for ip in [
            "0.1.2.3",
            "100.64.0.1",
            "198.18.0.1",
            "203.0.113.2",
            "224.0.0.1",
            "2001::1",
            "2002::1",
            "3fff::1",
            "64:ff9b::808:808",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
    #[test]
    fn content_and_retry_policy_fail_closed_at_bounds() {
        for body in ["<main>Evidence<script>steal()</script><p>Second fact</p></main>","<table><tr><td>Evidence</td><td>Second fact</td></tr></table><style>url(https://evil.test)</style>"] {
            let text=extract(body.as_bytes(),"text/html; charset=utf-8").unwrap();assert!(text.contains("Evidence"));assert!(text.contains("Second fact"));assert!(!text.contains("steal"));assert!(!text.contains("evil"));
        }
        for content_type in [
            "application/pdf",
            "text/html; charset=latin1",
            "image/svg+xml",
        ] {
            assert!(extract(b"data", content_type).is_err());
        }
        assert!(extract(&[0xff], "text/plain").is_err());
        assert!(extract("é".as_bytes(), "text/plain; charset=us-ascii").is_err());
        for len in [MAX_EXTRACTED - 1, MAX_EXTRACTED] {
            assert!(extract("x".repeat(len).as_bytes(), "text/plain").is_ok());
        }
        assert!(extract("x".repeat(MAX_EXTRACTED + 1).as_bytes(), "text/plain").is_err());
        assert!(matches!(
            outcome(429, Some("300"), 1),
            Err(Failure::Retry(300))
        ));
        for h in ["301", "Wed, 21 Oct 2026 07:28:00 GMT", "broken", ""] {
            assert!(matches!(
                outcome(429, Some(h), 1),
                Err(Failure::Pause("retry_review"))
            ));
        }
        assert!(matches!(
            outcome(402, None, 1),
            Err(Failure::Pause("provider_credits_required"))
        ));
        assert!(matches!(
            outcome(401, None, 1),
            Err(Failure::Pause("credentials_or_access"))
        ));
        assert!(matches!(
            outcome(503, None, 3),
            Err(Failure::Fail("retry_exhausted"))
        ));
    }
    // Real TLS, HTTP and hostname validation with a test-only private socket. The
    // production public-IP validator above is never disabled or parameterized.
    async fn tls_server(
        response: Vec<u8>,
    ) -> (
        SocketAddr,
        reqwest::Certificate,
        tokio::task::JoinHandle<String>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _ = client().build().unwrap();
        let cert = rcgen::generate_simple_self_signed(vec!["research.example.com".into()]).unwrap();
        let root = reqwest::Certificate::from_der(cert.cert.der()).unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()),
        );
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.cert.der().clone()], key)
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let Ok(mut tls) = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config))
                .accept(socket)
                .await
            else {
                return String::new();
            };
            let mut data = vec![];
            let mut buf = [0; 1024];
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = tls.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let _ = tls.write_all(&response).await;
            let _ = tls.shutdown().await;
            String::from_utf8_lossy(&data).into_owned()
        });
        (address, root, task)
    }
    #[tokio::test]
    async fn pinned_connector_uses_only_the_vetted_socket_and_keeps_host_and_tls_identity() {
        let (addr,root,server)=tls_server(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 8\r\nConnection: close\r\n\r\nEvidence".to_vec()).await;
        let u = Url::parse(&format!("https://research.example.com:{}/", addr.port())).unwrap();
        let response = pinned_request(&u, &[addr], client().add_root_certificate(root))
            .await
            .unwrap();
        assert_eq!(bytes(response, 1).await.unwrap(), b"Evidence");
        let request = server.await.unwrap().to_lowercase();
        assert!(request.contains("host: research.example.com:"));
        for header in [
            "cookie:",
            "authorization:",
            "x-api-key:",
            "accept-encoding:",
        ] {
            assert!(!request.contains(header), "{request}");
        }
        let (addr, root, server) = tls_server(b"".to_vec()).await;
        let u = Url::parse(&format!("https://wrong.example.com:{}/", addr.port())).unwrap();
        assert!(
            pinned_request(&u, &[addr], client().add_root_certificate(root))
                .await
                .is_err()
        );
        assert!(server.await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn pinned_connector_does_not_follow_redirects_and_bounded_reader_rejects_compression_and_overflow(
    ) {
        let (addr,root,server)=tls_server(b"HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()).await;
        let u = Url::parse(&format!("https://research.example.com:{}/", addr.port())).unwrap();
        let r = pinned_request(&u, &[addr], client().add_root_certificate(root))
            .await
            .unwrap();
        assert_eq!(r.status(), 302);
        assert!(validate_url(r.headers()["location"].to_str().unwrap()).is_err());
        server.await.unwrap();
        for headers in [
            "Content-Encoding: gzip\r\nContent-Length: 0".to_string(),
            format!("Content-Length: {}", MAX_RESPONSE + 1),
        ] {
            let (addr, root, server) = tls_server(
                format!("HTTP/1.1 200 OK\r\n{headers}\r\nConnection: close\r\n\r\n").into_bytes(),
            )
            .await;
            let u = Url::parse(&format!("https://research.example.com:{}/", addr.port())).unwrap();
            let r = pinned_request(&u, &[addr], client().add_root_certificate(root))
                .await
                .unwrap();
            assert!(bytes(r, 1).await.is_err());
            server.await.unwrap();
        }
    }
    #[test]
    fn pinned_connector_ignores_environment_proxies_in_an_isolated_process() {
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","agents::transport::tests::pinned_connector_uses_only_the_vetted_socket_and_keeps_host_and_tls_identity","--nocapture"])
            .env("HTTPS_PROXY","http://127.0.0.1:9").env("https_proxy","http://127.0.0.1:9")
            .env("ALL_PROXY","http://127.0.0.1:9").env("all_proxy","http://127.0.0.1:9")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    #[tokio::test]
    async fn real_reader_enforces_exact_response_byte_boundary() {
        for n in [MAX_RESPONSE - 1, MAX_RESPONSE, MAX_RESPONSE + 1] {
            let mut response=format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n").into_bytes();
            response.extend(std::iter::repeat_n(b'x', n));
            let (addr, root, server) = tls_server(response).await;
            let u = Url::parse(&format!("https://research.example.com:{}/", addr.port())).unwrap();
            let result = bytes(
                pinned_request(&u, &[addr], client().add_root_certificate(root))
                    .await
                    .unwrap(),
                1,
            )
            .await;
            assert_eq!(result.is_ok(), n <= MAX_RESPONSE);
            if let Ok(data) = result {
                assert_eq!(data.len(), n);
            }
            server.await.unwrap();
        }
    }
}
