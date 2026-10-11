//! SMTP submission for IMAP accounts: the shared TLS/connect helpers (moved
//! here from `setup.rs`) and a phase-tracked submission client.
//!
//! `docs/imap-design.md` ("Sending"): submission goes over the SAME
//! fingerprint-pinning TLS path as IMAP (implicit TLS on 465, STARTTLS on 587
//! or any port the user sets; never a plaintext fallback), with the same
//! certificate rules. Two things live here:
//!
//! * **The setup dry-run** ([`test_smtp`], [`probe_smtp_certificate`]) that
//!   `setup.rs` re-exports unchanged. It authenticates WITHOUT sending mail,
//!   proving the credential and the pin before an account is saved. Its
//!   observable behaviour — error mapping, no credentials before TLS, pin
//!   rejection in the handshake before AUTH — is identical to before this
//!   module existed; the tests moved with it.
//!
//! * **The submission client** ([`SmtpSession`]) that actually sends. It is
//!   built on the `mail-send` crate's LOW-LEVEL primitives, NOT its one-shot
//!   `send()`: that call merges the message-body write and the server's final
//!   reply, so it cannot tell "the server definitely did not take the message"
//!   from "we do not know whether it did". The whole delivery-safety model
//!   turns on that distinction, so this module drives the phases itself —
//!   MAIL FROM, each RCPT, DATA, the body, the final reply — and records which
//!   [`Phase`] a failure happened in, mapping it to one of three outcomes:
//!   [`SmtpOutcome::Accepted`], [`SmtpOutcome::NotDelivered`] (definite), or
//!   [`SmtpOutcome::Unknown`] (the message data may have been received).
//!
//! NOTHING here is wired into `MailSend`/the UI yet (Phase 4 later runs own
//! identities, Message-ID, the Sent-copy `APPEND` and `send_supported`). This
//! run ships the library module only; the sole shipped behaviour change is the
//! new dependency.
//!
//! ## Why we validate addresses and frame the body ourselves
//!
//! `mail-send` does NOT validate caller-supplied addresses: an address
//! containing CRLF puts an injected `RCPT TO` on the wire (proven in the Phase
//! 4 spike). So [`validate_envelope_address`] runs on MAIL FROM and every RCPT
//! BEFORE any network I/O, and a bad address fails with zero bytes sent.
//!
//! `mail-send`'s `write_message` dot-stuffs after CR or LF but NOT at byte 0,
//! and does not normalise a lone `\n` to CRLF. Rather than depend on those
//! edges, [`frame_message`] normalises line endings to CRLF, dot-stuffs every
//! line that begins with `.` (including the first), and appends the
//! terminating `\r\n.\r\n` — one tested function — and we write its output as
//! raw bytes, so `write_message` never runs and cannot double-stuff.

use std::sync::Arc;
use std::time::Duration;

use smtp_proto::Response;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::provider::imap::policy;
use crate::provider::imap::settings::SecurityMode;
use crate::provider::imap::tls::{
    CertificateInfo, CollectingCertVerifier, PinnedCertVerifier, Sha256Fingerprint,
};
use crate::provider::imap::connection;
use crate::provider::ProviderError;

// ===========================================================================
// Shared TLS/connect helpers (moved from setup.rs; both the dry-run and the
// submission client use the SAME pinned connector and EHLO identity).
// ===========================================================================

/// How long the SMTP dry-run may take overall. Kept short; it is a probe.
const SMTP_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// A conservative EHLO identifier. The hostname is not security-relevant and
/// some servers reject an empty one, so a fixed client name is used. Shared by
/// the dry-run and the submission client so both present the same identity.
pub(crate) fn ehlo_name() -> &'static str {
    "threestrands.local"
}

/// Build the rustls `TlsConnector` that enforces the account's certificate
/// rules: a pin matches the exact self-signed leaf, no pin defers to WebPKI
/// against the platform roots. This is the ONE place both the setup dry-run
/// and the submission client turn a `pinned` fingerprint into a connector, so
/// the two can never drift apart on trust policy. Verification is never
/// disabled — there is no accept-anything path here (contrast mail-send's own
/// `allow_invalid_certs`, which we never call).
pub(crate) fn pinned_tls_connector(
    pinned: Option<Sha256Fingerprint>,
) -> tokio_rustls::TlsConnector {
    let verifier: Arc<dyn rustls::client::danger::ServerCertVerifier> = match pinned {
        Some(fp) => PinnedCertVerifier::pinned(fp),
        None => PinnedCertVerifier::webpki_only(),
    };
    build_connector(verifier)
}

fn build_connector(
    verifier: Arc<dyn rustls::client::danger::ServerCertVerifier>,
) -> tokio_rustls::TlsConnector {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls_config))
}

/// Minimal SMTP line write + reply read helpers over any async stream, used by
/// the dry-run path. (The submission client uses mail-send's own reader.)
trait SmtpStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> SmtpStream for T {}

async fn connect_smtp_tls_with_verifier(
    host: &str,
    port: u16,
    security: SecurityMode,
    connector: tokio_rustls::TlsConnector,
) -> Result<Box<dyn SmtpStream>, ProviderError> {
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| ProviderError::InvalidOperation(format!("invalid SMTP server name: {e}")))?;
    let transport = |message: String| ProviderError::TransientTransport(message);

    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| transport(format!("SMTP connect failed: {e}")))?;

    let mut stream: Box<dyn SmtpStream> = match security {
        SecurityMode::ImplicitTls => {
            let tls = connector
                .connect(server_name, tcp)
                .await
                .map_err(|e| transport(format!("SMTP TLS handshake failed: {e}")))?;
            Box::new(tls)
        }
        SecurityMode::StartTls => {
            let mut plain = tcp;
            read_smtp_reply(&mut plain, 220).await?; // greeting
            smtp_command(&mut plain, &format!("EHLO {}\r\n", ehlo_name()), 250).await?;
            smtp_command(&mut plain, "STARTTLS\r\n", 220).await?;
            let tls = connector
                .connect(server_name, plain)
                .await
                .map_err(|e| transport(format!("SMTP STARTTLS handshake failed: {e}")))?;
            Box::new(tls)
        }
    };

    if matches!(security, SecurityMode::ImplicitTls) {
        read_smtp_reply(&mut *stream, 220).await?;
    }
    Ok(stream)
}

async fn smtp_command<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + ?Sized>(
    stream: &mut S,
    command: &str,
    expected: u16,
) -> Result<(), ProviderError> {
    write_line(stream, command).await?;
    read_smtp_reply(stream, expected).await
}

async fn write_line<S: tokio::io::AsyncWrite + Unpin + ?Sized>(
    stream: &mut S,
    line: &str,
) -> Result<(), ProviderError> {
    stream
        .write_all(line.as_bytes())
        .await
        .map_err(|e| ProviderError::TransientTransport(format!("SMTP write failed: {e}")))?;
    stream
        .flush()
        .await
        .map_err(|e| ProviderError::TransientTransport(format!("SMTP flush failed: {e}")))
}

/// Read one SMTP reply (handling multi-line `250-...` continuations) and check
/// its status code equals `expected`.
async fn read_smtp_reply<S: tokio::io::AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    expected: u16,
) -> Result<(), ProviderError> {
    let mut byte = [0u8; 1];
    let buf = loop {
        let mut line = Vec::new();
        loop {
            let n = stream
                .read(&mut byte)
                .await
                .map_err(|e| ProviderError::TransientTransport(format!("SMTP read failed: {e}")))?;
            if n == 0 {
                return Err(ProviderError::TransientTransport(
                    "the SMTP connection closed early".into(),
                ));
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
            if line.len() > 4096 {
                return Err(ProviderError::TransientTransport(
                    "the SMTP server sent an overlong reply".into(),
                ));
            }
        }
        let is_final = line.get(3).is_none_or(|&c| c == b' ');
        if is_final {
            break line;
        }
    };
    let text = String::from_utf8_lossy(&buf);
    let code: u16 = text
        .get(0..3)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| ProviderError::TransientTransport(format!("unparsable SMTP reply: {text}")))?;
    if code == expected {
        Ok(())
    } else {
        Err(ProviderError::TransientTransport(format!(
            "SMTP server replied {code} (expected {expected}): {}",
            text.trim()
        )))
    }
}

/// Run an SMTP `EHLO` + `AUTH LOGIN` dry-run over the pinning-TLS path,
/// authenticating WITHOUT sending any mail, then `QUIT`. Behaviour is identical
/// to the pre-move `setup::test_smtp`; `setup.rs` re-exports this.
pub async fn test_smtp(
    host: &str,
    port: u16,
    security: SecurityMode,
    pinned: Option<Sha256Fingerprint>,
    username: &str,
    password: &str,
) -> Result<(), ProviderError> {
    let probe = smtp_dry_run(host, port, security, pinned, username, password);
    match tokio::time::timeout(SMTP_PROBE_TIMEOUT, probe).await {
        Ok(result) => result,
        Err(_) => Err(ProviderError::TransientTransport(
            "the SMTP server did not respond in time".into(),
        )),
    }
}

async fn smtp_dry_run(
    host: &str,
    port: u16,
    security: SecurityMode,
    pinned: Option<Sha256Fingerprint>,
    username: &str,
    password: &str,
) -> Result<(), ProviderError> {
    use base64::{engine::general_purpose::STANDARD, Engine};

    let connector = pinned_tls_connector(pinned);
    let mut stream = connect_smtp_tls_with_verifier(host, port, security, connector).await?;
    smtp_command(&mut *stream, &format!("EHLO {}\r\n", ehlo_name()), 250).await?;

    smtp_command(&mut *stream, "AUTH LOGIN\r\n", 334)
        .await
        .map_err(|_| {
            ProviderError::InvalidOperation("the server did not offer AUTH LOGIN".into())
        })?;
    write_line(&mut *stream, &format!("{}\r\n", STANDARD.encode(username))).await?;
    read_smtp_reply(&mut *stream, 334).await?;
    write_line(&mut *stream, &format!("{}\r\n", STANDARD.encode(password))).await?;
    match read_smtp_reply(&mut *stream, 235).await {
        Ok(()) => {}
        Err(_) => {
            return Err(ProviderError::ReauthenticationRequired(
                "the SMTP server rejected the credentials".into(),
            ))
        }
    }

    let _ = write_line(&mut *stream, "QUIT\r\n").await;
    Ok(())
}

/// Connect and inspect SMTP's certificate without ever sending credentials.
/// Behaviour is identical to the pre-move `setup::probe_smtp_certificate`.
pub async fn probe_smtp_certificate(
    host: &str,
    port: u16,
    security: SecurityMode,
) -> Result<connection::CertificateProbe, ProviderError> {
    let probe = async {
        let verifier = CollectingCertVerifier::new();
        let connector = build_connector(verifier.clone());
        let stream = connect_smtp_tls_with_verifier(host, port, security, connector).await?;
        drop(stream);
        let leaf = verifier.collected_leaf().ok_or_else(|| {
            ProviderError::TransientTransport("the SMTP server presented no certificate".into())
        })?;
        Ok(connection::CertificateProbe {
            certificate: CertificateInfo::from_leaf(&leaf),
            trusted_by_platform: verifier.trusted_by_platform(),
        })
    };
    tokio::time::timeout(SMTP_PROBE_TIMEOUT, probe)
        .await
        .map_err(|_| {
            ProviderError::TransientTransport("the SMTP server did not respond in time".into())
        })?
}

/// Pick a sensible default SMTP port: 465 implicit / 587 STARTTLS.
pub fn default_smtp_port(security: SecurityMode) -> u16 {
    match security {
        SecurityMode::ImplicitTls => 465,
        SecurityMode::StartTls => 587,
    }
}

// ===========================================================================
// Input safety: address validation, body framing, reply sanitising.
// ===========================================================================

/// Validate ONE envelope address (the `local@domain` that goes inside the
/// angle brackets of `MAIL FROM` / `RCPT TO`) before any network I/O.
///
/// This is a conservative SYNTACTIC gate, not an RFC 5322 parser. It rejects:
/// an empty address; any control or structural byte that could inject a new
/// command or break the envelope (`CR`, `LF`, `NUL`, space, `<`, `>`); an
/// address over [`policy::MAX_SMTP_ADDRESS_BYTES`]; anything without exactly
/// one `@` with a non-empty local and domain part; and a non-ASCII local part
/// (SMTPUTF8 is not negotiated in this run, so we reject rather than risk a
/// server that cannot handle it). The point is that a bad address can never
/// reach the wire — [`SmtpSession::send`] runs this on MAIL FROM and every
/// RCPT first, and a failure returns [`SmtpOutcome::NotDelivered`] with zero
/// bytes sent.
pub fn validate_envelope_address(addr: &str) -> Result<(), ProviderError> {
    let reject = |m: &str| {
        Err(ProviderError::PermanentClientRejection(format!(
            "invalid email address: {m}"
        )))
    };

    if addr.is_empty() {
        return reject("empty");
    }
    policy::check_smtp_address_len(addr.len())?;

    // Structural / control bytes that could inject a command or break the
    // envelope. CR/LF are the injection vector the spike proved.
    for b in addr.bytes() {
        match b {
            b'\r' | b'\n' => return reject("contains a line break"),
            0 => return reject("contains a NUL byte"),
            b' ' | b'\t' => return reject("contains whitespace"),
            b'<' | b'>' => return reject("contains angle brackets"),
            _ if b < 0x20 || b == 0x7f => return reject("contains a control character"),
            _ => {}
        }
    }

    // Exactly one '@', non-empty local and domain.
    let mut parts = addr.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next();
    let extra = parts.next();
    let (local, domain) = match (domain, extra) {
        (Some(domain), None) => (local, domain),
        _ => return reject("must contain exactly one '@'"),
    };
    if local.is_empty() {
        return reject("the local part before '@' is empty");
    }
    if domain.is_empty() {
        return reject("the domain after '@' is empty");
    }
    // SMTPUTF8 is not negotiated this run, so a non-ASCII LOCAL part is a
    // permanent rejection with a clear message rather than a wire risk. (A
    // non-ASCII DOMAIN would need IDNA/punycode handling that also lands
    // later; reject it the same way for now.)
    if !addr.is_ascii() {
        return reject(
            "contains non-ASCII characters; internationalised addresses (SMTPUTF8) are not supported yet",
        );
    }
    Ok(())
}

/// CRLF-normalise and dot-stuff a raw message for the DATA phase, returning the
/// bytes to put on the wire INCLUDING the terminating `\r\n.\r\n`.
///
/// We do this ourselves rather than lean on `mail-send::write_message` because
/// that function does not dot-stuff a `.` at byte 0 and does not normalise a
/// lone `\n`. Here:
/// * every lone `\n` and lone `\r` becomes `\r\n` (idempotent on existing
///   CRLF), so the body is well-formed RFC 5322 line endings on the wire;
/// * any line that BEGINS with `.` — including the very first line — gets an
///   extra leading `.` (RFC 5321 §4.5.2 transparency), so a body line of `.`
///   can never be read as the end-of-data terminator;
/// * the result ends with `\r\n.\r\n`.
///
/// The caller writes the returned bytes directly, so `write_message` never
/// runs and no second dot-stuffing happens.
pub fn frame_message(raw: &[u8]) -> Vec<u8> {
    // 1. Normalise line endings to CRLF. Walk bytes, treating \r\n, lone \r
    //    and lone \n all as one line break.
    let mut normalised: Vec<u8> = Vec::with_capacity(raw.len() + 16);
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'\r' => {
                normalised.push(b'\r');
                normalised.push(b'\n');
                // Swallow a following \n so \r\n stays one break.
                if raw.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => {
                normalised.push(b'\r');
                normalised.push(b'\n');
            }
            other => normalised.push(other),
        }
        i += 1;
    }

    // 2. Dot-stuff: at the start of the message and after every CRLF, a line
    //    beginning with '.' gets a doubled '.'.
    let mut stuffed: Vec<u8> = Vec::with_capacity(normalised.len() + 16);
    let mut at_line_start = true;
    let mut j = 0;
    while j < normalised.len() {
        let b = normalised[j];
        if at_line_start && b == b'.' {
            stuffed.push(b'.');
        }
        stuffed.push(b);
        // The next byte is at a line start iff we just emitted the \n of a CRLF.
        if b == b'\n' {
            at_line_start = true;
        } else {
            at_line_start = false;
        }
        j += 1;
    }

    // 3. Terminator. Ensure the data ends with CRLF before the dot line.
    if !stuffed.ends_with(b"\r\n") {
        stuffed.extend_from_slice(b"\r\n");
    }
    stuffed.extend_from_slice(b".\r\n");
    stuffed
}

/// Strip control characters from a server reply's text and cap it to
/// [`policy::MAX_SMTP_REPLY_TEXT_BYTES`], so a hostile or buggy server cannot
/// smuggle control bytes into an error or a log, nor bloat it. Tabs and normal
/// printable characters survive; CR, LF and other controls become a space,
/// then runs of whitespace collapse.
fn sanitise_reply_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(policy::MAX_SMTP_REPLY_TEXT_BYTES));
    let mut last_was_space = false;
    for ch in text.chars() {
        let mapped = if ch.is_control() || ch == '\t' { ' ' } else { ch };
        if mapped == ' ' {
            if last_was_space {
                continue;
            }
            last_was_space = true;
        } else {
            last_was_space = false;
        }
        // Cap on bytes, not chars, and never split a multi-byte char.
        if out.len() + mapped.len_utf8() > policy::MAX_SMTP_REPLY_TEXT_BYTES {
            break;
        }
        out.push(mapped);
    }
    out.trim().to_string()
}

// ===========================================================================
// Phase model and classification.
// ===========================================================================

/// Where in the submission a failure happened. The ONE distinction the
/// delivery-safety model depends on is whether the failure is before or during
/// the body: everything up to and including the server's `354` reply to DATA
/// is DEFINITE (the server never took responsibility), the body write and the
/// wait for the final reply are the UNKNOWN zone (the server may hold the whole
/// message), and the final reply itself is definite again once fully read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// TCP connect, TLS/STARTTLS handshake, EHLO, AUTH. No message bytes.
    Prepare,
    /// `MAIL FROM`.
    MailFrom,
    /// A `RCPT TO`.
    Rcpt,
    /// The `DATA` command, up to and including the server's `354` reply.
    DataRequest,
    /// Writing the message body and its terminating `.` — the UNKNOWN zone
    /// opens with the first body byte.
    Body,
    /// Waiting for and reading the server's final reply after the body.
    FinalReply,
}

impl Phase {
    /// Whether a failure in this phase is UNKNOWN (the message data may have
    /// been received) rather than a definite non-delivery. True from the first
    /// body byte through the final-reply wait; false everywhere before, and
    /// false once the final reply has been fully read (that is a definite
    /// server verdict, classified separately by its code).
    pub fn is_unknown_on_failure(self) -> bool {
        matches!(self, Phase::Body | Phase::FinalReply)
    }
}

/// How a definite failure (a phase whose failure is NOT unknown) maps to a
/// [`ProviderError`] class, given whether the server's reply was transient
/// (4xx) or permanent (5xx), or there was no code (a transport failure).
///
/// This is a PURE function with a table test: an unreachable server or a drop
/// before `354` is `TransientTransport`; an AUTH rejection is the reauth class
/// (matching how IMAP classifies `AUTHENTICATIONFAILED`); a 5xx on the
/// envelope is permanent; a 4xx is retryable transport.
pub fn classify_definite(phase: Phase, reply: Option<&SmtpReply>) -> ProviderError {
    match (phase, reply) {
        // AUTH failure during prepare is a credential problem: pause/reauth.
        (Phase::Prepare, Some(r)) if r.code == 535 || (r.code >= 530 && r.code <= 539) => {
            ProviderError::ReauthenticationRequired(format!(
                "the SMTP server rejected the credentials ({})",
                r.summary()
            ))
        }
        // A permanent (5xx) reply on the envelope is a permanent rejection
        // (e.g. 553/550 on MAIL FROM naming the identity; 554 to DATA).
        (_, Some(r)) if (500..=599).contains(&r.code) => {
            ProviderError::PermanentClientRejection(format!(
                "the SMTP server rejected the message ({})",
                r.summary()
            ))
        }
        // A transient (4xx) reply is retryable transport (e.g. 450 on RCPT,
        // 451 as a final reply).
        (_, Some(r)) if (400..=499).contains(&r.code) => {
            ProviderError::TransientTransport(format!(
                "the SMTP server temporarily rejected the message ({})",
                r.summary()
            ))
        }
        // No code: a connect/DNS/refused/drop/timeout BEFORE 354. Transient
        // transport — the server never took responsibility.
        (_, Some(r)) => ProviderError::InvalidOperation(format!(
            "unexpected SMTP reply ({})",
            r.summary()
        )),
        (_, None) => ProviderError::TransientTransport(
            "the SMTP connection failed before the server accepted the message".into(),
        ),
    }
}

// ===========================================================================
// Public types.
// ===========================================================================

/// Where to submit: host, port, security mode and an optional pin. Mirrors the
/// account's stored SMTP settings for one `host:port`.
#[derive(Clone, Debug)]
pub struct SmtpEndpoint {
    pub host: String,
    pub port: u16,
    pub security: SecurityMode,
    pub pinned: Option<Sha256Fingerprint>,
}

/// SMTP credentials. The password is redacted from `Debug` so it cannot land
/// in a log line by accident.
#[derive(Clone)]
pub struct SmtpCredentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for SmtpCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A server reply, preserved for the UI: the numeric code and the sanitised,
/// length-capped text. Keeping both lets a later run show "553 ... names the
/// identity" verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmtpReply {
    pub code: u16,
    pub text: String,
}

impl SmtpReply {
    fn from_response(resp: &Response<String>) -> Self {
        Self {
            code: resp.code,
            text: sanitise_reply_text(&resp.message),
        }
    }

    /// A short one-line form for an error message: `"<code> <text>"`.
    pub fn summary(&self) -> String {
        if self.text.is_empty() {
            self.code.to_string()
        } else {
            format!("{} {}", self.code, self.text)
        }
    }
}

/// A definite non-delivery: the server never took responsibility for the
/// message. Safe to resend.
#[derive(Debug)]
pub struct SmtpFailure {
    pub phase: Phase,
    pub error: ProviderError,
    /// The server's reply, when the failure was a reply rather than a
    /// transport error.
    pub reply: Option<SmtpReply>,
}

/// An UNKNOWN result: the message data may have been received. Must NOT be
/// resent automatically (the design's "may have been sent: check before
/// resending").
#[derive(Clone, Debug)]
pub struct SmtpUnknown {
    pub phase: Phase,
    pub detail: String,
}

/// The result of a submission attempt. Three states, because the one the
/// delivery-safety model turns on is the middle one.
#[derive(Debug)]
pub enum SmtpOutcome {
    /// The server accepted the message (2xx final reply). Carries that reply.
    Accepted { reply: SmtpReply },
    /// Definite non-delivery; the server never took responsibility.
    NotDelivered(SmtpFailure),
    /// The message data may or may not have been received; do not resend
    /// automatically.
    Unknown(SmtpUnknown),
}

/// Timeouts for the submission phases, injectable so tests can drive the
/// "hung before 354 => NotDelivered" and "hung final reply => Unknown" cases
/// under paused tokio time without waiting the real ceilings out.
#[derive(Clone, Copy, Debug)]
pub struct SmtpTimeouts {
    pub connect: Duration,
    pub command: Duration,
    pub final_reply: Duration,
}

impl Default for SmtpTimeouts {
    fn default() -> Self {
        Self {
            connect: policy::SMTP_CONNECT_TIMEOUT,
            command: policy::SMTP_COMMAND_TIMEOUT,
            final_reply: policy::SMTP_FINAL_REPLY_TIMEOUT,
        }
    }
}

/// A connected, TLS-secured, authenticated SMTP session with NOTHING sent yet.
/// Built by [`SmtpSession::connect`]; consumed by [`SmtpSession::send`].
pub struct SmtpSession {
    client: mail_send::SmtpClient<TlsStream<TcpStream>>,
    timeouts: SmtpTimeouts,
}

impl SmtpSession {
    /// Connect, establish TLS (implicit or STARTTLS), EHLO and AUTH. Every
    /// failure in this phase is a DEFINITE non-delivery — nothing has been
    /// sent — so this returns `Err(SmtpFailure)` rather than an outcome enum.
    ///
    /// Uses mail-send's builder with OUR pinned connector injected into
    /// `tls_connector`, so a wrong fingerprint fails in the TLS handshake
    /// BEFORE any AUTH bytes reach the wire. With `SecurityMode::StartTls`
    /// against a server that does not advertise STARTTLS, mail-send returns
    /// `MissingStartTls` and never falls back to a plaintext AUTH.
    pub async fn connect(
        endpoint: &SmtpEndpoint,
        creds: &SmtpCredentials,
    ) -> Result<SmtpSession, SmtpFailure> {
        Self::connect_with_timeouts(endpoint, creds, SmtpTimeouts::default()).await
    }

    /// [`connect`](Self::connect) with explicit timeouts (tests).
    pub async fn connect_with_timeouts(
        endpoint: &SmtpEndpoint,
        creds: &SmtpCredentials,
        timeouts: SmtpTimeouts,
    ) -> Result<SmtpSession, SmtpFailure> {
        let connector = pinned_tls_connector(endpoint.pinned);
        let mut builder = mail_send::SmtpClientBuilder::new(endpoint.host.clone(), endpoint.port)
            .map_err(|e| SmtpFailure {
                phase: Phase::Prepare,
                error: ProviderError::InvalidOperation(format!("invalid SMTP host: {e}")),
                reply: None,
            })?
            .implicit_tls(matches!(endpoint.security, SecurityMode::ImplicitTls))
            .helo_host(ehlo_name())
            .timeout(timeouts.connect)
            .credentials(mail_send::Credentials::new(
                creds.username.clone(),
                creds.password.clone(),
            ));
        builder.tls_connector = connector;

        match builder.connect().await {
            Ok(client) => Ok(SmtpSession { client, timeouts }),
            Err(e) => Err(SmtpFailure {
                phase: Phase::Prepare,
                error: map_connect_error(&e),
                reply: connect_reply(&e),
            }),
        }
    }

    /// Submit a message: validate the envelope (no bytes on failure), then
    /// MAIL FROM, each RCPT, DATA, the framed body, and the final reply,
    /// recording the [`Phase`] of any failure to decide DEFINITE vs UNKNOWN.
    ///
    /// Partial-RCPT policy: if ANY recipient is rejected, abort the whole send
    /// with `RSET`/`QUIT` and return [`SmtpOutcome::NotDelivered`] naming that
    /// recipient. We never deliver to a subset silently.
    pub async fn send(
        mut self,
        envelope_from: &str,
        rcpts: &[String],
        raw_message: &[u8],
    ) -> SmtpOutcome {
        // --- Input safety: validate everything BEFORE any network I/O. ---
        if let Err(e) = validate_envelope_address(envelope_from) {
            return self.abort_not_delivered(Phase::MailFrom, e, None).await;
        }
        if rcpts.is_empty() {
            return self
                .abort_not_delivered(
                    Phase::Rcpt,
                    ProviderError::PermanentClientRejection("no recipients".into()),
                    None,
                )
                .await;
        }
        if let Err(e) = policy::check_smtp_recipient_count(rcpts.len()) {
            return self.abort_not_delivered(Phase::Rcpt, e, None).await;
        }
        for rcpt in rcpts {
            if let Err(e) = validate_envelope_address(rcpt) {
                return self.abort_not_delivered(Phase::Rcpt, e, None).await;
            }
        }

        // --- MAIL FROM ---
        match self
            .cmd_definite(Phase::MailFrom, format!("MAIL FROM:<{envelope_from}>\r\n"))
            .await
        {
            Ok(reply) if is_2xx(reply.code) => {}
            Ok(reply) => {
                let err = classify_definite(Phase::MailFrom, Some(&reply));
                return self
                    .abort_not_delivered(Phase::MailFrom, err, Some(reply))
                    .await;
            }
            Err(fail) => return SmtpOutcome::NotDelivered(fail),
        }

        // --- RCPT TO (every one must be accepted) ---
        for rcpt in rcpts {
            match self
                .cmd_definite(Phase::Rcpt, format!("RCPT TO:<{rcpt}>\r\n"))
                .await
            {
                Ok(reply) if is_2xx(reply.code) => {}
                Ok(reply) => {
                    let err = ProviderError::from(classify_rcpt_rejection(rcpt, &reply));
                    return self.abort_not_delivered(Phase::Rcpt, err, Some(reply)).await;
                }
                Err(fail) => return SmtpOutcome::NotDelivered(fail),
            }
        }

        // --- DATA request (up to and including the server's 354) ---
        match self.cmd_definite(Phase::DataRequest, "DATA\r\n".to_string()).await {
            Ok(reply) if reply.code == 354 => {}
            Ok(reply) => {
                let err = classify_definite(Phase::DataRequest, Some(&reply));
                return self
                    .abort_not_delivered(Phase::DataRequest, err, Some(reply))
                    .await;
            }
            Err(fail) => return SmtpOutcome::NotDelivered(fail),
        }

        // === UNKNOWN zone opens here: the first body byte is on the wire. ===
        let framed = frame_message(raw_message);
        if let Err(e) = self.write_body(&framed).await {
            // A drop/error DURING the body write is conservatively UNKNOWN:
            // the server may already hold a prefix (or the whole thing).
            return SmtpOutcome::Unknown(SmtpUnknown {
                phase: Phase::Body,
                detail: format!("the connection failed while sending the message body: {e}"),
            });
        }

        // --- Final reply (its own, longer, timeout; expiry => UNKNOWN) ---
        match tokio::time::timeout(self.timeouts.final_reply, self.client.read()).await {
            Err(_) => SmtpOutcome::Unknown(SmtpUnknown {
                phase: Phase::FinalReply,
                detail: "the server did not reply after the message body within the timeout".into(),
            }),
            Ok(Err(_)) => SmtpOutcome::Unknown(SmtpUnknown {
                phase: Phase::FinalReply,
                detail: "the connection dropped before the server's final reply".into(),
            }),
            Ok(Ok(resp)) => {
                let reply = SmtpReply::from_response(&resp);
                if is_2xx(reply.code) {
                    // Best-effort QUIT; never let cleanup rewrite the verdict.
                    self.best_effort_quit().await;
                    SmtpOutcome::Accepted { reply }
                } else {
                    // A 4xx/5xx FINAL reply is a DEFINITE non-delivery: the
                    // server fully answered and refused.
                    let err = classify_definite(Phase::FinalReply, Some(&reply));
                    self.best_effort_quit().await;
                    SmtpOutcome::NotDelivered(SmtpFailure {
                        phase: Phase::FinalReply,
                        error: err,
                        reply: Some(reply),
                    })
                }
            }
        }
    }

    /// Send one envelope command with the per-command timeout, returning the
    /// parsed reply. A transport error / timeout here (before 354) is a
    /// DEFINITE failure in `phase`.
    async fn cmd_definite(
        &mut self,
        phase: Phase,
        command: String,
    ) -> Result<SmtpReply, SmtpFailure> {
        match tokio::time::timeout(self.timeouts.command, self.client.cmd(command.as_bytes()))
            .await
        {
            Ok(Ok(resp)) => Ok(SmtpReply::from_response(&resp)),
            Ok(Err(_)) | Err(_) => Err(SmtpFailure {
                phase,
                error: classify_definite(phase, None),
                reply: None,
            }),
        }
    }

    /// Write the already-framed body bytes directly. (We do NOT call
    /// mail-send's `write_message`, which would dot-stuff a second time.)
    async fn write_body(&mut self, framed: &[u8]) -> std::io::Result<()> {
        self.client.stream.write_all(framed).await?;
        self.client.stream.flush().await
    }

    /// Abort on a definite pre-body failure: best-effort RSET+QUIT (bounded),
    /// then return NotDelivered. Cleanup never turns a definite failure into
    /// unknown — any cleanup error is swallowed.
    async fn abort_not_delivered(
        mut self,
        phase: Phase,
        error: ProviderError,
        reply: Option<SmtpReply>,
    ) -> SmtpOutcome {
        self.best_effort_rset().await;
        self.best_effort_quit().await;
        SmtpOutcome::NotDelivered(SmtpFailure { phase, error, reply })
    }

    async fn best_effort_rset(&mut self) {
        let quick = Duration::from_secs(5);
        let _ = tokio::time::timeout(quick, self.client.cmd(b"RSET\r\n")).await;
    }

    async fn best_effort_quit(&mut self) {
        let quick = Duration::from_secs(5);
        let _ = tokio::time::timeout(quick, self.client.cmd(b"QUIT\r\n")).await;
    }
}

/// Does a code mean "ok" (2xx)?
fn is_2xx(code: u16) -> bool {
    (200..=299).contains(&code)
}

/// Classify a RCPT rejection, naming the recipient. A 5xx is permanent; a 4xx
/// is transient. Returns a [`ProviderError`].
fn classify_rcpt_rejection(rcpt: &str, reply: &SmtpReply) -> ProviderError {
    if (500..=599).contains(&reply.code) {
        ProviderError::PermanentClientRejection(format!(
            "the server rejected the recipient <{rcpt}> ({})",
            reply.summary()
        ))
    } else if (400..=499).contains(&reply.code) {
        ProviderError::TransientTransport(format!(
            "the server temporarily rejected the recipient <{rcpt}> ({})",
            reply.summary()
        ))
    } else {
        ProviderError::InvalidOperation(format!(
            "unexpected reply to RCPT <{rcpt}> ({})",
            reply.summary()
        ))
    }
}

/// Map a mail-send connect error to a [`ProviderError`] class. AUTH failure is
/// the reauth class (like IMAP's `AUTHENTICATIONFAILED`); a missing STARTTLS,
/// a TLS error (which includes a pin mismatch) and a timeout are all definite,
/// transport-class failures — nothing was sent.
fn map_connect_error(e: &mail_send::Error) -> ProviderError {
    use mail_send::Error as E;
    match e {
        E::AuthenticationFailed(resp) => ProviderError::ReauthenticationRequired(format!(
            "the SMTP server rejected the credentials ({})",
            sanitise_reply_text(&resp.message)
        )),
        E::Auth(_) | E::UnsupportedAuthMechanism | E::MissingCredentials => {
            ProviderError::ReauthenticationRequired(
                "the SMTP server rejected authentication".into(),
            )
        }
        E::MissingStartTls => ProviderError::TransientTransport(
            "the SMTP server does not offer STARTTLS; refusing to send credentials in the clear"
                .into(),
        ),
        E::Tls(err) => ProviderError::TransientTransport(format!(
            "SMTP TLS failed (certificate pin or handshake): {err}"
        )),
        E::Io(err) => {
            ProviderError::TransientTransport(format!("could not reach the SMTP server: {err}"))
        }
        E::Timeout => {
            ProviderError::TransientTransport("the SMTP server did not respond in time".into())
        }
        E::InvalidTLSName => {
            ProviderError::InvalidOperation("invalid SMTP server name for TLS".into())
        }
        E::UnexpectedReply(resp) => {
            // A non-2xx at EHLO/greeting during prepare.
            let reply = SmtpReply::from_response(resp);
            classify_definite(Phase::Prepare, Some(&reply))
        }
        other => ProviderError::TransientTransport(format!("SMTP connection failed: {other}")),
    }
}

/// Extract a server reply from a connect error when one is available (an AUTH
/// rejection or an unexpected reply carry the server's `Response`).
fn connect_reply(e: &mail_send::Error) -> Option<SmtpReply> {
    use mail_send::Error as E;
    match e {
        E::AuthenticationFailed(resp) | E::UnexpectedReply(resp) => {
            Some(SmtpReply::from_response(resp))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
