//! The IMAP provider's NON-SECRET account settings and their store.
//!
//! `docs/imap-design.md` ("Account setup" / "Save", "Data model"): once an
//! IMAP account is set up, its server coordinates and user choices are
//! persisted so later phases can reconnect and sync without re-running setup.
//! Everything here is non-secret and belongs in the local database and in a
//! settings export: hosts, ports, the security mode, the IMAP/SMTP usernames,
//! mailbox-name overrides, the Archive-folder choice, the user-label storage
//! mode and its container mailbox, the ordered identity list, the pinned
//! leaf-certificate SHA-256 per `host:port`, and `server_saves_sent`.
//!
//! The PASSWORD is deliberately NOT here. It lives only in the OS keychain as
//! [`StoredCredential::ImapPassword`](crate::credentials::StoredCredential),
//! never in the `imap_account_settings` table and never in the transfer
//! export. Keeping the secret out of this type by construction is what lets
//! the whole struct be serialized into a settings export safely.
//!
//! The pinned-fingerprint map feeds the design's cert-trust rule: a
//! self-signed server is trusted only against the exact leaf the user pinned
//! at setup. The TLS verifier ([`PinnedCertVerifier`](super::tls)) refuses
//! any other leaf on a pinned `host:port`, and a pin changes only when the
//! user trusts a reviewed certificate in setup or **Server Settings**, never
//! as a fallback.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::tls::{parse_sha256_fingerprint, Sha256Fingerprint};

/// How a mail connection reaches TLS. Mirrors
/// [`TlsMode`](super::connection::TlsMode) but is the *persisted* spelling,
/// serialized as a stable snake_case string into both the database and the
/// transfer export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    /// Implicit TLS from the first byte (classically port 993 / 465).
    ImplicitTls,
    /// Connect in the clear then `STARTTLS`-upgrade before any credential
    /// (classically port 143 / 587, or Bridge's 1143 / 1025).
    #[serde(alias = "starttls")]
    StartTls,
}

impl SecurityMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImplicitTls => "implicit_tls",
            Self::StartTls => "starttls",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "implicit_tls" => Some(Self::ImplicitTls),
            "starttls" => Some(Self::StartTls),
            _ => None,
        }
    }

    /// The connection layer's [`TlsMode`](super::connection::TlsMode) for this
    /// persisted mode.
    pub fn tls_mode(self) -> super::connection::TlsMode {
        match self {
            Self::ImplicitTls => super::connection::TlsMode::Implicit,
            Self::StartTls => super::connection::TlsMode::StartTls,
        }
    }
}

/// How this account stores ThreeStrands user labels, decided at setup from the
/// server's `PERMANENTFLAGS` (see `docs/imap-design.md`, "User labels").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelStorage {
    /// IMAP keywords — the default when INBOX's `PERMANENTFLAGS` includes `\*`.
    Keywords,
    /// Label folders — copies under a container mailbox the user picks.
    Folders,
    /// No user-label storage available (no `\*` and no container chosen).
    None,
}

impl LabelStorage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keywords => "keywords",
            Self::Folders => "folders",
            Self::None => "none",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "keywords" => Some(Self::Keywords),
            "folders" => Some(Self::Folders),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// One sending identity: an address and an optional display name. The account's
/// ordered identity list (`docs/imap-design.md`, "Identities") is non-secret
/// and travels in the settings export.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// All non-secret settings for one IMAP account. Serialized as-is into the
/// transfer export (see `crate::transfer`), so adding a field here is a
/// transfer-format change that must stay backward compatible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImapAccountSettings {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: SecurityMode,
    pub imap_username: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SecurityMode,
    pub smtp_username: String,
    /// System-mailbox name overrides (e.g. a user-chosen Sent mailbox),
    /// `system label -> mailbox name`. Empty is the common case.
    #[serde(default)]
    pub mailbox_overrides: BTreeMap<String, String>,
    /// The chosen Archive mailbox, or `None` to decide at Slice 3's mapping
    /// screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_mailbox: Option<String>,
    pub label_storage: LabelStorage,
    /// Container mailbox for label-folder mode (`None` otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_container: Option<String>,
    /// Ordered sending identities; the first is the login address.
    #[serde(default)]
    pub identities: Vec<Identity>,
    /// Pinned leaf-certificate SHA-256, keyed by `host:port` (uppercase
    /// colon-separated hex, as [`Sha256Fingerprint::to_hex`] prints it).
    #[serde(default)]
    pub pinned_fingerprints: BTreeMap<String, String>,
    /// Whether the server saves its own copy of sent mail (so we skip
    /// `APPEND`). Detected on first send in Slice 4; stored here now.
    #[serde(default)]
    pub server_saves_sent: bool,
}

/// The `host:port` key used in the pinned-fingerprint map.
pub fn host_port_key(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

impl ImapAccountSettings {
    /// The pin recorded for `host:port`, parsed back to bytes, if any.
    pub fn pinned_for(&self, host: &str, port: u16) -> Option<Sha256Fingerprint> {
        self.pinned_fingerprints
            .get(&host_port_key(host, port))
            .and_then(|hex| parse_sha256_fingerprint(hex).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ImapAccountSettings {
        ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1143,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@proton.me".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_security: SecurityMode::StartTls,
            smtp_username: "me@proton.me".into(),
            mailbox_overrides: BTreeMap::new(),
            archive_mailbox: Some("Archive".into()),
            label_storage: LabelStorage::Folders,
            label_container: Some("Labels".into()),
            identities: vec![Identity {
                address: "me@proton.me".into(),
                display_name: Some("Me".into()),
            }],
            pinned_fingerprints: BTreeMap::new(),
            server_saves_sent: true,
        }
    }

    #[test]
    fn starttls_keeps_its_transfer_spelling_and_accepts_the_legacy_setup_spelling() {
        assert_eq!(
            serde_json::to_string(&SecurityMode::StartTls).unwrap(),
            "\"start_tls\""
        );
        for value in ["\"start_tls\"", "\"starttls\""] {
            assert_eq!(
                serde_json::from_str::<SecurityMode>(value).unwrap(),
                SecurityMode::StartTls
            );
        }
        // Existing database rows retain their established spelling.
        assert_eq!(SecurityMode::StartTls.as_str(), "starttls");
        assert_eq!(SecurityMode::parse("starttls"), Some(SecurityMode::StartTls));
    }

    #[test]
    fn settings_round_trip_through_the_table_without_a_password_column() {
        let database = crate::db::Database::open_memory();
        let load = || database.imap_account_settings("me@proton.me").unwrap();
        assert!(load().is_none(), "no row before save");
        let settings = sample();
        database.save_imap_account_settings("me@proton.me", &settings).unwrap();
        assert_eq!(load().unwrap(), settings);
        // A second save updates in place rather than erroring on the PK.
        let changed = ImapAccountSettings {
            server_saves_sent: false,
            ..settings.clone()
        };
        database.save_imap_account_settings("me@proton.me", &changed).unwrap();
        assert_eq!(load().unwrap(), changed);
    }

    #[test]
    fn settings_are_read_back_only_for_their_own_account() {
        let database = crate::db::Database::open_memory();
        database
            .save_imap_account_settings("other@example.com", &sample())
            .unwrap();
        assert!(database
            .imap_account_settings("me@proton.me")
            .unwrap()
            .is_none());
    }

    #[test]
    fn security_mode_and_label_storage_strings_round_trip() {
        for mode in [SecurityMode::ImplicitTls, SecurityMode::StartTls] {
            assert_eq!(SecurityMode::parse(mode.as_str()), Some(mode));
        }
        for storage in [LabelStorage::Keywords, LabelStorage::Folders, LabelStorage::None] {
            assert_eq!(LabelStorage::parse(storage.as_str()), Some(storage));
        }
    }
}
