//! Typed configuration, secrets, and per-connector operations for every
//! replicated-sync transport kind. One place that knows how a
//! `sync_transports` row's `(kind, config_json)` maps to a live adapter, so
//! the engine, the Settings status, and connector removal never match on a
//! kind string themselves.
//!
//! Adding a connector kind means adding a variant to [`TransportConfig`],
//! [`TransportSecrets`] (if it has a secret), and [`Connector`] — nothing
//! else in `replicated_sync.rs` or the commands needs to change.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use threestrands_sync_transport::SyncTransport;

use crate::ipfs_transport::IpfsRpcTransport;
use crate::s3_transport::{S3Config, S3Credentials, S3Transport};
use crate::sync_folder::SyncFolderTransport;

pub(crate) const FOLDER_KIND: &str = "folder";
pub(crate) const IPFS_RPC_KIND: &str = "ipfs_rpc";
pub(crate) const S3_KIND: &str = "s3";

/// Whether this app version knows how to handle connectors of `kind`.
pub(crate) fn is_known_kind(kind: &str) -> bool {
    matches!(kind, FOLDER_KIND | IPFS_RPC_KIND | S3_KIND)
}

// ================================ Configuration ==============================

/// A folder connector's persisted config. `path` stays a `String` (written
/// with `to_string_lossy`) exactly as rows have always stored it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderConfig {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// An IPFS RPC connector's persisted config. The access token is never
/// here; see [`TransportSecrets::IpfsRpcToken`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpfsRpcConfig {
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The non-secret configuration of one connector, as persisted in
/// `sync_transports(kind, config_json)`. Older app versions read these
/// same JSON shapes and ignore the optional `label`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportConfig {
    Folder(FolderConfig),
    IpfsRpc(IpfsRpcConfig),
    S3(S3Config),
}

impl TransportConfig {
    /// Parses a persisted row. `None` for an unknown kind or malformed
    /// JSON — such a row is skipped, never a panic.
    pub fn from_row(kind: &str, config_json: &str) -> Option<Self> {
        match kind {
            FOLDER_KIND => serde_json::from_str(config_json).ok().map(Self::Folder),
            IPFS_RPC_KIND => serde_json::from_str(config_json).ok().map(Self::IpfsRpc),
            S3_KIND => serde_json::from_str(config_json).ok().map(Self::S3),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Folder(_) => FOLDER_KIND,
            Self::IpfsRpc(_) => IPFS_RPC_KIND,
            Self::S3(_) => S3_KIND,
        }
    }

    pub fn to_config_json(&self) -> Result<String, String> {
        match self {
            Self::Folder(config) => serde_json::to_string(config),
            Self::IpfsRpc(config) => serde_json::to_string(config),
            Self::S3(config) => serde_json::to_string(config),
        }
        .map_err(|error| error.to_string())
    }

    /// The user-chosen name, if any.
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Folder(config) => config.label.as_deref(),
            Self::IpfsRpc(config) => config.label.as_deref(),
            Self::S3(config) => config.label.as_deref(),
        }
        .filter(|label| !label.trim().is_empty())
    }

    /// Sets (or, with `None` or a blank string, clears) the name.
    // Reached from Settings once the connector commands land.
    #[allow(dead_code)]
    pub fn set_label(&mut self, label: Option<&str>) {
        let label = label.map(str::trim).filter(|label| !label.is_empty()).map(str::to_string);
        match self {
            Self::Folder(config) => config.label = label,
            Self::IpfsRpc(config) => config.label = label,
            Self::S3(config) => config.label = label,
        }
    }

    /// Where this connector points, for display — never a credential.
    pub fn location(&self) -> String {
        match self {
            Self::Folder(config) => std::path::Path::new(&config.path).display().to_string(),
            Self::IpfsRpc(config) => config.base_url.clone(),
            Self::S3(config) => config.display_location(),
        }
    }

    /// Whether "delete files and disconnect" can remove this connector's
    /// synchronized data. An IPFS pin set can't be reliably unpinned for
    /// every object, so pinned data stays with the provider.
    pub fn supports_delete_data(&self) -> bool {
        matches!(self, Self::Folder(_) | Self::S3(_))
    }

    /// Checks that `secrets` is the right kind for this connector (or
    /// absent where the connector has none, or where it's optional).
    fn check_secrets(&self, secrets: Option<&TransportSecrets>) -> Result<(), String> {
        match (self, secrets) {
            (Self::Folder(_), None)
            | (Self::IpfsRpc(_), None | Some(TransportSecrets::IpfsRpcToken(_)))
            | (Self::S3(_), Some(TransportSecrets::S3(_))) => Ok(()),
            (Self::S3(_), None) => Err("S3 storage needs an access key and secret".to_string()),
            _ => Err("Those credentials don't belong to this kind of connector".to_string()),
        }
    }

    /// Validates the config (and its secrets, where the adapter checks them
    /// at construction) before anything is persisted. The folder and IPFS
    /// kinds are checked when the live adapter opens, as they always have
    /// been; only S3 is rejected up front.
    pub fn validate(&self, instance_id: &str, secrets: Option<&TransportSecrets>) -> Result<(), String> {
        self.check_secrets(secrets)?;
        if let (Self::S3(config), Some(TransportSecrets::S3(credentials))) = (self, secrets) {
            S3Transport::new(instance_id, config, credentials).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

// ================================== Secrets ==================================

/// A connector's secret half. Stored only in the OS keychain, under a
/// per-kind service and the instance id; never in SQLite, never exported.
#[derive(Clone, PartialEq, Eq)]
pub enum TransportSecrets {
    IpfsRpcToken(String),
    S3(S3Credentials),
}

impl std::fmt::Debug for TransportSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IpfsRpcToken(_) => f.write_str("IpfsRpcToken(<redacted>)"),
            Self::S3(credentials) => write!(f, "S3({credentials:?})"),
        }
    }
}

const IPFS_TOKEN_KEYCHAIN_SERVICE: &str = "app.threestrands.replicated-sync.ipfs-rpc";
const S3_CREDENTIALS_KEYCHAIN_SERVICE: &str = "app.threestrands.replicated-sync.s3";

/// The keychain service holding secrets for `kind`, if that kind has any.
fn secret_service(kind: &str) -> Option<&'static str> {
    match kind {
        IPFS_RPC_KIND => Some(IPFS_TOKEN_KEYCHAIN_SERVICE),
        S3_KIND => Some(S3_CREDENTIALS_KEYCHAIN_SERVICE),
        _ => None,
    }
}

impl TransportSecrets {
    fn kind(&self) -> &'static str {
        match self {
            Self::IpfsRpcToken(_) => IPFS_RPC_KIND,
            Self::S3(_) => S3_KIND,
        }
    }

    pub fn store(&self, instance_id: &str) -> Result<(), String> {
        let service = secret_service(self.kind()).expect("every secret kind has a keychain service");
        let value = match self {
            Self::IpfsRpcToken(token) => token.clone(),
            Self::S3(credentials) => serde_json::to_string(credentials).map_err(|error| error.to_string())?,
        };
        secret_store::set(service, instance_id, &value)
    }

    /// Loads the stored secret for a connector of `kind`. `Ok(None)` when
    /// nothing is stored or the kind has no secret.
    pub fn load(kind: &str, instance_id: &str) -> Result<Option<Self>, String> {
        let Some(service) = secret_service(kind) else { return Ok(None) };
        let Some(value) = secret_store::get(service, instance_id)? else { return Ok(None) };
        match kind {
            IPFS_RPC_KIND => Ok(Some(Self::IpfsRpcToken(value))),
            // A malformed keychain value is reported without echoing it.
            S3_KIND => serde_json::from_str(&value)
                .map(|credentials| Some(Self::S3(credentials)))
                .map_err(|_| "The stored S3 credentials are unreadable; replace them in Settings".to_string()),
            _ => Ok(None),
        }
    }

    /// Deletes whatever secret a connector of `kind` has; a no-op when
    /// nothing is stored.
    pub fn delete(kind: &str, instance_id: &str) -> Result<(), String> {
        match secret_service(kind) {
            Some(service) => secret_store::delete(service, instance_id),
            None => Ok(()),
        }
    }

    /// Deletes this instance's secret under every kind — for a row whose
    /// kind is unknown or already gone.
    pub fn delete_every_kind(instance_id: &str) -> Result<(), String> {
        for kind in [IPFS_RPC_KIND, S3_KIND] {
            Self::delete(kind, instance_id)?;
        }
        Ok(())
    }
}

/// The OS keychain in the app; a process-wide in-memory map in tests, so
/// no test ever reads or writes the developer's real keychain.
#[cfg(not(test))]
mod secret_store {
    use keyring::Entry;

    fn display(error: impl std::fmt::Display) -> String {
        error.to_string()
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        match Entry::new(service, account).map_err(display)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(display(error)),
        }
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        Entry::new(service, account).map_err(display)?.set_password(value).map_err(display)
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        match Entry::new(service, account).map_err(display)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(display(error)),
        }
    }
}

#[cfg(test)]
pub(crate) mod secret_store {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    fn map() -> &'static Mutex<HashMap<(String, String), String>> {
        static MAP: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
        MAP.get_or_init(Default::default)
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        Ok(map().lock().unwrap().get(&(service.to_string(), account.to_string())).cloned())
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
        map().lock().unwrap().insert((service.to_string(), account.to_string()), value.to_string());
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        map().lock().unwrap().remove(&(service.to_string(), account.to_string()));
        Ok(())
    }

    /// Every stored value, for tests asserting where a secret did or
    /// didn't land.
    pub fn values_for(account: &str) -> Vec<String> {
        map()
            .lock()
            .unwrap()
            .iter()
            .filter(|((_, stored_account), _)| stored_account == account)
            .map(|(_, value)| value.clone())
            .collect()
    }
}

// ================================= Connector =================================

/// What "Test connection" reports, per kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorProbe {
    /// A folder has nothing to probe beyond opening it.
    Folder,
    IpfsRpc(crate::ipfs_transport::ProbeReport),
    S3(crate::s3_transport::S3ProbeReport),
}

/// One live connector, keeping its concrete adapter so the operations the
/// provider-neutral [`SyncTransport`] trait deliberately lacks (storage
/// estimate, delete-all, probe) are available without matching on a kind
/// string.
pub enum Connector {
    Folder(SyncFolderTransport),
    IpfsRpc(IpfsRpcTransport),
    S3(S3Transport),
}

impl Connector {
    /// Opens the live adapter for `config`, with `secrets` loaded by the
    /// caller. `Err` when the folder can't be opened, the endpoint is
    /// invalid, or a required secret is missing.
    pub async fn open(
        instance_id: &str,
        config: &TransportConfig,
        secrets: Option<TransportSecrets>,
    ) -> Result<Self, String> {
        match config {
            TransportConfig::Folder(folder) => SyncFolderTransport::open(instance_id, std::path::Path::new(&folder.path))
                .await
                .map(Self::Folder)
                .map_err(|error| error.to_string()),
            TransportConfig::IpfsRpc(ipfs) => {
                let token = match secrets {
                    Some(TransportSecrets::IpfsRpcToken(token)) => Some(token),
                    _ => None,
                };
                IpfsRpcTransport::new(instance_id, &ipfs.base_url, token, crate::replicated_sync::SPACE_ID.as_bytes())
                    .map(Self::IpfsRpc)
                    .map_err(|error| error.to_string())
            }
            TransportConfig::S3(s3) => match secrets {
                Some(TransportSecrets::S3(credentials)) => {
                    S3Transport::new(instance_id, s3, &credentials).map(Self::S3).map_err(|error| error.to_string())
                }
                _ => Err("No S3 credentials are stored for this connector".to_string()),
            },
        }
    }

    /// Opens a persisted connector, loading its secrets from the keychain.
    /// An unreadable IPFS token is treated as absent, as it always has
    /// been; missing S3 credentials fail the open.
    pub async fn open_persisted(instance_id: &str, config: &TransportConfig) -> Result<Self, String> {
        let secrets = TransportSecrets::load(config.kind(), instance_id);
        let secrets = match config {
            TransportConfig::IpfsRpc(_) => secrets.ok().flatten(),
            _ => secrets?,
        };
        Self::open(instance_id, config, secrets).await
    }

    pub fn transport(&self) -> &dyn SyncTransport {
        match self {
            Self::Folder(transport) => transport,
            Self::IpfsRpc(transport) => transport,
            Self::S3(transport) => transport,
        }
    }

    pub fn into_transport(self) -> Arc<dyn SyncTransport> {
        match self {
            Self::Folder(transport) => Arc::new(transport),
            Self::IpfsRpc(transport) => Arc::new(transport),
            Self::S3(transport) => Arc::new(transport),
        }
    }

    /// Bytes stored in this connector's corpus, where the storage can say.
    pub async fn corpus_size_bytes(&self) -> Option<u64> {
        match self {
            Self::Folder(folder) => folder.corpus_size_bytes().await.ok(),
            Self::S3(s3) => s3.corpus_size_bytes().await.ok(),
            Self::IpfsRpc(_) => None,
        }
    }

    /// Removes this connector's synchronized data from its storage. Only
    /// the kinds [`TransportConfig::supports_delete_data`] names support it.
    pub async fn delete_all_corpus_data(&self) -> Result<(), String> {
        match self {
            Self::Folder(folder) => folder.delete_all_corpus_data().await.map_err(|error| error.to_string()),
            Self::S3(s3) => s3.delete_all_corpus_data().await.map_err(|error| error.to_string()),
            Self::IpfsRpc(_) => Err("Pinned IPFS data stays with the provider; remove it there".to_string()),
        }
    }

    pub async fn probe(&self) -> Result<ConnectorProbe, String> {
        match self {
            Self::Folder(_) => Ok(ConnectorProbe::Folder),
            Self::IpfsRpc(ipfs) => ipfs
                .probe_capabilities()
                .await
                .map(ConnectorProbe::IpfsRpc)
                .map_err(|error| error.to_string()),
            Self::S3(s3) => Ok(ConnectorProbe::S3(s3.probe().await)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_config() -> S3Config {
        S3Config {
            endpoint: "https://s3.us-east-1.amazonaws.com".to_string(),
            region: "us-east-1".to_string(),
            bucket: "sync-bucket".to_string(),
            prefix: String::new(),
            path_style: false,
            label: None,
        }
    }

    fn s3_credentials() -> S3Credentials {
        S3Credentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "s3-secret-value".to_string(),
            session_token: None,
        }
    }

    #[test]
    fn rows_written_before_typed_configs_still_parse() {
        // The exact JSON the pre-refactor `add_*_transport` functions wrote.
        let folder = TransportConfig::from_row("folder", r#"{"path":"/Users/me/Dropbox/Sync"}"#).unwrap();
        assert_eq!(folder, TransportConfig::Folder(FolderConfig { path: "/Users/me/Dropbox/Sync".to_string(), label: None }));
        let ipfs = TransportConfig::from_row("ipfs_rpc", r#"{"baseUrl":"https://rpc.filebase.io"}"#).unwrap();
        assert_eq!(ipfs.location(), "https://rpc.filebase.io");
        let s3 = TransportConfig::from_row(
            "s3",
            r#"{"endpoint":"https://s3.example.com","region":"auto","bucket":"b-1","prefix":"p","pathStyle":true}"#,
        )
        .unwrap();
        assert_eq!(s3.kind(), "s3");
        assert_eq!(s3.location(), "https://s3.example.com · b-1/p");
    }

    #[test]
    fn unknown_kinds_and_malformed_json_yield_none() {
        assert_eq!(TransportConfig::from_row("carrier-pigeon", r#"{"path":"/x"}"#), None);
        assert_eq!(TransportConfig::from_row("folder", "not json"), None);
        assert_eq!(TransportConfig::from_row("folder", r#"{"baseUrl":"https://x"}"#), None);
        assert_eq!(TransportConfig::from_row("s3", r#"{"endpoint":"https://x"}"#), None);
    }

    #[test]
    fn every_kind_round_trips_through_its_row_with_a_label() {
        let mut configs = vec![
            TransportConfig::Folder(FolderConfig { path: "/tmp/sync".to_string(), label: None }),
            TransportConfig::IpfsRpc(IpfsRpcConfig { base_url: "https://rpc.filebase.io".to_string(), label: None }),
            TransportConfig::S3(s3_config()),
        ];
        for config in &mut configs {
            config.set_label(Some("  Work  "));
            assert_eq!(config.label(), Some("Work"));
            let json = config.to_config_json().unwrap();
            assert_eq!(TransportConfig::from_row(config.kind(), &json).as_ref(), Some(&*config));
            config.set_label(Some("   "));
            assert_eq!(config.label(), None);
            assert!(!config.to_config_json().unwrap().contains("label"));
        }
    }

    #[test]
    fn a_labelled_folder_row_stays_readable_by_the_old_path_lookup() {
        // Older app versions read only `path`; the added `label` must not
        // move or rename it.
        let mut config = TransportConfig::Folder(FolderConfig { path: "/tmp/sync".to_string(), label: None });
        config.set_label(Some("Laptop"));
        let value: serde_json::Value = serde_json::from_str(&config.to_config_json().unwrap()).unwrap();
        assert_eq!(value["path"], "/tmp/sync");
    }

    #[test]
    fn only_folder_and_s3_support_deleting_data() {
        assert!(TransportConfig::Folder(FolderConfig { path: "/x".to_string(), label: None }).supports_delete_data());
        assert!(TransportConfig::S3(s3_config()).supports_delete_data());
        assert!(!TransportConfig::IpfsRpc(IpfsRpcConfig { base_url: "https://x".to_string(), label: None }).supports_delete_data());
    }

    #[test]
    fn validation_matches_secrets_to_their_kind() {
        let s3 = TransportConfig::S3(s3_config());
        assert!(s3.validate("s3", Some(&TransportSecrets::S3(s3_credentials()))).is_ok());
        assert!(s3.validate("s3", None).is_err());
        assert!(s3.validate("s3", Some(&TransportSecrets::IpfsRpcToken("t".to_string()))).is_err());

        let folder = TransportConfig::Folder(FolderConfig { path: "/x".to_string(), label: None });
        assert!(folder.validate("f", None).is_ok());
        assert!(folder.validate("f", Some(&TransportSecrets::S3(s3_credentials()))).is_err());

        let ipfs = TransportConfig::IpfsRpc(IpfsRpcConfig { base_url: "https://x".to_string(), label: None });
        assert!(ipfs.validate("i", None).is_ok());
        assert!(ipfs.validate("i", Some(&TransportSecrets::IpfsRpcToken("t".to_string()))).is_ok());

        let mut bad = s3_config();
        bad.endpoint = "http://s3.example.com".to_string();
        assert!(TransportConfig::S3(bad).validate("s3", Some(&TransportSecrets::S3(s3_credentials()))).is_err());
    }

    #[test]
    fn secrets_round_trip_per_kind_and_delete_cleanly() {
        let id = format!("secrets-{}", uuid::Uuid::new_v4());
        TransportSecrets::S3(s3_credentials()).store(&id).unwrap();
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), Some(TransportSecrets::S3(s3_credentials())));
        assert_eq!(TransportSecrets::load("ipfs_rpc", &id).unwrap(), None);
        assert_eq!(TransportSecrets::load("folder", &id).unwrap(), None);

        TransportSecrets::delete("s3", &id).unwrap();
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), None);

        TransportSecrets::IpfsRpcToken("tok".to_string()).store(&id).unwrap();
        TransportSecrets::S3(s3_credentials()).store(&id).unwrap();
        TransportSecrets::delete_every_kind(&id).unwrap();
        assert!(secret_store::values_for(&id).is_empty());
    }

    #[test]
    fn unreadable_s3_credentials_are_reported_without_echoing_them() {
        let id = format!("garbled-{}", uuid::Uuid::new_v4());
        secret_store::set(S3_CREDENTIALS_KEYCHAIN_SERVICE, &id, "garbled-secret-material").unwrap();
        let error = TransportSecrets::load("s3", &id).unwrap_err();
        assert!(!error.contains("garbled-secret-material"));
    }

    #[test]
    fn secrets_debug_output_is_redacted() {
        let debug = format!(
            "{:?} {:?}",
            TransportSecrets::IpfsRpcToken("token-value".to_string()),
            TransportSecrets::S3(s3_credentials())
        );
        assert!(!debug.contains("token-value"));
        assert!(!debug.contains("s3-secret-value"));
        assert!(!debug.contains("AKIAEXAMPLE"));
    }

    #[tokio::test]
    async fn an_s3_connector_without_credentials_does_not_open() {
        let config = TransportConfig::S3(s3_config());
        let id = format!("no-credentials-{}", uuid::Uuid::new_v4());
        assert!(Connector::open_persisted(&id, &config).await.is_err());
    }

    #[tokio::test]
    async fn connector_operations_dispatch_to_the_folder_adapter() {
        let path = std::env::temp_dir().join(format!("threestrands-connector-ops-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let config = TransportConfig::Folder(FolderConfig { path: path.to_string_lossy().into_owned(), label: None });
        let connector = Connector::open_persisted("folder", &config).await.unwrap();
        let cid = threestrands_sync_transport::Cid::for_bytes(b"ops");
        connector.transport().put_object(&cid, b"ops").await.unwrap();
        assert!(connector.corpus_size_bytes().await.unwrap() > 0);
        assert_eq!(connector.probe().await.unwrap(), ConnectorProbe::Folder);
        connector.delete_all_corpus_data().await.unwrap();
        assert_eq!(
            connector.transport().get_object(&cid).await,
            Err(threestrands_sync_transport::TransportError::NotFound)
        );
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[tokio::test]
    async fn connector_operations_dispatch_to_the_s3_adapter() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let config = TransportConfig::S3(server.config(""));
        let connector = Connector::open("s3", &config, Some(TransportSecrets::S3(FakeS3Server::credentials())))
            .await
            .unwrap();
        let cid = threestrands_sync_transport::Cid::for_bytes(b"ops");
        connector.transport().put_object(&cid, b"ops").await.unwrap();
        assert_eq!(connector.corpus_size_bytes().await, Some(3));
        match connector.probe().await.unwrap() {
            ConnectorProbe::S3(report) => assert!(report.can_delete),
            other => panic!("unexpected {other:?}"),
        }
        connector.delete_all_corpus_data().await.unwrap();
        assert!(server.state().objects.is_empty());
    }

    #[tokio::test]
    async fn an_ipfs_connector_refuses_to_delete_data() {
        let config = TransportConfig::IpfsRpc(IpfsRpcConfig { base_url: "https://rpc.filebase.io".to_string(), label: None });
        let connector = Connector::open("ipfs", &config, None).await.unwrap();
        assert!(connector.delete_all_corpus_data().await.is_err());
        assert_eq!(connector.corpus_size_bytes().await, None);
    }
}
