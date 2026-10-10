//! Account-setup orchestration: the "test and save" probe.
//!
//! `docs/imap-design.md` ("Account setup", steps 4-6): before an IMAP account
//! is saved, its entered settings are tested end to end over the wire —
//! IMAP `CAPABILITY` / `LOGIN` / `LIST`, and an SMTP `EHLO` / `AUTH` dry-run
//! that authenticates WITHOUT sending any mail. Only on success is the account
//! adopted and the password written to the keychain. Problems are reported in
//! plain language (wrong password, certificate not trusted, port blocked),
//! reusing the Slice 1 RFC 5530 -> [`ProviderError`] map for IMAP failures.
//!
//! This module owns the WIRE work; the tauri commands in `crate::lib` own the
//! keychain write, the `imap_account_settings` row and the account adoption.
//!
//! ## Slice boundary
//!
//! The SMTP dry-run here is a minimal, self-contained `EHLO` + `AUTH LOGIN`
//! probe over the same pinning-TLS path as IMAP, NOT a `lettre` client. The
//! design recommends `lettre` for actual sending (Slice 4), but lettre cannot
//! take our custom pinning verifier (it needs the root-store route), so wiring
//! it in only to authenticate would add a dependency AND a second TLS trust
//! path this slice would have to reconcile. The dry-run proves the credential
//! and the TLS pin against the submission server, which is all setup needs;
//! real submission (and `lettre`) is Slice 4.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::provider::imap::connection::{self, ConnectionConfig};
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::settings::SecurityMode;
use crate::provider::imap::tls::{PinnedCertVerifier, Sha256Fingerprint};
use crate::provider::ProviderError;

/// How long the SMTP dry-run may take overall. Kept short; it is a probe.
const SMTP_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What the "test" step reports back when it succeeds: the server's IMAP
/// capabilities, whether INBOX allows custom keywords (so the UI can offer
/// keyword labels), and whether a `\*`-less server means label folders are
/// needed instead.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub imap_capabilities: Vec<String>,
    pub mailbox_count: usize,
    /// INBOX `PERMANENTFLAGS` carried `\*` — keyword labels are storable.
    pub supports_keywords: bool,
    pub smtp_ok: bool,
}

/// Translate a wire error into a short, plain-language sentence for the setup
/// UI. The RFC 5530 classification already happened in Slice 1's error map;
/// this turns the resulting [`ProviderError`] into user-facing guidance.
pub fn plain_language(error: &ProviderError) -> String {
    match error {
        ProviderError::ReauthenticationRequired(_) | ProviderError::Authentication(_) => {
            "The username or password was rejected. If this account uses two-factor \
             authentication, you may need an app-specific password."
                .to_string()
        }
        ProviderError::TransientTransport(message) => {
            format!(
                "Could not reach the server. Check the host, port, and that the port is not \
                 blocked. ({message})"
            )
        }
        ProviderError::PermanentClientRejection(message) => message.clone(),
        ProviderError::InvalidOperation(message) => {
            format!("The server rejected the request: {message}")
        }
        other => format!("The connection test failed: {other}"),
    }
}

/// Probe the IMAP server's certificate without logging in, for the cert-trust
/// step. Thin pass-through to the connection layer so the command module has
/// one import surface.
pub async fn probe_imap_certificate(
    host: &str,
    port: u16,
    security: SecurityMode,
) -> Result<connection::CertificateProbe, ProviderError> {
    let config = ConnectionConfig {
        host: host.to_string(),
        port,
        tls_mode: security.tls_mode(),
        pinned_fingerprint: None,
    };
    connection::probe_certificate(&config).await
}

/// Run the IMAP half of "test and save": connect with the (optionally pinned)
/// settings, LOGIN, read CAPABILITY, SELECT INBOX for its PERMANENTFLAGS, and
/// LIST mailboxes. Returns the parts of a [`TestReport`] the IMAP side owns.
pub async fn test_imap(
    host: &str,
    port: u16,
    security: SecurityMode,
    pinned: Option<Sha256Fingerprint>,
    username: &str,
    password: &str,
) -> Result<(Vec<String>, usize, bool), ProviderError> {
    let config = ConnectionConfig {
        host: host.to_string(),
        port,
        tls_mode: security.tls_mode(),
        pinned_fingerprint: pinned,
    };
    let mut session = connection::connect(&config, username, password).await?;
    let capabilities = session.capabilities().await?;
    let inbox = session.select("INBOX").await?;
    let mailboxes = session.list_mailboxes().await?;
    let _ = session.logout().await;
    Ok((capabilities, mailboxes.len(), inbox.permanent_keywords))
}

/// Run an SMTP `EHLO` + `AUTH LOGIN` dry-run over the same pinning-TLS path as
/// IMAP, authenticating WITHOUT sending any mail, then `QUIT`. Returns `Ok(())`
/// when the server accepts the credentials, or a [`ProviderError`] mapped to a
/// plain-language string by the caller.
///
/// Implicit TLS wraps the socket first; STARTTLS reads the greeting, issues
/// `EHLO` + `STARTTLS`, upgrades the SAME stream, then re-`EHLO`s — credentials
/// never cross the wire before TLS, matching the IMAP rule.
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

    let _ = rustls::crypto::ring::default_provider().install_default();
    let verifier = match pinned {
        Some(fp) => PinnedCertVerifier::pinned(fp),
        None => PinnedCertVerifier::webpki_only(),
    };
    let tls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| ProviderError::InvalidOperation(format!("invalid SMTP server name: {e}")))?;
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls_config));

    let transport = |message: String| ProviderError::TransientTransport(message);

    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| transport(format!("SMTP connect failed: {e}")))?;

    // Reach a TLS stream, exactly like IMAP's two modes.
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
            smtp_command(&mut plain, &format!("EHLO {}\r\n", ehlo_name())).await?;
            smtp_command(&mut plain, "STARTTLS\r\n").await?;
            let tls = connector
                .connect(server_name, plain)
                .await
                .map_err(|e| transport(format!("SMTP STARTTLS handshake failed: {e}")))?;
            Box::new(tls)
        }
    };

    // Implicit TLS has not yet read the greeting; STARTTLS already did on the
    // plaintext socket and now re-EHLOs over TLS.
    if matches!(security, SecurityMode::ImplicitTls) {
        read_smtp_reply(&mut *stream, 220).await?;
    }
    smtp_command(&mut *stream, &format!("EHLO {}\r\n", ehlo_name())).await?;

    // AUTH LOGIN: base64 username then password. A rejection here is an
    // authentication failure (SMTP 535), mapped to the same reauth category.
    smtp_command(&mut *stream, "AUTH LOGIN\r\n")
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

    // Dry run: never send mail. QUIT cleanly.
    let _ = write_line(&mut *stream, "QUIT\r\n").await;
    Ok(())
}

/// A conservative EHLO identifier. The hostname is not security-relevant and
/// some servers reject an empty one, so a fixed client name is used.
fn ehlo_name() -> &'static str {
    "threestrands.local"
}

/// Minimal SMTP line write + reply read helpers over any async stream.
trait SmtpStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> SmtpStream for T {}

async fn smtp_command<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + ?Sized>(
    stream: &mut S,
    command: &str,
) -> Result<(), ProviderError> {
    write_line(stream, command).await?;
    read_smtp_reply(stream, 250).await
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
/// its status code equals `expected`. A different code is a transport-level
/// failure the caller re-classifies.
async fn read_smtp_reply<S: tokio::io::AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    expected: u16,
) -> Result<(), ProviderError> {
    let mut byte = [0u8; 1];
    // Read until a line whose 4th char is a space (final line of a reply).
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

/// Pick a sensible default port for a security mode when the user leaves it
/// blank. IMAP: 993 implicit / 143 STARTTLS. SMTP: 465 implicit / 587 STARTTLS.
pub fn default_imap_port(security: SecurityMode) -> u16 {
    match security {
        SecurityMode::ImplicitTls => 993,
        SecurityMode::StartTls => 143,
    }
}

pub fn default_smtp_port(security: SecurityMode) -> u16 {
    match security {
        SecurityMode::ImplicitTls => 465,
        SecurityMode::StartTls => 587,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_language_maps_each_error_category_to_guidance() {
        assert!(plain_language(&ProviderError::ReauthenticationRequired("x".into()))
            .contains("password was rejected"));
        assert!(plain_language(&ProviderError::TransientTransport("refused".into()))
            .contains("Could not reach the server"));
        assert!(plain_language(&ProviderError::PermanentClientRejection("over quota".into()))
            .contains("over quota"));
        assert!(plain_language(&ProviderError::InvalidOperation("bad".into()))
            .contains("rejected the request"));
    }

    #[test]
    fn default_ports_follow_the_security_mode() {
        assert_eq!(default_imap_port(SecurityMode::ImplicitTls), 993);
        assert_eq!(default_imap_port(SecurityMode::StartTls), 143);
        assert_eq!(default_smtp_port(SecurityMode::ImplicitTls), 465);
        assert_eq!(default_smtp_port(SecurityMode::StartTls), 587);
    }

    // The SMTP reply reader must handle multi-line continuations and only act
    // on the final line's status code.
    #[tokio::test]
    async fn smtp_reply_reader_handles_multiline_and_checks_the_final_code() {
        use std::io::Cursor;
        // A 250 multi-line EHLO response.
        let mut ok = Cursor::new(b"250-mail.example.com\r\n250-SIZE 1024\r\n250 AUTH LOGIN\r\n".to_vec());
        assert!(read_smtp_reply(&mut ok, 250).await.is_ok());
        // A 535 auth failure where 235 was expected.
        let mut rejected = Cursor::new(b"535 5.7.8 bad credentials\r\n".to_vec());
        assert!(read_smtp_reply(&mut rejected, 235).await.is_err());
        // A dropped connection mid-reply is a transport error.
        let mut truncated = Cursor::new(b"25".to_vec());
        assert!(read_smtp_reply(&mut truncated, 250).await.is_err());
    }

    // Optional live coverage of the IMAP half of "test and save" against the
    // Dovecot test container. GATED on `DOVECOT_TEST_FP` so the default
    // `cargo test` never needs docker, exactly like Slice 1's live tests:
    //   scripts/dovecot-test-server.sh up
    //   DOVECOT_TEST_FP="$(scripts/dovecot-test-server.sh fingerprint)" \
    //     cargo test --lib provider::imap::setup::tests::live_ -- --nocapture
    // Proves the whole real IMAP setup path end to end: STARTTLS over a stream
    // we upgrade, the pin accepting the live self-signed cert, LOGIN over TLS,
    // CAPABILITY, SELECT INBOX, and LIST.
    #[tokio::test]
    async fn live_test_imap_against_dovecot() {
        let Ok(fp_str) = std::env::var("DOVECOT_TEST_FP") else {
            eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
            return;
        };
        let fp = crate::provider::imap::tls::parse_sha256_fingerprint(&fp_str)
            .expect("DOVECOT_TEST_FP must be a 64-hex-digit SHA-256 fingerprint");
        let (capabilities, mailbox_count, _supports_keywords) = test_imap(
            "127.0.0.1",
            11143,
            SecurityMode::StartTls,
            Some(fp),
            "test@threestrands.test",
            "testpassword",
        )
        .await
        .expect("the live IMAP test-and-save probe should succeed");
        assert!(
            capabilities.iter().any(|c| c.contains("IMAP4REV1")),
            "expected IMAP4rev1 in capabilities, got {capabilities:?}"
        );
        // At least INBOX is listed.
        assert!(mailbox_count >= 1, "expected at least one mailbox");
    }

    // A wrong pin must be rejected against the live server too — the pin is
    // wired into the real handshake, not just the unit verifier.
    #[tokio::test]
    async fn live_test_imap_rejects_a_wrong_pin() {
        if std::env::var("DOVECOT_TEST_FP").is_err() {
            eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
            return;
        }
        let wrong = Sha256Fingerprint::from_bytes([0xCD; 32]);
        let result = test_imap(
            "127.0.0.1",
            11143,
            SecurityMode::StartTls,
            Some(wrong),
            "test@threestrands.test",
            "testpassword",
        )
        .await;
        assert!(
            matches!(result, Err(ProviderError::TransientTransport(_))),
            "a wrong pin must fail the handshake as transport, got {result:?}"
        );
    }
}
