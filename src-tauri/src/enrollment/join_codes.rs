//! Join codes: an enrolled device hands a new one a single pasteable code
//! carrying connector settings, their credentials, and a one-time
//! invitation to the group's keys. See `threestrands_sync_envelope`'s
//! `join_code` module for the code format and the signed objects.
//!
//! Flow:
//! 1. **Create** (inviter): publish a signed [`Invitation`] sealing the
//!    current epoch key to the invite X25519 key, record it (CID, expiry,
//!    invite public key — never the secret or the code text), return the
//!    code.
//! 2. **Redeem** (joiner): fetch the invitation by the code's CID, verify
//!    it against the code, open the epoch key, adopt the roster, and publish
//!    an [`InvitationRedemption`] signed by the invite key and the device
//!    key. The joiner can read at once.
//! 3. **Admit** (inviter, next cycle): the first valid redemption of an
//!    open, unexpired invitation is trusted and the inviter rotates the
//!    epoch. Every device applies that rotation through the existing path,
//!    admitting the joiner. Later redemptions are rejected. Expired and
//!    cancelled invitations also rotate, so a leaked code only ever opens
//!    data written before its invitation closed. Expired and cancelled
//!    invitation objects are deleted at once; a redeemed one stays until the
//!    code's expiry so members that sync later can verify the redemption.
//!
//! The inviter's clock is the one enforced; the joiner's expiry check is a
//! courtesy. Only the inviting device can admit.

use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use threestrands_sync_envelope::{
    compute_cid, decode_join_code, decode_signed_invitation, encode_join_code, encode_signed_invitation,
    encode_signed_invitation_redemption, generate_invite_secret, invite_ed25519_signing_key, invite_x25519_secret,
    seal_to_x25519, sign_invitation, sign_invitation_redemption, try_open_sealed_box, verify_invitation,
    verify_invitation_redemption, InvitationRedemption, Invitation, JoinCode, JoinConnector, SignedInvitation,
    SignedInvitationRedemption, VerifyingKey, X25519PublicKey, JOIN_CODE_VERSION,
};
use threestrands_sync_transport::{Cid as TransportCid, SyncTransport, TransportError};

use super::{
    default_device_name, open_earlier_epoch_keys, protocol_marker_missing, roster_entry_verifying_key, seal_earlier_epoch_keys,
    store_earlier_epoch_keys, EnrollmentStatus, EpochKeyStore, LEGACY_SPACE_REFUSAL, MAX_DEVICE_LABEL_CHARS,
};
use crate::db::Database;
use crate::error_text::display;
use crate::replicated_sync::{decode_id, encode_id, x25519_public_bytes, DeviceIdentity, LocalKeys};
use crate::sync_connectors::{
    Connector, ConnectorCredentials, FolderConfig, TransportConfig, TransportSecrets, FOLDER_KIND, IPFS_RPC_KIND,
    S3_KIND,
};

/// Shortest and longest join-code lifetime Settings may request, in hours.
/// The UI offers 1 hour, 24 hours, and 7 days.
pub(crate) const MIN_JOIN_CODE_HOURS: u32 = 1;
pub(crate) const MAX_JOIN_CODE_HOURS: u32 = 7 * 24;

/// How long a redemption whose invitation this device hasn't seen yet is
/// retried on later sweeps (the invitation may still be syncing in), after
/// which it is ignored for good: the longest code lifetime plus a day.
const REDEMPTION_RETRY_WINDOW_MS: i64 = (MAX_JOIN_CODE_HOURS as i64 + 24) * 3_600_000;

fn rfc3339_from_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|time| time.to_rfc3339()).unwrap_or_default()
}

// ================================ API types ==================================

/// One connector to include in a new join code.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCodeConnectorChoice {
    pub instance_id: String,
    pub include_credentials: bool,
}

/// A join code this device created, as Settings lists it. Never includes
/// the code text, which this device doesn't keep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutstandingJoinCode {
    pub invitation_cid: String,
    pub created_at: String,
    pub expires_at: String,
    /// `open`, `redeemed`, `expired`, or `cancelled`.
    pub status: String,
    pub redeemed_by_device_id: Option<String>,
    pub redeemed_by_name: Option<String>,
    /// Redemptions refused because the code was already used, expired, or
    /// cancelled.
    pub rejected_attempts: i64,
}

/// What a pasted join code contains, before anything is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCodePreview {
    pub inviter_name: String,
    pub expires_at: String,
    /// By this device's clock.
    pub expired: bool,
    pub connectors: Vec<JoinCodeConnectorPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCodeConnectorPreview {
    pub index: usize,
    pub kind: String,
    /// `false` for a connector kind this app version doesn't know; it is
    /// skipped when joining.
    pub supported: bool,
    pub location: String,
    pub label: Option<String>,
    pub credentials_included: bool,
    /// A shared folder: this device must choose its own copy.
    pub needs_folder: bool,
    pub folder_name: Option<String>,
    /// Credentials were left out of the code and this kind requires them.
    pub needs_credentials: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinFolderChoice {
    pub connector_index: usize,
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCredentialsChoice {
    pub connector_index: usize,
    pub credentials: ConnectorCredentials,
}

/// Something Settings should tell the user about a join code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCodeNotice {
    pub redemption_cid: String,
    /// `joined`: a device joined with a join code. `rejectedAttempt`: a
    /// device tried a code of ours that was already used, expired, or
    /// cancelled.
    pub kind: String,
    pub device_id: String,
    pub device_name: String,
    pub inviter_device_id: Option<String>,
    pub inviter_name: Option<String>,
    pub at: String,
}

/// A folder connector carries only a name hint: paths differ per device.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FolderHint {
    folder_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

// ================================= Helpers ===================================

impl Database {
    /// This device's shared name, for the code and the redemption.
    fn self_device_name(&self, identity: &DeviceIdentity) -> Result<String, String> {
        let device_id = encode_id(identity.device_id.as_bytes());
        self.ensure_self_device_name(&device_id)?;
        let label: Option<String> = self
            .connection()?
            .query_row("SELECT label FROM sync_device_labels WHERE device_id=?1", params![device_id], |row| row.get(0))
            .optional()
            .map_err(display)?;
        Ok(label
            .unwrap_or_else(default_device_name)
            .chars()
            .take(MAX_DEVICE_LABEL_CHARS)
            .collect())
    }
}

fn folder_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn new_instance_id(kind: &str) -> String {
    let prefix = match kind {
        IPFS_RPC_KIND => "ipfs-rpc",
        other => other,
    };
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

/// Puts `bytes` on every transport; returns the instance ids that accepted.
async fn put_everywhere(transports: &[Arc<dyn SyncTransport>], cid: &str, bytes: &[u8]) -> Vec<String> {
    let mut accepted = Vec::new();
    for transport in transports {
        if transport.put_object(&TransportCid(cid.to_string()), bytes).await.is_ok() {
            accepted.push(transport.instance_id().0);
        }
    }
    accepted
}

async fn delete_everywhere(transports: &[Arc<dyn SyncTransport>], cid: &str) {
    for transport in transports {
        // Best effort: a provider deletion, not a logical one.
        let _ = transport.delete_object(&TransportCid(cid.to_string())).await;
    }
}

// ================================== Create ===================================

/// Creates a join code for `choices`, valid for `expires_in_hours`.
pub(crate) async fn create_join_code(
    database: &Database,
    identity: &DeviceIdentity,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
    choices: &[JoinCodeConnectorChoice],
    expires_in_hours: u32,
    now_ms: i64,
) -> Result<String, String> {
    if !(MIN_JOIN_CODE_HOURS..=MAX_JOIN_CODE_HOURS).contains(&expires_in_hours) {
        return Err(format!(
            "A join code can last from {MIN_JOIN_CODE_HOURS} hour to {} days",
            MAX_JOIN_CODE_HOURS / 24
        ));
    }
    if choices.is_empty() {
        return Err("Choose at least one connector to include in the join code".to_string());
    }
    let rows = database.configured_transports()?;
    let mut connectors = Vec::with_capacity(choices.len());
    let mut selected_ids = Vec::with_capacity(choices.len());
    for choice in choices {
        let row = rows
            .iter()
            .find(|row| row.instance_id == choice.instance_id)
            .ok_or_else(|| "One of the chosen connectors no longer exists".to_string())?;
        let config = row
            .config()
            .ok_or_else(|| "One of the chosen connectors can't be read by this version of ThreeStrands".to_string())?;
        let config_json = match &config {
            TransportConfig::Folder(folder) => serde_json::to_string(&FolderHint {
                folder_name: folder_name(&folder.path),
                label: config.label().map(str::to_string),
            })
            .map_err(display)?,
            _ => config.to_config_json()?,
        };
        let secrets_json = if choice.include_credentials {
            TransportSecrets::load(config.kind(), &row.instance_id)?
                .map(|secrets| secrets.to_portable_json())
                .transpose()?
        } else {
            None
        };
        connectors.push(JoinConnector { kind: config.kind().to_string(), config_json, secrets_json });
        selected_ids.push(row.instance_id.clone());
    }

    let secret = generate_invite_secret();
    let expires_at_ms = now_ms + i64::from(expires_in_hours) * 3_600_000;
    let mut code = JoinCode {
        version: JOIN_CODE_VERSION,
        invite_secret: ByteBuf::from(secret.to_vec()),
        // Placeholder until the invitation is published: validating the
        // code first means nothing is published for a code that can't be
        // encoded (too many connectors, oversized fields).
        invitation_cid: compute_cid(b"placeholder"),
        inviter_name: database.self_device_name(identity)?,
        expires_at_ms,
        connectors,
    };
    encode_join_code(&code).map_err(|error| error.to_string())?;

    let (recovery_ed25519, recovery_x25519) = database
        .recovery_public_keys()?
        .ok_or_else(|| "No recovery keys on record for this sync group".to_string())?;
    let invite_signing_key = invite_ed25519_signing_key(&secret);
    let invite_x25519_public = X25519PublicKey::from(&invite_x25519_secret(&secret)).to_bytes();
    let invitation = Invitation {
        inviter_device_id: identity.device_id,
        invite_ed25519_public: ByteBuf::from(invite_signing_key.verifying_key().to_bytes().to_vec()),
        invite_x25519_public: ByteBuf::from(invite_x25519_public.to_vec()),
        key_epoch: keys.key_epoch,
        sealed_epoch_key: ByteBuf::from(seal_to_x25519(&invite_x25519_public, &keys.k_epoch)),
        roster: database.full_roster_snapshot()?,
        recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
        recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
        created_at_ms: now_ms,
        expires_at_ms,
        earlier_epoch_keys: seal_earlier_epoch_keys(keys, &invite_x25519_public)?,
    };
    let signed = sign_invitation(&identity.signing_key, invitation).map_err(display)?;
    let bytes = encode_signed_invitation(&signed).map_err(display)?;
    let cid = compute_cid(&bytes);
    let accepted = put_everywhere(transports, &cid, &bytes).await;
    if !accepted.iter().any(|id| selected_ids.contains(id)) {
        delete_everywhere(transports, &cid).await;
        return Err(
            "Couldn't save the invitation to any of the chosen connectors. Check that they're reachable, then try again."
                .to_string(),
        );
    }
    database.mark_control_object_seen(&cid, "invitation")?;
    database
        .connection()?
        .execute(
            "INSERT INTO replicated_sync_invitations(invitation_cid, direction, status, inviter_device_id, inviter_name,
                 invite_ed25519_public, created_at, expires_at_ms)
             VALUES (?1,'outgoing','open',?2,?3,?4,?5,?6)",
            params![
                cid,
                encode_id(identity.device_id.as_bytes()),
                code.inviter_name,
                invite_signing_key.verifying_key().to_bytes().to_vec(),
                rfc3339_from_ms(now_ms),
                expires_at_ms,
            ],
        )
        .map_err(display)?;

    code.invitation_cid = cid;
    encode_join_code(&code).map_err(|error| error.to_string())
}

// ================================== Preview ==================================

fn connector_preview(index: usize, connector: &JoinConnector) -> JoinCodeConnectorPreview {
    let credentials_included = connector.secrets_json.is_some();
    let unsupported = JoinCodeConnectorPreview {
        index,
        kind: connector.kind.clone(),
        supported: false,
        location: String::new(),
        label: None,
        credentials_included,
        needs_folder: false,
        folder_name: None,
        needs_credentials: false,
    };
    if connector.kind == FOLDER_KIND {
        let Ok(hint) = serde_json::from_str::<FolderHint>(&connector.config_json) else { return unsupported };
        return JoinCodeConnectorPreview {
            supported: true,
            location: hint.folder_name.clone(),
            label: hint.label,
            needs_folder: true,
            folder_name: Some(hint.folder_name),
            ..unsupported
        };
    }
    let Some(config) = TransportConfig::from_row(&connector.kind, &connector.config_json) else { return unsupported };
    JoinCodeConnectorPreview {
        supported: true,
        location: config.location(),
        label: config.label().map(str::to_string),
        needs_credentials: connector.kind == S3_KIND && !credentials_included,
        ..unsupported
    }
}

/// Parses pasted code text without side effects.
pub(crate) fn preview_join_code(text: &str, now_ms: i64) -> Result<JoinCodePreview, String> {
    let code = decode_join_code(text).map_err(|error| error.to_string())?;
    Ok(JoinCodePreview {
        inviter_name: code.inviter_name.clone(),
        expires_at: rfc3339_from_ms(code.expires_at_ms),
        expired: now_ms > code.expires_at_ms,
        connectors: code.connectors.iter().enumerate().map(|(index, connector)| connector_preview(index, connector)).collect(),
    })
}

// ================================== Redeem ===================================

/// The connectors a code describes, completed with this device's folder
/// choices and any credentials the code left out. Unsupported kinds are
/// skipped.
pub(crate) fn prepare_join_connectors(
    code: &JoinCode,
    folders: &[JoinFolderChoice],
    credentials: Vec<JoinCredentialsChoice>,
) -> Result<Vec<(TransportConfig, Option<TransportSecrets>)>, String> {
    let mut provided: std::collections::HashMap<usize, ConnectorCredentials> =
        credentials.into_iter().map(|choice| (choice.connector_index, choice.credentials)).collect();
    let mut prepared = Vec::new();
    for (index, connector) in code.connectors.iter().enumerate() {
        let config = if connector.kind == FOLDER_KIND {
            let Ok(hint) = serde_json::from_str::<FolderHint>(&connector.config_json) else { continue };
            let choice = folders.iter().find(|choice| choice.connector_index == index).ok_or_else(|| {
                format!("Choose this device's copy of the shared folder \"{}\".", hint.folder_name)
            })?;
            TransportConfig::Folder(FolderConfig { path: choice.path.clone(), label: hint.label })
        } else {
            match TransportConfig::from_row(&connector.kind, &connector.config_json) {
                Some(config) => config,
                None => continue,
            }
        };
        let secrets = match (provided.remove(&index), &connector.secrets_json) {
            (Some(credentials), _) => Some(credentials.into_secrets()?),
            (None, Some(json)) => Some(TransportSecrets::from_portable_json(&connector.kind, json)?),
            (None, None) => None,
        };
        config.validate("join", secrets.as_ref())?;
        prepared.push((config, secrets));
    }
    if prepared.is_empty() {
        return Err(
            "None of the connectors in this join code work with this version of ThreeStrands. Update this app, then paste it again."
                .to_string(),
        );
    }
    Ok(prepared)
}

/// An invitation fetched by the code's CID and verified against the code.
pub(crate) struct OpenedInvitation {
    signed: SignedInvitation,
    k_epoch: [u8; 32],
    earlier_epoch_keys: Vec<(u32, [u8; 32])>,
}

/// Checks fetched bytes against the code: exactly the CID it names, signed
/// by the inviter in its own roster, carrying the invite keys the code's
/// secret derives, and sealing an epoch key that secret opens.
pub(crate) fn verify_fetched_invitation(bytes: &[u8], code: &JoinCode) -> Result<OpenedInvitation, String> {
    let damaged = || "This join code's invitation is damaged or doesn't match the code. Ask for a new one.".to_string();
    if compute_cid(bytes) != code.invitation_cid {
        return Err(damaged());
    }
    let signed = decode_signed_invitation(bytes).map_err(|_| damaged())?;
    let invitation = &signed.invitation;
    let inviter = invitation
        .roster
        .iter()
        .find(|entry| entry.device_id == invitation.inviter_device_id && entry.status == "active")
        .ok_or_else(damaged)?;
    let inviter_key = roster_entry_verifying_key(inviter).map_err(|_| damaged())?;
    verify_invitation(&inviter_key, &signed).map_err(|_| damaged())?;
    let secret = code.invite_secret();
    let expected_ed25519 = invite_ed25519_signing_key(&secret).verifying_key().to_bytes();
    let x25519_secret = invite_x25519_secret(&secret);
    let expected_x25519 = X25519PublicKey::from(&x25519_secret).to_bytes();
    if invitation.invite_ed25519_public.as_slice() != expected_ed25519
        || invitation.invite_x25519_public.as_slice() != expected_x25519
    {
        return Err(damaged());
    }
    let k_epoch: [u8; 32] = try_open_sealed_box(&x25519_secret.to_bytes(), &invitation.sealed_epoch_key)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(damaged)?;
    let earlier_epoch_keys =
        open_earlier_epoch_keys(&x25519_secret.to_bytes(), &invitation.earlier_epoch_keys).ok_or_else(damaged)?;
    Ok(OpenedInvitation { signed, k_epoch, earlier_epoch_keys })
}

pub(crate) const INVITATION_NOT_FOUND: &str = "Couldn't find this join code's invitation in its connectors. If it uses a shared folder, check that you chose the right folder and that your sync app has finished downloading, then try again.";
pub(crate) const CREDENTIALS_REJECTED: &str = "Storage rejected the credentials for this join code's connector. Check the access key, or ask for a new join code.";
pub(crate) const STORAGE_UNREACHABLE: &str = "Couldn't reach this join code's storage. Check your connection, then try again.";

/// Fetches and verifies the invitation from the first transport that has
/// it. When none does, the error says why, most specific first: an object
/// that doesn't match the code, storage that rejected the credentials,
/// storage that couldn't be reached, or simply no invitation there.
pub(crate) async fn find_invitation(
    code: &JoinCode,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<OpenedInvitation, String> {
    let mut mismatch = None;
    let mut credentials_rejected = false;
    let mut unreachable = false;
    for transport in transports {
        let bytes = match transport.get_object(&TransportCid(code.invitation_cid.clone())).await {
            Ok(bytes) => bytes,
            Err(TransportError::Authentication(_)) => {
                credentials_rejected = true;
                continue;
            }
            Err(TransportError::Transient(_) | TransportError::Quota(_)) => {
                unreachable = true;
                continue;
            }
            Err(_) => continue,
        };
        match verify_fetched_invitation(&bytes, code) {
            Ok(opened) => return Ok(opened),
            Err(error) => mismatch = Some(error),
        }
    }
    Err(mismatch.unwrap_or_else(|| {
        if credentials_rejected {
            CREDENTIALS_REJECTED
        } else if unreachable {
            STORAGE_UNREACHABLE
        } else {
            INVITATION_NOT_FOUND
        }
        .to_string()
    }))
}

/// Joins with an already verified invitation: publishes the redemption
/// first (so a failure leaves nothing half-joined), then adopts the roster
/// and epoch key and records the pending admission.
pub(crate) async fn commit_redemption(
    database: &Database,
    identity: &DeviceIdentity,
    epoch_keys: &dyn EpochKeyStore,
    code: &JoinCode,
    opened: OpenedInvitation,
    transports: &[Arc<dyn SyncTransport>],
    now_ms: i64,
) -> Result<(), String> {
    let invitation = &opened.signed.invitation;
    let self_x25519_public = x25519_public_bytes(&identity.x25519_secret);
    let redemption = InvitationRedemption {
        invitation_cid: code.invitation_cid.clone(),
        device_id: identity.device_id,
        ed25519_public: ByteBuf::from(identity.verifying_key.to_bytes().to_vec()),
        x25519_public: ByteBuf::from(self_x25519_public.to_vec()),
        device_name: database.self_device_name(identity)?,
        created_at_ms: now_ms,
    };
    let signed = sign_invitation_redemption(&invite_ed25519_signing_key(&code.invite_secret()), &identity.signing_key, redemption)
        .map_err(display)?;
    let bytes = encode_signed_invitation_redemption(&signed).map_err(display)?;
    let redemption_cid = compute_cid(&bytes);
    if put_everywhere(transports, &redemption_cid, &bytes).await.is_empty() {
        return Err("Couldn't reach any of this join code's connectors to finish joining. Try again.".to_string());
    }

    database.set_beta_features_enabled(true)?;
    database.adopt_roster(&invitation.roster)?;
    database.set_recovery_public_keys(
        invitation.recovery_ed25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
        invitation.recovery_x25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
    )?;
    database.trust_device_keys(identity.device_id.as_bytes(), &identity.verifying_key, &self_x25519_public)?;
    epoch_keys.store(invitation.key_epoch, &opened.k_epoch)?;
    database.set_active_epoch(invitation.key_epoch)?;
    database.record_epoch_activation(invitation.key_epoch, &code.invitation_cid)?;
    store_earlier_epoch_keys(database, epoch_keys, &opened.earlier_epoch_keys, &code.invitation_cid)?;
    database.mark_control_object_seen(&code.invitation_cid, "invitation")?;
    database.mark_control_object_seen(&redemption_cid, "invitation_redemption")?;
    database
        .connection()?
        .execute(
            "INSERT OR REPLACE INTO replicated_sync_invitations(invitation_cid, direction, status, inviter_device_id,
                 inviter_name, created_at, expires_at_ms, redemption_cid)
             VALUES (?1,'incoming','pending',?2,?3,?4,?5,?6)",
            params![
                code.invitation_cid,
                encode_id(invitation.inviter_device_id.as_bytes()),
                code.inviter_name,
                rfc3339_from_ms(now_ms),
                code.expires_at_ms,
                redemption_cid,
            ],
        )
        .map_err(display)?;
    Ok(())
}

/// The whole join: parse, check, open the code's connectors, verify the
/// invitation, save the connectors, then join. Nothing is saved unless the
/// invitation verifies.
pub(crate) async fn join_with_code(
    database: &Database,
    identity: &DeviceIdentity,
    epoch_keys: &dyn EpochKeyStore,
    text: &str,
    folders: &[JoinFolderChoice],
    credentials: Vec<JoinCredentialsChoice>,
    now_ms: i64,
) -> Result<(), String> {
    let code = decode_join_code(text).map_err(|error| error.to_string())?;
    if !matches!(database.enrollment_status()?, EnrollmentStatus::NotStarted) {
        return Err("This device already belongs to a sync group or is joining one. Leave it first, then use the join code.".to_string());
    }
    if now_ms > code.expires_at_ms {
        return Err("This join code expired. Ask for a new one.".to_string());
    }
    let prepared = prepare_join_connectors(&code, folders, credentials)?;
    let mut opened_connectors = Vec::with_capacity(prepared.len());
    let mut transports: Vec<Arc<dyn SyncTransport>> = Vec::with_capacity(prepared.len());
    let mut open_error = None;
    for (config, secrets) in prepared {
        let instance_id = new_instance_id(config.kind());
        match Connector::open(&instance_id, &config, secrets.clone()).await {
            Ok(connector) => {
                transports.push(connector.into_transport());
                opened_connectors.push((instance_id, config, secrets));
            }
            Err(error) => open_error = Some(error),
        }
    }
    if transports.is_empty() {
        return Err(open_error.unwrap_or_else(|| "Couldn't open any of this join code's connectors".to_string()));
    }
    let opened = find_invitation(&code, &transports).await?;
    if protocol_marker_missing(&transports).await {
        return Err(LEGACY_SPACE_REFUSAL.to_string());
    }
    for (instance_id, config, secrets) in &opened_connectors {
        database.add_transport(instance_id, config, secrets.as_ref())?;
    }
    commit_redemption(database, identity, epoch_keys, &code, opened, &transports, now_ms).await
}

// =============================== Sweep hooks =================================

/// What the enrollment sweep should do with one join-code control object.
pub(super) enum JoinObjectOutcome {
    /// Handled (or deliberately ignored): mark it seen.
    Done,
    /// Its invitation hasn't reached this device yet: look again next sweep.
    RetryLater,
}

/// Another device's invitation: remembered (with its invite public key) so
/// this device can recognize the joiner's redemption and show who joined.
/// Only invitations signed by a device already in this device's roster
/// count.
pub(super) fn apply_incoming_invitation(
    database: &Database,
    identity: &DeviceIdentity,
    signed: SignedInvitation,
    cid: &str,
) -> Result<(), String> {
    let invitation = &signed.invitation;
    if invitation.inviter_device_id == identity.device_id {
        return Ok(());
    }
    let roster = database.known_device_roster().map_err(String::from)?;
    let Some((_, inviter_key)) = roster.iter().find(|(device_id, _)| *device_id == invitation.inviter_device_id) else {
        return Ok(());
    };
    if verify_invitation(inviter_key, &signed).is_err() {
        return Ok(());
    }
    database
        .connection()?
        .execute(
            "INSERT OR IGNORE INTO replicated_sync_invitations(invitation_cid, direction, status, inviter_device_id,
                 invite_ed25519_public, created_at, expires_at_ms)
             VALUES (?1,'observed','observed',?2,?3,?4,?5)",
            params![
                cid,
                encode_id(invitation.inviter_device_id.as_bytes()),
                invitation.invite_ed25519_public.to_vec(),
                rfc3339_from_ms(invitation.created_at_ms),
                invitation.expires_at_ms,
            ],
        )
        .map_err(display)?;
    Ok(())
}

/// A joiner's redemption. The inviter queues it for [`process_join_codes`]
/// to admit or reject; every other group member records it so it can show
/// who joined once the admission rotation arrives.
pub(super) fn apply_incoming_redemption(
    database: &Database,
    identity: &DeviceIdentity,
    signed: SignedInvitationRedemption,
    cid: &str,
    now_ms: i64,
) -> Result<JoinObjectOutcome, String> {
    let redemption = &signed.redemption;
    if redemption.device_id == identity.device_id {
        return Ok(JoinObjectOutcome::Done);
    }
    let invitation: Option<(String, Option<Vec<u8>>, String)> = database
        .connection()?
        .query_row(
            "SELECT direction, invite_ed25519_public, inviter_device_id FROM replicated_sync_invitations WHERE invitation_cid=?1",
            params![redemption.invitation_cid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(display)?;
    let Some((direction, Some(invite_public), inviter_device_id)) = invitation else {
        let still_syncing = invitation.is_none() && now_ms < redemption.created_at_ms.saturating_add(REDEMPTION_RETRY_WINDOW_MS);
        return Ok(if still_syncing { JoinObjectOutcome::RetryLater } else { JoinObjectOutcome::Done });
    };
    if direction == "incoming" {
        return Ok(JoinObjectOutcome::Done);
    }
    let Ok(invite_public): Result<[u8; 32], _> = invite_public.try_into() else { return Ok(JoinObjectOutcome::Done) };
    let Ok(invite_key) = VerifyingKey::from_bytes(&invite_public) else { return Ok(JoinObjectOutcome::Done) };
    if verify_invitation_redemption(&invite_key, &signed).is_err() {
        return Ok(JoinObjectOutcome::Done);
    }
    let state = if direction == "outgoing" { "pending" } else { "observed" };
    database
        .connection()?
        .execute(
            "INSERT OR IGNORE INTO replicated_sync_invitation_redemptions(redemption_cid, invitation_cid, inviter_device_id,
                 device_id, ed25519_public, x25519_public, device_name, created_at_ms, state, received_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                cid,
                redemption.invitation_cid,
                inviter_device_id,
                encode_id(redemption.device_id.as_bytes()),
                redemption.ed25519_public.to_vec(),
                redemption.x25519_public.to_vec(),
                redemption.device_name,
                redemption.created_at_ms,
                state,
                rfc3339_from_ms(now_ms),
            ],
        )
        .map_err(display)?;
    Ok(JoinObjectOutcome::Done)
}

/// On the joiner: once a rotation lists this device as active, its join
/// is complete.
pub(super) fn note_admission_if_listed(database: &Database, identity: &DeviceIdentity, roster: &[threestrands_sync_envelope::RosterEntry]) -> Result<(), String> {
    if roster.iter().any(|entry| entry.device_id == identity.device_id && entry.status == "active") {
        database
            .connection()?
            .execute(
                "UPDATE replicated_sync_invitations SET status='admitted' WHERE direction='incoming' AND status='pending'",
                [],
            )
            .map_err(display)?;
    }
    Ok(())
}

// ============================ Inviter maintenance ============================

type PendingRedemption = (String, String, String, Vec<u8>, Vec<u8>, String);

/// On the inviter, once per sync cycle: admits the first valid redemption
/// of each open, unexpired invitation, rejects the rest, expires old
/// invitations, then rotates once if anything closed. Returns whether it
/// rotated, so the caller reloads its keys.
pub(crate) async fn process_join_codes(
    database: &Database,
    identity: &DeviceIdentity,
    keys: &LocalKeys,
    epoch_keys: &dyn EpochKeyStore,
    transports: &[Arc<dyn SyncTransport>],
    now_ms: i64,
) -> Result<bool, String> {
    let pending: Vec<PendingRedemption> = {
        let connection = database.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT r.redemption_cid, r.invitation_cid, r.device_id, r.ed25519_public, r.x25519_public, r.device_name
                 FROM replicated_sync_invitation_redemptions r
                 JOIN replicated_sync_invitations i ON i.invitation_cid = r.invitation_cid
                 WHERE r.state='pending' AND i.direction='outgoing'
                 ORDER BY r.created_at_ms, r.redemption_cid",
            )
            .map_err(display)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        rows
    };

    let mut rotate = false;
    let mut closed = Vec::new();
    for (redemption_cid, invitation_cid, device_id_hex, ed25519_public, x25519_public, device_name) in pending {
        let (status, expires_at_ms): (String, i64) = database
            .connection()?
            .query_row(
                "SELECT status, expires_at_ms FROM replicated_sync_invitations WHERE invitation_cid=?1",
                params![invitation_cid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(display)?;
        let already_known: bool = database
            .connection()?
            .query_row("SELECT EXISTS(SELECT 1 FROM sync_devices WHERE device_id=?1)", params![device_id_hex], |row| row.get(0))
            .map_err(display)?;
        let keys_valid = <[u8; 32]>::try_from(ed25519_public.as_slice())
            .ok()
            .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
            .zip(<[u8; 32]>::try_from(x25519_public.as_slice()).ok());
        // A redemption may never take over an existing device id: that
        // would replace a trusted device's keys.
        let admit = status == "open" && now_ms <= expires_at_ms && !already_known;
        match (admit, keys_valid) {
            (true, Some((verifying_key, x25519))) => {
                let device_id = decode_id(&device_id_hex)?;
                database.trust_device_keys(&device_id, &verifying_key, &x25519)?;
                if !device_name.trim().is_empty() {
                    database.materialize_device_name(&device_id_hex, Some(device_name.trim()))?;
                }
                let connection = database.connection()?;
                connection
                    .execute(
                        "UPDATE replicated_sync_invitations SET status='redeemed', redeemed_by_device_id=?2, redemption_cid=?3
                         WHERE invitation_cid=?1",
                        params![invitation_cid, device_id_hex, redemption_cid],
                    )
                    .map_err(display)?;
                connection
                    .execute(
                        "UPDATE replicated_sync_invitation_redemptions SET state='admitted' WHERE redemption_cid=?1",
                        params![redemption_cid],
                    )
                    .map_err(display)?;
                // The invitation object stays until the code's own expiry
                // so group members that sync later can still verify this
                // redemption; see the cleanup below.
                rotate = true;
            }
            _ => {
                database
                    .connection()?
                    .execute(
                        "UPDATE replicated_sync_invitation_redemptions SET state='rejected' WHERE redemption_cid=?1",
                        params![redemption_cid],
                    )
                    .map_err(display)?;
            }
        }
    }

    let expired: Vec<String> = {
        let connection = database.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT invitation_cid FROM replicated_sync_invitations
                 WHERE direction='outgoing' AND status='open' AND expires_at_ms < ?1",
            )
            .map_err(display)?;
        let rows = statement
            .query_map(params![now_ms], |row| row.get(0))
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        rows
    };
    for invitation_cid in expired {
        database
            .connection()?
            .execute(
                "UPDATE replicated_sync_invitations SET status='expired' WHERE invitation_cid=?1",
                params![invitation_cid],
            )
            .map_err(display)?;
        rotate = true;
        closed.push(invitation_cid);
    }

    // Redeemed invitations are deleted once their code would have expired.
    // After the admission rotation the sealed key inside only opens data
    // written before it, so keeping the object that long costs little.
    let redeemed_past_expiry: Vec<String> = {
        let connection = database.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT invitation_cid FROM replicated_sync_invitations
                 WHERE direction='outgoing' AND status='redeemed' AND object_deleted=0 AND expires_at_ms < ?1",
            )
            .map_err(display)?;
        let rows = statement
            .query_map(params![now_ms], |row| row.get(0))
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        rows
    };
    closed.extend(redeemed_past_expiry);

    if rotate {
        super::rotate_epoch(database, identity, keys, epoch_keys, transports, None).await?;
    }
    for invitation_cid in closed {
        delete_everywhere(transports, &invitation_cid).await;
        mark_object_deleted(database, &invitation_cid)?;
    }
    Ok(rotate)
}

/// Cancels an open join code: closes it, rotates, and deletes its
/// invitation object.
pub(crate) async fn cancel_join_code(
    database: &Database,
    identity: &DeviceIdentity,
    keys: &LocalKeys,
    epoch_keys: &dyn EpochKeyStore,
    transports: &[Arc<dyn SyncTransport>],
    invitation_cid: &str,
) -> Result<(), String> {
    let changed = database
        .connection()?
        .execute(
            "UPDATE replicated_sync_invitations SET status='cancelled'
             WHERE invitation_cid=?1 AND direction='outgoing' AND status='open'",
            params![invitation_cid],
        )
        .map_err(display)?;
    if changed == 0 {
        return Err("That join code isn't open anymore.".to_string());
    }
    super::rotate_epoch(database, identity, keys, epoch_keys, transports, None).await?;
    delete_everywhere(transports, invitation_cid).await;
    mark_object_deleted(database, invitation_cid)
}

fn mark_object_deleted(database: &Database, invitation_cid: &str) -> Result<(), String> {
    database
        .connection()?
        .execute(
            "UPDATE replicated_sync_invitations SET object_deleted=1 WHERE invitation_cid=?1",
            params![invitation_cid],
        )
        .map_err(display)?;
    Ok(())
}

// ============================ Listing and notices ============================

impl Database {
    /// Join codes this device created, newest first.
    pub fn outstanding_join_codes(&self) -> Result<Vec<OutstandingJoinCode>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT i.invitation_cid, i.created_at, i.expires_at_ms, i.status, i.redeemed_by_device_id,
                        (SELECT r.device_name FROM replicated_sync_invitation_redemptions r WHERE r.redemption_cid = i.redemption_cid),
                        (SELECT COUNT(*) FROM replicated_sync_invitation_redemptions r
                          WHERE r.invitation_cid = i.invitation_cid AND r.state='rejected')
                 FROM replicated_sync_invitations i
                 WHERE i.direction='outgoing'
                 ORDER BY i.created_at DESC",
            )
            .map_err(display)?;
        let rows = statement
            .query_map([], |row| {
                Ok(OutstandingJoinCode {
                    invitation_cid: row.get(0)?,
                    created_at: row.get(1)?,
                    expires_at: rfc3339_from_ms(row.get(2)?),
                    status: row.get(3)?,
                    redeemed_by_device_id: row.get(4)?,
                    redeemed_by_name: row.get(5)?,
                    rejected_attempts: row.get(6)?,
                })
            })
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        Ok(rows)
    }

    /// Undismissed join-code notices: devices that joined with a code (once
    /// they're actually in the roster), and refused attempts on this
    /// device's own codes.
    pub fn join_code_notices(&self) -> Result<Vec<JoinCodeNotice>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT r.redemption_cid,
                        CASE WHEN r.state='rejected' THEN 'rejectedAttempt' ELSE 'joined' END,
                        r.device_id, r.device_name, r.inviter_device_id,
                        (SELECT label FROM sync_device_labels l WHERE l.device_id = r.inviter_device_id),
                        r.created_at_ms
                 FROM replicated_sync_invitation_redemptions r
                 WHERE r.notice_dismissed = 0
                   AND (r.state = 'rejected'
                        OR (r.state IN ('admitted','observed')
                            AND EXISTS (SELECT 1 FROM sync_devices d
                                        WHERE d.device_id = r.device_id AND d.status='active' AND d.is_self=0)))
                 ORDER BY r.created_at_ms",
            )
            .map_err(display)?;
        let rows = statement
            .query_map([], |row| {
                Ok(JoinCodeNotice {
                    redemption_cid: row.get(0)?,
                    kind: row.get(1)?,
                    device_id: row.get(2)?,
                    device_name: row.get(3)?,
                    inviter_device_id: row.get(4)?,
                    inviter_name: row.get(5)?,
                    at: rfc3339_from_ms(row.get(6)?),
                })
            })
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        Ok(rows)
    }

    pub fn dismiss_join_code_notice(&self, redemption_cid: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE replicated_sync_invitation_redemptions SET notice_dismissed=1 WHERE redemption_cid=?1",
                params![redemption_cid],
            )
            .map_err(display)?;
        Ok(())
    }

    /// The inviter's name while this device waits for its join-code
    /// admission; `None` once admitted or when it didn't join by code.
    pub(super) fn awaiting_admission_from(&self) -> Result<Option<String>, String> {
        let name: Option<Option<String>> = self
            .connection()?
            .query_row(
                "SELECT inviter_name FROM replicated_sync_invitations WHERE direction='incoming' AND status='pending' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(display)?;
        Ok(name.map(|name| name.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| "another device".to_string())))
    }
}
