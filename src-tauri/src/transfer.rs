use std::{collections::HashSet, fs, path::Path};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use chrono::Utc;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{
    ai::AiProvider,
    correspondence::validate_retention_days,
    db::Database,
    models::{Account, SplitInbox},
};

const FORMAT: &str = "dispatch-settings";
const VERSION: u32 = 1;
const EXTENSION: &str = "dispatch-settings";
const ARGON_MEMORY_KIB: u32 = 19_456;
const ARGON_ITERATIONS: u32 = 2;
const ARGON_PARALLELISM: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TEXT_LENGTH: usize = 2_048;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiFeaturePreferences {
    pub draft_assist: bool,
    pub summarize: bool,
    // Version 1 exports originally included this flag. Keep emitting and
    // accepting it so transfers remain compatible across app updates even
    // though the webview no longer exposes the feature.
    #[serde(default)]
    pub classify: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferPreferences {
    pub theme: String,
    pub font_scale: i64,
    pub font_family: String,
    pub auto_read_delay_seconds: i64,
    pub load_remote_images: bool,
    pub selected_account_id: Option<String>,
    pub ai_provider: AiProvider,
    pub ai_model: String,
    pub ai_endpoint: String,
    pub ai_features: AiFeaturePreferences,
}

impl TransferPreferences {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.theme.as_str(), "light" | "dark" | "system") {
            return Err("The transfer contains an invalid theme".to_string());
        }
        if !(80..=140).contains(&self.font_scale) {
            return Err("The transfer contains an invalid font scale".to_string());
        }
        validate_required_text("font family", &self.font_family, 200)?;
        if !(0..=60).contains(&self.auto_read_delay_seconds) {
            return Err("The transfer contains an invalid read delay".to_string());
        }
        if let Some(account_id) = &self.selected_account_id {
            validate_text("selected account", account_id, 320)?;
        }
        validate_text("AI model", &self.ai_model, MAX_TEXT_LENGTH)?;
        validate_text("AI endpoint", &self.ai_endpoint, MAX_TEXT_LENGTH)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferAccount {
    pub email: String,
    pub display_name: Option<String>,
    pub color: String,
    pub sort_order: i64,
}

impl From<Account> for TransferAccount {
    fn from(account: Account) -> Self {
        Self {
            email: account.email,
            display_name: account.display_name,
            color: account.color,
            sort_order: account.sort_order,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferSplitInbox {
    pub id: String,
    pub name: String,
    pub match_kind: String,
    pub match_value: String,
    pub sort_order: i64,
    pub created_at: String,
    // Split Inboxes were global when version 1 was introduced. Missing owners
    // are migrated to the first exported account, matching the database's
    // migration for locally stored rules.
    #[serde(default)]
    pub account_id: String,
}

impl From<SplitInbox> for TransferSplitInbox {
    fn from(split: SplitInbox) -> Self {
        Self {
            id: split.id,
            name: split.name,
            match_kind: split.match_kind,
            match_value: split.match_value,
            sort_order: split.sort_order,
            created_at: split.created_at,
            account_id: split.account_id,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransferPayload {
    version: u32,
    exported_at: String,
    preferences: TransferPreferences,
    accounts: Vec<TransferAccount>,
    split_inboxes: Vec<TransferSplitInbox>,
    retention_days: Option<i64>,
}

impl TransferPayload {
    fn migrate_legacy_fields(&mut self) {
        let fallback_account_id = self.accounts.first().map(|account| account.email.as_str());
        for split in &mut self.split_inboxes {
            if split.account_id.is_empty() {
                split.account_id = fallback_account_id.unwrap_or_default().to_string();
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!("Unsupported transfer version {}", self.version));
        }
        self.preferences.validate()?;
        validate_retention_days(self.retention_days)
            .map_err(|_| "The transfer contains an invalid retention period".to_string())?;
        let mut account_emails = HashSet::new();
        for account in &self.accounts {
            validate_required_text("account email", &account.email, 320)?;
            if account.email.trim() != account.email
                || !account.email.contains('@')
                || !account_emails.insert(account.email.to_ascii_lowercase())
            {
                return Err("The transfer contains an invalid account email".to_string());
            }
            if let Some(name) = &account.display_name {
                validate_required_text("account display name", name, 200)?;
            }
            if account.color.len() != 7
                || !account.color.starts_with('#')
                || !account.color[1..]
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return Err("The transfer contains an invalid account color".to_string());
            }
        }
        let mut split_ids = HashSet::new();
        for split in &self.split_inboxes {
            validate_required_text("Split Inbox id", &split.id, 128)?;
            if !split_ids.insert(&split.id) {
                return Err("The transfer contains a duplicate Split Inbox".to_string());
            }
            validate_required_text("Split Inbox name", &split.name, 200)?;
            validate_required_text(
                "Split Inbox match value",
                &split.match_value,
                MAX_TEXT_LENGTH,
            )?;
            if !matches!(split.match_kind.as_str(), "domain" | "label" | "pattern") {
                return Err("The transfer contains an invalid Split Inbox rule".to_string());
            }
            if !account_emails.contains(&split.account_id.to_ascii_lowercase()) {
                return Err(
                    "The transfer contains a Split Inbox for an unknown account".to_string()
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptedEnvelope {
    format: String,
    version: u32,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub preferences: TransferPreferences,
    pub account_count: usize,
    pub split_inbox_count: usize,
}

pub fn export(
    database: &Database,
    preferences: TransferPreferences,
    password: &str,
) -> Result<Option<String>, String> {
    preferences.validate()?;
    validate_password(password)?;
    let accounts = database
        .list_accounts()?
        .into_iter()
        .map(TransferAccount::from)
        .collect();
    let split_inboxes = database
        .list_split_inboxes()?
        .into_iter()
        .map(TransferSplitInbox::from)
        .collect();
    let payload = TransferPayload {
        version: VERSION,
        exported_at: Utc::now().to_rfc3339(),
        preferences,
        accounts,
        split_inboxes,
        retention_days: database.retention_days()?,
    };
    let encoded = encrypt(&payload, password)?;
    let Some(path) = rfd::FileDialog::new()
        .set_title("Export ThreeStrands settings")
        .add_filter("ThreeStrands settings", &[EXTENSION])
        // Keep the legacy extension and envelope marker so settings exported
        // by Dispatch remain directly importable after the rename.
        .set_file_name("threestrands-settings.dispatch-settings")
        .save_file()
    else {
        return Ok(None);
    };
    fs::write(&path, encoded).map_err(|error| format!("Could not write the export: {error}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

pub fn import(database: &Database, password: &str) -> Result<Option<ImportResult>, String> {
    validate_password(password)?;
    let Some(path) = rfd::FileDialog::new()
        .set_title("Import ThreeStrands settings")
        .add_filter("ThreeStrands settings", &[EXTENSION])
        .pick_file()
    else {
        return Ok(None);
    };
    let payload = read_and_decrypt(&path, password)?;
    payload.validate()?;
    database.import_transfer_data(
        &payload.accounts,
        &payload.split_inboxes,
        payload.retention_days,
    )?;
    Ok(Some(ImportResult {
        preferences: payload.preferences,
        account_count: payload.accounts.len(),
        split_inbox_count: payload.split_inboxes.len(),
    }))
}

fn read_and_decrypt(path: &Path, password: &str) -> Result<TransferPayload, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("Could not read the export: {error}"))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err("The selected file is too large to be a ThreeStrands settings export".to_string());
    }
    let encoded = fs::read(path).map_err(|error| format!("Could not read the export: {error}"))?;
    decrypt(&encoded, password)
}

fn encrypt(payload: &TransferPayload, password: &str) -> Result<Vec<u8>, String> {
    let plaintext = serde_json::to_vec(payload).map_err(display)?;
    let mut salt = [0_u8; SALT_LEN];
    let mut nonce = [0_u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(display)?;
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| "Could not encrypt the settings export".to_string())?;
    serde_json::to_vec_pretty(&EncryptedEnvelope {
        format: FORMAT.to_string(),
        version: VERSION,
        kdf: "argon2id".to_string(),
        cipher: "xchacha20poly1305".to_string(),
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
    .map_err(display)
}

fn decrypt(encoded: &[u8], password: &str) -> Result<TransferPayload, String> {
    let envelope: EncryptedEnvelope = serde_json::from_slice(encoded)
        .map_err(|_| "This is not a valid ThreeStrands settings export".to_string())?;
    if envelope.format != FORMAT
        || envelope.version != VERSION
        || envelope.kdf != "argon2id"
        || envelope.cipher != "xchacha20poly1305"
    {
        return Err("This ThreeStrands settings export uses an unsupported format".to_string());
    }
    let salt = STANDARD
        .decode(envelope.salt)
        .map_err(|_| "The settings export is damaged".to_string())?;
    let nonce = STANDARD
        .decode(envelope.nonce)
        .map_err(|_| "The settings export is damaged".to_string())?;
    if salt.len() != SALT_LEN || nonce.len() != NONCE_LEN {
        return Err("The settings export is damaged".to_string());
    }
    let ciphertext = STANDARD
        .decode(envelope.ciphertext)
        .map_err(|_| "The settings export is damaged".to_string())?;
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(display)?;
    let plaintext = cipher
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| "The password is incorrect or the settings export is damaged".to_string())?;
    let mut payload: TransferPayload = serde_json::from_slice(&plaintext)
        .map_err(|_| "The settings export contains invalid data".to_string())?;
    payload.migrate_legacy_fields();
    Ok(payload)
}

fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], String> {
    let params = Params::new(
        ARGON_MEMORY_KIB,
        ARGON_ITERATIONS,
        ARGON_PARALLELISM,
        Some(KEY_LEN),
    )
    .map_err(display)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0_u8; KEY_LEN];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(display)?;
    Ok(key)
}

fn validate_password(password: &str) -> Result<(), String> {
    if password.chars().count() < 8 {
        return Err("Use a password with at least 8 characters".to_string());
    }
    if password.len() > 1_024 {
        return Err("The password is too long".to_string());
    }
    Ok(())
}

fn validate_text(label: &str, value: &str, max_length: usize) -> Result<(), String> {
    if value.len() > max_length || value.chars().any(char::is_control) {
        return Err(format!("The transfer contains an invalid {label}"));
    }
    Ok(())
}

fn validate_required_text(label: &str, value: &str, max_length: usize) -> Result<(), String> {
    validate_text(label, value, max_length)?;
    if value.trim().is_empty() {
        return Err(format!("The transfer contains an invalid {label}"));
    }
    Ok(())
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> TransferPayload {
        TransferPayload {
            version: VERSION,
            exported_at: "2026-03-06T00:00:00Z".to_string(),
            preferences: TransferPreferences {
                theme: "dark".to_string(),
                font_scale: 110,
                font_family: "system".to_string(),
                auto_read_delay_seconds: 2,
                load_remote_images: false,
                selected_account_id: Some("person@example.com".to_string()),
                ai_provider: AiProvider::None,
                ai_model: String::new(),
                ai_endpoint: String::new(),
                ai_features: AiFeaturePreferences {
                    draft_assist: false,
                    summarize: false,
                    classify: false,
                },
            },
            accounts: vec![TransferAccount {
                email: "person@example.com".to_string(),
                display_name: Some("Person".to_string()),
                color: "#4285F4".to_string(),
                sort_order: 0,
            }],
            split_inboxes: vec![],
            retention_days: Some(90),
        }
    }

    #[test]
    fn encrypted_transfer_round_trips_without_plaintext_account_data() {
        let encoded = encrypt(&payload(), "correct horse").unwrap();
        let serialized = String::from_utf8(encoded.clone()).unwrap();
        assert!(!serialized.contains("person@example.com"));
        let decoded = decrypt(&encoded, "correct horse").unwrap();
        assert_eq!(decoded.accounts[0].email, "person@example.com");
        assert_eq!(decoded.preferences.theme, "dark");
    }

    #[test]
    fn dispatch_v1_envelope_marker_remains_importable() {
        let encoded = encrypt(&payload(), "correct horse").unwrap();
        let envelope: EncryptedEnvelope = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(envelope.format, "dispatch-settings");
        assert_eq!(envelope.version, 1);
        assert_eq!(
            decrypt(&encoded, "correct horse").unwrap().accounts[0].email,
            "person@example.com"
        );
    }

    #[test]
    fn incorrect_password_cannot_decrypt_transfer() {
        let encoded = encrypt(&payload(), "correct horse").unwrap();
        let error = decrypt(&encoded, "wrong password").unwrap_err();
        assert!(error.contains("password is incorrect"));
    }

    #[test]
    fn preferences_reject_unknown_provider() {
        let serialized = serde_json::to_value(&payload().preferences).unwrap();
        let mut object = serialized.as_object().unwrap().clone();
        object.insert(
            "aiProvider".to_string(),
            serde_json::Value::String("secret-provider".to_string()),
        );
        assert!(serde_json::from_value::<TransferPreferences>(object.into()).is_err());
    }

    #[test]
    fn decrypt_accepts_legacy_ai_classify_flag() {
        let mut legacy_payload = payload();
        legacy_payload.preferences.ai_features.classify = true;
        let encoded = encrypt(&legacy_payload, "correct horse").unwrap();

        let decoded = decrypt(&encoded, "correct horse").unwrap();

        assert!(decoded.preferences.ai_features.classify);
    }

    #[test]
    fn transfer_retention_uses_the_native_retention_policy() {
        for retention_days in [None, Some(30), Some(90), Some(365)] {
            let mut candidate = payload();
            candidate.retention_days = retention_days;
            candidate.validate().unwrap();
        }

        for retention_days in [Some(-1), Some(0), Some(31), Some(366)] {
            let mut candidate = payload();
            candidate.retention_days = retention_days;
            assert_eq!(
                candidate.validate().unwrap_err(),
                "The transfer contains an invalid retention period"
            );
        }
    }

    #[test]
    fn legacy_split_inbox_without_account_is_assigned_to_first_account() {
        let mut serialized = serde_json::to_value(payload()).unwrap();
        serialized["splitInboxes"] = serde_json::json!([{
            "id": "legacy-rule",
            "name": "Legacy rule",
            "matchKind": "domain",
            "matchValue": "example.com",
            "sortOrder": 0,
            "createdAt": "2026-03-06T00:00:00Z"
        }]);
        let mut decoded: TransferPayload = serde_json::from_value(serialized).unwrap();

        decoded.migrate_legacy_fields();
        decoded.validate().unwrap();

        assert_eq!(decoded.split_inboxes[0].account_id, "person@example.com");
    }

    #[test]
    fn transfer_still_rejects_unrecognized_fields() {
        let mut serialized = serde_json::to_value(payload()).unwrap();
        serialized["preferences"]["aiFeatures"]["unexpected"] = serde_json::json!(true);

        assert!(serde_json::from_value::<TransferPayload>(serialized).is_err());
    }
}
