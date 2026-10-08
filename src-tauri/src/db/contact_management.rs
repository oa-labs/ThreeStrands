//! Atomic address-book imports and explicit merges. Suppression is local to
//! this installation and intentionally independent of saved-profile lifetime.
use super::contacts::save_contact_on;
use super::{Database, DbResult};
use crate::models::{ContactProfile, ContactRecord, KeepInTouch, SaveContactRequest};
use crate::sync_state::{
    record_replicated_deletion_in_transaction, record_replicated_write_in_transaction,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use std::collections::BTreeSet;
use threestrands_sync_protocol::EntityType;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactImportResult {
    pub imported: usize,
    pub skipped: usize,
}

fn read_record(connection: &Connection, id: &str) -> DbResult<ContactRecord> {
    let mut record = connection.query_row(
        "SELECT id,display_name,role,company,location,bio,notes,links_json,photo_data,favorite,birthday,kit_interval_days,kit_started_at,kit_snoozed_until,kit_snoozed_at,kit_last_touch_at FROM contacts WHERE id=?1", [id],
        |row| Ok(ContactRecord {
            id: row.get(0)?, display_name: row.get(1)?, role: row.get(2)?, company: row.get(3)?, location: row.get(4)?, bio: row.get(5)?, notes: row.get(6)?,
            links: serde_json::from_str(&row.get::<_, String>(7)?).map_err(|error| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error)))?,
            photo_data: row.get(8)?, favorite: row.get(9)?, birthday: row.get(10)?, addresses: Vec::new(),
            keep_in_touch: KeepInTouch { interval_days: row.get(11)?, started_at: row.get(12)?, snoozed_until: row.get(13)?, snoozed_at: row.get(14)?, last_touch_at: row.get(15)? },
        })
    ).optional()?.ok_or("Saved contact not found")?;
    let mut statement = connection.prepare(
        "SELECT email FROM contact_addresses WHERE contact_id=?1 ORDER BY position,rowid",
    )?;
    record.addresses = statement
        .query_map([id], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(record)
}

fn record_contact(tx: &Transaction, id: &str) -> DbResult<()> {
    let value = serde_json::to_value(read_record(tx, id)?).map_err(|error| error.to_string())?;
    let fields = value
        .as_object()
        .ok_or("Invalid contact")?
        .keys()
        .cloned()
        .collect();
    record_replicated_write_in_transaction(tx, EntityType::Contact, id, &fields, &value)
}

fn merge_text(target: &mut Option<String>, other: Option<String>) {
    if let Some(other) = other.filter(|value| !value.is_empty()) {
        match target {
            Some(current) if current != &other => {
                current.push_str("\n\n");
                current.push_str(&other);
            }
            None => *target = Some(other),
            _ => {}
        }
    }
}

impl Database {
    /// All accepted rows and their replica writes commit together. A record
    /// sharing any existing address is skipped, never silently overwritten.
    pub fn import_contacts(
        &self,
        contacts: &[SaveContactRequest],
    ) -> DbResult<ContactImportResult> {
        if contacts.len() > crate::contact_interchange::MAX_IMPORT_CONTACTS {
            return Err("Too many contacts in one import".into());
        }
        let sync = self
            .replicated_sync_active()
            .map_err(super::DatabaseError::from)?;
        self.with_transaction(|tx| {
            let mut result = ContactImportResult {
                imported: 0,
                skipped: 0,
            };
            for contact in contacts {
                // IDs and reminder state from a file never select existing records.
                let mut contact = contact.clone();
                contact.id = None;
                contact.keep_in_touch = None;
                let mut duplicate = false;
                for email in &contact.addresses {
                    duplicate |= tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM contact_addresses WHERE email=?1)",
                        [email.trim().to_ascii_lowercase()],
                        |row| row.get::<_, bool>(0),
                    )?;
                }
                if duplicate {
                    result.skipped += 1;
                    continue;
                }
                let id = save_contact_on(tx, &contact)?;
                if sync {
                    record_contact(tx, &id)?;
                }
                result.imported += 1;
            }
            Ok(result)
        })
    }

    /// `target_id` survives; the other saved profiles are removed. Address,
    /// group, profile, and replica changes are one transaction.
    pub fn merge_contacts(
        &self,
        target_id: &str,
        source_ids: &[String],
    ) -> DbResult<ContactProfile> {
        let sources: BTreeSet<_> = source_ids.iter().cloned().collect();
        if sources.is_empty() || sources.len() > 50 || sources.contains(target_id) {
            return Err("Choose a retained contact and 1 to 50 other saved contacts".into());
        }
        let sync = self
            .replicated_sync_active()
            .map_err(super::DatabaseError::from)?;
        self.with_transaction(|tx| {
            let mut target = read_record(tx, target_id)?;
            let mut group_fields = std::collections::BTreeMap::<String, BTreeSet<String>>::new();
            for source_id in &sources {
                let source = read_record(tx, source_id)?;
                let mut conflicts = Vec::new();
                for (label, retained, incoming) in [
                    ("Name", &mut target.display_name, source.display_name.clone()),
                    ("Role", &mut target.role, source.role), ("Company", &mut target.company, source.company),
                    ("Location", &mut target.location, source.location), ("Birthday", &mut target.birthday, source.birthday),
                ] {
                    if retained.is_none() { *retained = incoming; }
                    else if let Some(incoming) = incoming.filter(|value| retained.as_ref() != Some(value)) { conflicts.push(format!("{label}: {incoming}")); }
                }
                merge_text(&mut target.bio, source.bio);
                merge_text(&mut target.notes, source.notes);
                if !conflicts.is_empty() { merge_text(&mut target.notes, Some(format!("Merged profile details:\n{}", conflicts.join("\n")))); }
                for address in source.addresses { if !target.addresses.contains(&address) { target.addresses.push(address); } }
                for link in source.links { if !target.links.contains(&link) { target.links.push(link); } }
                target.favorite |= source.favorite;
                if target.photo_data.is_none() { target.photo_data = source.photo_data; }
                let latest_touch = [target.keep_in_touch.last_touch_at.clone(), source.keep_in_touch.last_touch_at.clone()]
                    .into_iter().flatten().max_by_key(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
                if target.keep_in_touch.interval_days.is_none() { target.keep_in_touch = source.keep_in_touch; }
                target.keep_in_touch.last_touch_at = latest_touch;
                let mut statement = tx.prepare("SELECT group_id FROM contact_group_members WHERE contact_id=?1")?;
                let groups = statement.query_map([source_id], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                drop(statement);
                for group in groups {
                    tx.execute("INSERT OR IGNORE INTO contact_group_members(group_id,contact_id) VALUES(?1,?2)", params![group, target_id])?;
                    group_fields.entry(group).or_default().extend([super::contact_groups::member_field(source_id), super::contact_groups::member_field(target_id)]);
                }
                tx.execute("DELETE FROM contact_group_members WHERE contact_id=?1", [source_id])?;
                // Free the addresses before validating/writing the retained profile.
                tx.execute("DELETE FROM contacts WHERE id=?1", [source_id])?;
                if sync { record_replicated_deletion_in_transaction(tx, EntityType::Contact, source_id)?; }
            }
            let request = SaveContactRequest { id: Some(target.id), display_name: target.display_name, role: target.role, company: target.company, location: target.location, bio: target.bio, notes: target.notes, links: target.links, photo_data: target.photo_data, favorite: target.favorite, addresses: target.addresses, birthday: target.birthday, keep_in_touch: Some(target.keep_in_touch) };
            save_contact_on(tx, &request)?;
            if sync { record_contact(tx, target_id)?; }
            for (group, fields) in group_fields {
                tx.execute("UPDATE contact_groups SET updated_at=?2 WHERE id=?1", params![group, chrono::Utc::now().to_rfc3339()])?;
                if sync {
                    // Only changed memberships are written, preserving concurrent edits.
                    let payload = super::contact_groups::contact_group_record_on(tx, &group)?.ok_or("Contact group not found")?;
                    let mut fields = fields;
                    let has_replica: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='contact_group' AND entity_id=?1)", [&group], |row| row.get(0))?;
                    if !has_replica { fields.extend(payload.as_object().unwrap().keys().cloned()); }
                    record_replicated_write_in_transaction(tx, EntityType::ContactGroup, &group, &fields, &payload)?;
                }
            }
            Ok(())
        })?;
        self.get_contact_profile(target_id)?
            .ok_or_else(|| "Merged contact could not be loaded".into())
    }

    pub fn list_contact_suppressions(&self) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT email FROM contact_suggestion_suppressions ORDER BY email")?;
            let values = statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            Ok(values)
        })
    }

    pub fn set_contact_suppressed(&self, email: &str, suppressed: bool) -> DbResult<()> {
        let email = email.trim().to_ascii_lowercase();
        let parsed =
            crate::correspondence::addresses(&email).map_err(super::DatabaseError::from)?;
        if email.len() > 320
            || parsed.len() != 1
            || parsed[0].1 != email
            || email.chars().any(char::is_control)
        {
            return Err("Enter one valid email address".into());
        }
        self.with_connection(|connection| {
            if suppressed {
                connection.execute(
                    "INSERT OR IGNORE INTO contact_suggestion_suppressions(email) VALUES(?1)",
                    [&email],
                )?;
            } else {
                connection.execute(
                    "DELETE FROM contact_suggestion_suppressions WHERE email=?1",
                    [&email],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str, email: &str) -> SaveContactRequest {
        SaveContactRequest {
            id: Some(id.into()),
            display_name: Some(id.into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec![email.into()],
            birthday: None,
            keep_in_touch: None,
        }
    }
    fn sync_into(to: &Database, from: &Database) {
        let touched = to
            .merge_replica_state(&from.load_replica_state().unwrap())
            .unwrap();
        to.materialize_touched_entities(&touched).unwrap();
        let pending: i64 = to
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM pending_entity_materializations",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn imports_skip_duplicate_ownership_and_rollback_profile_and_replica_changes_on_failure() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        let first = request("ignored", "one@example.com");
        let mut second = request("ignored", "two@example.com");
        let imported = db
            .import_contacts(&[first.clone(), first.clone(), second.clone()])
            .unwrap();
        assert_eq!((imported.imported, imported.skipped), (2, 1));
        assert!(db.get_contact_profile("ignored").unwrap().is_none());
        let before = db.load_replica_state().unwrap();
        let count = db.list_saved_contact_profiles().unwrap().len();
        second.addresses = vec!["three@example.com".into()];
        let mut invalid = request("bad", "four@example.com");
        invalid.links.push("javascript:alert(1)".into());
        assert!(db.import_contacts(&[second, invalid]).is_err());
        assert_eq!(db.list_saved_contact_profiles().unwrap().len(), count);
        assert_eq!(db.load_replica_state().unwrap(), before);
        let peer = Database::open_memory();
        sync_into(&peer, &db);
        assert_eq!(peer.list_saved_contact_profiles().unwrap().len(), 2);
    }

    #[test]
    fn merge_combines_profiles_and_groups_atomically_and_projects_on_existing_and_new_peers() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        // The target sorts before the source: projection must release source
        // email ownership before writing the combined target.
        let mut target = request("a-target", "one@example.com");
        target.notes = Some("First note".into());
        target.keep_in_touch = Some(KeepInTouch {
            interval_days: Some(30),
            started_at: Some("2026-09-01T00:00:00Z".into()),
            ..Default::default()
        });
        let mut source = request("z-source", "two@example.com");
        source.company = Some("Company".into());
        source.notes = Some("Second note".into());
        source.favorite = true;
        source.links = vec!["https://example.com".into()];
        source.birthday = Some("12-09".into());
        source.keep_in_touch = Some(KeepInTouch {
            interval_days: Some(7),
            started_at: Some("2026-09-02T00:00:00Z".into()),
            last_touch_at: Some("2026-09-10T05:00:00-04:00".into()),
            ..Default::default()
        });
        for item in [&target, &source] {
            db.save_contact_profile(item).unwrap();
            db.with_transaction(|tx| record_contact(tx, item.id.as_deref().unwrap()))
                .unwrap();
        }
        let group = db
            .create_contact_group("Friends", &["z-source".into()], &[])
            .unwrap()
            .group;
        let value = db.contact_group_record(&group.id).unwrap().unwrap();
        db.record_local_entity_write(EntityType::ContactGroup, &group.id, value, None)
            .unwrap();
        let peer = Database::open_memory();
        sync_into(&peer, &db);
        let merged = db.merge_contacts("a-target", &["z-source".into()]).unwrap();
        assert_eq!(merged.addresses, ["one@example.com", "two@example.com"]);
        assert_eq!(merged.company.as_deref(), Some("Company"));
        assert!(merged.favorite);
        let notes = merged.notes.as_deref().unwrap();
        assert!(
            notes.contains("First note")
                && notes.contains("Second note")
                && notes.contains("Name: z-source")
        );
        assert_eq!(merged.keep_in_touch.interval_days, Some(30));
        assert_eq!(
            merged.keep_in_touch.last_touch_at.as_deref(),
            Some("2026-09-10T05:00:00-04:00")
        );
        assert_eq!(
            db.get_contact_group(&group.id).unwrap().unwrap().member_ids,
            ["a-target"]
        );
        assert!(db.get_contact_profile("z-source").unwrap().is_none());
        for destination in [&peer, &Database::open_memory()] {
            sync_into(destination, &db);
            assert_eq!(
                destination
                    .get_contact_profile("a-target")
                    .unwrap()
                    .unwrap()
                    .addresses,
                merged.addresses
            );
            assert!(destination
                .get_contact_profile("z-source")
                .unwrap()
                .is_none());
            assert_eq!(
                destination
                    .get_contact_group(&group.id)
                    .unwrap()
                    .unwrap()
                    .member_ids,
                ["a-target"]
            );
        }
        sync_into(&db, &peer);
        assert_eq!(db.list_saved_contact_profiles().unwrap().len(), 1);
    }

    #[test]
    fn rejected_merges_leave_sources_groups_and_sync_untouched() {
        let db = Database::open_memory();
        db.set_beta_features_enabled(true).unwrap();
        let mut a = request("a", "a@example.com");
        a.notes = Some("a".repeat(8000));
        let mut b = request("b", "b@example.com");
        b.notes = Some("b".into());
        for request in [&a, &b] {
            db.save_contact_profile(request).unwrap();
        }
        let group = db
            .create_contact_group("Group", &["b".into()], &[])
            .unwrap()
            .group;
        let before = db.load_replica_state().unwrap();
        for sources in [
            vec!["b".into()],
            vec!["missing".into()],
            vec!["a".into()],
            vec![],
        ] {
            assert!(db.merge_contacts("a", &sources).is_err());
            assert_eq!(db.list_saved_contact_profiles().unwrap().len(), 2);
            assert_eq!(
                db.get_contact_group(&group.id).unwrap().unwrap().member_ids,
                ["b"]
            );
            assert_eq!(db.load_replica_state().unwrap(), before);
        }
    }

    #[test]
    fn suggestion_suppression_survives_database_reopen() {
        let directory = std::env::temp_dir().join(format!(
            "threestrands-contact-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("contacts.sqlite");
        {
            let db = Database::open(&path).unwrap();
            db.set_contact_suppressed("person@example.com", true)
                .unwrap();
        }
        {
            let db = Database::open(&path).unwrap();
            assert_eq!(
                db.list_contact_suppressions().unwrap(),
                ["person@example.com"]
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn suppression_is_normalized_reversible_independent_of_profiles_and_device_local() {
        let db = Database::open_memory();
        db.pin_contact("you@example.com", "person@example.com", Some("Person"))
            .unwrap();
        let id = db.list_saved_contact_profiles().unwrap()[0].id.clone();
        db.set_contact_suppressed(" PERSON@EXAMPLE.COM ", true)
            .unwrap();
        db.set_contact_suppressed("person@example.com", true)
            .unwrap();
        assert_eq!(
            db.list_contact_suppressions().unwrap(),
            ["person@example.com"]
        );
        assert!(db
            .list_contact_suggestions("you@example.com", "person", 10)
            .unwrap()
            .is_empty());
        assert_eq!(db.list_contact_profiles("person", 10).unwrap().len(), 1);
        db.delete_contact_profile(&id).unwrap();
        assert!(db
            .list_contact_suggestions("you@example.com", "person", 10)
            .unwrap()
            .is_empty());
        db.pin_contact("other@example.com", "person@example.com", None)
            .unwrap();
        assert!(db
            .list_contact_suggestions("other@example.com", "person", 10)
            .unwrap()
            .is_empty());
        assert!(Database::open_memory()
            .list_contact_suppressions()
            .unwrap()
            .is_empty());
        db.set_contact_suppressed("person@example.com", false)
            .unwrap();
        assert!(!db
            .list_contact_suggestions("you@example.com", "person", 10)
            .unwrap()
            .is_empty());
        for bad in [
            "invalid",
            "a@example.com,b@example.com",
            "Person <a@example.com>",
            "a@example.com\r\nBcc:b@example.com",
        ] {
            assert!(db.set_contact_suppressed(bad, true).is_err(), "{bad}");
        }
    }
}
