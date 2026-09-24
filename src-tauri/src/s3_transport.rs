//! A user-configured S3-compatible object storage transport adapter (AWS
//! S3, Cloudflare R2, Backblaze B2, Wasabi, MinIO, ...). Nothing here
//! branches on a provider name: presets that fill in an endpoint and
//! region live in the Settings UI only.
//!
//! Requests are signed with `rusty-s3` (sans-IO SigV4 presigning) and sent
//! over the app's own `reqwest` client, so this adapter keeps the same
//! origin validation, no-redirect policy, and typed error mapping as the
//! IPFS RPC adapter.
//!
//! Presigned URLs carry the signature and any session token in the query
//! string, so a URL must never reach an error message or log line: every
//! `reqwest::Error` goes through [`map_reqwest_error`], which strips it.
//!
//! Object layout beneath the configured bucket and optional prefix mirrors
//! the sync folder's corpus exactly, so a user can copy a folder corpus
//! into a bucket (or back) with ordinary tools and either adapter reads it:
//!
//! ```text
//! <prefix>/threestrands-sync/objects/<cid[..2]>/<cid>.block
//! <prefix>/threestrands-sync/heads/<hex device id>.head
//! ```
//!
//! Unlike the folder adapter this never writes a `format-v1` marker: an
//! object store has no directory to mark, and the folder adapter creates
//! its own marker when it opens a copied corpus.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use md5::{Digest, Md5};
use rand::{rngs::OsRng, RngCore};
use rusty_s3::{S3Action, UrlStyle};
use serde::{Deserialize, Serialize};

use threestrands_sync_envelope::{compute_cid, decode_signed_head, encode_signed_head, DeviceId, SignedDeviceHead};
use threestrands_sync_transport::{
    Cid, HeadLocator, ObjectLocator, ScanPage, SyncTransport, TransportCapabilities, TransportError,
    TransportHealth, TransportInstanceId,
};

use crate::endpoint_origin::{is_loopback_host, EndpointOrigin};

const CORPUS_DIR_NAME: &str = "threestrands-sync";
/// Same defensive ceiling as the folder adapter: far above any real
/// envelope chunk, only here to bound a corrupted or hostile response.
const MAX_OBJECT_BYTES: u64 = 8 * 1024 * 1024;
const SCAN_PAGE_SIZE: usize = 500;
/// Long enough for an 8 MiB upload to start on a slow link; S3 checks
/// expiry when a request begins, not when its body finishes.
const PRESIGN_EXPIRY: Duration = Duration::from_secs(120);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PREFIX_CHARS: usize = 256;
const MAX_REGION_CHARS: usize = 64;
const MAX_ACCESS_KEY_CHARS: usize = 256;
const MAX_SECRET_CHARS: usize = 1024;
const MAX_SESSION_TOKEN_CHARS: usize = 8192;
/// Bounds how much of a provider's error message is surfaced, so a verbose
/// server can't flood Settings or the transport's `last_error`.
const MAX_ERROR_MESSAGE_CHARS: usize = 300;

// ================================ Configuration ==============================

/// The non-secret, persisted configuration for one S3 connector — what
/// `sync_transports.config_json` holds for `kind='s3'`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub path_style: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl S3Config {
    /// `bucket/prefix` as shown to the user — never a credential.
    pub fn display_location(&self) -> String {
        let prefix = self.prefix.trim_matches('/');
        if prefix.is_empty() {
            format!("{} · {}", self.endpoint.trim_end_matches('/'), self.bucket)
        } else {
            format!("{} · {}/{}", self.endpoint.trim_end_matches('/'), self.bucket, prefix)
        }
    }
}

/// The secret half of an S3 connector. Lives only in the OS keychain; its
/// `Debug` output is redacted so it can't leak through a stray `{:?}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S3Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

impl std::fmt::Debug for S3Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Credentials")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &self.session_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedConfig {
    origin: EndpointOrigin,
    region: String,
    bucket: String,
    /// Normalized: no leading or trailing slash; empty when unset.
    prefix: String,
    path_style: bool,
}

fn validate_config(config: &S3Config) -> Result<ValidatedConfig, String> {
    let origin = EndpointOrigin::parse(&config.endpoint, "Endpoint URL")?;
    let region = validate_region(&config.region)?;
    let bucket = validate_bucket(&config.bucket)?;
    let prefix = validate_prefix(&config.prefix)?;
    let host_is_address = is_loopback_host(&origin.host)
        || origin.host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().is_ok();
    if !config.path_style && host_is_address {
        return Err("Use path-style addressing for an IP address or localhost endpoint".to_string());
    }
    Ok(ValidatedConfig {
        origin,
        region,
        bucket,
        prefix,
        path_style: config.path_style,
    })
}

fn validate_region(region: &str) -> Result<String, String> {
    let region = region.trim();
    if region.is_empty() {
        return Err("Region is required (use \"auto\" if your provider says so)".to_string());
    }
    if region.len() > MAX_REGION_CHARS
        || !region.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Region may contain only letters, digits, hyphens, and underscores".to_string());
    }
    Ok(region.to_string())
}

/// The S3 bucket naming rules: 3–63 characters of lowercase letters,
/// digits, dots, and hyphens, beginning and ending with a letter or digit,
/// with no consecutive dots, and not shaped like an IPv4 address.
fn validate_bucket(bucket: &str) -> Result<String, String> {
    let bucket = bucket.trim();
    let valid_chars = bucket
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
    let valid_ends = bucket.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && bucket.chars().last().is_some_and(|c| c.is_ascii_alphanumeric());
    if !(3..=63).contains(&bucket.len()) || !valid_chars || !valid_ends || bucket.contains("..") {
        return Err(
            "Bucket names are 3–63 lowercase letters, digits, dots, or hyphens, starting and ending with a letter or digit"
                .to_string(),
        );
    }
    if bucket.parse::<std::net::Ipv4Addr>().is_ok() {
        return Err("A bucket name can't be an IP address".to_string());
    }
    Ok(bucket.to_string())
}

/// An optional folder inside the bucket: slash-separated segments of
/// letters, digits, `.`, `_`, and `-`. Refuses anything that could climb
/// out of the configured prefix or collide with another key's meaning.
fn validate_prefix(prefix: &str) -> Result<String, String> {
    let trimmed = prefix.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.len() > MAX_PREFIX_CHARS {
        return Err(format!("Folder prefix must be at most {MAX_PREFIX_CHARS} characters"));
    }
    if trimmed.starts_with('/') {
        return Err("Folder prefix must not start with a slash".to_string());
    }
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            return Err("Folder prefix must not contain empty segments (\"//\")".to_string());
        }
        if segment == "." || segment == ".." {
            return Err("Folder prefix must not contain \".\" or \"..\" segments".to_string());
        }
        if !segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            return Err("Folder prefix may contain only letters, digits, \".\", \"_\", \"-\", and \"/\"".to_string());
        }
    }
    Ok(trimmed.to_string())
}

fn validate_credentials(credentials: &S3Credentials) -> Result<rusty_s3::Credentials, String> {
    let key = credentials.access_key_id.trim();
    let secret = credentials.secret_access_key.trim();
    let token = credentials.session_token.as_deref().map(str::trim).filter(|token| !token.is_empty());
    let printable = |value: &str| value.chars().all(|c| c.is_ascii_graphic());
    if key.is_empty() || key.len() > MAX_ACCESS_KEY_CHARS || !printable(key) || key.contains('/') {
        return Err("Access key ID is missing or contains characters an access key never has".to_string());
    }
    if secret.is_empty() || secret.len() > MAX_SECRET_CHARS || !printable(secret) {
        return Err("Secret access key is missing or contains spaces or control characters".to_string());
    }
    if let Some(token) = token {
        if token.len() > MAX_SESSION_TOKEN_CHARS || !printable(token) {
            return Err("Session token contains spaces or control characters".to_string());
        }
        return Ok(rusty_s3::Credentials::new_with_token(key, secret, token));
    }
    Ok(rusty_s3::Credentials::new(key, secret))
}

// ================================ Error mapping ==============================

/// Extracts the text of the first `<tag>…</tag>` element. S3 error bodies
/// are small, flat documents; only `Code` and `Message` are ever read, so
/// echoed request details (`StringToSign`, `CanonicalRequest`, the access
/// key id) in a verbose error never reach the user.
fn xml_element<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = start + body[start..].find(&close)?;
    Some(body[start..end].trim())
}

fn provider_message(status: reqwest::StatusCode, body: &str) -> String {
    let code = xml_element(body, "Code");
    let message = xml_element(body, "Message");
    let text = match (code, message) {
        (Some(code), Some(message)) if !message.is_empty() => format!("{code}: {message}"),
        (Some(code), _) => code.to_string(),
        _ => format!("HTTP {status}"),
    };
    text.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

/// Maps a non-success S3 response to a typed transport error.
fn map_status_error(status: reqwest::StatusCode, body: &str) -> TransportError {
    let code = xml_element(body, "Code").unwrap_or_default();
    let message = provider_message(status, body);
    match code {
        "RequestTimeTooSkewed" => {
            return TransportError::Authentication(
                "This device's clock differs too much from the storage server's. Check the system date and time."
                    .to_string(),
            )
        }
        "InvalidAccessKeyId" | "SignatureDoesNotMatch" | "AccessDenied" | "ExpiredToken" | "InvalidToken"
        | "TokenRefreshRequired" => return TransportError::Authentication(message),
        "SlowDown" | "ServiceUnavailable" => return TransportError::Quota(message),
        "NoSuchBucket" => return TransportError::Permanent(format!("The bucket does not exist ({message})")),
        _ => {}
    }
    match status.as_u16() {
        300..=399 => TransportError::Permanent(
            "The storage endpoint redirected the request; redirects are never followed. Check the region and path-style setting."
                .to_string(),
        ),
        401 | 403 => TransportError::Authentication(message),
        429 => TransportError::Quota(message),
        500..=599 => TransportError::Transient(message),
        _ => TransportError::Permanent(message),
    }
}

/// Every reqwest failure that reaches here is network-level (timeout,
/// refused or reset connection, truncated body). The URL is stripped first:
/// a presigned URL carries the request signature and any session token.
fn map_reqwest_error(error: reqwest::Error) -> TransportError {
    TransportError::Transient(error.without_url().to_string())
}

fn is_not_found(status: reqwest::StatusCode, body: &str) -> bool {
    status == reqwest::StatusCode::NOT_FOUND
        || matches!(xml_element(body, "Code"), Some("NoSuchKey" | "NotFound"))
}

// ================================= Transport =================================

/// What "Test connection" reports before the user saves a connector, so a
/// missing permission is explained up front instead of surfacing later as
/// a stuck enrollment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct S3ProbeReport {
    /// Whether the endpoint answered at all (any HTTP response).
    pub reachable: bool,
    pub can_list: bool,
    pub can_write: bool,
    pub can_read: bool,
    pub can_delete: bool,
    /// `Some(true)` when bucket versioning is enabled — deleted and
    /// replaced objects then keep old versions until lifecycle rules remove
    /// them. `None` when the key may not read the versioning setting.
    pub versioning_enabled: Option<bool>,
    /// The first failure, in plain language, if any check failed.
    pub error: Option<String>,
}

pub struct S3Transport {
    instance_id: TransportInstanceId,
    bucket: rusty_s3::Bucket,
    credentials: rusty_s3::Credentials,
    /// `<prefix>/threestrands-sync` (or just `threestrands-sync`), with no
    /// trailing slash. Every key this adapter touches starts with it.
    root: String,
    client: reqwest::Client,
}

impl S3Transport {
    pub fn new(
        instance_id: impl Into<String>,
        config: &S3Config,
        credentials: &S3Credentials,
    ) -> Result<Self, TransportError> {
        let validated = validate_config(config).map_err(TransportError::Permanent)?;
        let credentials = validate_credentials(credentials).map_err(TransportError::Authentication)?;
        // A trailing slash makes `Url::join` append the bucket to any path
        // prefix instead of replacing it.
        let endpoint = url::Url::parse(&format!("{}/", validated.origin.base_url()))
            .map_err(|error| TransportError::Permanent(format!("Invalid endpoint URL: {error}")))?;
        let style = if validated.path_style { UrlStyle::Path } else { UrlStyle::VirtualHost };
        let bucket = rusty_s3::Bucket::new(endpoint, style, validated.bucket.clone(), validated.region.clone())
            .map_err(|error| TransportError::Permanent(format!("Invalid bucket endpoint: {error:?}")))?;
        let root = if validated.prefix.is_empty() {
            CORPUS_DIR_NAME.to_string()
        } else {
            format!("{}/{CORPUS_DIR_NAME}", validated.prefix)
        };
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // Never follow a redirect: a redirected presigned request would
            // hand its signature to whatever origin the server named.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| TransportError::Permanent(error.without_url().to_string()))?;
        Ok(Self {
            instance_id: TransportInstanceId(instance_id.into()),
            bucket,
            credentials,
            root,
            client,
        })
    }

    fn object_key(&self, cid: &Cid) -> Result<String, TransportError> {
        let name = sanitize_cid(&cid.0)?;
        Ok(format!("{}/objects/{}/{name}.block", self.root, &name[..2]))
    }

    fn head_key(&self, device_id: &DeviceId) -> String {
        let tag: String = device_id.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
        format!("{}/heads/{tag}.head", self.root)
    }

    fn objects_prefix(&self) -> String {
        format!("{}/objects/", self.root)
    }

    /// Parses `<root>/objects/<shard>/<cid>.block` back into a CID, or
    /// `None` for any key that isn't exactly that shape.
    fn cid_from_object_key(&self, key: &str) -> Option<Cid> {
        let rest = key.strip_prefix(&self.objects_prefix())?;
        let (shard, file) = rest.split_once('/')?;
        let name = file.strip_suffix(".block")?;
        let name = sanitize_cid(name).ok()?;
        (shard == &name[..2]).then(|| Cid(name.to_string()))
    }

    /// Sends one presigned request. `Err` here means no HTTP response was
    /// received (or the server tried to redirect); HTTP error statuses are
    /// returned as `Ok` for the caller to interpret.
    async fn send(
        &self,
        method: reqwest::Method,
        url: url::Url,
        headers: &[(&'static str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, TransportError> {
        let mut request = self.client.request(method, url);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request.send().await.map_err(map_reqwest_error)?;
        if response.status().is_redirection() {
            return Err(map_status_error(response.status(), ""));
        }
        Ok(response)
    }

    async fn error_from(response: reqwest::Response) -> TransportError {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        map_status_error(status, &body)
    }

    async fn put_key(&self, key: &str, bytes: &[u8]) -> Result<(), TransportError> {
        if bytes.len() as u64 > MAX_OBJECT_BYTES {
            return Err(TransportError::Permanent("object exceeds the S3 adapter's size limit".to_string()));
        }
        // Content-MD5 is the integrity check every S3-compatible service
        // honors: the server rejects a body corrupted in transit. Reads are
        // verified independently by CID.
        let md5 = base64::engine::general_purpose::STANDARD.encode(Md5::digest(bytes));
        let mut action = self.bucket.put_object(Some(&self.credentials), key);
        action.headers_mut().insert("content-md5", md5.clone());
        let url = action.sign(PRESIGN_EXPIRY);
        let response = self
            .send(
                reqwest::Method::PUT,
                url,
                &[("content-md5", md5), ("content-type", "application/octet-stream".to_string())],
                Some(bytes.to_vec()),
            )
            .await?;
        if !response.status().is_success() {
            return Err(Self::error_from(response).await);
        }
        Ok(())
    }

    /// Fetches one key with a hard size cap. `NotFound` for a missing key.
    async fn get_key(&self, key: &str) -> Result<Vec<u8>, TransportError> {
        let url = self.bucket.get_object(Some(&self.credentials), key).sign(PRESIGN_EXPIRY);
        let mut response = self.send(reqwest::Method::GET, url, &[], None).await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if is_not_found(status, &body) {
                return Err(TransportError::NotFound);
            }
            return Err(map_status_error(status, &body));
        }
        if response.content_length().is_some_and(|length| length > MAX_OBJECT_BYTES) {
            return Err(TransportError::Corruption("stored object exceeds the size limit".to_string()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(map_reqwest_error)? {
            if (bytes.len() + chunk.len()) as u64 > MAX_OBJECT_BYTES {
                return Err(TransportError::Corruption("stored object exceeds the size limit".to_string()));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn delete_key(&self, key: &str) -> Result<(), TransportError> {
        let url = self.bucket.delete_object(Some(&self.credentials), key).sign(PRESIGN_EXPIRY);
        let response = self.send(reqwest::Method::DELETE, url, &[], None).await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        // S3 itself answers 204 for an absent key; some compatible services
        // answer 404. Either way the key is gone.
        if is_not_found(status, &body) {
            return Ok(());
        }
        Err(map_status_error(status, &body))
    }

    /// One `ListObjectsV2` page under `prefix`, resuming after
    /// `start_after`. Returns the raw keys with sizes and whether more
    /// pages remain.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        max_keys: usize,
    ) -> Result<(Vec<(String, u64)>, bool), TransportError> {
        let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
        action.with_prefix(prefix);
        action.with_max_keys(max_keys);
        if let Some(start_after) = start_after {
            action.with_start_after(start_after);
        }
        let url = action.sign(PRESIGN_EXPIRY);
        let response = self.send(reqwest::Method::GET, url, &[], None).await?;
        if !response.status().is_success() {
            return Err(Self::error_from(response).await);
        }
        let body = response.text().await.map_err(map_reqwest_error)?;
        let parsed = rusty_s3::actions::ListObjectsV2::parse_response(&body)
            .map_err(|_| TransportError::Corruption("the storage server returned an unreadable object listing".to_string()))?;
        let truncated = parsed.next_continuation_token.is_some();
        let keys = parsed
            .contents
            .into_iter()
            .map(|content| (content.key, content.size))
            .collect();
        Ok((keys, truncated))
    }

    /// Every key (with its size) under this connector's corpus root.
    async fn list_all_corpus_keys(&self) -> Result<Vec<(String, u64)>, TransportError> {
        let prefix = format!("{}/", self.root);
        let mut all = Vec::new();
        let mut start_after: Option<String> = None;
        loop {
            let (keys, truncated) = self.list_page(&prefix, start_after.as_deref(), 1000).await?;
            let last = keys.last().map(|(key, _)| key.clone());
            all.extend(keys.into_iter().filter(|(key, _)| key.starts_with(&prefix)));
            match (truncated, last) {
                (true, Some(last)) => start_after = Some(last),
                _ => break,
            }
        }
        Ok(all)
    }

    /// Removes every object under this connector's corpus root — "delete
    /// synchronized data." Never touches a key outside
    /// `<prefix>/threestrands-sync/`, and reports rather than swallows a
    /// partial failure.
    pub async fn delete_all_corpus_data(&self) -> Result<(), TransportError> {
        let mut errors = Vec::new();
        for (key, _) in self.list_all_corpus_keys().await? {
            if let Err(error) = self.delete_key(&key).await {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(TransportError::Permanent(format!(
                "could not delete every corpus object: {}",
                errors.join("; ")
            )))
        }
    }

    /// Total bytes stored under this connector's corpus root, for the
    /// Settings storage estimate.
    pub async fn corpus_size_bytes(&self) -> Result<u64, TransportError> {
        Ok(self.list_all_corpus_keys().await?.iter().map(|(_, size)| size).sum())
    }

    /// Checks what this key may do: list, then write, read back, and
    /// delete one random object under `<root>/probe/`, then (best-effort)
    /// read the bucket's versioning setting. Stops at the first failed
    /// step, leaving the later checks `false`.
    pub async fn probe(&self) -> S3ProbeReport {
        let mut report = S3ProbeReport::default();

        let mut list = self.bucket.list_objects_v2(Some(&self.credentials));
        list.with_prefix(format!("{}/", self.root));
        list.with_max_keys(1);
        let response = match self.send(reqwest::Method::GET, list.sign(PRESIGN_EXPIRY), &[], None).await {
            Ok(response) => response,
            Err(error) => {
                report.error = Some(error.to_string());
                return report;
            }
        };
        report.reachable = true;
        if !response.status().is_success() {
            report.error = Some(Self::error_from(response).await.to_string());
            return report;
        }
        report.can_list = true;

        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let nonce_hex: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let key = format!("{}/probe/{nonce_hex}", self.root);
        let payload = format!("threestrands connection test {nonce_hex}").into_bytes();

        if let Err(error) = self.put_key(&key, &payload).await {
            report.error = Some(error.to_string());
            return report;
        }
        report.can_write = true;

        match self.get_key(&key).await {
            Ok(bytes) if bytes == payload => report.can_read = true,
            Ok(_) => {
                report.error = Some("The storage server returned different bytes than were written".to_string());
            }
            Err(error) => report.error = Some(error.to_string()),
        }

        // Always try to clean up, even after a failed read.
        match self.delete_key(&key).await {
            Ok(()) => report.can_delete = true,
            Err(error) => {
                report.error.get_or_insert(error.to_string());
            }
        }

        report.versioning_enabled = self.versioning_enabled().await;
        report
    }

    /// `GetBucketVersioning`: `Some(true)` only for `Enabled` — a
    /// `Suspended` bucket stops creating new versions. `None` if the key
    /// may not read the setting or the server doesn't implement it.
    async fn versioning_enabled(&self) -> Option<bool> {
        let url = rusty_s3::signing::sign(
            &jiff::Timestamp::now(),
            rusty_s3::Method::Get,
            self.bucket.base_url().clone(),
            self.credentials.key(),
            self.credentials.secret(),
            self.credentials.token(),
            self.bucket.region(),
            PRESIGN_EXPIRY.as_secs(),
            std::iter::once(("versioning", "")),
            std::iter::empty(),
        );
        let response = self.send(reqwest::Method::GET, url, &[], None).await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body = response.text().await.ok()?;
        Some(xml_element(&body, "Status") == Some("Enabled"))
    }
}

fn sanitize_cid(cid: &str) -> Result<&str, TransportError> {
    if cid.len() < 8 || cid.len() > 256 || !cid.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(TransportError::Permanent("invalid content identifier".to_string()));
    }
    Ok(cid)
}

#[async_trait]
impl SyncTransport for S3Transport {
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
        let key = self.object_key(cid)?;
        // Idempotent by CID: re-putting the same content-addressed bytes
        // overwrites them with themselves. S3 has strong read-after-write
        // consistency, so a successful PUT is durable and readable.
        self.put_key(&key, bytes).await?;
        Ok(ObjectLocator {
            cid: cid.clone(),
            remote_id: Some(key),
        })
    }

    async fn get_object(&self, cid: &Cid) -> Result<Vec<u8>, TransportError> {
        let key = self.object_key(cid)?;
        let bytes = self.get_key(&key).await?;
        // Storage responses are never trusted on their word.
        if compute_cid(&bytes) != cid.0 {
            return Err(TransportError::Corruption(
                "stored object bytes do not match the requested CID".to_string(),
            ));
        }
        Ok(bytes)
    }

    async fn publish_head(&self, head: &SignedDeviceHead) -> Result<HeadLocator, TransportError> {
        let key = self.head_key(&head.head.device_id);
        let bytes = encode_signed_head(head).map_err(|error| TransportError::Permanent(error.to_string()))?;
        self.put_key(&key, &bytes).await?;
        Ok(HeadLocator {
            device_id: head.head.device_id,
            remote_id: Some(key),
        })
    }

    async fn resolve_heads(&self, known: &[HeadLocator]) -> Result<Vec<SignedDeviceHead>, TransportError> {
        let mut heads = Vec::new();
        for locator in known {
            match self.get_key(&self.head_key(&locator.device_id)).await {
                // Signature verification happens in the sync layer, which
                // has the roster's public keys. A malformed head stays
                // pending rather than failing the whole resolution.
                Ok(bytes) => {
                    if let Ok(signed) = decode_signed_head(&bytes) {
                        heads.push(signed);
                    }
                }
                Err(TransportError::NotFound) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(heads)
    }

    async fn scan(&self, cursor: Option<&str>) -> Result<Option<ScanPage>, TransportError> {
        // The cursor is the last key of the previous page, resent as
        // `start-after`: unlike a continuation token it never expires, and
        // it resumes correctly after new objects arrive. Anything that
        // isn't one of our object keys (a different format, a reset
        // corpus) means "fall back to head-based discovery."
        if let Some(cursor) = cursor {
            if !cursor.starts_with(&self.objects_prefix()) {
                return Ok(None);
            }
        }
        let (keys, truncated) = self.list_page(&self.objects_prefix(), cursor, SCAN_PAGE_SIZE).await?;
        let next_cursor = if truncated { keys.last().map(|(key, _)| key.clone()) } else { None };
        let objects = keys
            .iter()
            .filter_map(|(key, _)| {
                self.cid_from_object_key(key).map(|cid| ObjectLocator {
                    cid,
                    remote_id: Some(key.clone()),
                })
            })
            .collect();
        Ok(Some(ScanPage { objects, next_cursor }))
    }

    async fn delete_object(&self, cid: &Cid) -> Result<(), TransportError> {
        let key = self.object_key(cid)?;
        self.delete_key(&key).await
    }

    async fn health(&self) -> Result<TransportHealth, TransportError> {
        match self.list_page(&format!("{}/", self.root), None, 1).await {
            Ok(_) => Ok(TransportHealth::Healthy),
            Err(error) => Ok(TransportHealth::Unavailable(error.to_string())),
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    fn config(endpoint: &str) -> S3Config {
        S3Config {
            endpoint: endpoint.to_string(),
            region: "us-east-1".to_string(),
            bucket: "sync-bucket".to_string(),
            prefix: String::new(),
            path_style: false,
            label: None,
        }
    }

    fn credentials() -> S3Credentials {
        S3Credentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "secret/with+chars".to_string(),
            session_token: None,
        }
    }

    #[test]
    fn accepts_a_typical_aws_config() {
        let validated = validate_config(&config("https://s3.us-east-1.amazonaws.com")).unwrap();
        assert_eq!(validated.bucket, "sync-bucket");
        assert_eq!(validated.prefix, "");
    }

    #[test]
    fn rejects_plaintext_http_to_a_remote_host_but_allows_loopback_path_style() {
        assert!(validate_config(&config("http://s3.example.com")).is_err());
        let mut local = config("http://127.0.0.1:9000");
        local.path_style = true;
        assert!(validate_config(&local).is_ok());
    }

    #[test]
    fn requires_path_style_for_an_address_endpoint() {
        let error = validate_config(&config("http://127.0.0.1:9000")).unwrap_err();
        assert!(error.contains("path-style"), "{error}");
        assert!(validate_config(&config("https://10.0.0.5")).is_err());
    }

    #[test]
    fn rejects_user_info_query_and_fragment_in_the_endpoint() {
        assert!(validate_config(&config("https://key:secret@s3.example.com")).is_err());
        assert!(validate_config(&config("https://s3.example.com/?X-Amz-Signature=abc")).is_err());
        assert!(validate_config(&config("https://s3.example.com/#frag")).is_err());
    }

    #[test]
    fn bucket_names_follow_the_s3_rules() {
        for good in ["abc", "my-sync.bucket", "a1b2c3", &"a".repeat(63)] {
            assert!(validate_bucket(good).is_ok(), "{good}");
        }
        for bad in ["ab", &"a".repeat(64), "Upper", "-leading", "trailing-", "dot..dot", "192.168.1.1", "under_score", "sp ace", ""] {
            assert!(validate_bucket(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn prefixes_cannot_climb_or_smuggle_characters() {
        assert_eq!(validate_prefix("").unwrap(), "");
        assert_eq!(validate_prefix("threestrands/").unwrap(), "threestrands");
        assert_eq!(validate_prefix("a/b_c/d-e.f").unwrap(), "a/b_c/d-e.f");
        for bad in ["/leading", "a//b", "../escape", "a/../b", "a/./b", ".", "a b", "a?b", "a%2fb", &"a".repeat(257)] {
            assert!(validate_prefix(bad).is_err(), "{bad}");
        }
        assert!(validate_prefix(&"a".repeat(256)).is_ok());
    }

    #[test]
    fn regions_are_plain_tokens() {
        assert!(validate_region("auto").is_ok());
        assert!(validate_region("eu-central-003").is_ok());
        assert!(validate_region("").is_err());
        assert!(validate_region("us east").is_err());
        assert!(validate_region(&"a".repeat(65)).is_err());
    }

    #[test]
    fn credentials_reject_whitespace_and_control_characters() {
        assert!(validate_credentials(&credentials()).is_ok());
        let mut bad = credentials();
        bad.secret_access_key = "has space".to_string();
        assert!(validate_credentials(&bad).is_err());
        let mut bad = credentials();
        bad.access_key_id = String::new();
        assert!(validate_credentials(&bad).is_err());
        let mut bad = credentials();
        bad.access_key_id = "AKIA/INVALID".to_string();
        assert!(validate_credentials(&bad).is_err());
        let mut bad = credentials();
        bad.session_token = Some("tok\nen".to_string());
        assert!(validate_credentials(&bad).is_err());
    }

    #[test]
    fn credentials_debug_output_is_redacted() {
        let mut creds = credentials();
        creds.session_token = Some("session-token-value".to_string());
        let debug = format!("{creds:?}");
        assert!(!debug.contains("AKIAEXAMPLE"));
        assert!(!debug.contains("secret/with+chars"));
        assert!(!debug.contains("session-token-value"));
    }

    #[test]
    fn config_json_round_trips_with_defaults() {
        let parsed: S3Config =
            serde_json::from_str(r#"{"endpoint":"https://s3.example.com","region":"auto","bucket":"b-1"}"#).unwrap();
        assert_eq!(parsed.prefix, "");
        assert!(!parsed.path_style);
        assert_eq!(parsed.label, None);
        let json = serde_json::to_string(&parsed).unwrap();
        assert_eq!(serde_json::from_str::<S3Config>(&json).unwrap(), parsed);
    }

    #[test]
    fn object_and_head_keys_mirror_the_folder_layout() {
        let mut with_prefix = config("https://s3.example.com");
        with_prefix.prefix = "team/threestrands".to_string();
        let transport = S3Transport::new("s3-a", &with_prefix, &credentials()).unwrap();
        let cid = Cid::for_bytes(b"layout");
        let key = transport.object_key(&cid).unwrap();
        assert_eq!(key, format!("team/threestrands/threestrands-sync/objects/{}/{}.block", &cid.0[..2], cid.0));
        assert_eq!(transport.cid_from_object_key(&key), Some(cid.clone()));

        let device = DeviceId::from_bytes([0xab; 16]);
        assert_eq!(
            transport.head_key(&device),
            format!("team/threestrands/threestrands-sync/heads/{}.head", "ab".repeat(16))
        );

        let bare = S3Transport::new("s3-b", &config("https://s3.example.com"), &credentials()).unwrap();
        assert!(bare.object_key(&cid).unwrap().starts_with("threestrands-sync/objects/"));
    }

    #[test]
    fn object_key_parsing_rejects_anything_but_the_exact_shape() {
        let transport = S3Transport::new("s3", &config("https://s3.example.com"), &credentials()).unwrap();
        let cid = Cid::for_bytes(b"shape");
        let prefix = transport.objects_prefix();
        let wrong_shard = format!("{prefix}zz/{}.block", cid.0);
        let wrong_suffix = format!("{prefix}{}/{}.tmp", &cid.0[..2], cid.0);
        let nested = format!("{prefix}{}/x/{}.block", &cid.0[..2], cid.0);
        let outside = format!("elsewhere/{}/{}.block", &cid.0[..2], cid.0);
        for key in [wrong_shard, wrong_suffix, nested, outside] {
            assert_eq!(transport.cid_from_object_key(&key), None, "{key}");
        }
    }

    #[test]
    fn error_codes_map_to_typed_errors() {
        use reqwest::StatusCode;
        let body = |code: &str| format!("<Error><Code>{code}</Code><Message>msg</Message></Error>");
        assert!(matches!(map_status_error(StatusCode::FORBIDDEN, &body("AccessDenied")), TransportError::Authentication(_)));
        assert!(matches!(map_status_error(StatusCode::FORBIDDEN, &body("SignatureDoesNotMatch")), TransportError::Authentication(_)));
        assert!(matches!(map_status_error(StatusCode::FORBIDDEN, &body("InvalidAccessKeyId")), TransportError::Authentication(_)));
        match map_status_error(StatusCode::FORBIDDEN, &body("RequestTimeTooSkewed")) {
            TransportError::Authentication(message) => assert!(message.contains("clock"), "{message}"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(map_status_error(StatusCode::SERVICE_UNAVAILABLE, &body("SlowDown")), TransportError::Quota(_)));
        assert!(matches!(map_status_error(StatusCode::TOO_MANY_REQUESTS, ""), TransportError::Quota(_)));
        assert!(matches!(map_status_error(StatusCode::INTERNAL_SERVER_ERROR, &body("InternalError")), TransportError::Transient(_)));
        assert!(matches!(map_status_error(StatusCode::MOVED_PERMANENTLY, ""), TransportError::Permanent(_)));
        assert!(matches!(map_status_error(StatusCode::NOT_FOUND, &body("NoSuchBucket")), TransportError::Permanent(_)));
        assert!(matches!(map_status_error(StatusCode::BAD_REQUEST, &body("InvalidArgument")), TransportError::Permanent(_)));
    }

    #[test]
    fn error_messages_surface_only_code_and_message() {
        let verbose = "<Error><Code>SignatureDoesNotMatch</Code><Message>bad sig</Message>\
            <AWSAccessKeyId>AKIAEXAMPLE</AWSAccessKeyId><StringToSign>AWS4-HMAC-SHA256 secret-ish</StringToSign>\
            <CanonicalRequest>GET /?X-Amz-Security-Token=tok</CanonicalRequest></Error>";
        let message = map_status_error(reqwest::StatusCode::FORBIDDEN, verbose).to_string();
        assert!(message.contains("SignatureDoesNotMatch"));
        assert!(!message.contains("AKIAEXAMPLE"));
        assert!(!message.contains("StringToSign"));
        assert!(!message.contains("X-Amz-Security-Token"));
    }

    #[test]
    fn provider_messages_are_bounded() {
        let long = format!("<Error><Code>X</Code><Message>{}</Message></Error>", "m".repeat(5000));
        assert!(provider_message(reqwest::StatusCode::BAD_REQUEST, &long).chars().count() <= MAX_ERROR_MESSAGE_CHARS);
    }
}

/// A minimal, in-memory, S3-compatible server implementing exactly the
/// subset this adapter uses — path-style `PUT`/`GET`/`DELETE` object,
/// `ListObjectsV2`, and `GetBucketVersioning` — with fault injection. It
/// recomputes every request's SigV4 presigned signature from the
/// configured credentials, so a wrong secret or a mangled signed header
/// fails exactly as it would against a real service.
#[cfg(test)]
pub(crate) mod fake_server {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, MutexGuard};

    use axum::body::Bytes;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Router;
    use base64::Engine;
    use md5::{Digest, Md5};

    pub const ACCESS_KEY: &str = "AKIAFAKEEXAMPLE";
    pub const SECRET_KEY: &str = "fake/secret+key";
    pub const REGION: &str = "us-test-1";
    pub const BUCKET: &str = "sync-bucket";

    #[derive(Default)]
    pub struct FaultInjection {
        /// The next N requests fail with this status and S3 error code.
        pub fail_next: Option<(usize, StatusCode, &'static str)>,
        /// `DELETE` always fails with `AccessDenied`.
        pub deny_delete: bool,
        /// `PUT` always fails with `AccessDenied`.
        pub deny_put: bool,
        pub corrupt_next_get: bool,
        pub oversize_next_get: bool,
        pub redirect_next: bool,
        /// Errors echo the full request URI inside `<CanonicalRequest>`,
        /// like a verbose real-world server would.
        pub echo_request_in_errors: bool,
        /// `GetBucketVersioning` status element; `None` answers 403.
        pub versioning_status: Option<&'static str>,
        pub requests: usize,
    }

    #[derive(Default)]
    pub struct ServerState {
        pub objects: BTreeMap<String, Vec<u8>>,
        pub faults: FaultInjection,
        pub session_token: Option<String>,
        addr: Option<std::net::SocketAddr>,
    }

    type SharedState = Arc<Mutex<ServerState>>;

    pub struct FakeS3Server {
        pub addr: std::net::SocketAddr,
        state: SharedState,
    }

    impl FakeS3Server {
        pub async fn spawn() -> Self {
            let state: SharedState = Arc::new(Mutex::new(ServerState::default()));
            let app = Router::new()
                .route("/{bucket}/", get(bucket_get))
                .route("/{bucket}/{*key}", get(object).put(object).delete(object))
                .with_state(state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            state.lock().unwrap().addr = Some(addr);
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self { addr, state }
        }

        pub fn endpoint(&self) -> String {
            format!("http://{}", self.addr)
        }

        pub fn config(&self, prefix: &str) -> super::S3Config {
            super::S3Config {
                endpoint: self.endpoint(),
                region: REGION.to_string(),
                bucket: BUCKET.to_string(),
                prefix: prefix.to_string(),
                path_style: true,
                label: None,
            }
        }

        pub fn credentials() -> super::S3Credentials {
            super::S3Credentials {
                access_key_id: ACCESS_KEY.to_string(),
                secret_access_key: SECRET_KEY.to_string(),
                session_token: None,
            }
        }

        pub fn state(&self) -> MutexGuard<'_, ServerState> {
            self.state.lock().unwrap()
        }
    }

    fn s3_error(status: StatusCode, code: &str, extra: &str) -> Response {
        (
            status,
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>injected {code}</Message>{extra}</Error>"),
        )
            .into_response()
    }

    fn query_pairs(uri: &Uri) -> Vec<(String, String)> {
        uri.query()
            .map(|query| url::form_urlencoded::parse(query.as_bytes()).into_owned().collect())
            .unwrap_or_default()
    }

    fn query_one<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        pairs.iter().find(|(candidate, _)| candidate == key).map(|(_, value)| value.as_str())
    }

    /// Recomputes the presigned SigV4 signature with the server's own copy
    /// of the credentials and compares it to the one the client sent.
    fn verify_signature(state: &ServerState, addr: std::net::SocketAddr, method: &Method, uri: &Uri, headers: &HeaderMap) -> bool {
        let pairs = query_pairs(uri);
        let (Some(date), Some(signature), Some(credential), Some(expires), Some(signed_headers)) = (
            query_one(&pairs, "X-Amz-Date"),
            query_one(&pairs, "X-Amz-Signature"),
            query_one(&pairs, "X-Amz-Credential"),
            query_one(&pairs, "X-Amz-Expires"),
            query_one(&pairs, "X-Amz-SignedHeaders"),
        ) else {
            return false;
        };
        if !credential.starts_with(&format!("{ACCESS_KEY}/")) {
            return false;
        }
        if query_one(&pairs, "X-Amz-Security-Token") != state.session_token.as_deref() {
            return false;
        }
        // SigV4 dates are UTC with a literal `Z`, not a parseable offset.
        let Ok(timestamp) = jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", date)
            .and_then(|datetime| datetime.to_zoned(jiff::tz::TimeZone::UTC))
            .map(|zoned| zoned.timestamp())
        else {
            return false;
        };
        let rusty_method = match *method {
            Method::GET => rusty_s3::Method::Get,
            Method::PUT => rusty_s3::Method::Put,
            Method::DELETE => rusty_s3::Method::Delete,
            _ => return false,
        };
        let other_query: Vec<(String, String)> =
            pairs.iter().filter(|(key, _)| !key.starts_with("X-Amz-")).cloned().collect();
        let mut header_values = Vec::new();
        for name in signed_headers.split(';').filter(|name| *name != "host") {
            let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) else { return false };
            header_values.push((name.to_string(), value.to_string()));
        }
        let url = url::Url::parse(&format!("http://{addr}{}", uri.path())).unwrap();
        let expected = rusty_s3::signing::sign(
            &timestamp,
            rusty_method,
            url,
            ACCESS_KEY,
            SECRET_KEY,
            state.session_token.as_deref(),
            REGION,
            expires.parse().unwrap_or(0),
            other_query.iter().map(|(key, value)| (key.as_str(), value.as_str())),
            header_values.iter().map(|(key, value)| (key.as_str(), value.as_str())),
        );
        expected
            .query_pairs()
            .find(|(key, _)| key == "X-Amz-Signature")
            .is_some_and(|(_, value)| value == signature)
    }

    /// Shared per-request gatekeeping: counts the request, applies
    /// injected faults, and checks the signature.
    #[allow(clippy::result_large_err)]
    fn gate(guard: &mut ServerState, addr: std::net::SocketAddr, method: &Method, uri: &Uri, headers: &HeaderMap) -> Result<(), Response> {
        guard.faults.requests += 1;
        let extra = if guard.faults.echo_request_in_errors {
            format!("<CanonicalRequest>{uri}</CanonicalRequest>")
        } else {
            String::new()
        };
        if guard.faults.redirect_next {
            guard.faults.redirect_next = false;
            return Err((StatusCode::MOVED_PERMANENTLY, [("location", "https://elsewhere.example/")], "").into_response());
        }
        if let Some((remaining, status, code)) = guard.faults.fail_next {
            if remaining > 0 {
                guard.faults.fail_next = if remaining > 1 { Some((remaining - 1, status, code)) } else { None };
                return Err(s3_error(status, code, &extra));
            }
        }
        if !verify_signature(guard, addr, method, uri, headers) {
            return Err(s3_error(StatusCode::FORBIDDEN, "SignatureDoesNotMatch", &extra));
        }
        Ok(())
    }

    async fn bucket_get(
        State(state): State<SharedState>,
        Path(bucket): Path<String>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> Response {
        let mut guard = state.lock().unwrap();
        let addr = guard.addr.expect("address recorded at spawn");
        if let Err(response) = gate(&mut guard, addr, &method, &uri, &headers) {
            return response;
        }
        if bucket != BUCKET {
            return s3_error(StatusCode::NOT_FOUND, "NoSuchBucket", "");
        }
        let pairs = query_pairs(&uri);
        if query_one(&pairs, "versioning").is_some() {
            return match guard.faults.versioning_status {
                Some(status) => (
                    StatusCode::OK,
                    format!("<VersioningConfiguration xmlns=\"{S3_NS}\"><Status>{status}</Status></VersioningConfiguration>"),
                )
                    .into_response(),
                None => s3_error(StatusCode::FORBIDDEN, "AccessDenied", ""),
            };
        }
        if query_one(&pairs, "list-type") != Some("2") {
            return s3_error(StatusCode::BAD_REQUEST, "InvalidArgument", "");
        }
        let prefix = query_one(&pairs, "prefix").unwrap_or("");
        let start_after = query_one(&pairs, "start-after");
        let max_keys: usize = query_one(&pairs, "max-keys").and_then(|value| value.parse().ok()).unwrap_or(1000);
        let matching: Vec<(&String, &Vec<u8>)> = guard
            .objects
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .filter(|(key, _)| start_after.is_none_or(|after| key.as_str() > after))
            .collect();
        let truncated = matching.len() > max_keys;
        let mut xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"{S3_NS}\"><Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><MaxKeys>{max_keys}</MaxKeys><IsTruncated>{truncated}</IsTruncated>");
        for (key, bytes) in matching.iter().take(max_keys) {
            xml.push_str(&format!(
                "<Contents><Key>{key}</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified><ETag>&quot;etag&quot;</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                bytes.len()
            ));
        }
        if truncated {
            xml.push_str("<NextContinuationToken>opaque-token</NextContinuationToken>");
        }
        xml.push_str("</ListBucketResult>");
        (StatusCode::OK, xml).into_response()
    }

    async fn object(
        State(state): State<SharedState>,
        Path((bucket, key)): Path<(String, String)>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let mut guard = state.lock().unwrap();
        let addr = guard.addr.expect("address recorded at spawn");
        if let Err(response) = gate(&mut guard, addr, &method, &uri, &headers) {
            return response;
        }
        if bucket != BUCKET {
            return s3_error(StatusCode::NOT_FOUND, "NoSuchBucket", "");
        }
        match method {
            Method::PUT => {
                if guard.faults.deny_put {
                    return s3_error(StatusCode::FORBIDDEN, "AccessDenied", "");
                }
                let provided = headers.get("content-md5").and_then(|value| value.to_str().ok());
                if provided != Some(content_md5(&body).as_str()) {
                    return s3_error(StatusCode::BAD_REQUEST, "BadDigest", "");
                }
                guard.objects.insert(key, body.to_vec());
                StatusCode::OK.into_response()
            }
            Method::GET => {
                let Some(bytes) = guard.objects.get(&key).cloned() else {
                    return s3_error(StatusCode::NOT_FOUND, "NoSuchKey", "");
                };
                if guard.faults.oversize_next_get {
                    guard.faults.oversize_next_get = false;
                    return (StatusCode::OK, vec![0u8; (super::MAX_OBJECT_BYTES + 1) as usize]).into_response();
                }
                if guard.faults.corrupt_next_get {
                    guard.faults.corrupt_next_get = false;
                    let mut corrupted = bytes;
                    match corrupted.first_mut() {
                        Some(byte) => *byte ^= 0xFF,
                        None => corrupted.push(0xFF),
                    }
                    return (StatusCode::OK, corrupted).into_response();
                }
                (StatusCode::OK, bytes).into_response()
            }
            Method::DELETE => {
                if guard.faults.deny_delete {
                    return s3_error(StatusCode::FORBIDDEN, "AccessDenied", "");
                }
                guard.objects.remove(&key);
                StatusCode::NO_CONTENT.into_response()
            }
            _ => s3_error(StatusCode::METHOD_NOT_ALLOWED, "MethodNotAllowed", ""),
        }
    }

    const S3_NS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

    fn content_md5(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(Md5::digest(bytes))
    }
}

#[cfg(test)]
mod transport_tests {
    use super::fake_server::{FakeS3Server, ACCESS_KEY, SECRET_KEY};
    use super::*;
    use axum::http::StatusCode;
    use threestrands_sync_envelope::{sign_device_head, DeviceHead, SigningKey};
    use threestrands_sync_transport::conformance;

    fn open(server: &FakeS3Server, instance_id: &str, prefix: &str) -> S3Transport {
        S3Transport::new(instance_id, &server.config(prefix), &FakeS3Server::credentials()).unwrap()
    }

    fn signed_head(signing_key: &SigningKey, device: [u8; 16], sequence: u64) -> SignedDeviceHead {
        let head = DeviceHead {
            sync_space_id: b"space".to_vec(),
            device_id: DeviceId::from_bytes(device),
            epoch: 1,
            state_sequence: sequence,
            state_cid: Some("bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e".to_string()),
            published_at_ms: 0,
        };
        sign_device_head(signing_key, head).unwrap()
    }

    /// Asserts nothing secret or signature-bearing leaked into `text`.
    fn assert_no_secrets(text: &str, token: Option<&str>) {
        assert!(!text.contains(SECRET_KEY), "secret leaked: {text}");
        assert!(!text.contains("X-Amz-Signature"), "signature leaked: {text}");
        assert!(!text.contains("X-Amz-Credential"), "credential scope leaked: {text}");
        if let Some(token) = token {
            assert!(!text.contains(token), "session token leaked: {text}");
        }
    }

    #[tokio::test]
    async fn passes_the_shared_conformance_suite() {
        let server_a = FakeS3Server::spawn().await;
        let server_b = FakeS3Server::spawn().await;
        conformance::run_all(&open(&server_a, "a", ""), &open(&server_b, "b", "")).await;
    }

    #[tokio::test]
    async fn passes_the_shared_conformance_suite_under_a_prefix() {
        let server = FakeS3Server::spawn().await;
        conformance::run_all(&open(&server, "a", "team/one"), &open(&server, "b", "team/two")).await;
        assert!(server.state().objects.keys().all(|key| key.starts_with("team/")));
    }

    #[tokio::test]
    async fn a_wrong_secret_is_an_authentication_error_that_leaks_nothing() {
        let server = FakeS3Server::spawn().await;
        server.state().faults.echo_request_in_errors = true;
        let mut credentials = FakeS3Server::credentials();
        credentials.secret_access_key = "the-wrong-secret".to_string();
        let transport = S3Transport::new("s3", &server.config(""), &credentials).unwrap();
        let error = transport.put_object(&Cid::for_bytes(b"x"), b"x").await.unwrap_err();
        assert!(matches!(error, TransportError::Authentication(_)), "{error:?}");
        let text = error.to_string();
        assert_no_secrets(&text, None);
        assert!(!text.contains("the-wrong-secret"));
        assert!(!text.contains(ACCESS_KEY));
    }

    #[tokio::test]
    async fn a_session_token_is_signed_and_required_when_configured() {
        let server = FakeS3Server::spawn().await;
        server.state().session_token = Some("session-token-abc".to_string());
        let cid = Cid::for_bytes(b"with token");

        let without = open(&server, "s3", "");
        assert!(matches!(without.put_object(&cid, b"with token").await, Err(TransportError::Authentication(_))));

        let mut credentials = FakeS3Server::credentials();
        credentials.session_token = Some("session-token-abc".to_string());
        let with = S3Transport::new("s3", &server.config(""), &credentials).unwrap();
        with.put_object(&cid, b"with token").await.unwrap();
        assert_eq!(with.get_object(&cid).await.unwrap(), b"with token");
    }

    #[tokio::test]
    async fn provider_errors_echoing_the_request_never_leak_signatures_or_tokens() {
        let server = FakeS3Server::spawn().await;
        server.state().session_token = Some("session-token-abc".to_string());
        server.state().faults.echo_request_in_errors = true;
        let mut credentials = FakeS3Server::credentials();
        credentials.session_token = Some("session-token-abc".to_string());
        let transport = S3Transport::new("s3", &server.config(""), &credentials).unwrap();

        for (status, code) in [
            (StatusCode::FORBIDDEN, "AccessDenied"),
            (StatusCode::INTERNAL_SERVER_ERROR, "InternalError"),
            (StatusCode::SERVICE_UNAVAILABLE, "SlowDown"),
            (StatusCode::BAD_REQUEST, "InvalidArgument"),
        ] {
            server.state().faults.fail_next = Some((3, status, code));
            let errors = [
                transport.put_object(&Cid::for_bytes(b"e"), b"e").await.unwrap_err().to_string(),
                transport.get_object(&Cid::for_bytes(b"e")).await.unwrap_err().to_string(),
                transport.scan(None).await.unwrap_err().to_string(),
            ];
            for text in errors {
                assert_no_secrets(&text, Some("session-token-abc"));
            }
        }
        let report = {
            server.state().faults.fail_next = Some((1, StatusCode::FORBIDDEN, "AccessDenied"));
            transport.probe().await
        };
        assert_no_secrets(report.error.as_deref().unwrap_or_default(), Some("session-token-abc"));
    }

    #[tokio::test]
    async fn network_errors_strip_the_presigned_url() {
        // Bind then drop a listener so the port is closed.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let mut config = FakeS3Server::spawn().await.config("");
        config.endpoint = format!("http://{addr}");
        let mut credentials = FakeS3Server::credentials();
        credentials.session_token = Some("session-token-abc".to_string());
        let transport = S3Transport::new("s3", &config, &credentials).unwrap();

        let error = transport.get_object(&Cid::for_bytes(b"n")).await.unwrap_err();
        assert!(matches!(error, TransportError::Transient(_)), "{error:?}");
        assert_no_secrets(&error.to_string(), Some("session-token-abc"));
        assert!(!error.to_string().contains("127.0.0.1"), "URL leaked: {error}");

        let report = transport.probe().await;
        assert!(!report.reachable);
        assert_no_secrets(report.error.as_deref().unwrap(), Some("session-token-abc"));
    }

    #[tokio::test]
    async fn rejects_a_redirect_rather_than_following_it() {
        let server = FakeS3Server::spawn().await;
        let transport = open(&server, "s3", "");
        server.state().faults.redirect_next = true;
        let before = server.state().faults.requests;
        let error = transport.get_object(&Cid::for_bytes(b"r")).await.unwrap_err();
        assert!(matches!(error, TransportError::Permanent(_)), "{error:?}");
        assert!(error.to_string().contains("redirect"));
        assert_eq!(server.state().faults.requests, before + 1, "the redirect must not be followed");
    }

    #[tokio::test]
    async fn corrupted_and_oversized_responses_are_rejected() {
        let server = FakeS3Server::spawn().await;
        let transport = open(&server, "s3", "");
        let cid = Cid::for_bytes(b"integrity");
        transport.put_object(&cid, b"integrity").await.unwrap();

        server.state().faults.corrupt_next_get = true;
        assert!(matches!(transport.get_object(&cid).await, Err(TransportError::Corruption(_))));

        server.state().faults.oversize_next_get = true;
        assert!(matches!(transport.get_object(&cid).await, Err(TransportError::Corruption(_))));

        assert_eq!(transport.get_object(&cid).await.unwrap(), b"integrity");
    }

    #[tokio::test]
    async fn refuses_to_upload_an_object_over_the_size_limit() {
        let server = FakeS3Server::spawn().await;
        let transport = open(&server, "s3", "");
        let bytes = vec![0u8; (MAX_OBJECT_BYTES + 1) as usize];
        assert!(matches!(transport.put_object(&Cid::for_bytes(&bytes), &bytes).await, Err(TransportError::Permanent(_))));
        assert!(server.state().objects.is_empty());
    }

    #[tokio::test]
    async fn throttling_and_outages_are_retryable_not_permanent() {
        let server = FakeS3Server::spawn().await;
        let transport = open(&server, "s3", "");
        let cid = Cid::for_bytes(b"retry");

        server.state().faults.fail_next = Some((1, StatusCode::SERVICE_UNAVAILABLE, "SlowDown"));
        assert!(matches!(transport.put_object(&cid, b"retry").await, Err(TransportError::Quota(_))));
        server.state().faults.fail_next = Some((1, StatusCode::INTERNAL_SERVER_ERROR, "InternalError"));
        assert!(matches!(transport.put_object(&cid, b"retry").await, Err(TransportError::Transient(_))));
        server.state().faults.fail_next = Some((1, StatusCode::FORBIDDEN, "RequestTimeTooSkewed"));
        match transport.put_object(&cid, b"retry").await {
            Err(TransportError::Authentication(message)) => assert!(message.contains("clock")),
            other => panic!("unexpected {other:?}"),
        }
        transport.put_object(&cid, b"retry").await.unwrap();
    }

    #[tokio::test]
    async fn heads_round_trip_and_skip_missing_or_malformed_ones() {
        let server = FakeS3Server::spawn().await;
        let transport = open(&server, "s3", "p");
        let signing_key = SigningKey::generate(&mut OsRng);

        let head = signed_head(&signing_key, [1; 16], 4);
        transport.publish_head(&head).await.unwrap();
        // Republishing replaces the device's head.
        let newer = signed_head(&signing_key, [1; 16], 5);
        transport.publish_head(&newer).await.unwrap();

        let malformed = DeviceId::from_bytes([2; 16]);
        server.state().objects.insert(transport.head_key(&malformed), b"not a head".to_vec());

        let known = [
            HeadLocator { device_id: DeviceId::from_bytes([1; 16]), remote_id: None },
            HeadLocator { device_id: malformed, remote_id: None },
            HeadLocator { device_id: DeviceId::from_bytes([3; 16]), remote_id: None },
        ];
        let heads = transport.resolve_heads(&known).await.unwrap();
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].head.state_sequence, 5);
    }

    #[tokio::test]
    async fn scan_pages_with_start_after_skips_foreign_keys_and_rejects_unknown_cursors() {
        let server = FakeS3Server::spawn().await;
        let transport = open(&server, "s3", "");
        let mut expected = Vec::new();
        {
            let mut state = server.state();
            for index in 0..(SCAN_PAGE_SIZE + 17) {
                let bytes = format!("scan {index}").into_bytes();
                let cid = Cid::for_bytes(&bytes);
                state.objects.insert(transport.object_key(&cid).unwrap(), bytes);
                expected.push(cid);
            }
            // Keys that must never surface as objects.
            state.objects.insert("threestrands-sync/objects/zz/not-a-cid.block".to_string(), vec![1]);
            state.objects.insert("threestrands-sync/objects/ab/abcdefghij.tmp".to_string(), vec![1]);
            state.objects.insert("threestrands-sync/heads/00.head".to_string(), vec![1]);
            state.objects.insert("unrelated/file".to_string(), vec![1]);
        }

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = transport.scan(cursor.as_deref()).await.unwrap().unwrap();
            pages += 1;
            seen.extend(page.objects.into_iter().map(|object| object.cid));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(pages, 2);
        seen.sort();
        expected.sort();
        assert_eq!(seen, expected);

        assert_eq!(transport.scan(Some("opaque-continuation-token")).await.unwrap(), None);
        assert_eq!(transport.scan(Some("threestrands-sync/heads/x")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn deleting_corpus_data_stays_inside_the_prefix() {
        let server = FakeS3Server::spawn().await;
        let transport = open(&server, "s3", "mine");
        for index in 0..3 {
            let bytes = format!("corpus {index}").into_bytes();
            transport.put_object(&Cid::for_bytes(&bytes), &bytes).await.unwrap();
        }
        transport.publish_head(&signed_head(&SigningKey::generate(&mut OsRng), [9; 16], 1)).await.unwrap();
        {
            let mut state = server.state();
            state.objects.insert("mine/other-app/file".to_string(), vec![1]);
            state.objects.insert("mine-too/threestrands-sync/objects/x".to_string(), vec![1]);
            state.objects.insert("threestrands-sync/heads/root.head".to_string(), vec![1]);
        }
        let size = transport.corpus_size_bytes().await.unwrap();
        assert!(size > 0);

        transport.delete_all_corpus_data().await.unwrap();

        let remaining: Vec<String> = server.state().objects.keys().cloned().collect();
        assert_eq!(
            remaining,
            vec![
                "mine-too/threestrands-sync/objects/x".to_string(),
                "mine/other-app/file".to_string(),
                "threestrands-sync/heads/root.head".to_string(),
            ]
        );
        assert_eq!(transport.corpus_size_bytes().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn probe_reports_full_access_and_leaves_nothing_behind() {
        let server = FakeS3Server::spawn().await;
        server.state().faults.versioning_status = Some("Suspended");
        let report = open(&server, "s3", "").probe().await;
        assert_eq!(
            report,
            S3ProbeReport {
                reachable: true,
                can_list: true,
                can_write: true,
                can_read: true,
                can_delete: true,
                versioning_enabled: Some(false),
                error: None,
            }
        );
        assert!(server.state().objects.is_empty());
    }

    #[tokio::test]
    async fn probe_reports_versioning_when_enabled_and_unknown_when_denied() {
        let server = FakeS3Server::spawn().await;
        server.state().faults.versioning_status = Some("Enabled");
        assert_eq!(open(&server, "s3", "").probe().await.versioning_enabled, Some(true));
        server.state().faults.versioning_status = None;
        assert_eq!(open(&server, "s3", "").probe().await.versioning_enabled, None);
    }

    #[tokio::test]
    async fn probe_explains_a_missing_permission() {
        let server = FakeS3Server::spawn().await;
        server.state().faults.deny_delete = true;
        let report = open(&server, "s3", "").probe().await;
        assert!(report.can_list && report.can_write && report.can_read);
        assert!(!report.can_delete);
        assert!(report.error.as_deref().unwrap().contains("AccessDenied"));

        server.state().faults.deny_delete = false;
        server.state().faults.deny_put = true;
        let report = open(&server, "s3", "").probe().await;
        assert!(report.can_list);
        assert!(!report.can_write && !report.can_read);

        let mut credentials = FakeS3Server::credentials();
        credentials.secret_access_key = "wrong".to_string();
        let report = S3Transport::new("s3", &server.config(""), &credentials).unwrap().probe().await;
        assert!(report.reachable);
        assert!(!report.can_list);
    }

    #[tokio::test]
    async fn health_reflects_whether_the_key_can_list() {
        let server = FakeS3Server::spawn().await;
        assert_eq!(open(&server, "s3", "").health().await.unwrap(), TransportHealth::Healthy);
        let mut credentials = FakeS3Server::credentials();
        credentials.secret_access_key = "wrong".to_string();
        let transport = S3Transport::new("s3", &server.config(""), &credentials).unwrap();
        assert!(matches!(transport.health().await.unwrap(), TransportHealth::Unavailable(_)));
    }

    #[tokio::test]
    async fn one_failing_connector_never_blocks_another() {
        let healthy = FakeS3Server::spawn().await;
        let failing = FakeS3Server::spawn().await;
        failing.state().faults.fail_next = Some((usize::MAX, StatusCode::INTERNAL_SERVER_ERROR, "InternalError"));
        let cid = Cid::for_bytes(b"independent");
        assert!(open(&failing, "bad", "").put_object(&cid, b"independent").await.is_err());
        open(&healthy, "good", "").put_object(&cid, b"independent").await.unwrap();
    }

    /// A corpus written by the folder adapter, copied byte for byte into a
    /// bucket, reads back through the S3 adapter — the layouts match.
    #[tokio::test]
    async fn reads_a_corpus_copied_from_a_sync_folder() {
        let folder_path = std::env::temp_dir().join(format!("threestrands-s3-portability-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder_path).unwrap();
        let folder = crate::sync_folder::SyncFolderTransport::open("folder", &folder_path).await.unwrap();
        let mut cids = Vec::new();
        for index in 0..5 {
            let bytes = format!("portable {index}").into_bytes();
            let cid = Cid::for_bytes(&bytes);
            folder.put_object(&cid, &bytes).await.unwrap();
            cids.push(cid);
        }
        let signing_key = SigningKey::generate(&mut OsRng);
        folder.publish_head(&signed_head(&signing_key, [7; 16], 3)).await.unwrap();

        let server = FakeS3Server::spawn().await;
        fn copy_tree(dir: &std::path::Path, base: &std::path::Path, objects: &mut std::collections::BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    copy_tree(&path, base, objects);
                } else {
                    let key = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                    objects.insert(key, std::fs::read(&path).unwrap());
                }
            }
        }
        copy_tree(&folder_path, &folder_path, &mut server.state().objects);
        std::fs::remove_dir_all(&folder_path).unwrap();

        let transport = open(&server, "s3", "");
        for cid in &cids {
            transport.get_object(cid).await.unwrap();
        }
        let mut scanned: Vec<Cid> = transport.scan(None).await.unwrap().unwrap().objects.into_iter().map(|object| object.cid).collect();
        scanned.sort();
        cids.sort();
        assert_eq!(scanned, cids);
        let heads = transport
            .resolve_heads(&[HeadLocator { device_id: DeviceId::from_bytes([7; 16]), remote_id: None }])
            .await
            .unwrap();
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].head.state_sequence, 3);
    }
}
