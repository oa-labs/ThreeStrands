//! Contacts, contact groups, keep-in-touch, and sender context.

use crate::models::{
    ContactActivity, ContactFiles, ContactGroup, ContactGroupRecipients, ContactProfile,
    ContactRecord, ContactSuggestion, ContactTimelineItem, SaveContactRequest, ThreadTask,
};
use tauri::State;

use crate::{contact_interchange, db, models, sender_trust};
use crate::{
    database_result, kick_replicated_sync, record_synced_value, run_database_task, AppState,
};

#[tauri::command]
pub(crate) async fn list_contact_suggestions(
    account_id: String,
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactSuggestion>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_suggestions(&account_id, &query, limit.unwrap_or(8))).await
}

#[tauri::command]
pub(crate) async fn list_contact_profiles(
    query: String,
    limit: Option<usize>,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactProfile>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_profiles_for_account(&query, limit.unwrap_or(500), account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn resolve_contact_ids(
    emails: Vec<String>,
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_ids_for_addresses(&emails)).await
}

#[tauri::command]
pub(crate) async fn get_contact_profile(
    id: String,
    state: State<'_, AppState>,
) -> Result<Option<ContactProfile>, String> {
    let database = state.database.clone();
    run_database_task(move || database.get_contact_profile(&id)).await
}

#[tauri::command(async)]
pub(crate) fn save_contact_profile(
    request: SaveContactRequest,
    state: State<'_, AppState>,
) -> Result<ContactProfile, String> {
    let profile = state.database.save_contact_profile(&request)?;
    record_synced_value(
        &state,
        threestrands_sync_protocol::EntityType::Contact,
        &profile.id,
        &ContactRecord::from(&profile),
        None,
    )?;
    Ok(profile)
}

#[tauri::command]
pub(crate) async fn preview_contact_import() -> Result<Option<contact_interchange::ContactImportPreview>, String> {
    use std::io::Read;
    let Some(file) = rfd::AsyncFileDialog::new().set_title("Import contacts")
        .add_filter("Contact files", &["csv", "vcf", "vcard"]).pick_file().await else { return Ok(None); };
    let path = file.path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let format = match path.extension().and_then(|extension| extension.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
            "csv" => contact_interchange::ContactFormat::Csv,
            "vcf" | "vcard" => contact_interchange::ContactFormat::Vcard,
            _ => return Err("Choose a CSV or vCard file".into()),
        };
        let mut data = Vec::new();
        std::fs::File::open(path).map_err(|error| error.to_string())?
            .take(contact_interchange::MAX_IMPORT_BYTES + 1).read_to_end(&mut data).map_err(|error| error.to_string())?;
        if data.len() as u64 > contact_interchange::MAX_IMPORT_BYTES { return Err("Contact files must be at most 10 MiB".into()); }
        let text = String::from_utf8(data).map_err(|_| "Save the contact file with UTF-8 encoding")?;
        contact_interchange::parse(&text, format).map(Some)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn import_contacts(contacts: Vec<SaveContactRequest>, state: State<'_, AppState>) -> Result<db::contact_management::ContactImportResult, String> {
    let database = state.database.clone();
    let result = run_database_task(move || database.import_contacts(&contacts)).await?;
    kick_replicated_sync(&state);
    Ok(result)
}

#[tauri::command]
pub(crate) async fn export_contacts(format: contact_interchange::ContactFormat, state: State<'_, AppState>) -> Result<bool, String> {
    let database = state.database.clone();
    let contacts = run_database_task(move || database.list_saved_contact_profiles()).await?;
    if contacts.is_empty() { return Err("Save a contact before exporting".into()); }
    let (extension, label) = match format { contact_interchange::ContactFormat::Csv => ("csv", "CSV"), contact_interchange::ContactFormat::Vcard => ("vcf", "vCard") };
    let Some(file) = rfd::AsyncFileDialog::new().set_title("Export saved contacts")
        .set_file_name(format!("ThreeStrands Contacts.{extension}"))
        .add_filter(label, &[extension]).save_file().await else { return Ok(false); };
    let path = file.path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = contact_interchange::export(&contacts, format)?;
        std::fs::write(path, bytes).map_err(|error| error.to_string())?;
        Ok(true)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn merge_contacts(target_id: String, source_ids: Vec<String>, state: State<'_, AppState>) -> Result<ContactProfile, String> {
    let database = state.database.clone();
    let profile = run_database_task(move || database.merge_contacts(&target_id, &source_ids)).await?;
    kick_replicated_sync(&state);
    Ok(profile)
}

#[tauri::command]
pub(crate) async fn list_contact_suppressions(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_suppressions()).await
}

#[tauri::command]
pub(crate) async fn set_contact_suppressed(email: String, suppressed: bool, state: State<'_, AppState>) -> Result<(), String> {
    let database = state.database.clone();
    run_database_task(move || database.set_contact_suppressed(&email, suppressed)).await
}

#[tauri::command]
pub(crate) async fn list_keep_in_touch(state: State<'_, AppState>) -> Result<Vec<ContactProfile>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_keep_in_touch()).await
}

fn record_synced_contact(state: &State<'_, AppState>, profile: &ContactProfile) -> Result<(), String> {
    record_synced_value(
        state,
        threestrands_sync_protocol::EntityType::Contact,
        &profile.id,
        &ContactRecord::from(profile),
        None,
    )
}

#[tauri::command(async)]
pub(crate) fn set_keep_in_touch(
    ids: Vec<String>,
    interval_days: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactProfile>, String> {
    let profiles = state.database.set_keep_in_touch(&ids, interval_days)?;
    for profile in &profiles {
        record_synced_contact(&state, profile)?;
    }
    Ok(profiles)
}

#[tauri::command(async)]
pub(crate) fn snooze_keep_in_touch(
    id: String,
    until: Option<String>,
    state: State<'_, AppState>,
) -> Result<ContactProfile, String> {
    let profile = state.database.snooze_keep_in_touch(&id, until.as_deref())?;
    record_synced_contact(&state, &profile)?;
    Ok(profile)
}

#[tauri::command(async)]
pub(crate) fn mark_contacted(id: String, state: State<'_, AppState>) -> Result<ContactProfile, String> {
    let profile = state.database.mark_contacted(&id)?;
    record_synced_contact(&state, &profile)?;
    Ok(profile)
}

#[tauri::command(async)]
pub(crate) fn delete_contact_profile(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let groups = state.database.contact_group_ids_for_contact(&id)?;
    state.database.delete_contact_profile(&id)?;
    state
        .database
        .record_local_entity_deletion(threestrands_sync_protocol::EntityType::Contact, &id)?;
    for group_id in groups {
        let fields = std::collections::BTreeSet::from([db::contact_groups::member_field(&id)]);
        record_contact_group_fields(&state, &group_id, fields)?;
    }
    kick_replicated_sync(&state);
    Ok(())
}

/// Replicates the named fields of a group's sync record. A removed member
/// is absent from the record, so naming its field unsets it.
fn record_contact_group_fields(
    state: &State<'_, AppState>,
    id: &str,
    fields: std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if fields.is_empty() {
        return Ok(());
    }
    let Some(record) = state.database.contact_group_record(id)? else {
        return Ok(());
    };
    record_synced_value(state, threestrands_sync_protocol::EntityType::ContactGroup, id, &record, Some(fields))
}

fn record_contact_group_write(
    state: &State<'_, AppState>,
    write: db::contact_groups::ContactGroupWrite,
) -> Result<ContactGroup, String> {
    for profile in &write.saved_contacts {
        record_synced_contact(state, profile)?;
    }
    record_contact_group_fields(state, &write.group.id, write.fields)?;
    Ok(write.group)
}

#[tauri::command]
pub(crate) async fn list_contact_groups(state: State<'_, AppState>) -> Result<Vec<ContactGroup>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_groups()).await
}

#[tauri::command]
pub(crate) async fn list_contact_group_recipients(state: State<'_, AppState>) -> Result<Vec<ContactGroupRecipients>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_group_recipients()).await
}

#[tauri::command(async)]
pub(crate) fn create_contact_group(
    name: String,
    contact_ids: Vec<String>,
    emails: Vec<String>,
    state: State<'_, AppState>,
) -> Result<ContactGroup, String> {
    let write = state.database.create_contact_group(&name, &contact_ids, &emails)?;
    record_contact_group_write(&state, write)
}

#[tauri::command(async)]
pub(crate) fn rename_contact_group(id: String, name: String, state: State<'_, AppState>) -> Result<ContactGroup, String> {
    let write = state.database.rename_contact_group(&id, &name)?;
    record_contact_group_write(&state, write)
}

#[tauri::command(async)]
pub(crate) fn add_contact_group_members(
    id: String,
    contact_ids: Vec<String>,
    emails: Vec<String>,
    state: State<'_, AppState>,
) -> Result<ContactGroup, String> {
    let write = state.database.add_contact_group_members(&id, &contact_ids, &emails)?;
    record_contact_group_write(&state, write)
}

#[tauri::command(async)]
pub(crate) fn remove_contact_group_members(
    id: String,
    contact_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<ContactGroup, String> {
    let write = state.database.remove_contact_group_members(&id, &contact_ids)?;
    record_contact_group_write(&state, write)
}

#[tauri::command(async)]
pub(crate) fn delete_contact_group(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.delete_contact_group(&id)?;
    state
        .database
        .record_local_entity_deletion(threestrands_sync_protocol::EntityType::ContactGroup, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn contact_timeline(
    id: String,
    offset: usize,
    limit: usize,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactTimelineItem>, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_timeline_for_account(&id, offset, limit, account_id.as_deref())).await
}

#[tauri::command]
pub(crate) async fn contact_activity(id: String, state: State<'_, AppState>) -> Result<ContactActivity, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_activity(&id)).await
}

#[tauri::command]
pub(crate) async fn sender_history(account_id: String, email: String, limit: Option<usize>, state: State<'_, AppState>) -> Result<models::SenderHistory, String> {
    let database = state.database.clone();
    run_database_task(move || database.sender_history(&account_id, &email, limit.unwrap_or(db::MAX_SENDER_THREADS))).await
}

#[tauri::command]
pub(crate) async fn message_trust(message_id: String, state: State<'_, AppState>) -> Result<Option<sender_trust::MessageTrust>, String> {
    let database = state.database.clone();
    run_database_task(move || database.message_trust(&message_id)).await
}

#[tauri::command]
pub(crate) async fn sender_familiarity(message_id: String, email: String, name: String, state: State<'_, AppState>) -> Result<models::SenderFamiliarity, String> {
    let database = state.database.clone();
    run_database_task(move || database.sender_familiarity(&message_id, &email, &name)).await
}

#[tauri::command]
pub(crate) async fn context_candidates(query: models::ContextQuery, state: State<'_, AppState>) -> Result<Vec<models::WorkspaceConversation>, String> {
    let database = state.database.clone();
    run_database_task(move || database.context_candidates(&query)).await
}

#[tauri::command]
pub(crate) async fn contact_files(id: String, limit: usize, state: State<'_, AppState>) -> Result<ContactFiles, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_files(&id, limit)).await
}

#[tauri::command]
pub(crate) async fn list_contact_tasks(id: String, state: State<'_, AppState>) -> Result<Vec<ThreadTask>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_tasks(&id)).await
}

#[tauri::command(async)]
pub(crate) fn pin_contact(
    account_id: String,
    email: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    database_result(
        state
            .database
            .pin_contact(&account_id, &email, display_name.as_deref()),
    )?;
    if let Some(profile) = state
        .database
        .list_contact_profiles(&email, 100)?
        .into_iter()
        .find(|profile| {
            profile
                .addresses
                .iter()
                .any(|address| address.eq_ignore_ascii_case(&email))
        })
    {
        record_synced_value(
            &state,
            threestrands_sync_protocol::EntityType::Contact,
            &profile.id,
            &ContactRecord::from(&profile),
            None,
        )?;
    }
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn unpin_contact(
    account_id: String,
    email: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    database_result(state.database.unpin_contact(&account_id, &email))?;
    if let Some(profile) = state
        .database
        .list_contact_profiles(&email, 100)?
        .into_iter()
        .find(|profile| {
            profile
                .addresses
                .iter()
                .any(|address| address.eq_ignore_ascii_case(&email))
        })
    {
        if !profile.id.starts_with("derived:") {
            record_synced_value(
                &state,
                threestrands_sync_protocol::EntityType::Contact,
                &profile.id,
                &ContactRecord::from(&profile),
                None,
            )?;
        }
    }
    Ok(())
}
