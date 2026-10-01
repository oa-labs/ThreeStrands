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
    error_text::display,
    models::{is_known_account_provider, Account, AvailabilityPreferences, Snippet, SplitInbox, ContactProfile},
};

const FORMAT: &str = "dispatch-settings";
const VERSION: u32 = 3;
const EXTENSION: &str = "dispatch-settings";
const ARGON_MEMORY_KIB: u32 = 19_456;
const ARGON_ITERATIONS: u32 = 2;
const ARGON_PARALLELISM: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
// 1 GiB accommodates the JSON and encryption base64 overhead of 5,000
// contacts with maximum-sized photos and bounded profile fields.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_TEXT_LENGTH: usize = 2_048;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiFeaturePreferences {
    pub draft_assist: bool,
    pub summarize: bool,
    #[serde(default)]
    pub action_extraction: bool,
    #[serde(default)]
    pub contact_enrichment: bool,
    // Version 1 exports originally included this flag. Keep emitting and
    // accepting it so transfers remain compatible across app updates even
    // though the webview no longer exposes the feature.
    #[serde(default)]
    pub classify: bool,
    // Added in 0.45 under format version 3; earlier exports omit both and
    // import with proactive suggestions off.
    #[serde(default)]
    pub proactive_briefs: bool,
    #[serde(default)]
    pub proactive_known_senders_only: bool,
    // Added in 0.46 under format version 3; earlier exports import with
    // thread chat off.
    #[serde(default)]
    pub thread_chat: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferPreferences {
    pub theme: String,
    // Added after version 2 shipped; exports produced before the accent
    // picker existed omit the field and default to the brand purple.
    #[serde(default = "default_accent")]
    pub accent: String,
    pub font_scale: i64,
    pub font_family: String,
    pub auto_read_delay_seconds: i64,
    pub load_remote_images: bool,
    pub selected_account_id: Option<String>,
    pub ai_provider: AiProvider,
    pub ai_model: String,
    // Added in 0.53 under format version 3; earlier exports omit it and
    // import with no fast model, so every feature keeps using `ai_model`.
    #[serde(default)]
    pub ai_fast_model: String,
    pub ai_endpoint: String,
    pub ai_features: AiFeaturePreferences,
    #[serde(default = "default_availability_preferences")]
    pub availability_preferences: AvailabilityPreferences,
}

fn default_accent() -> String {
    "purple".to_string()
}

fn default_availability_preferences() -> AvailabilityPreferences {
    AvailabilityPreferences {
        time_zone: "UTC".to_string(),
        working_windows: (1..=5)
            .map(|weekday| crate::models::AvailabilityWindow {
                weekday,
                start: "09:00".to_string(),
                end: "17:00".to_string(),
            })
            .collect(),
        default_duration_minutes: 30,
        slot_increment_minutes: 15,
    }
}

impl TransferPreferences {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.theme.as_str(), "light" | "dark" | "system") {
            return Err("The transfer contains an invalid theme".to_string());
        }
        if !matches!(
            self.accent.as_str(),
            "purple" | "blue" | "teal" | "green" | "amber" | "rose" | "graphite"
        ) {
            return Err("The transfer contains an invalid accent color".to_string());
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
        validate_text("AI fast model", &self.ai_fast_model, MAX_TEXT_LENGTH)?;
        validate_text("AI endpoint", &self.ai_endpoint, MAX_TEXT_LENGTH)?;
        crate::availability::validate_preferences(&self.availability_preferences)
            .map_err(|_| "The transfer contains invalid availability preferences".to_string())?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferAccount {
    pub email: String,
    pub display_name: Option<String>,
    pub color: String,
    // Every account exported before this field existed authenticated
    // through Gmail, so an absent value defaults to it rather than an empty
    // string — no `migrate_legacy_fields` fixup needed, unlike
    // `TransferSplitInbox::account_id`.
    #[serde(default = "default_account_provider")]
    pub provider: String,
    pub sort_order: i64,
}

fn default_account_provider() -> String {
    "gmail".to_string()
}

impl From<Account> for TransferAccount {
    fn from(account: Account) -> Self {
        Self {
            email: account.email,
            display_name: account.display_name,
            color: account.color,
            provider: account.provider,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferSnippet {
    pub id: String,
    pub name: String,
    pub body: String,
    pub created_at: String,
}

impl From<Snippet> for TransferSnippet {
    fn from(snippet: Snippet) -> Self {
        Self {
            id: snippet.id,
            name: snippet.name,
            body: snippet.body,
            created_at: snippet.created_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferContact {
    pub id:String,
    pub display_name:Option<String>, pub role:Option<String>, pub company:Option<String>,
    pub location:Option<String>, pub bio:Option<String>, pub notes:Option<String>,
    pub links:Vec<String>, pub photo_data:Option<String>, pub favorite:bool, pub addresses:Vec<String>,
}
impl From<ContactProfile> for TransferContact {
    fn from(c:ContactProfile)->Self { Self{id:c.id,display_name:c.display_name,role:c.role,company:c.company,location:c.location,bio:c.bio,notes:c.notes,links:c.links,photo_data:c.photo_data,favorite:c.favorite,addresses:c.addresses} }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransferPayload {
    version: u32,
    exported_at: String,
    preferences: TransferPreferences,
    accounts: Vec<TransferAccount>,
    split_inboxes: Vec<TransferSplitInbox>,
    #[serde(default)]
    snippets: Vec<TransferSnippet>,
    #[serde(default)]
    contacts: Vec<TransferContact>,
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
        if !(1..=VERSION).contains(&self.version) {
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
            if !is_known_account_provider(&account.provider) {
                return Err("The transfer contains an unrecognized account provider".to_string());
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
        let mut snippet_ids = HashSet::new();
        for snippet in &self.snippets {
            validate_required_text("Snippet id", &snippet.id, 128)?;
            if !snippet_ids.insert(&snippet.id) {
                return Err("The transfer contains a duplicate Snippet".to_string());
            }
            validate_required_text("Snippet name", &snippet.name, 200)?;
            // Unlike the other transferred text fields, a snippet body is
            // stored HTML and may legitimately contain newlines, so it isn't
            // run through `validate_text`'s single-line control-character
            // check — only its length and emptiness matter here.
            if snippet.body.len() > 20_000 {
                return Err("The transfer contains an invalid Snippet body".to_string());
            }
            if snippet.body.trim().is_empty() {
                return Err("The transfer contains an invalid Snippet body".to_string());
            }
        }
        let mut contact_ids=HashSet::new();
        let mut addresses=HashSet::new();
        for contact in &self.contacts {
            validate_required_text("contact id",&contact.id,128)?;
            if !contact_ids.insert(&contact.id) || contact.addresses.is_empty() || contact.addresses.len()>100 {return Err("The transfer contains an invalid contact".into())}
            for email in &contact.addresses {validate_required_text("contact email",email,320)?; if !email.contains('@') || !addresses.insert(email.to_ascii_lowercase()){return Err("The transfer contains a duplicate or invalid contact address".into())}}
            for (name,value,max) in [("display name",&contact.display_name,200),("role",&contact.role,200),("company",&contact.company,200),("location",&contact.location,200),("bio",&contact.bio,4000),("notes",&contact.notes,8000)] {if let Some(value)=value {validate_required_text(name,value,max)?;}}
            if contact.links.len()>20 || contact.links.iter().any(|link| {
                if link.len()>2048 { return true; }
                match url::Url::parse(link) { Ok(url)=>url.scheme()!="https"||url.host_str().is_none(), Err(_)=>true }
            }){return Err("The transfer contains an invalid contact link".into())}
            if let Some(photo)=&contact.photo_data {use base64::{engine::general_purpose::STANDARD,Engine};let bytes=STANDARD.decode(photo).map_err(|_|"The transfer contains an invalid contact photo")?;crate::image_format::validate_contact_photo(&bytes)?;}
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
    pub snippet_count: usize,
    pub contact_count: usize,
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
    let snippets = database
        .list_snippets()?
        .into_iter()
        .map(TransferSnippet::from)
        .collect();
    let contacts=database.list_saved_contact_profiles()?.into_iter().map(TransferContact::from).collect();
    let payload = TransferPayload {
        version: VERSION,
        exported_at: Utc::now().to_rfc3339(),
        preferences,
        accounts,
        split_inboxes,
        snippets,
        contacts,
        retention_days: database.retention_days()?,
    };
    let encoded = encrypt(&payload, password)?;
    validate_export_size(encoded.len() as u64)?;
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

fn validate_export_size(size:u64)->Result<(),String>{
    if size>MAX_FILE_BYTES {Err("The settings export exceeds the maximum supported file size".to_string())} else {Ok(())}
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
    apply_import(database, payload).map(Some)
}

fn apply_import(database: &Database, payload: TransferPayload) -> Result<ImportResult, String> {
    payload.validate()?;
    database.import_transfer_data(
        &payload.accounts,
        &payload.split_inboxes,
        &payload.snippets,
        &payload.contacts,
        payload.retention_days,
    )?;
    Ok(ImportResult {
        preferences: payload.preferences,
        account_count: payload.accounts.len(),
        split_inbox_count: payload.split_inboxes.len(),
        snippet_count: payload.snippets.len(),
        contact_count: payload.contacts.len(),
    })
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
    seal(&plaintext, password, &salt, &nonce)
}

fn seal(
    plaintext: &[u8],
    password: &str,
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>, String> {
    let key = derive_key(password, salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(display)?;
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(nonce), plaintext)
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
        || !(1..=VERSION).contains(&envelope.version)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_size_validation_matches_the_import_limit() {
        assert!(validate_export_size(0).is_ok());
        assert!(validate_export_size(MAX_FILE_BYTES).is_ok());
        assert!(validate_export_size(MAX_FILE_BYTES+1).is_err());
    }

    fn payload() -> TransferPayload {
        TransferPayload {
            version: VERSION,
            exported_at: "2026-03-06T00:00:00Z".to_string(),
            preferences: TransferPreferences {
                theme: "dark".to_string(),
                accent: "rose".to_string(),
                font_scale: 110,
                font_family: "system".to_string(),
                auto_read_delay_seconds: 2,
                load_remote_images: false,
                selected_account_id: Some("person@example.com".to_string()),
                ai_provider: AiProvider::None,
                ai_model: String::new(),
                ai_fast_model: String::new(),
                ai_endpoint: String::new(),
                ai_features: AiFeaturePreferences {
                    draft_assist: false,
                    summarize: false,
                    action_extraction: false,
                    contact_enrichment: false,
                    classify: false,
                    proactive_briefs: false,
                    proactive_known_senders_only: false,
                    thread_chat: false,
                },
                availability_preferences: default_availability_preferences(),
            },
            accounts: vec![TransferAccount {
                email: "person@example.com".to_string(),
                display_name: Some("Person".to_string()),
                color: "#4285F4".to_string(),
                provider: "gmail".to_string(),
                sort_order: 0,
            }],
            split_inboxes: vec![],
            snippets: vec![],
            contacts: vec![],
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
    fn current_envelope_uses_v3() {
        let encoded = encrypt(&payload(), "correct horse").unwrap();
        let envelope: EncryptedEnvelope = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(envelope.format, "dispatch-settings");
        assert_eq!(envelope.version, 3);
        assert_eq!(
            decrypt(&encoded, "correct horse").unwrap().accounts[0].email,
            "person@example.com"
        );
    }

    #[test]
    fn v1_payload_defaults_availability_preferences() {
        let mut serialized = serde_json::to_value(payload()).unwrap();
        serialized["version"] = serde_json::json!(1);
        serialized["preferences"].as_object_mut().unwrap().remove("availabilityPreferences");
        serialized["preferences"]["aiFeatures"]
            .as_object_mut()
            .unwrap()
            .remove("actionExtraction");
        let decoded: TransferPayload = serde_json::from_value(serialized).unwrap();
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.preferences.availability_preferences.default_duration_minutes, 30);
        assert!(!decoded.preferences.ai_features.action_extraction);
        decoded.validate().unwrap();
    }

    #[test]
    fn version_two_export_imports_with_empty_contacts_and_disabled_contact_ai() {
        let mut legacy=serde_json::to_value(payload()).unwrap();
        legacy["version"]=serde_json::json!(2);
        legacy.as_object_mut().unwrap().remove("contacts");
        legacy["preferences"]["aiFeatures"].as_object_mut().unwrap().remove("contactEnrichment");
        let decoded:TransferPayload=serde_json::from_value(legacy).unwrap();
        decoded.validate().unwrap();
        assert!(decoded.contacts.is_empty());
        assert!(!decoded.preferences.ai_features.contact_enrichment);
    }

    #[test]
    fn an_export_from_before_proactive_suggestions_imports_with_them_off() {
        let mut legacy = serde_json::to_value(payload()).unwrap();
        let features = legacy["preferences"]["aiFeatures"].as_object_mut().unwrap();
        features.remove("proactiveBriefs");
        features.remove("proactiveKnownSendersOnly");
        features.insert("summarize".into(), serde_json::json!(true));
        let decoded: TransferPayload = serde_json::from_value(legacy).unwrap();
        decoded.validate().unwrap();
        assert!(decoded.preferences.ai_features.summarize);
        assert!(!decoded.preferences.ai_features.proactive_briefs);
        assert!(!decoded.preferences.ai_features.proactive_known_senders_only);
    }

    #[test]
    fn an_export_from_before_thread_chat_imports_with_it_off() {
        let mut legacy = serde_json::to_value(payload()).unwrap();
        legacy["preferences"]["aiFeatures"].as_object_mut().unwrap().remove("threadChat");
        legacy["preferences"]["aiFeatures"]["proactiveBriefs"] = serde_json::json!(true);
        let decoded: TransferPayload = serde_json::from_value(legacy).unwrap();
        decoded.validate().unwrap();
        assert!(decoded.preferences.ai_features.proactive_briefs);
        assert!(!decoded.preferences.ai_features.thread_chat);

        let mut current = payload();
        current.preferences.ai_features.thread_chat = true;
        let round_trip = decrypt(&encrypt(&current, "correct horse").unwrap(), "correct horse").unwrap();
        assert!(round_trip.preferences.ai_features.thread_chat);
    }

    #[test]
    fn proactive_suggestion_preferences_round_trip_through_an_encrypted_export() {
        let mut current = payload();
        current.preferences.ai_features.proactive_briefs = true;
        current.preferences.ai_features.proactive_known_senders_only = true;
        let decoded = decrypt(&encrypt(&current, "correct horse").unwrap(), "correct horse").unwrap();
        assert!(decoded.preferences.ai_features.proactive_briefs);
        assert!(decoded.preferences.ai_features.proactive_known_senders_only);
    }

    #[test]
    fn an_export_from_before_the_accent_field_existed_defaults_to_purple() {
        let mut serialized = serde_json::to_value(payload()).unwrap();
        serialized["preferences"]
            .as_object_mut()
            .unwrap()
            .remove("accent");
        let decoded: TransferPayload = serde_json::from_value(serialized).unwrap();
        assert_eq!(decoded.preferences.accent, "purple");
        decoded.validate().unwrap();
    }

    #[test]
    fn preferences_reject_unknown_accent_color() {
        let mut candidate = payload();
        candidate.preferences.accent = "neon".to_string();
        assert_eq!(
            candidate.validate().unwrap_err(),
            "The transfer contains an invalid accent color"
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
        // The original version 1 exporter required and emitted `classify`;
        // the frozen fixture carries it set to true.
        let decoded = decrypt(fixtures::V1_INITIAL, fixtures::PASSWORD).unwrap();
        assert!(decoded.preferences.ai_features.classify);

        // Still emitted, so a re-export keeps the flag for older builds.
        let round_trip = decrypt(&encrypt(&decoded, "correct horse").unwrap(), "correct horse").unwrap();
        assert!(round_trip.preferences.ai_features.classify);
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
    fn an_export_from_before_the_provider_field_existed_defaults_to_gmail() {
        let mut serialized = serde_json::to_value(payload()).unwrap();
        serialized["accounts"] = serde_json::json!([{
            "email": "person@example.com",
            "displayName": "Person",
            "color": "#4285F4",
            "sortOrder": 0
        }]);
        let decoded: TransferPayload = serde_json::from_value(serialized).unwrap();
        assert_eq!(decoded.accounts[0].provider, "gmail");
        decoded.validate().unwrap();
    }

    #[test]
    fn validate_rejects_an_unrecognized_account_provider() {
        let mut candidate = payload();
        candidate.accounts[0].provider = "imap".to_string();
        assert_eq!(
            candidate.validate().unwrap_err(),
            "The transfer contains an unrecognized account provider"
        );
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
    fn snippets_round_trip_through_the_encrypted_transfer() {
        let mut candidate = payload();
        candidate.snippets = vec![TransferSnippet {
            id: "snippet-1".to_string(),
            name: "Zoom link".to_string(),
            body: "We can use my Zoom link: zoom.us/1234567890".to_string(),
            created_at: "2026-03-06T00:00:00Z".to_string(),
        }];
        candidate.validate().unwrap();
        let encoded = encrypt(&candidate, "correct horse").unwrap();
        let decoded = decrypt(&encoded, "correct horse").unwrap();
        assert_eq!(decoded.snippets[0].name, "Zoom link");
    }

    #[test]
    fn an_export_from_before_snippets_existed_defaults_to_an_empty_list() {
        let mut serialized = serde_json::to_value(payload()).unwrap();
        serialized.as_object_mut().unwrap().remove("snippets");
        let decoded: TransferPayload = serde_json::from_value(serialized).unwrap();
        assert!(decoded.snippets.is_empty());
        decoded.validate().unwrap();
    }

    #[test]
    fn validate_rejects_a_duplicate_snippet_id() {
        let mut candidate = payload();
        candidate.snippets = vec![
            TransferSnippet {
                id: "dup".to_string(),
                name: "One".to_string(),
                body: "Body one".to_string(),
                created_at: "2026-03-06T00:00:00Z".to_string(),
            },
            TransferSnippet {
                id: "dup".to_string(),
                name: "Two".to_string(),
                body: "Body two".to_string(),
                created_at: "2026-03-06T00:00:00Z".to_string(),
            },
        ];
        assert_eq!(
            candidate.validate().unwrap_err(),
            "The transfer contains a duplicate Snippet"
        );
    }

    #[test]
    fn transfer_still_rejects_unrecognized_fields() {
        let mut serialized = serde_json::to_value(payload()).unwrap();
        serialized["preferences"]["aiFeatures"]["unexpected"] = serde_json::json!(true);

        assert!(serde_json::from_value::<TransferPayload>(serialized).is_err());
    }

    /// Frozen encrypted exports, one per shape each format version actually
    /// shipped, reconstructed from the historical `TransferPayload` structs
    /// (serde emits fields in declaration order) and sealed with the
    /// unchanged argon2id + XChaCha20-Poly1305 envelope:
    ///
    /// - `v1-initial`: first exporter (a67a66f). No account `provider`, no
    ///   Split Inbox `accountId`, required `classify` flag.
    /// - `v1-final`: last version 1 exporter (d7fd17b). Adds account
    ///   `provider` and Split Inbox `accountId`.
    /// - `v2-initial`: first version 2 exporter (e6bc2dc). Adds
    ///   `availabilityPreferences`.
    /// - `v2-final`: last version 2 exporter (56d1439). Adds `accent`,
    ///   `actionExtraction`, and `snippets`.
    /// - `v3-initial`: first version 3 exporter (29705ca). Adds `contacts`
    ///   and `contactEnrichment`.
    /// - `v3-0.52`: last export before the fast model (0.52.0). Every field
    ///   through `threadChat`, no `aiFastModel`.
    /// - `v3-current`: what this build exports. The only fixture with a
    ///   regenerate helper (`regenerate_current_settings_transfer_fixture`).
    ///
    /// Every file except `v3-current` is frozen: never regenerate or edit
    /// it, because it stands in for a file a user already has on disk.
    mod fixtures {
        pub(super) const PASSWORD: &str = "correct horse battery staple";
        pub(super) const V1_INITIAL: &[u8] =
            include_bytes!("../tests/fixtures/settings-transfer/v1-initial.dispatch-settings");
        pub(super) const V1_FINAL: &[u8] =
            include_bytes!("../tests/fixtures/settings-transfer/v1-final.dispatch-settings");
        pub(super) const V2_INITIAL: &[u8] =
            include_bytes!("../tests/fixtures/settings-transfer/v2-initial.dispatch-settings");
        pub(super) const V2_FINAL: &[u8] =
            include_bytes!("../tests/fixtures/settings-transfer/v2-final.dispatch-settings");
        pub(super) const V3_INITIAL: &[u8] =
            include_bytes!("../tests/fixtures/settings-transfer/v3-initial.dispatch-settings");
        pub(super) const V3_0_52: &[u8] =
            include_bytes!("../tests/fixtures/settings-transfer/v3-0.52.dispatch-settings");
        pub(super) const V3_CURRENT: &[u8] =
            include_bytes!("../tests/fixtures/settings-transfer/v3-current.dispatch-settings");
        pub(super) const V3_CURRENT_PATH: &str = "tests/fixtures/settings-transfer/v3-current.dispatch-settings";
        pub(super) const V3_CURRENT_SALT: [u8; super::SALT_LEN] = [0x60; super::SALT_LEN];
        pub(super) const V3_CURRENT_NONCE: [u8; super::NONCE_LEN] = [0xe0; super::NONCE_LEN];
        /// The webview's `readExportablePreferences()` shape, shared with
        /// `src/userPreferences.test.ts`.
        pub(super) const WEBVIEW_PREFERENCES: &str =
            include_str!("../tests/fixtures/settings-transfer/webview-preferences.json");
    }

    fn envelope_version(bytes: &[u8]) -> u32 {
        serde_json::from_slice::<EncryptedEnvelope>(bytes).unwrap().version
    }

    fn import_fixture(bytes: &[u8]) -> (Database, ImportResult) {
        let database = Database::open_memory();
        let payload = decrypt(bytes, fixtures::PASSWORD).unwrap();
        let result = apply_import(&database, payload).unwrap();
        (database, result)
    }

    fn accounts_of(database: &Database) -> Vec<(String, Option<String>, String, String, i64)> {
        database
            .list_accounts()
            .unwrap()
            .into_iter()
            .map(|a| (a.email, a.display_name, a.color, a.provider, a.sort_order))
            .collect()
    }

    fn split_owners_of(database: &Database) -> Vec<(String, String, String, String)> {
        database
            .list_split_inboxes()
            .unwrap()
            .into_iter()
            .map(|s| (s.id, s.match_kind, s.match_value, s.account_id))
            .collect()
    }

    fn assert_default_availability(preferences: &TransferPreferences) {
        let availability = &preferences.availability_preferences;
        assert_eq!(availability.time_zone, "UTC");
        assert_eq!(availability.default_duration_minutes, 30);
        assert_eq!(availability.slot_increment_minutes, 15);
        let weekdays: Vec<u8> = availability.working_windows.iter().map(|w| w.weekday).collect();
        assert_eq!(weekdays, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn frozen_v1_initial_export_imports_with_migrated_defaults() {
        assert_eq!(envelope_version(fixtures::V1_INITIAL), 1);
        let (database, result) = import_fixture(fixtures::V1_INITIAL);

        let preferences = &result.preferences;
        assert_eq!(preferences.theme, "dark");
        assert_eq!(preferences.accent, "purple");
        assert_eq!(preferences.font_scale, 110);
        assert_eq!(preferences.font_family, "Georgia");
        assert_eq!(preferences.auto_read_delay_seconds, 3);
        assert!(preferences.load_remote_images);
        assert_eq!(preferences.selected_account_id.as_deref(), Some("legacy@example.com"));
        assert!(matches!(preferences.ai_provider, AiProvider::Anthropic));
        assert_eq!(preferences.ai_model, "example-model-v1");
        assert_eq!(preferences.ai_endpoint, "");
        let features = &preferences.ai_features;
        assert!(features.draft_assist && features.summarize && features.classify);
        assert!(!features.action_extraction && !features.contact_enrichment);
        assert!(!features.proactive_briefs && !features.proactive_known_senders_only && !features.thread_chat);
        assert_default_availability(preferences);

        assert_eq!((result.account_count, result.split_inbox_count, result.snippet_count, result.contact_count), (2, 1, 0, 0));
        assert_eq!(
            accounts_of(&database),
            vec![
                ("legacy@example.com".into(), Some("Legacy Person".into()), "#4285F4".into(), "gmail".into(), 0),
                ("second@example.org".into(), None, "#0F9D58".into(), "gmail".into(), 1),
            ]
        );
        assert!(database.list_accounts().unwrap().iter().all(|a| a.status == "needs_reauth"));
        // Version 1 Split Inboxes were global; the owner migrates to the first account.
        assert_eq!(
            split_owners_of(&database),
            vec![("split-v1".into(), "domain".into(), "shop.example.com".into(), "legacy@example.com".into())]
        );
        assert!(database.list_snippets().unwrap().is_empty());
        assert!(database.list_saved_contact_profiles().unwrap().is_empty());
        assert_eq!(database.retention_days().unwrap(), Some(90));
    }

    #[test]
    fn frozen_v1_final_export_keeps_explicit_split_inbox_owners() {
        assert_eq!(envelope_version(fixtures::V1_FINAL), 1);
        let (database, result) = import_fixture(fixtures::V1_FINAL);

        let preferences = &result.preferences;
        assert_eq!(preferences.theme, "light");
        assert_eq!(preferences.accent, "purple");
        assert_eq!(preferences.font_scale, 95);
        assert_eq!(preferences.auto_read_delay_seconds, 0);
        assert!(!preferences.load_remote_images);
        assert_eq!(preferences.selected_account_id, None);
        assert!(matches!(preferences.ai_provider, AiProvider::OpenRouter));
        assert_eq!(preferences.ai_model, "example/model");
        assert_eq!(preferences.ai_endpoint, "https://api.example.test/v1");
        assert!(!preferences.ai_features.draft_assist && preferences.ai_features.summarize);
        assert!(!preferences.ai_features.classify);
        assert_default_availability(preferences);

        assert_eq!(
            accounts_of(&database),
            vec![
                ("owner@example.com".into(), Some("Owner".into()), "#DB4437".into(), "gmail".into(), 0),
                ("work@example.org".into(), Some("Work".into()), "#F4B400".into(), "gmail".into(), 1),
            ]
        );
        assert_eq!(
            split_owners_of(&database),
            vec![
                ("split-owned".into(), "label".into(), "Team".into(), "work@example.org".into()),
                ("split-unowned".into(), "pattern".into(), "newsletter".into(), "owner@example.com".into()),
            ]
        );
        assert_eq!(database.retention_days().unwrap(), None);
    }

    #[test]
    fn frozen_v2_initial_export_imports_availability_without_later_fields() {
        assert_eq!(envelope_version(fixtures::V2_INITIAL), 2);
        let (database, result) = import_fixture(fixtures::V2_INITIAL);

        let preferences = &result.preferences;
        assert_eq!(preferences.theme, "system");
        assert_eq!(preferences.accent, "purple");
        assert_eq!(preferences.font_family, "Inter");
        assert!(matches!(preferences.ai_provider, AiProvider::OpenAi));
        assert!(preferences.ai_features.draft_assist);
        assert!(!preferences.ai_features.action_extraction);
        let availability = &preferences.availability_preferences;
        assert_eq!(availability.time_zone, "America/New_York");
        assert_eq!(availability.default_duration_minutes, 45);
        assert_eq!(availability.slot_increment_minutes, 15);
        let windows: Vec<(u8, &str, &str)> = availability
            .working_windows
            .iter()
            .map(|w| (w.weekday, w.start.as_str(), w.end.as_str()))
            .collect();
        assert_eq!(windows, vec![(1, "08:30", "16:30"), (3, "10:00", "18:00")]);

        assert_eq!((result.account_count, result.split_inbox_count, result.snippet_count, result.contact_count), (1, 1, 0, 0));
        assert_eq!(
            split_owners_of(&database),
            vec![("split-v2".into(), "domain".into(), "calendar.example.com".into(), "planner@example.com".into())]
        );
        assert_eq!(database.retention_days().unwrap(), Some(30));
    }

    #[test]
    fn frozen_v2_final_export_imports_accent_actions_and_snippets() {
        assert_eq!(envelope_version(fixtures::V2_FINAL), 2);
        let (database, result) = import_fixture(fixtures::V2_FINAL);

        let preferences = &result.preferences;
        assert_eq!(preferences.accent, "teal");
        assert!(matches!(preferences.ai_provider, AiProvider::Fireworks));
        assert_eq!(preferences.ai_model, "accounts/example/models/demo");
        assert!(preferences.ai_features.action_extraction);
        assert!(!preferences.ai_features.contact_enrichment);
        assert_eq!(preferences.availability_preferences.time_zone, "Europe/Berlin");
        assert_eq!(preferences.availability_preferences.working_windows.len(), 2);
        assert_eq!(preferences.availability_preferences.default_duration_minutes, 60);

        assert_eq!((result.account_count, result.split_inbox_count, result.snippet_count, result.contact_count), (1, 0, 1, 0));
        let snippets = database.list_snippets().unwrap();
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].id, "snippet-v2");
        assert_eq!(snippets[0].name, "Scheduling");
        assert_eq!(
            snippets[0].body,
            "<p>Here is my calendar:</p>\n<p>https://calendar.example.com/writer</p>"
        );
        assert_eq!(snippets[0].created_at, "2026-09-21T15:00:00+00:00");
        assert!(database.list_saved_contact_profiles().unwrap().is_empty());
        assert_eq!(database.retention_days().unwrap(), Some(365));
    }

    #[test]
    fn frozen_v3_initial_export_imports_contacts_without_proactive_or_chat_flags() {
        assert_eq!(envelope_version(fixtures::V3_INITIAL), 3);
        let (database, result) = import_fixture(fixtures::V3_INITIAL);

        let preferences = &result.preferences;
        assert_eq!(preferences.accent, "graphite");
        assert!(matches!(preferences.ai_provider, AiProvider::Custom));
        assert_eq!(preferences.ai_endpoint, "https://llm.example.test/v1");
        let features = &preferences.ai_features;
        assert!(features.contact_enrichment && features.summarize);
        assert!(!features.proactive_briefs && !features.proactive_known_senders_only && !features.thread_chat);

        assert_eq!(result.contact_count, 1);
        let contacts = database.list_saved_contact_profiles().unwrap();
        assert_eq!(contacts.len(), 1);
        let contact = &contacts[0];
        assert_eq!(contact.id, "contact-v3");
        assert_eq!(contact.display_name.as_deref(), Some("Ada Example"));
        assert_eq!(contact.role.as_deref(), Some("Engineer"));
        assert_eq!(contact.company.as_deref(), Some("Example Co"));
        assert_eq!(contact.location.as_deref(), Some("Remote"));
        assert_eq!(contact.bio.as_deref(), Some("Writes compilers."));
        assert_eq!(contact.notes.as_deref(), Some("Met at the conference."));
        assert_eq!(contact.links, vec!["https://ada.example.com".to_string()]);
        assert_eq!(contact.photo_data, None);
        assert!(contact.favorite);
        let mut addresses = contact.addresses.clone();
        addresses.sort();
        assert_eq!(addresses, vec!["ada.work@example.org".to_string(), "ada@example.com".to_string()]);
        assert_eq!(
            accounts_of(&database),
            vec![("contacts@example.com".into(), None, "#0F9D58".into(), "gmail".into(), 0)]
        );
        assert_eq!(database.retention_days().unwrap(), None);
    }

    /// The payload behind `v3-current`: every field this build knows about,
    /// set away from its default so a dropped or renamed field is visible.
    fn current_fixture_payload() -> TransferPayload {
        TransferPayload {
            version: VERSION,
            exported_at: "2026-09-29T12:00:00+00:00".to_string(),
            preferences: TransferPreferences {
                theme: "dark".to_string(),
                accent: "amber".to_string(),
                font_scale: 125,
                font_family: "Iowan Old Style".to_string(),
                auto_read_delay_seconds: 15,
                load_remote_images: true,
                selected_account_id: Some("current@example.com".to_string()),
                ai_provider: AiProvider::Anthropic,
                ai_model: "example-model-v3".to_string(),
                ai_fast_model: "example-fast-model".to_string(),
                ai_endpoint: String::new(),
                ai_features: AiFeaturePreferences {
                    draft_assist: true,
                    summarize: true,
                    action_extraction: true,
                    contact_enrichment: true,
                    classify: false,
                    proactive_briefs: true,
                    proactive_known_senders_only: true,
                    thread_chat: true,
                },
                availability_preferences: AvailabilityPreferences {
                    time_zone: "Asia/Tokyo".to_string(),
                    working_windows: vec![crate::models::AvailabilityWindow {
                        weekday: 0,
                        start: "07:00".to_string(),
                        end: "11:00".to_string(),
                    }],
                    default_duration_minutes: 25,
                    slot_increment_minutes: 5,
                },
            },
            accounts: vec![
                TransferAccount {
                    email: "current@example.com".to_string(),
                    display_name: Some("Current".to_string()),
                    color: "#123ABC".to_string(),
                    provider: "gmail".to_string(),
                    sort_order: 0,
                },
                TransferAccount {
                    email: "other@example.net".to_string(),
                    display_name: None,
                    color: "#ABCDEF".to_string(),
                    provider: "gmail".to_string(),
                    sort_order: 1,
                },
            ],
            split_inboxes: vec![TransferSplitInbox {
                id: "split-current".to_string(),
                name: "Builds".to_string(),
                match_kind: "pattern".to_string(),
                match_value: "build failed".to_string(),
                sort_order: 0,
                created_at: "2026-09-28T08:00:00+00:00".to_string(),
                account_id: "other@example.net".to_string(),
            }],
            snippets: vec![TransferSnippet {
                id: "snippet-current".to_string(),
                name: "Thanks".to_string(),
                body: "<p>Thanks!</p>".to_string(),
                created_at: "2026-09-27T08:00:00+00:00".to_string(),
            }],
            contacts: vec![TransferContact {
                id: "contact-current".to_string(),
                display_name: Some("Grace Example".to_string()),
                role: None,
                company: Some("Example Navy".to_string()),
                location: None,
                bio: None,
                notes: Some("Prefers email.".to_string()),
                links: vec![],
                photo_data: None,
                favorite: false,
                addresses: vec!["grace@example.com".to_string()],
            }],
            retention_days: Some(365),
        }
    }

    fn seal_current_fixture() -> Vec<u8> {
        let plaintext = serde_json::to_vec(&current_fixture_payload()).unwrap();
        seal(&plaintext, fixtures::PASSWORD, &fixtures::V3_CURRENT_SALT, &fixtures::V3_CURRENT_NONCE).unwrap()
    }

    #[test]
    fn frozen_v3_current_export_imports_every_current_field() {
        assert_eq!(envelope_version(fixtures::V3_CURRENT), VERSION);
        let (database, result) = import_fixture(fixtures::V3_CURRENT);

        let preferences = &result.preferences;
        assert_eq!(preferences.accent, "amber");
        assert_eq!(preferences.ai_model, "example-model-v3");
        assert_eq!(preferences.ai_fast_model, "example-fast-model");
        assert_eq!(preferences.font_family, "Iowan Old Style");
        assert_eq!(preferences.auto_read_delay_seconds, 15);
        let features = &preferences.ai_features;
        assert!(features.draft_assist && features.summarize && features.action_extraction);
        assert!(features.contact_enrichment && features.proactive_briefs);
        assert!(features.proactive_known_senders_only && features.thread_chat);
        assert!(!features.classify);
        assert_eq!(preferences.availability_preferences.time_zone, "Asia/Tokyo");
        assert_eq!(preferences.availability_preferences.slot_increment_minutes, 5);

        assert_eq!((result.account_count, result.split_inbox_count, result.snippet_count, result.contact_count), (2, 1, 1, 1));
        assert_eq!(
            accounts_of(&database),
            vec![
                ("current@example.com".into(), Some("Current".into()), "#123ABC".into(), "gmail".into(), 0),
                ("other@example.net".into(), None, "#ABCDEF".into(), "gmail".into(), 1),
            ]
        );
        assert_eq!(
            split_owners_of(&database),
            vec![("split-current".into(), "pattern".into(), "build failed".into(), "other@example.net".into())]
        );
        assert_eq!(database.list_snippets().unwrap()[0].body, "<p>Thanks!</p>");
        let contacts = database.list_saved_contact_profiles().unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].company.as_deref(), Some("Example Navy"));
        assert_eq!(contacts[0].addresses, vec!["grace@example.com".to_string()]);
        assert_eq!(database.retention_days().unwrap(), Some(365));
    }

    #[test]
    fn frozen_v3_0_52_export_imports_without_a_fast_model() {
        assert_eq!(envelope_version(fixtures::V3_0_52), 3);
        let (database, result) = import_fixture(fixtures::V3_0_52);

        let preferences = &result.preferences;
        preferences.validate().unwrap();
        assert!(matches!(preferences.ai_provider, AiProvider::Anthropic));
        assert_eq!(preferences.ai_model, "example-model-v3");
        assert_eq!(preferences.ai_fast_model, "");
        let features = &preferences.ai_features;
        assert!(features.contact_enrichment && features.thread_chat && features.proactive_briefs);
        assert_eq!(preferences.availability_preferences.time_zone, "Asia/Tokyo");
        assert_eq!((result.account_count, result.split_inbox_count, result.snippet_count, result.contact_count), (2, 1, 1, 1));
        assert_eq!(database.retention_days().unwrap(), Some(365));
    }

    /// Fails whenever this build's export bytes drift from the checked-in
    /// current fixture. That is intended: any change to the export shape
    /// must be deliberate. Before regenerating, copy the existing
    /// `v3-current.dispatch-settings` to a new frozen file (for example
    /// `v3-<release>.dispatch-settings`) with its own import test, so the
    /// preceding schema keeps regression coverage as AGENTS.md requires.
    #[test]
    fn this_build_still_exports_the_current_fixture_bytes() {
        assert!(
            seal_current_fixture() == fixtures::V3_CURRENT,
            "the export shape changed; freeze the old v3-current fixture, then run \
             `cargo test --lib transfer::tests::regenerate_current_settings_transfer_fixture -- --ignored`"
        );
    }

    /// Regenerates only `v3-current`. Run deliberately, after freezing the
    /// previous file (see `this_build_still_exports_the_current_fixture_bytes`):
    /// `cargo test --lib transfer::tests::regenerate_current_settings_transfer_fixture -- --ignored`.
    /// The older fixtures have no regenerate helper by design.
    #[test]
    #[ignore]
    fn regenerate_current_settings_transfer_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(fixtures::V3_CURRENT_PATH);
        std::fs::write(path, seal_current_fixture()).unwrap();
    }

    #[test]
    fn frozen_fixtures_reject_the_wrong_password() {
        for fixture in [fixtures::V1_INITIAL, fixtures::V2_FINAL, fixtures::V3_CURRENT] {
            assert_eq!(
                decrypt(fixture, "not the fixture password").unwrap_err(),
                "The password is incorrect or the settings export is damaged"
            );
        }
    }

    #[test]
    fn an_envelope_from_a_future_format_version_is_rejected() {
        let mut envelope: serde_json::Value = serde_json::from_slice(fixtures::V3_CURRENT).unwrap();
        envelope["version"] = serde_json::json!(VERSION + 1);
        let future = serde_json::to_vec_pretty(&envelope).unwrap();
        assert_eq!(
            decrypt(&future, fixtures::PASSWORD).unwrap_err(),
            "This ThreeStrands settings export uses an unsupported format"
        );

        envelope["version"] = serde_json::json!(0);
        assert_eq!(
            decrypt(&serde_json::to_vec(&envelope).unwrap(), fixtures::PASSWORD).unwrap_err(),
            "This ThreeStrands settings export uses an unsupported format"
        );
    }

    #[test]
    fn a_future_payload_version_inside_a_current_envelope_is_rejected_on_import() {
        let mut plaintext = serde_json::to_value(current_fixture_payload()).unwrap();
        plaintext["version"] = serde_json::json!(VERSION + 1);
        let sealed = seal(
            &serde_json::to_vec(&plaintext).unwrap(),
            fixtures::PASSWORD,
            &fixtures::V3_CURRENT_SALT,
            &fixtures::V3_CURRENT_NONCE,
        )
        .unwrap();
        let payload = decrypt(&sealed, fixtures::PASSWORD).unwrap();
        let database = Database::open_memory();
        assert_eq!(
            apply_import(&database, payload).unwrap_err(),
            format!("Unsupported transfer version {}", VERSION + 1)
        );
        assert!(database.list_accounts().unwrap().is_empty(), "a rejected import must not write");
    }

    /// Keys of a JSON object tree as dotted paths, e.g. `aiFeatures.summarize`.
    /// Arrays contribute the keys of their first element under `path[]`.
    fn key_paths(value: &serde_json::Value, prefix: &str, out: &mut std::collections::BTreeSet<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
                    out.insert(path.clone());
                    key_paths(child, &path, out);
                }
            }
            serde_json::Value::Array(items) => {
                if let Some(first) = items.first() {
                    key_paths(first, &format!("{prefix}[]"), out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn webview_preferences_fixture_matches_the_native_transfer_preferences() {
        let fixture: serde_json::Value = serde_json::from_str(fixtures::WEBVIEW_PREFERENCES).unwrap();
        // deny_unknown_fields: the webview may not send a key the native
        // side would reject, and every required native key must be present.
        let preferences: TransferPreferences = serde_json::from_value(fixture.clone()).unwrap();
        preferences.validate().unwrap();

        let mut sent = std::collections::BTreeSet::new();
        key_paths(&fixture, "", &mut sent);
        let mut native = std::collections::BTreeSet::new();
        key_paths(&serde_json::to_value(&preferences).unwrap(), "", &mut native);
        // `classify` is the one native-only key: kept for version 1
        // compatibility, no longer read or written by the webview.
        native.remove("aiFeatures.classify");
        assert_eq!(sent, native, "a native transfer preference is missing from the webview export (or vice versa)");
    }
}
