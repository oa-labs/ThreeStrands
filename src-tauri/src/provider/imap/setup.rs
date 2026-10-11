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
//! This module owns the IMAP WIRE work; the tauri commands in `crate::lib`
//! own the keychain write, the `imap_account_settings` row and the account
//! adoption.
//!
//! ## SMTP lives in `smtp.rs`
//!
//! The SMTP dry-run (`EHLO` + `AUTH LOGIN`, authenticating WITHOUT sending any
//! mail over the same pinning-TLS path as IMAP) and the SMTP certificate probe
//! used to live here. As of Phase 4 run 1 they moved to the sibling
//! [`smtp`](crate::provider::imap::smtp) module, next to the real submission
//! client, so both the dry-run and the client share one pinned-TLS connect
//! helper. `test_smtp`, `probe_smtp_certificate` and `default_smtp_port` are
//! re-exported from this module's public surface (see `mod.rs`) so no caller
//! changed, and their observable behaviour is identical.

use crate::provider::imap::connection::{self, ConnectionConfig};
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::settings::SecurityMode;
use crate::provider::imap::tls::Sha256Fingerprint;
use crate::provider::ProviderError;

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

/// Open an authenticated IMAP session for an account that is ALREADY set up,
/// using its stored, pinned settings and a password read from the keychain by
/// the caller. Slice 3's mailbox-discovery and confirmed-mapping commands use
/// this to reconnect without re-running the whole setup flow; it reuses Slice
/// 1's connection layer, so there is no plaintext path and the pin is enforced.
pub async fn connect_with_settings(
    settings: &crate::provider::imap::ImapAccountSettings,
    password: &str,
) -> Result<connection::ConnectedSession, ProviderError> {
    let pinned = settings.pinned_for(&settings.imap_host, settings.imap_port);
    let config = ConnectionConfig {
        host: settings.imap_host.clone(),
        port: settings.imap_port,
        tls_mode: settings.imap_security.tls_mode(),
        pinned_fingerprint: pinned,
    };
    connection::connect(&config, &settings.imap_username, password).await
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

/// Pick a sensible default IMAP port for a security mode when the user leaves
/// it blank: 993 implicit / 143 STARTTLS. (The SMTP default lives in
/// [`smtp::default_smtp_port`](crate::provider::imap::smtp::default_smtp_port).)
pub fn default_imap_port(security: SecurityMode) -> u16 {
    match security {
        SecurityMode::ImplicitTls => 993,
        SecurityMode::StartTls => 143,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::tls::Sha256Fingerprint;

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
    fn default_imap_ports_follow_the_security_mode() {
        assert_eq!(default_imap_port(SecurityMode::ImplicitTls), 993);
        assert_eq!(default_imap_port(SecurityMode::StartTls), 143);
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
