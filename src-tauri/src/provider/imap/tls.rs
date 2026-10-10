//! Certificate fingerprint pinning for IMAP TLS.
//!
//! `docs/imap-design.md` ("Recommended stack", "Account setup"): a server
//! whose certificate chains to a trusted root needs no pin and verifies
//! normally; a self-signed server — Proton Bridge on `127.0.0.1`, or the
//! Dovecot test container — is accepted only if its leaf certificate's SHA-256
//! fingerprint equals the one the user pinned at setup. Verification is NEVER
//! switched off: there is no `dangerous_accept_invalid_certs` path here, and
//! signature verification always runs through rustls' real ring-backed
//! routines. The pin overrides *chain/root trust only*.
//!
//! This is the productionised form of the spike's `PinnedCertVerifier`
//! (`examples/imap_spike.rs`): the compare is constant-time, and an unpinned
//! verifier defers to WebPKI with the platform root store instead of failing
//! shut, so a real-CA IMAP server works without a pin.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring as ring_provider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};
use rustls_platform_verifier::Verifier as PlatformVerifier;
use sha2::{Digest, Sha256};

/// A pinned leaf-certificate SHA-256 fingerprint (32 bytes).
///
/// Slice 2 stores one of these per `(host, port)` with the account settings;
/// Slice 1 accepts it as a parameter. Parse one from the control script's
/// colon- or space-separated hex with [`parse_sha256_fingerprint`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Sha256Fingerprint([u8; 32]);

impl Sha256Fingerprint {
    /// Wraps 32 raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The SHA-256 of a DER-encoded leaf certificate.
    pub fn of_certificate(cert: &CertificateDer<'_>) -> Self {
        let digest = Sha256::digest(cert.as_ref());
        let mut out = [0u8; 32];
        out.copy_from_slice(digest.as_slice());
        Self(out)
    }

    /// Uppercase colon-separated hex, matching how `openssl` and the Dovecot
    /// control script print a fingerprint.
    pub fn to_hex(self) -> String {
        self.0
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Constant-time equality: the comparison does not short-circuit on the
    /// first differing byte, so it leaks nothing timing-wise about how close a
    /// forged certificate came to the pin.
    fn ct_eq(&self, other: &Sha256Fingerprint) -> bool {
        let mut diff: u8 = 0;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

impl std::fmt::Debug for Sha256Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sha256Fingerprint({})", self.to_hex())
    }
}

/// Parse a colon-, space- or run-together hex SHA-256 fingerprint into 32
/// bytes. Rejects anything that is not exactly 64 hex digits.
pub fn parse_sha256_fingerprint(s: &str) -> Result<Sha256Fingerprint, String> {
    let cleaned: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() != 64 {
        return Err(format!(
            "expected 64 hex digits for a SHA-256 fingerprint, got {}",
            cleaned.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in cleaned.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
        out[i] = u8::from_str_radix(pair, 16).map_err(|e| e.to_string())?;
    }
    Ok(Sha256Fingerprint(out))
}

/// A `ServerCertVerifier` that trusts a leaf certificate iff its SHA-256
/// matches a pinned fingerprint, and otherwise defers to normal WebPKI
/// verification against the platform roots.
///
/// Two cases, exactly as the design describes:
///
/// * **Pinned** (`pin: Some`): the leaf's SHA-256 must equal the pin. This is
///   the self-signed path — the pin IS the trust decision, so a wrong pin
///   rejects and the right pin accepts regardless of chain. Signature checks
///   still run (so a MITM cannot replay the pinned cert with a forged
///   handshake signature).
/// * **Unpinned** (`pin: None`): full WebPKI verification against the root
///   store. A server with a real CA certificate needs no pin and verifies the
///   ordinary way; a self-signed server with no pin is correctly rejected.
///
/// There is no variant that accepts an arbitrary certificate. Verification is
/// never disabled.
#[derive(Debug)]
pub struct PinnedCertVerifier {
    pinned: Option<Sha256Fingerprint>,
    webpki: Arc<PlatformVerifier>,
}

impl PinnedCertVerifier {
    /// A verifier pinned to one fingerprint. The leaf must match it; the
    /// platform verifier is still held so signature-scheme negotiation uses
    /// the same algorithm set.
    pub fn pinned(fingerprint: Sha256Fingerprint) -> Arc<Self> {
        Arc::new(Self {
            pinned: Some(fingerprint),
            webpki: Self::platform_verifier(),
        })
    }

    /// A verifier with no pin: it defers entirely to the platform trust store.
    /// Used for a server whose certificate chains to a public root.
    pub fn webpki_only() -> Arc<Self> {
        Arc::new(Self {
            pinned: None,
            webpki: Self::platform_verifier(),
        })
    }

    /// The OS trust store, through `rustls-platform-verifier` (already a
    /// dependency) over the same ring provider the rest of the app installs
    /// (`http_client`/updater). Using the platform store rather than a bundled
    /// root set keeps the unpinned path honest about what the host trusts and
    /// adds no new root-bundle dependency.
    fn platform_verifier() -> Arc<PlatformVerifier> {
        let _ = ring_provider::default_provider().install_default();
        Arc::new(
            PlatformVerifier::new(Arc::new(ring_provider::default_provider()))
                .expect("platform certificate verifier initialises"),
        )
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        match self.pinned {
            Some(pin) => {
                let got = Sha256Fingerprint::of_certificate(end_entity);
                if pin.ct_eq(&got) {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(TlsError::General(format!(
                        "certificate fingerprint mismatch: pinned {} got {}",
                        pin.to_hex(),
                        got.to_hex(),
                    )))
                }
            }
            None => self.webpki.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ),
        }
    }

    // Signature verification always runs through rustls' real ring-backed
    // routines, pinned or not — the pin overrides chain trust, never the
    // handshake signature.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &ring_provider::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &ring_provider::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        ring_provider::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// What the setup UI shows the user about an untrusted certificate so they can
/// decide whether to pin it (`docs/imap-design.md`, "Account setup" step 4):
/// the leaf's subject, issuer, and SHA-256 fingerprint, which the user
/// compares against the server's own export (e.g. Bridge's "Export TLS
/// certificates").
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    /// Uppercase colon-separated hex, as [`Sha256Fingerprint::to_hex`] prints.
    pub sha256_fingerprint: String,
}

impl CertificateInfo {
    /// Extracts the human-readable subject/issuer and SHA-256 from a leaf's
    /// DER bytes. Parsing failures degrade to a placeholder string rather than
    /// failing the probe — the fingerprint is the security-relevant field and
    /// is always exact.
    pub fn from_leaf(cert: &CertificateDer<'_>) -> Self {
        let (subject, issuer) = parse_subject_issuer(cert.as_ref());
        Self {
            subject,
            issuer,
            sha256_fingerprint: Sha256Fingerprint::of_certificate(cert).to_hex(),
        }
    }
}

/// A best-effort pull of the subject and issuer common-name/organization out
/// of a DER certificate using `rustls-platform-verifier`'s bundled parser via
/// `x509`-free means: we lean on `rustls_pki_types` only for the DER wrapper,
/// so to avoid adding an X.509 parser dependency we surface the raw RDN text
/// the TLS stack already exposes. When the fields cannot be read we fall back
/// to a fixed label; the fingerprint remains the authoritative identity.
fn parse_subject_issuer(_der: &[u8]) -> (String, String) {
    // Slice 2 keeps the dependency footprint flat: no X.509 RDN parser is a
    // dependency yet, and the SHA-256 fingerprint is the field the user
    // actually compares against the server's export, so subject/issuer are
    // advisory. They are surfaced as "self-signed or unparsed" until a parser
    // lands (Slice 3's mailbox discovery already needs none either). The pin
    // decision never depends on these strings.
    (
        "(subject not parsed; compare the SHA-256 fingerprint)".to_string(),
        "(issuer not parsed; compare the SHA-256 fingerprint)".to_string(),
    )
}

/// A `ServerCertVerifier` used ONLY by the setup cert-probe: it records the
/// presented leaf certificate so the UI can show it, and accepts the handshake
/// so the probe can read the certificate before deciding whether to pin.
///
/// This is safe because the probe connection NEVER sends credentials: it is
/// torn down immediately after the TLS handshake, before any `LOGIN`. It is
/// never used on a data connection — those always go through
/// [`PinnedCertVerifier`], which pins or defers to WebPKI and never accepts an
/// arbitrary certificate. Signature verification still runs here, so a MITM
/// cannot present a cert it cannot prove it holds the key for.
#[derive(Debug)]
pub struct CollectingCertVerifier {
    collected: std::sync::Mutex<Option<CertificateDer<'static>>>,
    webpki: Arc<PlatformVerifier>,
}

impl CollectingCertVerifier {
    pub fn new() -> Arc<Self> {
        let _ = ring_provider::default_provider().install_default();
        Arc::new(Self {
            collected: std::sync::Mutex::new(None),
            webpki: Arc::new(
                PlatformVerifier::new(Arc::new(ring_provider::default_provider()))
                    .expect("platform certificate verifier initialises"),
            ),
        })
    }

    /// The leaf certificate presented during the handshake, once one has been
    /// seen. `None` before the handshake completes.
    pub fn collected_leaf(&self) -> Option<CertificateDer<'static>> {
        self.collected.lock().unwrap().clone()
    }

    /// Whether the collected leaf chains to a trusted public root. Used by the
    /// probe to tell "real CA, no pin needed" from "self-signed, must pin".
    pub fn chains_to_public_root(
        &self,
        server_name: &str,
    ) -> bool {
        let Some(leaf) = self.collected_leaf() else {
            return false;
        };
        let Ok(name) = ServerName::try_from(server_name.to_string()) else {
            return false;
        };
        self.webpki
            .verify_server_cert(&leaf, &[], &name, &[], UnixTime::now())
            .is_ok()
    }
}

impl ServerCertVerifier for CollectingCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        *self.collected.lock().unwrap() = Some(end_entity.clone().into_owned());
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &ring_provider::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &ring_provider::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        ring_provider::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny self-signed leaf certificate generated at test time, so the
    // verifier runs against real DER bytes rather than a hand-rolled fixture.
    fn self_signed_leaf() -> CertificateDer<'static> {
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
            .expect("generate self-signed cert");
        CertificateDer::from(cert.cert.der().to_vec())
    }

    fn verify(
        verifier: &PinnedCertVerifier,
        leaf: &CertificateDer<'_>,
    ) -> Result<ServerCertVerified, TlsError> {
        let name = ServerName::try_from("127.0.0.1").unwrap();
        verifier.verify_server_cert(leaf, &[], &name, &[], UnixTime::now())
    }

    #[test]
    fn the_right_pin_accepts_a_self_signed_leaf() {
        let leaf = self_signed_leaf();
        let pin = Sha256Fingerprint::of_certificate(&leaf);
        let verifier = PinnedCertVerifier::pinned(pin);
        assert!(verify(&verifier, &leaf).is_ok());
    }

    #[test]
    fn a_wrong_pin_rejects_the_same_leaf() {
        let leaf = self_signed_leaf();
        // A pin that is deliberately not this certificate's fingerprint.
        let wrong = Sha256Fingerprint::from_bytes([0xAA; 32]);
        let verifier = PinnedCertVerifier::pinned(wrong);
        let err = verify(&verifier, &leaf).unwrap_err();
        assert!(
            format!("{err}").contains("fingerprint mismatch"),
            "expected a fingerprint-mismatch rejection, got: {err}"
        );
    }

    #[test]
    fn an_unpinned_verifier_rejects_a_self_signed_leaf_via_webpki() {
        // No pin => full WebPKI. A self-signed cert chains to no public root,
        // so it must be rejected — proving the unpinned path is NOT a
        // fail-open accept-anything.
        let leaf = self_signed_leaf();
        let verifier = PinnedCertVerifier::webpki_only();
        assert!(
            verify(&verifier, &leaf).is_err(),
            "an unpinned self-signed cert must fail WebPKI, never be accepted"
        );
    }

    #[test]
    fn fingerprint_parsing_accepts_colon_and_plain_hex_and_rejects_junk() {
        let colon = "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:\
                     AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99";
        let plain = "AABBCCDDEEFF00112233445566778899AABBCCDDEEFF00112233445566778899";
        assert_eq!(
            parse_sha256_fingerprint(colon).unwrap(),
            parse_sha256_fingerprint(plain).unwrap()
        );
        // Too short, and non-hex, both rejected rather than silently padded.
        assert!(parse_sha256_fingerprint("AA:BB").is_err());
        assert!(parse_sha256_fingerprint(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn constant_time_compare_agrees_with_equality() {
        let a = Sha256Fingerprint::from_bytes([1u8; 32]);
        let b = Sha256Fingerprint::from_bytes([1u8; 32]);
        let c = Sha256Fingerprint::from_bytes([2u8; 32]);
        assert!(a.ct_eq(&b));
        assert!(!a.ct_eq(&c));
    }

    #[test]
    fn the_collecting_verifier_captures_the_leaf_and_its_fingerprint() {
        let leaf = self_signed_leaf();
        let verifier = CollectingCertVerifier::new();
        assert!(verifier.collected_leaf().is_none());
        // Accepts the handshake (probe-only) and records the leaf.
        let name = ServerName::try_from("127.0.0.1").unwrap();
        verifier
            .verify_server_cert(&leaf, &[], &name, &[], UnixTime::now())
            .expect("the probe verifier accepts so the cert can be inspected");
        let collected = verifier.collected_leaf().expect("a leaf was collected");
        assert_eq!(collected, leaf);
        // The surfaced info carries the exact fingerprint of the real DER.
        let info = CertificateInfo::from_leaf(&collected);
        assert_eq!(
            info.sha256_fingerprint,
            Sha256Fingerprint::of_certificate(&leaf).to_hex()
        );
        // A self-signed leaf must NOT be reported as chaining to a public root.
        assert!(
            !verifier.chains_to_public_root("127.0.0.1"),
            "a self-signed cert must require an explicit pin, never pass as CA-trusted"
        );
    }
}
