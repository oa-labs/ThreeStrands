//! Read-only OS address book adapter. Never writes to CNContactStore.
//! Cache and selections are device-local; only deliberate local copies sync.
use crate::db::Database;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

static ACCESS: Mutex<()> = Mutex::new(());
pub(crate) const MAX_SYSTEM_CONTACTS: usize = 100_000;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContactSource {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemContact {
    pub id: String,
    pub source_id: String,
    pub display_name: Option<String>,
    pub addresses: Vec<String>,
    pub company: Option<String>,
    pub role: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactConnection {
    pub sources: Vec<ContactSource>,
    pub selected_sources: Vec<String>,
    pub refreshed_at: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemContactsStatus {
    pub supported: bool,
    pub permission: String,
    pub connection: ContactConnection,
    pub contact_count: usize,
}

trait AddressBook {
    fn supported(&self) -> bool;
    fn permission(&self) -> String;
    fn request(&self) -> Result<(), String>;
    fn sources(&self) -> Result<Vec<ContactSource>, String>;
    fn contacts(&self, selected: &[String]) -> Result<Vec<SystemContact>, String>;
}
struct Platform;

fn status_with(db: &Database, book: &impl AddressBook) -> Result<SystemContactsStatus, String> {
    let permission = book.permission();
    // Revocation immediately removes provider data on the next status/refresh.
    if !matches!(permission.as_str(), "authorized" | "limited") {
        db.clear_system_contact_cache().map_err(|e| e.to_string())?;
    }
    Ok(SystemContactsStatus {
        supported: book.supported(),
        permission,
        connection: db.system_contact_connection().map_err(|e| e.to_string())?,
        contact_count: db.system_contact_count().map_err(|e| e.to_string())?,
    })
}
fn discover_with(db: &Database, book: &impl AddressBook) -> Result<SystemContactsStatus, String> {
    if !book.supported() {
        return status_with(db, book);
    }
    book.request()?;
    if !matches!(book.permission().as_str(), "authorized" | "limited") {
        db.clear_system_contact_cache().map_err(|e| e.to_string())?;
        return Err("Contacts access is off. Allow ThreeStrands in System Settings → Privacy & Security → Contacts.".into());
    }
    let mut connection = db.system_contact_connection().map_err(|e| e.to_string())?;
    connection.sources = book.sources()?;
    db.store_system_contact_connection(&connection)
        .map_err(|e| e.to_string())?;
    status_with(db, book)
}
fn refresh_with(
    db: &Database,
    book: &impl AddressBook,
    selected: Option<Vec<String>>,
) -> Result<SystemContactsStatus, String> {
    let status = status_with(db, book)?;
    if let Some(selected) = selected.as_ref() {
        if selected.is_empty() {
            db.disconnect_system_contacts().map_err(|e| e.to_string())?;
            return status_with(db, book);
        }
    }
    if !status.supported || !matches!(status.permission.as_str(), "authorized" | "limited") {
        return Ok(status);
    }
    let mut connection = status.connection;
    connection.selected_sources = selected.unwrap_or(connection.selected_sources);
    if connection.selected_sources.is_empty() {
        return status_with(db, book);
    }
    connection.sources = book.sources()?;
    // Sources removed from macOS are removed locally, not treated as a failed sync.
    connection
        .selected_sources
        .retain(|id| connection.sources.iter().any(|source| source.id == *id));
    let mut seen = std::collections::HashSet::new();
    connection
        .selected_sources
        .retain(|id| seen.insert(id.clone()));
    let contacts = match book.contacts(&connection.selected_sources) {
        Ok(contacts) => contacts,
        Err(error) => {
            status_with(db, book)?;
            return Err(error);
        }
    };
    // Check again so a permission change while reading cannot publish stale data.
    if !matches!(book.permission().as_str(), "authorized" | "limited") {
        return status_with(db, book);
    }
    connection.refreshed_at = Some(chrono::Utc::now().to_rfc3339());
    db.replace_system_contacts(&connection, &contacts)
        .map_err(|e| e.to_string())?;
    status_with(db, book)
}

async fn run(
    db: Arc<Database>,
    operation: u8,
    selected: Option<Vec<String>>,
) -> Result<SystemContactsStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = ACCESS
            .lock()
            .map_err(|_| "Contacts connection is unavailable")?;
        match operation {
            1 => discover_with(&db, &Platform),
            2 => refresh_with(&db, &Platform, selected),
            _ => status_with(&db, &Platform),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn system_contacts_status(
    state: State<'_, crate::AppState>,
) -> Result<SystemContactsStatus, String> {
    run(state.database.clone(), 0, None).await
}
#[tauri::command]
pub async fn discover_system_contacts(
    state: State<'_, crate::AppState>,
) -> Result<SystemContactsStatus, String> {
    run(state.database.clone(), 1, None).await
}
#[tauri::command]
pub async fn refresh_system_contacts(
    selected_sources: Option<Vec<String>>,
    state: State<'_, crate::AppState>,
    app: tauri::AppHandle,
) -> Result<SystemContactsStatus, String> {
    let result = run(state.database.clone(), 2, selected_sources).await;
    let _ = app.emit("system-contacts-changed", ());
    result
}
pub(crate) fn spawn(db: Arc<Database>, handle: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(error) = run(db.clone(), 2, None).await {
                log::warn!("Mac Contacts refresh: {error}");
            }
            let _ = handle.emit("system-contacts-changed", ());
            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = (db, handle);
}

#[cfg(not(target_os = "macos"))]
impl AddressBook for Platform {
    fn supported(&self) -> bool {
        false
    }
    fn permission(&self) -> String {
        "unsupported".into()
    }
    fn request(&self) -> Result<(), String> {
        Err("System contacts are currently available only on macOS".into())
    }
    fn sources(&self) -> Result<Vec<ContactSource>, String> {
        Ok(vec![])
    }
    fn contacts(&self, _: &[String]) -> Result<Vec<SystemContact>, String> {
        Ok(vec![])
    }
}
#[cfg(target_os = "macos")]
#[path = "system_contacts_macos.rs"]
mod macos;

#[cfg(test)]
#[path = "system_contacts_tests.rs"]
mod tests;
