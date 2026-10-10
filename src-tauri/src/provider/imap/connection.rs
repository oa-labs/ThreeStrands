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
//!   enforces that cap with two semaphore permits and one mutex per role. ISP
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

/// The outcome of a setup-time certificate probe: the certificate the server
/// presented, and whether it already chains to a trusted public root (so no
/// pin is needed) or must be explicitly trusted (self-signed, e.g. Bridge).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateProbe {
    pub certificate: super::tls::CertificateInfo,
    /// `true` when the leaf verifies against the platform trust store, so the
    /// account can connect without pinning. `false` for a self-signed server,
    /// which requires the user to review and pin the fingerprint.
    pub trusted_by_platform: bool,
}

/// Probe a server's certificate WITHOUT logging in. Opens the socket, reaches
/// TLS exactly as a real connection would (implicit, or plaintext greeting +
/// STARTTLS), captures the presented leaf through
/// [`CollectingCertVerifier`](super::tls::CollectingCertVerifier), then tears
/// the connection down. No `LOGIN` is ever issued, so no credential is needed
/// or sent — this is the "stop before LOGIN on an untrusted cert" step of
/// `docs/imap-design.md` ("Account setup" step 4). The returned fingerprint is
/// what the user compares against the server's own export before pinning.
pub async fn probe_certificate(
    config: &ConnectionConfig,
) -> Result<CertificateProbe, ProviderError> {
    use super::tls::{CertificateInfo, CollectingCertVerifier};

    let _ = rustls::crypto::ring::default_provider().install_default();
    let verifier = CollectingCertVerifier::new();
    let tls_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();

    let tcp = TcpStream::connect((config.host.as_str(), config.port))
        .await
        .map_err(|io| map_imap_error(&async_imap::error::Error::Io(io)))?;

    let tcp = match config.tls_mode {
        TlsMode::Implicit => tcp,
        TlsMode::StartTls => {
            // Plaintext greeting then STARTTLS, exactly like `connect`, but we
            // never go on to LOGIN.
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
            client.into_inner()
        }
    };

    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = ServerName::try_from(config.host.clone())
        .map_err(|e| ProviderError::InvalidOperation(format!("invalid server name: {e}")))?;
    // The handshake must complete for the leaf to be captured. The collecting
    // verifier accepts it (signature checks still run); we discard the stream
    // immediately afterwards.
    let _tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|io| map_imap_error(&async_imap::error::Error::Io(io)))?;

    let leaf = verifier.collected_leaf().ok_or_else(|| {
        ProviderError::TransientTransport(
            "the server completed TLS without presenting a certificate".into(),
        )
    })?;
    let trusted_by_platform = verifier.trusted_by_platform();
    Ok(CertificateProbe {
        certificate: CertificateInfo::from_leaf(&leaf),
        trusted_by_platform,
    })
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
/// * A [`Mutex`] for each role allows only one command connection and one IDLE
///   connection. A reservation takes its role lock before a global permit,
///   so queued reservations never occupy the other role's slot.
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
    /// Held by the single long-lived IDLE connection.
    idle_lock: Arc<Mutex<()>>,
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
/// permits is taken; dropping it frees the slot. Each reservation also holds
/// its role's exclusive lock for its lifetime.
pub struct ConnectionLease {
    role: ConnectionRole,
    _permit: OwnedSemaphorePermit,
    _role_guard: tokio::sync::OwnedMutexGuard<()>,
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
            idle_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    /// Reserve the single slot for `role`. Await the role lock before taking
    /// a global permit so a queued reservation cannot block the other role.
    /// Returns a lease whose drop frees the slot and the role lock.
    pub async fn reserve(&self, role: ConnectionRole) -> ConnectionLease {
        let role_guard = match role {
            ConnectionRole::Command => self.command_lock.clone().lock_owned().await,
            ConnectionRole::Idle => self.idle_lock.clone().lock_owned().await,
        };
        // A permit cannot error unless the semaphore is closed, which we never
        // do; `expect` documents that invariant.
        let permit = self
            .connections
            .clone()
            .acquire_owned()
            .await
            .expect("connection semaphore is never closed");
        ConnectionLease {
            role,
            _permit: permit,
            _role_guard: role_guard,
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

    #[tokio::test]
    async fn queued_reservations_are_exclusive_per_role_and_leave_the_other_slot_free() {
        for (role, other_role) in [
            (ConnectionRole::Command, ConnectionRole::Idle),
            (ConnectionRole::Idle, ConnectionRole::Command),
        ] {
            let mgr = ImapConnectionManager::new(config());
            let first = mgr.reserve(role).await;
            let queued = mgr.reserve(role);
            tokio::pin!(queued);

            assert!(
                futures::poll!(&mut queued).is_pending(),
                "a second {role:?} must wait"
            );
            assert_eq!(
                mgr.available_slots(),
                1,
                "a queued {role:?} must not take a permit"
            );
            let other = tokio::time::timeout(
                std::time::Duration::from_millis(50),
                mgr.reserve(other_role),
            )
            .await
            .expect("a queued reservation must leave the other role's slot free");
            assert_eq!(mgr.available_slots(), 0);

            drop(first);
            let next = tokio::time::timeout(std::time::Duration::from_millis(50), &mut queued)
                .await
                .expect("dropping a lease releases its role for the queued reservation");
            assert_eq!(next.role(), role);
            assert_eq!(mgr.available_slots(), 0);
            drop(next);
            drop(other);
            assert_eq!(mgr.available_slots(), 2);
        }
    }

    #[tokio::test]
    async fn cancelling_a_queued_reservation_does_not_leak_a_slot_or_role_lock() {
        for role in [ConnectionRole::Command, ConnectionRole::Idle] {
            let mgr = ImapConnectionManager::new(config());
            let first = mgr.reserve(role).await;
            {
                let queued = mgr.reserve(role);
                tokio::pin!(queued);
                assert!(futures::poll!(&mut queued).is_pending());
            }
            assert_eq!(mgr.available_slots(), 1);
            drop(first);
            let replacement =
                tokio::time::timeout(std::time::Duration::from_millis(50), mgr.reserve(role))
                    .await
                    .expect("cancelling a waiter must leave the role available");
            drop(replacement);
            assert_eq!(mgr.available_slots(), 2);
        }
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
            .connect_leased(
                ConnectionRole::Command,
                "test@threestrands.test",
                "testpassword",
            )
            .await
        {
            Ok(pair) => pair,
            Err(e) => panic!(
                "STARTTLS + pinned TLS + LOGIN should succeed against the live container: {e:?}"
            ),
        };
        let status = session.select("INBOX").await.expect("SELECT INBOX");
        // The container's INBOX has UID counters; just prove we read them.
        assert!(
            status.uid_validity.is_some(),
            "INBOX should report UIDVALIDITY"
        );
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
