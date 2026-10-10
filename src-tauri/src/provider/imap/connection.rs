//! IMAP connection establishment and the per-account connection policy.
//!
//! `docs/imap-design.md` ("Recommended stack", "Account setup", "Sync"):
//!
//! * TLS is always on. We support BOTH **implicit TLS** (port 993: the socket
//!   is TLS from the first byte) and **STARTTLS** (any port, e.g. Bridge's
//!   1143: connect in the clear, issue `STARTTLS`, then upgrade the SAME
//!   stream). There is NO plaintext fallback — [`connect`] never issues
//!   `LOGIN` before the stream is TLS, so credentials can never cross the wire
//!   in the clear. A server configured for STARTTLS that fails to offer it is
//!   a hard error, never a downgrade.
//! * Certificates are verified by [`PinnedCertVerifier`](super::tls): the
//!   caller supplies the pinned SHA-256 (Slice 2 will store it); with no pin
//!   the platform trust store is used. Verification is never disabled.
//! * **Connections: at most two per account** — one idling on INBOX, one for
//!   commands — with commands run one at a time. [`ImapConnectionManager`]
//!   enforces that cap with two semaphore permits and a command mutex. ISP
//!   servers often cap connections per user across all the user's devices, so
//!   the client stays well under.
//!
//! The LOGIN step itself and session use ride on [`ImapSession`](super::session).
//! This module stops at "an authenticated session exists"; it does not sync,
//! fetch, or discover mailboxes.

use std::sync::Arc;

use async_imap::Client;
use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use crate::provider::ProviderError;

use super::error::map_imap_error;
use super::session::AsyncImapSession;
use super::tls::{PinnedCertVerifier, Sha256Fingerprint};

/// How the TLS layer is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Implicit TLS from the first byte (classically port 993).
    Implicit,
    /// Connect in the clear, then `STARTTLS`-upgrade the stream before LOGIN
    /// (classically port 143, or Bridge's 1143).
    StartTls,
}

/// Everything [`connect`] needs to reach one account's server.
#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub tls_mode: TlsMode,
    /// The pinned leaf-certificate fingerprint, or `None` to verify against
    /// the platform trust store (a real-CA server). Slice 2 supplies this from
    /// stored account settings; Slice 1 takes it as a parameter.
    pub pinned_fingerprint: Option<Sha256Fingerprint>,
}

/// The concrete authenticated session type this module produces: an
/// [`AsyncImapSession`] over a rustls TLS stream. Both TLS modes converge on
/// the same stream type (implicit wraps the TCP stream directly; STARTTLS
/// upgrades it), so callers get one type regardless of mode.
pub type ConnectedSession = AsyncImapSession<TlsStream<TcpStream>>;

/// Build a rustls `ClientConfig` whose verifier is the pinning verifier for
/// this config. The ring provider is installed process-wide (idempotent).
fn tls_client_config(config: &ConnectionConfig) -> ClientConfig {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let verifier = match config.pinned_fingerprint {
        Some(fp) => PinnedCertVerifier::pinned(fp),
        None => PinnedCertVerifier::webpki_only(),
    };
    ClientConfig::builder()
        .dangerous() // "dangerous" only names the custom-verifier API; the
        // verifier itself NEVER accepts an unverified cert — it pins or defers
        // to the platform store. No `dangerous_accept_invalid_certs`.
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

/// TLS-upgrade a TCP stream with the pinning verifier. Shared by both modes:
/// implicit TLS calls it immediately, STARTTLS after the plaintext `STARTTLS`
/// handshake.
async fn upgrade_to_tls(
    tcp: TcpStream,
    config: &ConnectionConfig,
) -> Result<TlsStream<TcpStream>, ProviderError> {
    let connector = TlsConnector::from(Arc::new(tls_client_config(config)));
    // The pinning verifier pins the certificate, not the name; but rustls
    // still requires a `ServerName`, and in the unpinned (WebPKI) path it is
    // the hostname that is checked, so pass the configured host.
    let server_name = ServerName::try_from(config.host.clone())
        .map_err(|e| ProviderError::InvalidOperation(format!("invalid server name: {e}")))?;
    connector
        .connect(server_name, tcp)
        .await
        .map_err(|io| map_imap_error(&async_imap::error::Error::Io(io)))
}

/// Open a TLS-secured, verified IMAP session and LOGIN. Never sends
/// credentials before TLS: implicit TLS wraps the socket before any IMAP
/// command, STARTTLS upgrades the stream before LOGIN, and there is no
/// plaintext-login branch at all.
pub async fn connect(
    config: &ConnectionConfig,
    username: &str,
    password: &str,
) -> Result<ConnectedSession, ProviderError> {
    let tcp = TcpStream::connect((config.host.as_str(), config.port))
        .await
        .map_err(|io| map_imap_error(&async_imap::error::Error::Io(io)))?;

    let tls = match config.tls_mode {
        TlsMode::Implicit => {
            // Socket is TLS from byte zero: upgrade before reading the greeting.
            upgrade_to_tls(tcp, config).await?
        }
        TlsMode::StartTls => {
            // Plaintext greeting, STARTTLS, then upgrade the SAME stream. We
            // issue NO other command — crucially never LOGIN — before the
            // upgrade.
            let mut client = Client::new(tcp);
            client
                .read_response()
                .await
                .map_err(|io| map_imap_error(&async_imap::error::Error::Io(io)))?
                .ok_or_else(|| map_imap_error(&async_imap::error::Error::ConnectionLost))?;
            client
                .run_command_and_check_ok("STARTTLS", None)
                .await
                .map_err(|e| map_imap_error(&e))?;
            let tcp = client.into_inner();
            upgrade_to_tls(tcp, config).await?
        }
    };

    // Only now, over a verified TLS stream, do we authenticate.
    let client = Client::new(tls);
    let session = client
        .login(username, password)
        .await
        .map_err(|(e, _client)| map_imap_error(&e))?;
    Ok(AsyncImapSession::new(session))
}

/// Per-account connection policy: at most two live connections (one idle, one
/// command), commands serialized one at a time.
///
/// `docs/imap-design.md` ("Sync" / "Connections"). The cap is enforced
/// structurally, not by convention:
///
/// * A [`Semaphore`] with **2 permits** bounds total connections. Acquiring a
///   permit is how a caller reserves one of the account's two slots; dropping
///   it (when the connection closes) returns the slot.
/// * A [`Mutex`] serializes commands so only one command connection runs a
///   command at a time, leaving the second slot free for the long-lived IDLE
///   connection.
///
/// One manager per account. It mints connections through [`connect`] but does
/// not itself hold them open — the later sync slices own connection lifecycle;
/// Slice 1 provides the enforced cap and the typed guards.
pub struct ImapConnectionManager {
    config: ConnectionConfig,
    /// Two permits = the two-connection cap.
    connections: Arc<Semaphore>,
    /// Held while a command runs, so commands serialize.
    command_lock: Arc<Mutex<()>>,
}

/// The two connection roles, so a caller names which slot it is taking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionRole {
    /// The single long-lived connection that idles on INBOX.
    Idle,
    /// The connection that runs commands (one at a time).
    Command,
}

/// A reserved connection slot. Holding this proves one of the account's two
/// permits is taken; dropping it frees the slot. A `Command` reservation also
/// holds the serialize-commands lock for its lifetime.
pub struct ConnectionLease {
    role: ConnectionRole,
    _permit: OwnedSemaphorePermit,
    // Held for a Command lease so only one command runs at a time; `None` for
    // an Idle lease, which does not serialize against commands.
    _command_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl ConnectionLease {
    pub fn role(&self) -> ConnectionRole {
        self.role
    }
}

impl ImapConnectionManager {
    /// At most `MAX_CONNECTIONS` live connections per account.
    pub const MAX_CONNECTIONS: usize = 2;

    pub fn new(config: ConnectionConfig) -> Self {
        Self {
            config,
            connections: Arc::new(Semaphore::new(Self::MAX_CONNECTIONS)),
            command_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    /// Reserve one of the two connection slots for `role`. Awaits a free slot;
    /// a `Command` reservation additionally awaits the command lock so only
    /// one command runs at a time. Returns a lease whose drop frees the slot.
    pub async fn reserve(&self, role: ConnectionRole) -> ConnectionLease {
        // A permit cannot error unless the semaphore is closed, which we never
        // do; `expect` documents that invariant.
        let permit = self
            .connections
            .clone()
            .acquire_owned()
            .await
            .expect("connection semaphore is never closed");
        let command_guard = match role {
            ConnectionRole::Command => Some(self.command_lock.clone().lock_owned().await),
            ConnectionRole::Idle => None,
        };
        ConnectionLease {
            role,
            _permit: permit,
            _command_guard: command_guard,
        }
    }

    /// How many of the two slots are free right now (test/diagnostic view).
    pub fn available_slots(&self) -> usize {
        self.connections.available_permits()
    }

    /// Reserve a slot and open an authenticated session on it. The returned
    /// lease must be held for as long as the session is alive so the slot
    /// stays accounted for. (No live caller in Slice 1 — Slice 2's account
    /// setup is the first to construct a real session.)
    pub async fn connect_leased(
        &self,
        role: ConnectionRole,
        username: &str,
        password: &str,
    ) -> Result<(ConnectedSession, ConnectionLease), ProviderError> {
        let lease = self.reserve(role).await;
        let session = connect(&self.config, username, password).await?;
        Ok((session, lease))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ConnectionConfig {
        ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 11143,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: None,
        }
    }

    #[test]
    fn the_cap_is_two_connections_per_account() {
        assert_eq!(ImapConnectionManager::MAX_CONNECTIONS, 2);
        let mgr = ImapConnectionManager::new(config());
        assert_eq!(mgr.available_slots(), 2);
    }

    #[tokio::test]
    async fn two_leases_exhaust_the_cap_and_a_third_waits() {
        let mgr = ImapConnectionManager::new(config());
        let idle = mgr.reserve(ConnectionRole::Idle).await;
        let cmd = mgr.reserve(ConnectionRole::Command).await;
        assert_eq!(mgr.available_slots(), 0);
        assert_eq!(idle.role(), ConnectionRole::Idle);
        assert_eq!(cmd.role(), ConnectionRole::Command);

        // A third reservation cannot proceed while both slots are held.
        let third = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            mgr.reserve(ConnectionRole::Command),
        )
        .await;
        assert!(third.is_err(), "third connection must block at the 2-cap");

        // Dropping one frees exactly one slot.
        drop(cmd);
        assert_eq!(mgr.available_slots(), 1);
        let _third = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            mgr.reserve(ConnectionRole::Command),
        )
        .await
        .expect("a freed slot lets the next reservation through");
    }

    #[tokio::test]
    async fn commands_serialize_one_at_a_time() {
        let mgr = ImapConnectionManager::new(config());
        // First command lease holds the command lock.
        let first = mgr.reserve(ConnectionRole::Command).await;
        // A second command reservation must wait on the command lock even
        // though a slot is free (only one command runs at a time).
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            mgr.reserve(ConnectionRole::Command),
        )
        .await;
        assert!(second.is_err(), "a second command must wait for the first");
        drop(first);
        // Now it proceeds.
        let _second = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            mgr.reserve(ConnectionRole::Command),
        )
        .await
        .expect("the command lock is released when the first command's lease drops");
    }

    #[tokio::test]
    async fn an_idle_lease_does_not_block_a_command_lease() {
        let mgr = ImapConnectionManager::new(config());
        let _idle = mgr.reserve(ConnectionRole::Idle).await;
        // The command slot is independent of the idle one: a command lease is
        // immediately available alongside the idle connection.
        let _cmd = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            mgr.reserve(ConnectionRole::Command),
        )
        .await
        .expect("the one idle + one command pairing is the intended steady state");
    }

    // Optional live integration coverage against the Dovecot test container.
    //
    // GATED on `DOVECOT_TEST_FP` so the default `cargo test` never needs
    // docker: when the env var is unset the test returns immediately (a clean
    // skip). To run it:
    //   scripts/dovecot-test-server.sh up
    //   DOVECOT_TEST_FP="$(scripts/dovecot-test-server.sh fingerprint)" \
    //     cargo test --lib provider::imap::connection::tests::live_ -- --nocapture
    // The pin is read from the env (never hardcoded; the cert regenerates when
    // the container is recreated). This proves the whole real path end to end:
    // STARTTLS over a stream we upgrade, the pinning verifier accepting the
    // live self-signed cert, LOGIN over TLS, and SELECT INBOX.
    #[tokio::test]
    async fn live_starttls_pin_login_select_against_dovecot() {
        use super::super::session::ImapSession;
        let Ok(fp_str) = std::env::var("DOVECOT_TEST_FP") else {
            eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
            return;
        };
        let fp = super::super::tls::parse_sha256_fingerprint(&fp_str)
            .expect("DOVECOT_TEST_FP must be a 64-hex-digit SHA-256 fingerprint");
        let cfg = ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 11143,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(fp),
        };
        let mgr = ImapConnectionManager::new(cfg);
        let (mut session, _lease) = match mgr
            .connect_leased(ConnectionRole::Command, "test@threestrands.test", "testpassword")
            .await
        {
            Ok(pair) => pair,
            Err(e) => panic!("STARTTLS + pinned TLS + LOGIN should succeed against the live container: {e:?}"),
        };
        let status = session.select("INBOX").await.expect("SELECT INBOX");
        // The container's INBOX has UID counters; just prove we read them.
        assert!(status.uid_validity.is_some(), "INBOX should report UIDVALIDITY");
        let _ = session.logout().await;
    }

    // The wrong pin must be rejected against the LIVE server too, not only in
    // the unit test — proving the pinning verifier is wired into the real
    // handshake. Also gated on the container being present.
    #[tokio::test]
    async fn live_wrong_pin_is_rejected_by_the_real_handshake() {
        if std::env::var("DOVECOT_TEST_FP").is_err() {
            eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
            return;
        }
        let cfg = ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 11143,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(Sha256Fingerprint::from_bytes([0xAB; 32])),
        };
        let err = match connect(&cfg, "test@threestrands.test", "testpassword").await {
            Ok(_) => panic!("a wrong pin must reject the live TLS handshake, never LOGIN"),
            Err(err) => err,
        };
        // The handshake failure surfaces as transport (an IO/TLS error), never
        // as a successful connection.
        assert!(
            matches!(err, ProviderError::TransientTransport(_)),
            "wrong-pin rejection should be a transport error, got {err:?}"
        );
    }
}
