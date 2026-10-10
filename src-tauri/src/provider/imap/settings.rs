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
//! The pinned-fingerprint map is the enforcement point for the design's
//! cert-trust rule: a self-signed server is trusted only against the exact
//! leaf the user pinned at setup, and a *changed* certificate on a pinned
//! `host:port` is a hard stop that re-prompts rather than a silent re-pin
//! (see [`ImapSettingsStore::pin_fingerprint`] /
//! [`ImapAccountSettings::fingerprint_decision`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::{Database, DbResult};

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

/// What to do with a certificate presented on connect, given what is pinned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FingerprintDecision {
    /// No pin for this `host:port`: a real-CA server verifies normally; a
    /// self-signed one must be reviewed and pinned at setup.
    Unpinned,
    /// The presented leaf matches the pin — connect.
    Matches,
    /// A DIFFERENT certificate is pinned for this `host:port`. Never a silent
    /// re-pin: pause the account and re-prompt the user to review the change.
    Changed,
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

    /// Decide what to do with the certificate `presented` on `host:port`,
    /// given what (if anything) is pinned. This is the single place the
    /// "never silently re-pin" rule is expressed.
    pub fn fingerprint_decision(
        &self,
        host: &str,
        port: u16,
        presented: &Sha256Fingerprint,
    ) -> FingerprintDecision {
        match self.pinned_for(host, port) {
            None => FingerprintDecision::Unpinned,
            Some(pinned) if &pinned == presented => FingerprintDecision::Matches,
            Some(_) => FingerprintDecision::Changed,
        }
    }
}

/// A narrow, account-scoped handle onto one account's `imap_account_settings`
/// row. Constructed with the account email it serves, so one account can never
/// read another's settings. A thin wrapper over the shared [`Database`].
#[derive(Clone)]
pub struct ImapSettingsStore {
    database: Arc<Database>,
    account_id: String,
}

impl ImapSettingsStore {
    pub fn new(database: Arc<Database>, account_id: impl Into<String>) -> Self {
        Self {
            database,
            account_id: account_id.into(),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Inserts or replaces this account's settings row.
    pub fn save(&self, settings: &ImapAccountSettings) -> DbResult<()> {
        self.database
            .with_connection(|connection| write_settings_row(connection, &self.account_id, settings))
    }

    /// Reads this account's settings, or `None` when it has no IMAP row (a
    /// Gmail account, or an IMAP account imported without having been set up
    /// here yet).
    pub fn load(&self) -> DbResult<Option<ImapAccountSettings>> {
        self.database
            .with_connection(|connection| read_settings_row(connection, &self.account_id))
    }

    /// Records the pin for `host:port`, failing loudly if a DIFFERENT
    /// certificate is already pinned there — the design forbids a silent
    /// re-pin. The caller re-prompts the user, who confirms a deliberate pin
    /// through [`Self::force_repin`].
    pub fn pin_fingerprint(
        &self,
        settings: &mut ImapAccountSettings,
        host: &str,
        port: u16,
        fingerprint: &Sha256Fingerprint,
    ) -> Result<(), String> {
        if let FingerprintDecision::Changed =
            settings.fingerprint_decision(host, port, fingerprint)
        {
            return Err(format!(
                "a different certificate is already pinned for {}; review the change before trusting it",
                host_port_key(host, port)
            ));
        }
        settings
            .pinned_fingerprints
            .insert(host_port_key(host, port), fingerprint.to_hex());
        Ok(())
    }

    /// Deliberately replaces the pin for `host:port` after the user has
    /// reviewed a certificate change. Separate from [`Self::pin_fingerprint`]
    /// so a re-pin is always an explicit, reviewed action, never a fallback.
    pub fn force_repin(
        &self,
        settings: &mut ImapAccountSettings,
        host: &str,
        port: u16,
        fingerprint: &Sha256Fingerprint,
    ) {
        settings
            .pinned_fingerprints
            .insert(host_port_key(host, port), fingerprint.to_hex());
    }
}

/// Writes (insert-or-replace) one account's settings row on a connection,
/// reused by the store's `save` and by the settings-transfer import so the
/// column list lives in exactly one place.
pub(crate) fn write_settings_row(
    connection: &rusqlite::Connection,
    account_id: &str,
    settings: &ImapAccountSettings,
) -> DbResult<()> {
    let mailbox_overrides = serde_json::to_string(&settings.mailbox_overrides)
        .map_err(crate::db::serialization_error)?;
    let identities =
        serde_json::to_string(&settings.identities).map_err(crate::db::serialization_error)?;
    let pins = serde_json::to_string(&settings.pinned_fingerprints)
        .map_err(crate::db::serialization_error)?;
    connection.execute(
        "INSERT INTO imap_account_settings(
             account_id, imap_host, imap_port, imap_security, imap_username,
             smtp_host, smtp_port, smtp_security, smtp_username,
             mailbox_overrides_json, archive_mailbox, label_storage, label_container,
             identities_json, pinned_fingerprints_json, server_saves_sent)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
         ON CONFLICT(account_id) DO UPDATE SET
             imap_host = excluded.imap_host,
             imap_port = excluded.imap_port,
             imap_security = excluded.imap_security,
             imap_username = excluded.imap_username,
             smtp_host = excluded.smtp_host,
             smtp_port = excluded.smtp_port,
             smtp_security = excluded.smtp_security,
             smtp_username = excluded.smtp_username,
             mailbox_overrides_json = excluded.mailbox_overrides_json,
             archive_mailbox = excluded.archive_mailbox,
             label_storage = excluded.label_storage,
             label_container = excluded.label_container,
             identities_json = excluded.identities_json,
             pinned_fingerprints_json = excluded.pinned_fingerprints_json,
             server_saves_sent = excluded.server_saves_sent",
        rusqlite::params![
            account_id,
            settings.imap_host,
            settings.imap_port as i64,
            settings.imap_security.as_str(),
            settings.imap_username,
            settings.smtp_host,
            settings.smtp_port as i64,
            settings.smtp_security.as_str(),
            settings.smtp_username,
            mailbox_overrides,
            settings.archive_mailbox,
            settings.label_storage.as_str(),
            settings.label_container,
            identities,
            pins,
            settings.server_saves_sent as i64,
        ],
    )?;
    Ok(())
}

/// Reads one account's settings row on a connection, or `None` when absent.
pub(crate) fn read_settings_row(
    connection: &rusqlite::Connection,
    account_id: &str,
) -> DbResult<Option<ImapAccountSettings>> {
    let row = connection
        .query_row(
            "SELECT imap_host, imap_port, imap_security, imap_username,
                    smtp_host, smtp_port, smtp_security, smtp_username,
                    mailbox_overrides_json, archive_mailbox, label_storage, label_container,
                    identities_json, pinned_fingerprints_json, server_saves_sent
             FROM imap_account_settings WHERE account_id = ?1",
            [account_id],
            |row| {
                Ok(RawSettingsRow {
                    imap_host: row.get(0)?,
                    imap_port: row.get::<_, i64>(1)?,
                    imap_security: row.get(2)?,
                    imap_username: row.get(3)?,
                    smtp_host: row.get(4)?,
                    smtp_port: row.get::<_, i64>(5)?,
                    smtp_security: row.get(6)?,
                    smtp_username: row.get(7)?,
                    mailbox_overrides_json: row.get(8)?,
                    archive_mailbox: row.get(9)?,
                    label_storage: row.get(10)?,
                    label_container: row.get(11)?,
                    identities_json: row.get(12)?,
                    pinned_fingerprints_json: row.get(13)?,
                    server_saves_sent: row.get::<_, i64>(14)? != 0,
                })
            },
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some(raw) => raw
            .into_settings()
            .map(Some)
            .map_err(crate::db::DatabaseError::corrupt),
    }
}

/// The raw column values before JSON/enum decoding, so the `query_row` closure
/// stays a pure `rusqlite` read and all fallible parsing happens after.
struct RawSettingsRow {
    imap_host: String,
    imap_port: i64,
    imap_security: String,
    imap_username: String,
    smtp_host: String,
    smtp_port: i64,
    smtp_security: String,
    smtp_username: String,
    mailbox_overrides_json: String,
    archive_mailbox: Option<String>,
    label_storage: String,
    label_container: Option<String>,
    identities_json: String,
    pinned_fingerprints_json: String,
    server_saves_sent: bool,
}

impl RawSettingsRow {
    fn into_settings(self) -> Result<ImapAccountSettings, String> {
        let imap_security = SecurityMode::parse(&self.imap_security)
            .ok_or_else(|| format!("unknown IMAP security mode: {}", self.imap_security))?;
        let smtp_security = SecurityMode::parse(&self.smtp_security)
            .ok_or_else(|| format!("unknown SMTP security mode: {}", self.smtp_security))?;
        let label_storage = LabelStorage::parse(&self.label_storage)
            .ok_or_else(|| format!("unknown label storage: {}", self.label_storage))?;
        let imap_port = u16::try_from(self.imap_port)
            .map_err(|_| format!("IMAP port out of range: {}", self.imap_port))?;
        let smtp_port = u16::try_from(self.smtp_port)
            .map_err(|_| format!("SMTP port out of range: {}", self.smtp_port))?;
        let mailbox_overrides =
            serde_json::from_str(&self.mailbox_overrides_json).map_err(|e| e.to_string())?;
        let identities = serde_json::from_str(&self.identities_json).map_err(|e| e.to_string())?;
        let pinned_fingerprints =
            serde_json::from_str(&self.pinned_fingerprints_json).map_err(|e| e.to_string())?;
        Ok(ImapAccountSettings {
            imap_host: self.imap_host,
            imap_port,
            imap_security,
            imap_username: self.imap_username,
            smtp_host: self.smtp_host,
            smtp_port,
            smtp_security,
            smtp_username: self.smtp_username,
            mailbox_overrides,
            archive_mailbox: self.archive_mailbox,
            label_storage,
            label_container: self.label_container,
            identities,
            pinned_fingerprints,
            server_saves_sent: self.server_saves_sent,
        })
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

    fn store() -> ImapSettingsStore {
        let database = Arc::new(Database::open_memory());
        ImapSettingsStore::new(database, "me@proton.me")
    }

    #[test]
    fn settings_round_trip_through_the_table_without_a_password_column() {
        let store = store();
        assert!(store.load().unwrap().is_none(), "no row before save");
        let settings = sample();
        store.save(&settings).unwrap();
        assert_eq!(store.load().unwrap().unwrap(), settings);
        // A second save updates in place rather than erroring on the PK.
        let changed = ImapAccountSettings {
            server_saves_sent: false,
            ..settings.clone()
        };
        store.save(&changed).unwrap();
        assert_eq!(store.load().unwrap().unwrap(), changed);
    }

    #[test]
    fn a_store_only_sees_its_own_accounts_row() {
        let database = Arc::new(Database::open_memory());
        let mine = ImapSettingsStore::new(database.clone(), "me@proton.me");
        let theirs = ImapSettingsStore::new(database, "other@example.com");
        theirs.save(&sample()).unwrap();
        assert!(mine.load().unwrap().is_none());
    }

    #[test]
    fn an_unpinned_host_reports_unpinned_then_matches_once_pinned() {
        let store = store();
        let mut settings = sample();
        let fp = Sha256Fingerprint::from_bytes([0x11; 32]);
        assert_eq!(
            settings.fingerprint_decision("127.0.0.1", 1143, &fp),
            FingerprintDecision::Unpinned
        );
        store
            .pin_fingerprint(&mut settings, "127.0.0.1", 1143, &fp)
            .unwrap();
        assert_eq!(
            settings.fingerprint_decision("127.0.0.1", 1143, &fp),
            FingerprintDecision::Matches
        );
    }

    #[test]
    fn a_changed_certificate_is_a_hard_stop_not_a_silent_repin() {
        let store = store();
        let mut settings = sample();
        let original = Sha256Fingerprint::from_bytes([0x11; 32]);
        let rotated = Sha256Fingerprint::from_bytes([0x22; 32]);
        store
            .pin_fingerprint(&mut settings, "127.0.0.1", 1143, &original)
            .unwrap();
        // The rotated cert is reported as a change...
        assert_eq!(
            settings.fingerprint_decision("127.0.0.1", 1143, &rotated),
            FingerprintDecision::Changed
        );
        // ...and pin_fingerprint refuses to overwrite it silently.
        let err = store
            .pin_fingerprint(&mut settings, "127.0.0.1", 1143, &rotated)
            .unwrap_err();
        assert!(err.contains("already pinned"), "{err}");
        // The pin is untouched by the refused attempt.
        assert_eq!(settings.pinned_for("127.0.0.1", 1143), Some(original));
        // Only an explicit, reviewed re-pin replaces it.
        store.force_repin(&mut settings, "127.0.0.1", 1143, &rotated);
        assert_eq!(settings.pinned_for("127.0.0.1", 1143), Some(rotated));
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
