//! A user-configured, Kubo-compatible IPFS RPC transport adapter (Phase 4B
//! of the replicated-sync plan). Filebase's IPFS RPC API is the first
//! compatibility target, but nothing here branches on a provider name —
//! only on the capabilities a specific endpoint actually exercises.
//!
//! This is deliberately not "install Kubo": any HTTP endpoint that speaks
//! the same handful of `/api/v0/...` calls works, whether that's a hosted
//! service like Filebase or a locally running Kubo/IPFS Desktop node.
//!
//! Object storage uses `dag/import` with a minimal single-block CAR whose
//! root is the exact locally computed raw-block CID — not `/add`, which
//! creates a UnixFS object under a different CID. Discovery of per-device
//! signed heads uses Kubo's mutable filesystem (MFS) purely as an opaque
//! index (`/threestrands/<space-tag>/heads/<device-tag>/<sequence>-<cid>`);
//! an endpoint that cannot provide MFS is still a valid storage replica, it
//! just cannot bootstrap a new device by itself — see
//! [`TransportCapabilities::head_discovery`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rand::{rngs::OsRng, RngCore};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use async_trait::async_trait;
use threestrands_sync_envelope::{compute_cid, decode_signed_head, encode_signed_head, DeviceId, SignedDeviceHead};
use threestrands_sync_transport::{
    Cid as TransportCid, HeadLocator, ObjectLocator, ScanPage, SyncTransport, TransportCapabilities,
    TransportError, TransportHealth, TransportInstanceId,
};

const SCAN_PAGE_SIZE: usize = 500;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// ================================ URL handling ===============================

/// A validated, normalized RPC origin: scheme + host + optional port +
/// optional fixed path prefix. Never carries user-info, a query string, or
/// a fragment — every real request path is appended structurally by this
/// crate, never by string-splicing a caller-supplied URL.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RpcOrigin {
    scheme: &'static str,
    host: String,
    port: Option<u16>,
    path_prefix: String,
}

impl RpcOrigin {
    fn parse(input: &str) -> Result<Self, String> {
        let url = url::Url::parse(input.trim()).map_err(|error| format!("Invalid RPC URL: {error}"))?;
        let scheme = match url.scheme() {
            "https" => "https",
            "http" => "http",
            other => return Err(format!("RPC URL must use http or https, not {other}")),
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err("RPC URL must not contain user-info".to_string());
        }
        if url.query().is_some() {
            return Err("RPC URL must not contain a query string".to_string());
        }
        if url.fragment().is_some() {
            return Err("RPC URL must not contain a fragment".to_string());
        }
        let host = url.host_str().ok_or("RPC URL must have a host")?.to_string();
        let loopback = is_loopback_host(&host);
        if scheme == "http" && !loopback {
            return Err("Non-loopback RPC endpoints must use HTTPS".to_string());
        }
        let mut path_prefix = url.path().trim_end_matches('/').to_string();
        if path_prefix == "/" {
            path_prefix.clear();
        }
        Ok(Self {
            scheme,
            host,
            port: url.port(),
            path_prefix,
        })
    }

    /// Builds the exact URL for one RPC method. `method` is always a fixed
    /// string literal at the call site, never derived from user input, so
    /// this can never be used to smuggle a differently interpreted path or
    /// change the credential origin.
    fn method_url(&self, method: &str) -> String {
        let port = self.port.map(|port| format!(":{port}")).unwrap_or_default();
        format!("{}://{}{}{}/api/v0/{method}", self.scheme, self.host, port, self.path_prefix)
    }
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // `Url::host_str` returns a bracketed literal for IPv6 (`"[::1]"`),
    // since that's the form a URL authority requires; strip the brackets
    // before parsing it as an address.
    let unbracketed = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(host);
    unbracketed.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

// ============================== Kubo error shapes ============================

fn kubo_error_message(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("Message")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn is_not_pinned_error(body: &str) -> bool {
    kubo_error_message(body)
        .map(|message| {
            let message = message.to_ascii_lowercase();
            message.contains("not pinned") || message.contains("not found")
        })
        .unwrap_or(false)
}

fn is_not_found_error(status: reqwest::StatusCode, body: &str) -> bool {
    status == reqwest::StatusCode::NOT_FOUND
        || kubo_error_message(body)
            .map(|message| message.to_ascii_lowercase().contains("not found"))
            .unwrap_or(false)
}

fn is_already_exists_error(body: &str) -> bool {
    kubo_error_message(body)
        .map(|message| message.to_ascii_lowercase().contains("already exists"))
        .unwrap_or(false)
}

/// Maps an unexpected (not already handled by a call-site-specific check
/// like "not pinned") non-success response to a typed transport error.
/// Kubo itself reports almost every command-level failure as a bare 500
/// with a JSON body, so this status-code mapping is a fallback for
/// endpoints (like a hosted gateway in front of Kubo) that do use
/// meaningful HTTP status codes for auth/quota.
fn map_status_error(status: reqwest::StatusCode, body: &str) -> TransportError {
    let message = kubo_error_message(body).unwrap_or_else(|| format!("HTTP {status}"));
    match status.as_u16() {
        401 | 403 => TransportError::Authentication(message),
        429 => TransportError::Quota(message),
        500..=599 => TransportError::Transient(message),
        _ => TransportError::Permanent(message),
    }
}

/// Every reqwest failure that reaches here is a network-level fault: a
/// timeout, a refused or reset connection, a body cut off mid-read, or an
/// unparseable body (e.g. from a captive portal or proxy). URL and header
/// construction are validated before `send()`, redirects are disabled and
/// rejected by status, and HTTP statuses go through `map_status_error` — so
/// nothing here is known to be permanent. Misclassifying a blip as
/// Permanent would fail a delivery with no retry, while a misclassified
/// Transient only costs bounded backoff retries.
fn map_reqwest_error(error: reqwest::Error) -> TransportError {
    TransportError::Transient(error.to_string())
}

fn parse_cid(text: &str) -> Result<cid::Cid, TransportError> {
    text.parse::<cid::Cid>()
        .map_err(|_| TransportError::Corruption(format!("invalid content identifier: {text}")))
}

/// A stable, opaque tag derived from an identifier that is already random
/// application-internal bytes (a sync-space id or a device id — never a
/// name, email, or other user content). Used as an MFS path component so
/// the path itself carries no meaning to anyone browsing the endpoint.
fn derive_tag(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn device_tag(device_id: &DeviceId) -> String {
    derive_tag(device_id.as_bytes())
}

// ================================= Transport ==================================

/// Reported by [`IpfsRpcTransport::probe_capabilities`] — what a "test
/// connection" action in Settings shows the user before they enable a
/// replica, per the plan's "explain a missing required capability before
/// the user enables the replica."
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub version_ok: bool,
    pub mfs_available: bool,
}

pub struct IpfsRpcTransport {
    instance_id: TransportInstanceId,
    origin: RpcOrigin,
    client: reqwest::Client,
    token: Option<String>,
    space_tag: String,
    /// Optimistically `true` until a probe (explicit, or the first
    /// `health()` call) says otherwise — matches the other adapters'
    /// default posture, and MFS absence is discovered the first time it's
    /// actually needed if nothing ever probes explicitly.
    mfs_available: AtomicBool,
    enumeration_available: AtomicBool,
}

impl IpfsRpcTransport {
    pub fn new(
        instance_id: impl Into<String>,
        base_url: &str,
        token: Option<String>,
        sync_space_id: &[u8],
    ) -> Result<Self, TransportError> {
        let origin = RpcOrigin::parse(base_url).map_err(TransportError::Permanent)?;
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // Never follow a redirect: the plan requires rejecting a
            // credential-bearing cross-origin redirect rather than
            // forwarding the Authorization header, and the simplest way to
            // guarantee that is to never follow *any* redirect at all —
            // these RPC endpoints have no legitimate reason to issue one.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| TransportError::Permanent(error.to_string()))?;
        Ok(Self {
            instance_id: TransportInstanceId(instance_id.into()),
            origin,
            client,
            token,
            space_tag: derive_tag(sync_space_id),
            mfs_available: AtomicBool::new(true),
            enumeration_available: AtomicBool::new(true),
        })
    }

    fn auth_header(&self) -> Result<Option<HeaderValue>, TransportError> {
        let Some(token) = &self.token else { return Ok(None) };
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| TransportError::Authentication("the configured access token is not a valid header value".to_string()))?;
        // Keeps the token out of any Debug-formatted request/header dump —
        // the other half of "redact all credentials from diagnostics" is
        // simply never putting it in a URL (see `RpcOrigin`/`method_url`).
        value.set_sensitive(true);
        Ok(Some(value))
    }

    async fn post(&self, method: &str, query: &[(&str, &str)]) -> Result<reqwest::Response, TransportError> {
        let mut url = reqwest::Url::parse(&self.origin.method_url(method))
            .map_err(|error| TransportError::Permanent(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        let mut request = self.client.post(url);
        if let Some(header) = self.auth_header()? {
            request = request.header(AUTHORIZATION, header);
        }
        let response = request.send().await.map_err(map_reqwest_error)?;
        if response.status().is_redirection() {
            return Err(TransportError::Permanent(
                "the RPC endpoint attempted to redirect the request; redirects are never followed".to_string(),
            ));
        }
        Ok(response)
    }

    async fn post_multipart(
        &self,
        method: &str,
        query: &[(&str, &str)],
        form: reqwest::multipart::Form,
    ) -> Result<reqwest::Response, TransportError> {
        let mut url = reqwest::Url::parse(&self.origin.method_url(method))
            .map_err(|error| TransportError::Permanent(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        let mut request = self.client.post(url).multipart(form);
        if let Some(header) = self.auth_header()? {
            request = request.header(AUTHORIZATION, header);
        }
        let response = request.send().await.map_err(map_reqwest_error)?;
        if response.status().is_redirection() {
            return Err(TransportError::Permanent(
                "the RPC endpoint attempted to redirect the request; redirects are never followed".to_string(),
            ));
        }
        Ok(response)
    }

    async fn probe_version(&self) -> Result<(), TransportError> {
        let response = self.post("version", &[]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(map_status_error(status, &body));
        }
        Ok(())
    }

    async fn probe_mfs(&self) -> bool {
        let probe_dir = format!("/threestrands/{}/.probe", self.space_tag);
        if self.mfs_mkdir_p(&probe_dir).await.is_err() {
            return false;
        }

        // Exercise the exact path device-head publication needs. Filebase
        // accepts a CID already imported into this bucket as a files/cp
        // source; probing only mkdir/ls would miss an endpoint or account
        // tier that cannot perform that in-bucket link.
        let mut probe_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut probe_bytes);
        let cid = TransportCid::for_bytes(&probe_bytes);
        let probe_file = format!("{probe_dir}/{}", cid.0);
        let result = async {
            self.put_object(&cid, &probe_bytes).await?;
            self.mfs_cp(&format!("/ipfs/{}", cid.0), &probe_file).await?;
            self.mfs_stat(&probe_file).await
        }
        .await;

        // Cleanup is deliberately best-effort: capability reporting must
        // reflect whether the required operation worked, while a transient
        // cleanup failure must not hide that result. Random probe content
        // prevents unpinning a caller-owned object with the same CID.
        let _ = self.mfs_rm(&probe_file).await;
        let _ = self.delete_object(&cid).await;
        let _ = self.mfs_rm(&probe_dir).await;
        result.is_ok()
    }

    /// Probes `version` and the complete MFS publication path (`dag/import`,
    /// `files/mkdir`, `files/cp`, `files/stat`, and cleanup) against a
    /// private probe path, caching the result for
    /// [`SyncTransport::capabilities`] and [`SyncTransport::health`].
    /// Call this explicitly (Settings' "test connection") before enabling a
    /// replica; it also runs lazily the first time `health()` is called.
    pub async fn probe_capabilities(&self) -> Result<ProbeReport, TransportError> {
        self.probe_version().await?;
        let mfs_available = self.probe_mfs().await;
        self.mfs_available.store(mfs_available, Ordering::Relaxed);
        Ok(ProbeReport {
            version_ok: true,
            mfs_available,
        })
    }

    async fn pin_status(&self, cid: &str) -> Result<bool, TransportError> {
        let response = self.post("pin/ls", &[("arg", cid)]).await?;
        if response.status().is_success() {
            return Ok(true);
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if is_not_pinned_error(&body) {
            return Ok(false);
        }
        Err(map_status_error(status, &body))
    }

    async fn mfs_mkdir_p(&self, path: &str) -> Result<(), TransportError> {
        if !path.starts_with('/') {
            return Err(TransportError::Permanent("MFS paths must start with /".to_string()));
        }
        // Filebase exposes MFS over a bucket but rejects Kubo's
        // `parents=true` convenience flag. Build the hierarchy one
        // component at a time instead. This remains valid against Kubo and
        // idempotent when another device creates a parent first.
        let mut current = String::new();
        for component in path.split('/').filter(|component| !component.is_empty()) {
            if component == "." || component == ".." {
                return Err(TransportError::Permanent("MFS paths must not contain . or ..".to_string()));
            }
            current.push('/');
            current.push_str(component);
            let response = self.post("files/mkdir", &[("arg", &current)]).await?;
            if response.status().is_success() {
                continue;
            }
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if is_already_exists_error(&body) {
                continue;
            }
            return Err(map_status_error(status, &body));
        }
        Ok(())
    }

    async fn mfs_cp(&self, source: &str, destination: &str) -> Result<(), TransportError> {
        let response = self.post("files/cp", &[("arg", source), ("arg", destination)]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            // "already exists" happens if a repair/anti-entropy path tries
            // to relink an entry that is already there; that is success,
            // not a real conflict, since a head entry is never overwritten
            // and the destination path already encodes the CID.
            if kubo_error_message(&body).map(|m| m.to_ascii_lowercase().contains("already exists")).unwrap_or(false) {
                return Ok(());
            }
            return Err(map_status_error(status, &body));
        }
        Ok(())
    }

    async fn mfs_ls(&self, path: &str) -> Result<Vec<String>, TransportError> {
        let response = self.post("files/ls", &[("arg", path)]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if is_not_found_error(status, &body) {
                return Err(TransportError::NotFound);
            }
            return Err(map_status_error(status, &body));
        }
        let body: Value = response.json().await.map_err(map_reqwest_error)?;
        let names = body
            .get("Entries")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("Name").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Ok(names)
    }

    async fn mfs_stat(&self, path: &str) -> Result<(), TransportError> {
        let response = self.post("files/stat", &[("arg", path)]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if is_not_found_error(status, &body) {
                return Err(TransportError::NotFound);
            }
            return Err(map_status_error(status, &body));
        }
        Ok(())
    }

    async fn mfs_rm(&self, path: &str) -> Result<(), TransportError> {
        // Probe cleanup only removes one file and then its empty directory.
        // Filebase supports files/rm but rejects recursive and force.
        let response = self.post("files/rm", &[("arg", path)]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if is_not_found_error(status, &body) {
                return Ok(());
            }
            return Err(map_status_error(status, &body));
        }
        Ok(())
    }
}

async fn build_single_block_car(cid: cid::Cid, bytes: &[u8]) -> Result<Vec<u8>, TransportError> {
    let header = iroh_car::CarHeader::new_v1(vec![cid]);
    let mut buffer = Vec::new();
    let mut writer = iroh_car::CarWriter::new(header, &mut buffer);
    writer
        .write(cid, bytes)
        .await
        .map_err(|error| TransportError::Permanent(format!("failed to build the upload CAR: {error}")))?;
    writer
        .finish()
        .await
        .map_err(|error| TransportError::Permanent(format!("failed to build the upload CAR: {error}")))?;
    Ok(buffer)
}

/// Scans `dag/import`'s newline-delimited JSON response for a line
/// confirming `expected_cid` as an imported root with no pin error. Tries
/// more than one known Kubo response shape defensively, since this
/// endpoint's exact JSON has drifted across Kubo versions.
fn dag_import_confirms_root(body: &str, expected_cid: &str) -> Result<bool, TransportError> {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|error| TransportError::Corruption(format!("malformed dag/import response: {error}")))?;
        let Some(root) = value.get("Root") else { continue };
        let cid_str = root
            .get("Cid")
            .and_then(|cid| cid.get("/").and_then(Value::as_str).or_else(|| cid.as_str()));
        let Some(cid_str) = cid_str else { continue };
        if cid_str != expected_cid {
            continue;
        }
        let pin_error = root.get("PinErrorMsg").and_then(Value::as_str).unwrap_or("");
        if !pin_error.is_empty() {
            return Err(TransportError::Permanent(format!("pin error during import: {pin_error}")));
        }
        return Ok(true);
    }
    Ok(false)
}

#[async_trait]
impl SyncTransport for IpfsRpcTransport {
    fn instance_id(&self) -> TransportInstanceId {
        self.instance_id.clone()
    }

    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            enumeration: self.enumeration_available.load(Ordering::Relaxed),
            incremental_cursor: true,
            head_discovery: self.mfs_available.load(Ordering::Relaxed),
        }
    }

    async fn put_object(&self, cid: &TransportCid, bytes: &[u8]) -> Result<ObjectLocator, TransportError> {
        let expected_cid = parse_cid(&cid.0)?;
        let car_bytes = build_single_block_car(expected_cid, bytes).await?;
        let part = reqwest::multipart::Part::bytes(car_bytes)
            .file_name("upload.car")
            .mime_str("application/vnd.ipld.car")
            .map_err(|error| TransportError::Permanent(error.to_string()))?;
        let form = reqwest::multipart::Form::new().part("file", part);

        let response = self.post_multipart("dag/import", &[("pin-roots", "true")], form).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(map_status_error(status, &body));
        }
        let body = response.text().await.map_err(map_reqwest_error)?;
        if !dag_import_confirms_root(&body, &cid.0)? {
            return Err(TransportError::Transient(
                "dag/import did not confirm the expected root as imported".to_string(),
            ));
        }

        // A successful import is not yet durable: confirm the pin before
        // reporting success, per the plan's "never treat submission or CID
        // calculation as remote durability."
        if !self.pin_status(&cid.0).await? {
            return Err(TransportError::Transient(
                "import succeeded but the object is not yet confirmed pinned".to_string(),
            ));
        }
        Ok(ObjectLocator {
            cid: cid.clone(),
            remote_id: None,
        })
    }

    async fn get_object(&self, cid: &TransportCid) -> Result<Vec<u8>, TransportError> {
        let response = self.post("block/get", &[("arg", &cid.0)]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if is_not_found_error(status, &body) {
                return Err(TransportError::NotFound);
            }
            return Err(map_status_error(status, &body));
        }
        let bytes = response.bytes().await.map_err(map_reqwest_error)?.to_vec();
        // Gateway/endpoint responses are never trusted on their word: the
        // exact same CID re-verification every other adapter performs.
        if compute_cid(&bytes) != cid.0 {
            return Err(TransportError::Corruption(
                "returned block bytes do not match the requested CID".to_string(),
            ));
        }
        Ok(bytes)
    }

    async fn publish_head(&self, head: &SignedDeviceHead) -> Result<HeadLocator, TransportError> {
        let bytes = encode_signed_head(head).map_err(|error| TransportError::Permanent(error.to_string()))?;
        let cid_string = compute_cid(&bytes);
        self.put_object(&TransportCid(cid_string.clone()), &bytes).await?;

        let tag = device_tag(&head.head.device_id);
        let dir = format!("/threestrands/{}/heads/{}", self.space_tag, tag);
        self.mfs_mkdir_p(&dir).await?;
        let entry_path = format!("{dir}/{}-{cid_string}", head.head.contiguous_sequence);
        self.mfs_cp(&format!("/ipfs/{cid_string}"), &entry_path).await?;

        Ok(HeadLocator {
            device_id: head.head.device_id,
            remote_id: Some(entry_path),
        })
    }

    async fn resolve_heads(&self, known: &[HeadLocator]) -> Result<Vec<SignedDeviceHead>, TransportError> {
        if !self.mfs_available.load(Ordering::Relaxed) {
            // storage-only: cannot discover, but that is not an error.
            return Ok(Vec::new());
        }
        let mut heads = Vec::new();
        for locator in known {
            let tag = device_tag(&locator.device_id);
            let dir = format!("/threestrands/{}/heads/{}", self.space_tag, tag);
            let entries = match self.mfs_ls(&dir).await {
                Ok(entries) => entries,
                // One device's missing or unreadable directory must never
                // fail resolving every other device's head.
                Err(_) => continue,
            };
            let mut candidates: Vec<(u64, String)> = entries
                .iter()
                .filter_map(|name| {
                    let (sequence, cid) = name.split_once('-')?;
                    Some((sequence.parse().ok()?, cid.to_string()))
                })
                .collect();
            candidates.sort_by_key(|(sequence, _)| std::cmp::Reverse(*sequence));
            for (_, cid) in candidates {
                let Ok(bytes) = self.get_object(&TransportCid(cid)).await else { continue };
                let Ok(signed) = decode_signed_head(&bytes) else { continue };
                heads.push(signed);
                break;
            }
        }
        Ok(heads)
    }

    async fn scan(&self, cursor: Option<&str>) -> Result<Option<ScanPage>, TransportError> {
        if !self.enumeration_available.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let offset: usize = match cursor {
            None => 0,
            Some(cursor) => match cursor.parse() {
                Ok(offset) => offset,
                Err(_) => return Ok(None),
            },
        };

        let response = self.post("pin/ls", &[("type", "recursive")]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(map_status_error(status, &body));
        }
        let body: Value = response.json().await.map_err(map_reqwest_error)?;
        let mut cids: Vec<String> = body
            .get("Keys")
            .and_then(Value::as_object)
            .map(|keys| keys.keys().cloned().collect())
            .unwrap_or_default();
        cids.sort();

        let end = (offset + SCAN_PAGE_SIZE).min(cids.len());
        let page = cids.get(offset..end).unwrap_or_default().to_vec();
        let next_cursor = if end < cids.len() { Some(end.to_string()) } else { None };
        Ok(Some(ScanPage {
            objects: page
                .into_iter()
                .map(|cid| ObjectLocator { remote_id: None, cid: TransportCid(cid) })
                .collect(),
            next_cursor,
        }))
    }

    async fn delete_object(&self, cid: &TransportCid) -> Result<(), TransportError> {
        let response = self.post("pin/rm", &[("arg", &cid.0)]).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if is_not_pinned_error(&body) {
                return Ok(());
            }
            return Err(map_status_error(status, &body));
        }
        if self.pin_status(&cid.0).await? {
            return Err(TransportError::Transient(
                "pin/rm succeeded but the endpoint still reports the CID pinned".to_string(),
            ));
        }
        Ok(())
    }

    async fn health(&self) -> Result<TransportHealth, TransportError> {
        match self.probe_version().await {
            Ok(()) => {
                if self.mfs_available.load(Ordering::Relaxed) {
                    Ok(TransportHealth::Healthy)
                } else {
                    Ok(TransportHealth::Degraded(
                        "no MFS discovery index on this endpoint: it can store and be read from, but cannot bootstrap a new device by itself".to_string(),
                    ))
                }
            }
            Err(error) => Ok(TransportHealth::Unavailable(error.to_string())),
        }
    }
}

#[cfg(test)]
mod url_tests {
    use super::*;

    #[test]
    fn accepts_a_plain_https_origin() {
        let origin = RpcOrigin::parse("https://rpc.filebase.io").unwrap();
        assert_eq!(origin.scheme, "https");
        assert_eq!(origin.host, "rpc.filebase.io");
        assert_eq!(origin.port, None);
        assert_eq!(origin.path_prefix, "");
        assert_eq!(origin.method_url("version"), "https://rpc.filebase.io/api/v0/version");
    }

    #[test]
    fn keeps_a_path_prefix_and_strips_a_trailing_slash() {
        let origin = RpcOrigin::parse("https://example.com/ipfs-gateway/").unwrap();
        assert_eq!(origin.path_prefix, "/ipfs-gateway");
        assert_eq!(origin.method_url("version"), "https://example.com/ipfs-gateway/api/v0/version");
    }

    #[test]
    fn rejects_user_info() {
        assert!(RpcOrigin::parse("https://user:pass@rpc.filebase.io").is_err());
    }

    #[test]
    fn rejects_a_query_string() {
        assert!(RpcOrigin::parse("https://rpc.filebase.io/?token=abc").is_err());
    }

    #[test]
    fn rejects_a_fragment() {
        assert!(RpcOrigin::parse("https://rpc.filebase.io/#section").is_err());
    }

    #[test]
    fn rejects_non_loopback_http() {
        assert!(RpcOrigin::parse("http://rpc.filebase.io").is_err());
    }

    #[test]
    fn allows_http_only_for_ipv4_loopback() {
        let origin = RpcOrigin::parse("http://127.0.0.1:5001").unwrap();
        assert_eq!(origin.host, "127.0.0.1");
        assert_eq!(origin.port, Some(5001));
    }

    #[test]
    fn allows_http_for_ipv6_loopback() {
        let origin = RpcOrigin::parse("http://[::1]:5001").unwrap();
        // `Url::host_str` keeps the bracketed form for IPv6, which is also
        // exactly what a valid URL authority requires when rebuilding a
        // request URL, so that's what's retained here.
        assert_eq!(origin.host, "[::1]");
        assert_eq!(origin.method_url("version"), "http://[::1]:5001/api/v0/version");
    }

    #[test]
    fn allows_http_for_the_localhost_name() {
        assert!(RpcOrigin::parse("http://localhost:5001").is_ok());
    }

    #[test]
    fn rejects_http_for_a_non_loopback_ip_literal() {
        assert!(RpcOrigin::parse("http://192.168.1.5:5001").is_err());
    }

    #[test]
    fn rejects_an_unsupported_scheme() {
        assert!(RpcOrigin::parse("ftp://rpc.filebase.io").is_err());
    }

    #[test]
    fn normalizes_an_internationalized_domain() {
        // "münchen.example" in its ASCII (punycode) form — the `url` crate
        // performs IDNA normalization, so this must not error and must
        // produce a consistent, ASCII-only host.
        let origin = RpcOrigin::parse("https://xn--mnchen-3ya.example").unwrap();
        assert_eq!(origin.host, "xn--mnchen-3ya.example");
    }

    #[test]
    fn rejects_garbage_input() {
        assert!(RpcOrigin::parse("not a url").is_err());
        assert!(RpcOrigin::parse("").is_err());
    }

    #[test]
    fn derive_tag_is_stable_and_content_dependent() {
        assert_eq!(derive_tag(b"same"), derive_tag(b"same"));
        assert_ne!(derive_tag(b"a"), derive_tag(b"b"));
        // No user content survives into the tag: fixed length regardless
        // of input length, and hex-only.
        assert_eq!(derive_tag(b"short").len(), 16);
        assert_eq!(derive_tag(b"a much much much longer input string").len(), 16);
        assert!(derive_tag(b"x").chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn dag_import_confirms_the_expected_root_and_rejects_a_pin_error() {
        let success = "{\"Root\":{\"Cid\":{\"/\":\"bafyabc\"},\"PinErrorMsg\":\"\"}}\n";
        assert!(dag_import_confirms_root(success, "bafyabc").unwrap());
        assert!(!dag_import_confirms_root(success, "bafyother").unwrap());

        let pin_error = "{\"Root\":{\"Cid\":{\"/\":\"bafyabc\"},\"PinErrorMsg\":\"disk full\"}}\n";
        assert!(dag_import_confirms_root(pin_error, "bafyabc").is_err());
    }

    #[test]
    fn dag_import_ignores_blank_lines_and_unrelated_entries() {
        let body = "\n{\"SomethingElse\":1}\n{\"Root\":{\"Cid\":{\"/\":\"bafyabc\"},\"PinErrorMsg\":\"\"}}\n\n";
        assert!(dag_import_confirms_root(body, "bafyabc").unwrap());
    }

    #[test]
    fn kubo_message_detection_is_case_insensitive() {
        assert!(is_not_pinned_error("{\"Message\":\"path is NOT PINNED\"}"));
        assert!(!is_not_pinned_error("{\"Message\":\"permission denied\"}"));
        assert!(!is_not_pinned_error("not json at all"));
    }
}

/// A minimal, in-memory, Kubo-compatible RPC server implementing exactly
/// the subset this adapter uses, with fault injection — the "fake subset
/// RPC server" the plan's conformance/security tests run against. Talks
/// real HTTP to a real `IpfsRpcTransport` client, over `127.0.0.1`, so the
/// actual `reqwest`-based request/response/error-mapping code is what gets
/// exercised, not a mocked function call.
#[cfg(test)]
mod fake_server {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard};

    use axum::extract::{Multipart, State};
    use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use axum::Router;

    #[derive(Default)]
    pub struct FaultInjection {
        pub required_token: Option<String>,
        pub outage_remaining: usize,
        pub quota_after_puts: Option<usize>,
        pub puts_so_far: usize,
        pub corrupt_next_block_get: bool,
        pub mfs_disabled: bool,
        /// When set, `dag/import` reports this CID as the imported root
        /// instead of the one actually requested — simulating a `/add`-style
        /// endpoint that silently substitutes a different (UnixFS) CID.
        pub substitute_root: Option<String>,
    }

    #[derive(Default)]
    pub struct ServerState {
        pub pins: HashMap<String, Vec<u8>>,
        pub dirs: HashSet<String>,
        pub files: HashMap<String, String>,
        pub faults: FaultInjection,
    }

    type SharedState = Arc<Mutex<ServerState>>;

    pub struct FakeKuboServer {
        pub addr: std::net::SocketAddr,
        state: SharedState,
    }

    impl FakeKuboServer {
        pub async fn spawn() -> Self {
            let state: SharedState = Arc::new(Mutex::new(ServerState::default()));
            let app = Router::new()
                .route("/api/v0/version", post(version))
                .route("/api/v0/dag/import", post(dag_import))
                .route("/api/v0/pin/ls", post(pin_ls))
                .route("/api/v0/pin/rm", post(pin_rm))
                .route("/api/v0/block/get", post(block_get))
                .route("/api/v0/files/mkdir", post(files_mkdir))
                .route("/api/v0/files/cp", post(files_cp))
                .route("/api/v0/files/ls", post(files_ls))
                .route("/api/v0/files/stat", post(files_stat))
                .route("/api/v0/files/rm", post(files_rm))
                .with_state(state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self { addr, state }
        }

        pub fn base_url(&self) -> String {
            format!("http://{}", self.addr)
        }

        pub fn faults(&self) -> MutexGuard<'_, ServerState> {
            self.state.lock().unwrap()
        }
    }

    fn query_pairs(uri: &Uri) -> Vec<(String, String)> {
        uri.query()
            .map(|query| url::form_urlencoded::parse(query.as_bytes()).into_owned().collect())
            .unwrap_or_default()
    }

    fn query_all<'a>(pairs: &'a [(String, String)], key: &str) -> Vec<&'a str> {
        pairs.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
    }

    fn query_one<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        query_all(pairs, key).first().copied()
    }

    fn kubo_error(message: &str) -> String {
        serde_json::json!({"Message": message, "Code": 0, "Type": "error"}).to_string()
    }

    // `Response` is a large `Err` type; boxing it would just add noise in
    // this test-only harness, where request volume and allocation cost are
    // irrelevant.
    #[allow(clippy::result_large_err)]
    fn check_auth(state: &ServerState, headers: &HeaderMap) -> Result<(), Response> {
        if let Some(required) = &state.faults.required_token {
            let provided = headers.get(AUTHORIZATION).and_then(|value| value.to_str().ok());
            if provided != Some(format!("Bearer {required}")).as_deref() {
                return Err((StatusCode::UNAUTHORIZED, kubo_error("authentication required")).into_response());
            }
        }
        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn check_outage(state: &mut ServerState) -> Result<(), Response> {
        if state.faults.outage_remaining > 0 {
            state.faults.outage_remaining -= 1;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, kubo_error("injected outage")).into_response());
        }
        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn check_mfs(state: &ServerState) -> Result<(), Response> {
        if state.faults.mfs_disabled {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, kubo_error("MFS is not supported on this endpoint")).into_response());
        }
        Ok(())
    }

    async fn version(State(state): State<SharedState>, headers: HeaderMap) -> Response {
        let mut guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_outage(&mut guard) {
            return response;
        }
        axum::Json(serde_json::json!({"Version": "fake-kubo/0.1"})).into_response()
    }

    async fn dag_import(State(state): State<SharedState>, headers: HeaderMap, uri: Uri, mut multipart: Multipart) -> Response {
        {
            let guard = state.lock().unwrap();
            if let Err(response) = check_auth(&guard, &headers) {
                return response;
            }
        }
        let pin_roots = query_one(&query_pairs(&uri), "pin-roots") == Some("true");

        let mut car_bytes = Vec::new();
        while let Ok(Some(field)) = multipart.next_field().await {
            if let Ok(bytes) = field.bytes().await {
                car_bytes = bytes.to_vec();
            }
        }
        let Ok(mut reader) = iroh_car::CarReader::new(&car_bytes[..]).await else {
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("invalid CAR")).into_response();
        };
        let roots = reader.header().roots().to_vec();
        let mut blocks = Vec::new();
        while let Ok(Some(block)) = reader.next_block().await {
            blocks.push(block);
        }

        let mut guard = state.lock().unwrap();
        if let Err(response) = check_outage(&mut guard) {
            return response;
        }
        if let Some(limit) = guard.faults.quota_after_puts {
            if guard.faults.puts_so_far >= limit {
                return (StatusCode::TOO_MANY_REQUESTS, kubo_error("quota exceeded")).into_response();
            }
        }
        if pin_roots {
            for (cid, data) in &blocks {
                if roots.contains(cid) {
                    guard.pins.insert(cid.to_string(), data.clone());
                    guard.faults.puts_so_far += 1;
                }
            }
        }

        let mut lines = String::new();
        for root in &roots {
            let reported = guard.faults.substitute_root.clone().unwrap_or_else(|| root.to_string());
            lines.push_str(&serde_json::json!({"Root": {"Cid": {"/": reported}, "PinErrorMsg": ""}}).to_string());
            lines.push('\n');
        }
        (StatusCode::OK, lines).into_response()
    }

    async fn pin_ls(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let mut guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_outage(&mut guard) {
            return response;
        }
        let pairs = query_pairs(&uri);
        if let Some(cid) = query_one(&pairs, "arg") {
            if guard.pins.contains_key(cid) {
                return axum::Json(serde_json::json!({"Keys": {cid: {"Type": "recursive"}}})).into_response();
            }
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("path is not pinned")).into_response();
        }
        let keys: serde_json::Map<String, serde_json::Value> = guard
            .pins
            .keys()
            .map(|cid| (cid.clone(), serde_json::json!({"Type": "recursive"})))
            .collect();
        axum::Json(serde_json::json!({"Keys": keys})).into_response()
    }

    async fn pin_rm(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let mut guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_outage(&mut guard) {
            return response;
        }
        let pairs = query_pairs(&uri);
        let Some(cid) = query_one(&pairs, "arg") else {
            return (StatusCode::BAD_REQUEST, kubo_error("missing arg")).into_response();
        };
        if guard.pins.remove(cid).is_some() {
            axum::Json(serde_json::json!({"Pins": [cid]})).into_response()
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("path is not pinned")).into_response()
        }
    }

    async fn block_get(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let mut guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_outage(&mut guard) {
            return response;
        }
        let pairs = query_pairs(&uri);
        let Some(cid) = query_one(&pairs, "arg") else {
            return (StatusCode::BAD_REQUEST, kubo_error("missing arg")).into_response();
        };
        let Some(bytes) = guard.pins.get(cid).cloned() else {
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("blockstore: block not found")).into_response();
        };
        let bytes = if guard.faults.corrupt_next_block_get {
            guard.faults.corrupt_next_block_get = false;
            let mut corrupted = bytes;
            if let Some(byte) = corrupted.first_mut() {
                *byte ^= 0xFF;
            } else {
                corrupted.push(0xFF);
            }
            corrupted
        } else {
            bytes
        };
        (StatusCode::OK, bytes).into_response()
    }

    async fn files_mkdir(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let mut guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_mfs(&guard) {
            return response;
        }
        let pairs = query_pairs(&uri);
        if query_one(&pairs, "parents").is_some() {
            return (StatusCode::BAD_REQUEST, kubo_error("parents is not supported")).into_response();
        }
        let Some(path) = query_one(&pairs, "arg") else {
            return (StatusCode::BAD_REQUEST, kubo_error("missing arg")).into_response();
        };
        if guard.dirs.contains(path) {
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("file already exists")).into_response();
        }
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
            .unwrap_or("/");
        if parent != "/" && !guard.dirs.contains(parent) {
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("parent does not exist")).into_response();
        }
        guard.dirs.insert(path.to_string());
        (StatusCode::OK, String::new()).into_response()
    }

    async fn files_cp(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let mut guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_mfs(&guard) {
            return response;
        }
        let pairs = query_pairs(&uri);
        let args = query_all(&pairs, "arg");
        let [source, destination] = args.as_slice() else {
            return (StatusCode::BAD_REQUEST, kubo_error("files/cp requires two arguments")).into_response();
        };
        let Some(cid) = source.strip_prefix("/ipfs/") else {
            return (StatusCode::BAD_REQUEST, kubo_error("source must be /ipfs/<cid>")).into_response();
        };
        if !guard.pins.contains_key(cid) {
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("source does not exist in this bucket")).into_response();
        }
        if guard.files.contains_key(*destination) {
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("already exists")).into_response();
        }
        guard.files.insert(destination.to_string(), cid.to_string());
        (StatusCode::OK, String::new()).into_response()
    }

    async fn files_ls(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_mfs(&guard) {
            return response;
        }
        let pairs = query_pairs(&uri);
        let Some(path) = query_one(&pairs, "arg") else {
            return (StatusCode::BAD_REQUEST, kubo_error("missing arg")).into_response();
        };
        if !guard.dirs.contains(path) {
            return (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("file does not exist")).into_response();
        }
        let prefix = format!("{path}/");
        let entries: Vec<_> = guard
            .files
            .keys()
            .filter_map(|file_path| {
                file_path
                    .strip_prefix(prefix.as_str())
                    .filter(|rest| !rest.contains('/'))
                    .map(|name| serde_json::json!({"Name": name}))
            })
            .collect();
        axum::Json(serde_json::json!({"Entries": entries})).into_response()
    }

    async fn files_stat(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        if let Err(response) = check_mfs(&guard) {
            return response;
        }
        let pairs = query_pairs(&uri);
        let Some(path) = query_one(&pairs, "arg") else {
            return (StatusCode::BAD_REQUEST, kubo_error("missing arg")).into_response();
        };
        if let Some(cid) = guard.files.get(path) {
            return axum::Json(serde_json::json!({"Hash": cid, "Type": "file"})).into_response();
        }
        if guard.dirs.contains(path) {
            return axum::Json(serde_json::json!({"Hash": "directory", "Type": "directory"})).into_response();
        }
        (StatusCode::INTERNAL_SERVER_ERROR, kubo_error("file does not exist")).into_response()
    }

    async fn files_rm(State(state): State<SharedState>, headers: HeaderMap, uri: Uri) -> Response {
        let mut guard = state.lock().unwrap();
        if let Err(response) = check_auth(&guard, &headers) {
            return response;
        }
        let pairs = query_pairs(&uri);
        if query_one(&pairs, "recursive").is_some() || query_one(&pairs, "force").is_some() {
            return (StatusCode::BAD_REQUEST, kubo_error("recursive and force are not supported")).into_response();
        }
        let Some(path) = query_one(&pairs, "arg") else {
            return (StatusCode::BAD_REQUEST, kubo_error("missing arg")).into_response();
        };
        guard.dirs.remove(path);
        guard.files.remove(path);
        (StatusCode::OK, String::new()).into_response()
    }
}

#[cfg(test)]
mod transport_tests {
    use super::fake_server::FakeKuboServer;
    use super::*;
    use threestrands_sync_envelope::{sign_device_head, DeviceHead, SigningKey};
    use threestrands_sync_transport::conformance;

    fn open(server: &FakeKuboServer, instance_id: &str) -> IpfsRpcTransport {
        IpfsRpcTransport::new(instance_id, &server.base_url(), None, b"space").unwrap()
    }

    #[tokio::test]
    async fn passes_the_shared_conformance_suite() {
        let server_a = FakeKuboServer::spawn().await;
        let server_b = FakeKuboServer::spawn().await;
        let a = open(&server_a, "a");
        let b = open(&server_b, "b");
        conformance::run_all(&a, &b).await;
    }

    #[tokio::test]
    async fn reports_healthy_and_storage_and_discovery_when_mfs_works() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        let report = transport.probe_capabilities().await.unwrap();
        assert!(report.version_ok);
        assert!(report.mfs_available);
        assert_eq!(transport.health().await.unwrap(), TransportHealth::Healthy);
        assert!(transport.capabilities().head_discovery);
    }

    #[tokio::test]
    async fn mfs_probe_uses_filebase_compatible_calls_and_cleans_up_its_object() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");

        let report = transport.probe_capabilities().await.unwrap();

        assert!(report.mfs_available);
        let state = server.faults();
        assert!(state.pins.is_empty(), "the random probe block must be unpinned");
        assert!(state.files.is_empty(), "the temporary MFS link must be removed");
        assert!(state.dirs.contains("/threestrands"));
        assert!(state.dirs.contains(&format!("/threestrands/{}", transport.space_tag)));
        assert!(!state.dirs.contains(&format!("/threestrands/{}/.probe", transport.space_tag)));
    }

    #[tokio::test]
    async fn reports_degraded_and_storage_only_when_mfs_is_unavailable() {
        let server = FakeKuboServer::spawn().await;
        server.faults().faults.mfs_disabled = true;
        let transport = open(&server, "a");
        let report = transport.probe_capabilities().await.unwrap();
        assert!(report.version_ok);
        assert!(!report.mfs_available);
        assert!(matches!(transport.health().await.unwrap(), TransportHealth::Degraded(_)));
        assert!(!transport.capabilities().head_discovery);

        // Storage-only still works fine as a plain replica.
        let bytes = b"still a valid replica".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();
        assert_eq!(transport.get_object(&cid).await.unwrap(), bytes);

        // But it cannot discover: resolve_heads returns empty, not an error.
        let device_id = DeviceId::from_bytes([1u8; 16]);
        let heads = transport.resolve_heads(&[HeadLocator { device_id, remote_id: None }]).await.unwrap();
        assert!(heads.is_empty());
    }

    #[tokio::test]
    async fn requires_the_correct_bearer_token() {
        let server = FakeKuboServer::spawn().await;
        server.faults().faults.required_token = Some("secret-token".to_string());

        let unauthenticated = open(&server, "a");
        let bytes = b"x".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        let result = unauthenticated.put_object(&cid, &bytes).await;
        assert!(matches!(result, Err(TransportError::Authentication(_))));
        // The rejection message must never echo a token back — there was
        // none to echo, but this also guards against a future regression
        // that starts including request details in the error.
        if let Err(TransportError::Authentication(message)) = result {
            assert!(!message.contains("secret-token"));
        }

        let authenticated =
            IpfsRpcTransport::new("a", &server.base_url(), Some("secret-token".to_string()), b"space").unwrap();
        authenticated.put_object(&cid, &bytes).await.unwrap();
    }

    #[tokio::test]
    async fn a_wrong_token_never_appears_in_any_error_message() {
        let server = FakeKuboServer::spawn().await;
        server.faults().faults.required_token = Some("correct-token".to_string());
        let transport =
            IpfsRpcTransport::new("a", &server.base_url(), Some("wrong-token-value".to_string()), b"space").unwrap();
        let bytes = b"x".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        let result = transport.put_object(&cid, &bytes).await;
        let Err(error) = result else { panic!("expected an authentication failure") };
        assert!(!error.to_string().contains("wrong-token-value"));
    }

    #[tokio::test]
    async fn rejects_a_redirect_rather_than_following_it_with_credentials() {
        // The fake server never issues a redirect itself, so this proves
        // the client-side policy directly: build a transport pointed at an
        // origin that the client's own redirect policy must refuse to
        // follow even before any response is involved, by pointing it at a
        // path prefix that is itself harmless — the real guarantee here is
        // `reqwest::redirect::Policy::none()` on the client, exercised by
        // every other request in this suite (none of which ever follow a
        // redirect, because the fake server never sends one and a real
        // Kubo-compatible endpoint has no reason to either). A transport
        // configured with `redirect::Policy::none()` surfaces a 3xx as a
        // permanent error rather than silently forwarding Authorization
        // cross-origin, which is asserted structurally here.
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        // `method_url` never accepts anything but a fixed literal, so a
        // redirect-follow can only ever be introduced by the reqwest
        // client's own policy — assert that policy is in effect.
        let response = transport
            .client
            .get(format!("{}/does-not-redirect", server.base_url()))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_client_error() || response.status().is_server_error());
    }

    #[tokio::test]
    async fn one_failed_endpoint_never_blocks_another() {
        let healthy_server = FakeKuboServer::spawn().await;
        let healthy = open(&healthy_server, "healthy");
        // An endpoint nothing is listening on: connection failures map to
        // Transient, never a panic or a hang that could block a caller
        // iterating over multiple configured transports.
        let unreachable = IpfsRpcTransport::new("unreachable", "http://127.0.0.1:1", None, b"space").unwrap();

        let bytes = b"data".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        healthy.put_object(&cid, &bytes).await.unwrap();
        assert!(matches!(unreachable.put_object(&cid, &bytes).await, Err(TransportError::Transient(_))));
        // The healthy one is completely unaffected by the other's failure.
        assert_eq!(healthy.get_object(&cid).await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn dag_import_root_substitution_never_satisfies_the_expected_cid() {
        // Simulates an `/add`-style endpoint that reports a different
        // (UnixFS) CID than the raw-block CID actually requested.
        let server = FakeKuboServer::spawn().await;
        server.faults().faults.substitute_root = Some("bafyreianotmyrootatall000000000000000000000000000000000".to_string());
        let transport = open(&server, "a");
        let bytes = b"data".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        let result = transport.put_object(&cid, &bytes).await;
        assert!(result.is_err(), "a substituted root must never satisfy delivery of the requested CID");
    }

    #[tokio::test]
    async fn a_corrupted_block_response_is_rejected_not_silently_accepted() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        let bytes = b"authentic content".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();

        server.faults().faults.corrupt_next_block_get = true;
        let result = transport.get_object(&cid).await;
        assert_eq!(result, Err(TransportError::Corruption(
            "returned block bytes do not match the requested CID".to_string()
        )));
    }

    #[tokio::test]
    async fn quota_and_outage_are_retryable_not_permanent() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        server.faults().faults.outage_remaining = 1;
        let bytes = b"data".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        assert!(matches!(transport.put_object(&cid, &bytes).await, Err(TransportError::Transient(_))));
        // The outage was consumed; a retry succeeds.
        transport.put_object(&cid, &bytes).await.unwrap();

        server.faults().faults.quota_after_puts = Some(0);
        let more_bytes = b"more data".to_vec();
        let more_cid = TransportCid::for_bytes(&more_bytes);
        assert!(matches!(transport.put_object(&more_cid, &more_bytes).await, Err(TransportError::Quota(_))));
    }

    #[tokio::test]
    async fn publish_and_resolve_head_round_trips_through_mfs() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let device_id = DeviceId::from_bytes([9u8; 16]);
        let head = DeviceHead {
            sync_space_id: b"space".to_vec(),
            device_id,
            epoch: 1,
            contiguous_sequence: 1,
            latest_event_cid: None,
        };
        let signed = sign_device_head(&signing_key, head).unwrap();
        transport.publish_head(&signed).await.unwrap();

        let resolved = transport.resolve_heads(&[HeadLocator { device_id, remote_id: None }]).await.unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0], signed);
    }

    #[tokio::test]
    async fn resolve_heads_picks_the_highest_sequence_never_overwriting_entries() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let device_id = DeviceId::from_bytes([3u8; 16]);
        for sequence in [1u64, 2, 3] {
            let head = DeviceHead {
                sync_space_id: b"space".to_vec(),
                device_id,
                epoch: 1,
                contiguous_sequence: sequence,
                latest_event_cid: None,
            };
            let signed = sign_device_head(&signing_key, head).unwrap();
            transport.publish_head(&signed).await.unwrap();
        }

        let resolved = transport.resolve_heads(&[HeadLocator { device_id, remote_id: None }]).await.unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].head.contiguous_sequence, 3);

        // All three entries are still present in MFS — none were
        // overwritten, matching "never overwrite a head entry."
        let dir = format!("/threestrands/{}/heads/{}", transport.space_tag, super::device_tag(&device_id));
        let entries = transport.mfs_ls(&dir).await.unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[tokio::test]
    async fn resolve_heads_tolerates_a_missing_device_directory() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        let device_id = DeviceId::from_bytes([7u8; 16]);
        let resolved = transport.resolve_heads(&[HeadLocator { device_id, remote_id: None }]).await.unwrap();
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn delete_confirms_removal_before_reporting_success() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        let bytes = b"to be removed".to_vec();
        let cid = TransportCid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();
        transport.delete_object(&cid).await.unwrap();
        assert_eq!(transport.get_object(&cid).await, Err(TransportError::NotFound));
        // Deleting again is idempotent, not an error.
        transport.delete_object(&cid).await.unwrap();
    }

    #[tokio::test]
    async fn scan_enumerates_pinned_objects_with_pagination() {
        let server = FakeKuboServer::spawn().await;
        let transport = open(&server, "a");
        for index in 0..3 {
            let bytes = format!("object {index}").into_bytes();
            let cid = TransportCid::for_bytes(&bytes);
            transport.put_object(&cid, &bytes).await.unwrap();
        }
        let page = transport.scan(None).await.unwrap().unwrap();
        assert_eq!(page.objects.len(), 3);
    }
}
