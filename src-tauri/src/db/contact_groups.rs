//! Named contact groups: global sets of saved contacts.
//!
//! A group replicates as one record whose membership is one field per
//! member (`member:<contact id>` = `true`), so devices that add or remove
//! different members concurrently merge without a conflict. A removed
//! member's field is written as unset. The local `updated_at` is never
//! replicated, so unrelated edits on two devices don't collide on it.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::*;
use crate::models::{ContactGroup, ContactGroupRecipients, ContactProfile, GroupRecipient, SaveContactRequest};

pub(crate) const MAX_CONTACT_GROUPS: usize = 200;
pub(crate) const MAX_GROUP_MEMBERS: usize = threestrands_sync_protocol::MAX_CONTACT_GROUP_MEMBERS;
pub(crate) const MAX_GROUP_NAME: usize = 100;
use threestrands_sync_protocol::CONTACT_GROUP_MEMBER_PREFIX as MEMBER_FIELD_PREFIX;

/// What a group write changed, so the caller replicates exactly those fields.
#[derive(Debug)]
pub struct ContactGroupWrite {
    pub group: ContactGroup,
    /// Sync record fields this write set or unset.
    pub fields: BTreeSet<String>,
    /// Contacts saved along the way: mail-derived contacts and typed
    /// addresses that had no saved contact yet.
    pub saved_contacts: Vec<ContactProfile>,
}

pub(crate) fn member_field(contact_id: &str) -> String {
    format!("{MEMBER_FIELD_PREFIX}{contact_id}")
}

pub(crate) fn validate_group_name(name: &str) -> DbResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_GROUP_NAME || name.chars().any(char::is_control) {
        return Err(format!("Group names must be between 1 and {MAX_GROUP_NAME} characters").into());
    }
    Ok(name.to_string())
}

fn group_by_id(connection: &Connection, id: &str) -> DbResult<Option<ContactGroup>> {
    let row = connection
        .query_row(
            "SELECT id,name,created_at,updated_at FROM contact_groups WHERE id=?1",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
        )
        .optional()?;
    let Some((id, name, created_at, updated_at)) = row else {
        return Ok(None);
    };
    let member_ids = present_member_ids(connection, &id)?;
    Ok(Some(ContactGroup { id, name, member_ids, created_at, updated_at }))
}

/// Members whose contact is on this device, by name.
fn present_member_ids(connection: &Connection, group_id: &str) -> DbResult<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT m.contact_id FROM contact_group_members m JOIN contacts c ON c.id=m.contact_id
         WHERE m.group_id=?1 ORDER BY c.display_name IS NULL, c.display_name COLLATE NOCASE, c.id",
    )?;
    let rows = statement.query_map([group_id], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Every stored member, including ones whose contact hasn't synced here yet.
fn stored_member_ids(connection: &Connection, group_id: &str) -> DbResult<BTreeSet<String>> {
    let mut statement = connection.prepare("SELECT contact_id FROM contact_group_members WHERE group_id=?1")?;
    let rows = statement.query_map([group_id], |row| row.get(0))?;
    rows.collect::<Result<BTreeSet<_>, _>>().map_err(Into::into)
}

fn ensure_unique_name(connection: &Connection, name: &str, except_id: Option<&str>) -> DbResult<()> {
    let mut statement = connection.prepare("SELECT id,name FROM contact_groups")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let folded = name.to_lowercase();
    for row in rows {
        let (id, existing) = row?;
        if Some(id.as_str()) != except_id && existing.to_lowercase() == folded {
            return Err(format!("A group named \u{201c}{existing}\u{201d} already exists").into());
        }
    }
    Ok(())
}

impl Database {
    pub fn list_contact_groups(&self) -> DbResult<Vec<ContactGroup>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id FROM contact_groups ORDER BY name COLLATE NOCASE, id")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids.iter()
                .filter_map(|id| group_by_id(connection, id).transpose())
                .collect()
        })
    }

    pub fn get_contact_group(&self, id: &str) -> DbResult<Option<ContactGroup>> {
        self.with_connection(|connection| group_by_id(connection, id))
    }

    /// Creates a group, optionally with its first members (see
    /// [`Self::add_contact_group_members`] for how members are resolved).
    pub fn create_contact_group(&self, name: &str, contact_ids: &[String], emails: &[String]) -> DbResult<ContactGroupWrite> {
        let name = validate_group_name(name)?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.with_transaction(|tx| {
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM contact_groups", [], |row| row.get(0))?;
            if count as usize >= MAX_CONTACT_GROUPS {
                return Err(format!("You can have at most {MAX_CONTACT_GROUPS} groups").into());
            }
            ensure_unique_name(tx, &name, None)?;
            tx.execute(
                "INSERT INTO contact_groups(id,name,created_at,updated_at) VALUES(?1,?2,?3,?3)",
                params![id, name, now],
            )?;
            Ok(())
        })?;
        let mut write = if contact_ids.is_empty() && emails.is_empty() {
            ContactGroupWrite {
                group: self.get_contact_group(&id)?.ok_or("Saved group could not be loaded")?,
                fields: BTreeSet::new(),
                saved_contacts: Vec::new(),
            }
        } else {
            match self.add_contact_group_members(&id, contact_ids, emails) {
                Ok(write) => write,
                Err(error) => {
                    self.delete_contact_group(&id)?;
                    return Err(error);
                }
            }
        };
        write.fields.extend(["id", "name", "createdAt"].map(String::from));
        Ok(write)
    }

    pub fn rename_contact_group(&self, id: &str, name: &str) -> DbResult<ContactGroupWrite> {
        let name = validate_group_name(name)?;
        self.with_transaction(|tx| {
            ensure_unique_name(tx, &name, Some(id))?;
            let changed = tx.execute(
                "UPDATE contact_groups SET name=?1,updated_at=?2 WHERE id=?3",
                params![name, Utc::now().to_rfc3339(), id],
            )?;
            if changed == 0 {
                return Err("Group not found".into());
            }
            Ok(())
        })?;
        Ok(ContactGroupWrite {
            group: self.get_contact_group(id)?.ok_or("Group not found")?,
            fields: BTreeSet::from(["name".to_string()]),
            saved_contacts: Vec::new(),
        })
    }

    /// Adds contacts to a group. A mail-derived contact (`derived:<email>`)
    /// is saved first, and a typed address with no saved contact becomes a
    /// new contact, so every member is a saved contact.
    pub fn add_contact_group_members(&self, id: &str, contact_ids: &[String], emails: &[String]) -> DbResult<ContactGroupWrite> {
        if contact_ids.len() + emails.len() > MAX_GROUP_MEMBERS {
            return Err(format!("A group can have at most {MAX_GROUP_MEMBERS} members").into());
        }
        if self.get_contact_group(id)?.is_none() {
            return Err("Group not found".into());
        }
        let mut saved_contacts = Vec::new();
        let mut resolved = Vec::new();
        for contact_id in contact_ids {
            let saved = self.ensure_saved_contact(contact_id)?;
            if contact_id.starts_with("derived:") {
                saved_contacts.extend(self.get_contact_profile(&saved)?);
            }
            resolved.push(saved);
        }
        for raw in emails {
            let email = raw.trim().to_ascii_lowercase();
            if !email.contains('@') || email.len() > 320 || email.chars().any(char::is_whitespace) {
                return Err(format!("\u{201c}{}\u{201d} isn't a valid email address", raw.trim()).into());
            }
            if let Some(owner) = self.contact_ids_for_addresses(std::slice::from_ref(&email))?.remove(&email) {
                resolved.push(owner);
                continue;
            }
            let profile = self.save_contact_profile(&SaveContactRequest {
                id: None,
                display_name: None,
                role: None,
                company: None,
                location: None,
                bio: None,
                notes: None,
                links: Vec::new(),
                photo_data: None,
                favorite: false,
                addresses: vec![email],
                birthday: None,
                keep_in_touch: None,
            })?;
            resolved.push(profile.id.clone());
            saved_contacts.push(profile);
        }
        let fields = self.with_transaction(|tx| {
            let existing = stored_member_ids(tx, id)?;
            let added = resolved.iter().filter(|member| !existing.contains(*member)).cloned().collect::<BTreeSet<_>>();
            if existing.len() + added.len() > MAX_GROUP_MEMBERS {
                return Err(format!("A group can have at most {MAX_GROUP_MEMBERS} members").into());
            }
            for member in &added {
                tx.execute("INSERT INTO contact_group_members(group_id,contact_id) VALUES(?1,?2)", params![id, member])?;
            }
            if !added.is_empty() {
                tx.execute("UPDATE contact_groups SET updated_at=?1 WHERE id=?2", params![Utc::now().to_rfc3339(), id])?;
            }
            Ok(added.iter().map(|member| member_field(member)).collect())
        })?;
        Ok(ContactGroupWrite {
            group: self.get_contact_group(id)?.ok_or("Group not found")?,
            fields,
            saved_contacts,
        })
    }

    pub fn remove_contact_group_members(&self, id: &str, contact_ids: &[String]) -> DbResult<ContactGroupWrite> {
        let fields = self.with_transaction(|tx| {
            if group_by_id(tx, id)?.is_none() {
                return Err("Group not found".into());
            }
            let mut fields = BTreeSet::new();
            for member in contact_ids {
                if tx.execute(
                    "DELETE FROM contact_group_members WHERE group_id=?1 AND contact_id=?2",
                    params![id, member],
                )? > 0
                {
                    fields.insert(member_field(member));
                }
            }
            if !fields.is_empty() {
                tx.execute("UPDATE contact_groups SET updated_at=?1 WHERE id=?2", params![Utc::now().to_rfc3339(), id])?;
            }
            Ok(fields)
        })?;
        Ok(ContactGroupWrite {
            group: self.get_contact_group(id)?.ok_or("Group not found")?,
            fields,
            saved_contacts: Vec::new(),
        })
    }

    /// Deletes a group. Its contacts are untouched.
    pub fn delete_contact_group(&self, id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM contact_groups WHERE id=?1", [id])?;
            Ok(())
        })
    }

    /// Groups that list `contact_id`, so deleting the contact can replicate
    /// each group's lost member.
    pub fn contact_group_ids_for_contact(&self, contact_id: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT group_id FROM contact_group_members WHERE contact_id=?1 ORDER BY group_id")?;
            let rows = statement.query_map([contact_id], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    /// Every group with its present members, in member order, each with
    /// the primary address compose sends to.
    pub fn list_contact_group_recipients(&self) -> DbResult<Vec<ContactGroupRecipients>> {
        let groups = self.list_contact_groups()?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.display_name,(SELECT json_group_array(email) FROM (SELECT a.email FROM contact_addresses a WHERE a.contact_id=c.id ORDER BY a.position,a.email))
                 FROM contacts c WHERE c.id=?1",
            )?;
            groups
                .into_iter()
                .map(|group| {
                    let mut members = Vec::with_capacity(group.member_ids.len());
                    for contact_id in group.member_ids {
                        let row = statement
                            .query_row([&contact_id], |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)))
                            .optional()?;
                        let Some((display_name, addresses)) = row else { continue };
                        let addresses: Vec<String> = serde_json::from_str(&addresses).unwrap_or_default();
                        let Some(email) = addresses.first().cloned() else { continue };
                        members.push(GroupRecipient { contact_id, display_name, email, addresses });
                    }
                    Ok(ContactGroupRecipients { id: group.id, name: group.name, members })
                })
                .collect()
        })
    }

    pub(crate) fn list_contact_group_ids(&self) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id FROM contact_groups ORDER BY id")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    /// The group's complete sync record, with every stored member.
    pub(crate) fn contact_group_record(&self, id: &str) -> DbResult<Option<Value>> {
        self.with_connection(|connection| {
            let row = connection
                .query_row(
                    "SELECT name,created_at FROM contact_groups WHERE id=?1",
                    [id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let Some((name, created_at)) = row else {
                return Ok(None);
            };
            let mut record = Map::new();
            record.insert("id".into(), Value::String(id.to_string()));
            record.insert("name".into(), Value::String(name));
            record.insert("createdAt".into(), Value::String(created_at));
            for member in stored_member_ids(connection, id)? {
                record.insert(member_field(&member), Value::Bool(true));
            }
            Ok(Some(Value::Object(record)))
        })
    }

    /// Materializes a resolved group record. Membership is replaced by the
    /// record's `member:*` fields; the name is not checked for uniqueness,
    /// since two devices may each have created the same name.
    pub(crate) fn upsert_synced_contact_group(&self, id: &str, payload: &Value) -> DbResult<()> {
        let name = payload.get("name").and_then(Value::as_str).ok_or("A synced group has no name")?;
        let now = Utc::now().to_rfc3339();
        let created_at = payload.get("createdAt").and_then(Value::as_str).unwrap_or(&now).to_string();
        let members = payload
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, value)| value.as_bool() == Some(true))
            .filter_map(|(field, _)| field.strip_prefix(MEMBER_FIELD_PREFIX))
            .filter(|member| !member.is_empty())
            .take(MAX_GROUP_MEMBERS)
            .collect::<Vec<_>>();
        self.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO contact_groups(id,name,created_at,updated_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name,created_at=excluded.created_at,updated_at=excluded.updated_at",
                params![id, name, created_at, now],
            )?;
            tx.execute("DELETE FROM contact_group_members WHERE group_id=?1", [id])?;
            for member in &members {
                tx.execute("INSERT INTO contact_group_members(group_id,contact_id) VALUES(?1,?2)", params![id, member])?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(database: &Database, name: &str, email: &str) -> String {
        database
            .save_contact_profile(&SaveContactRequest {
                id: None,
                display_name: Some(name.into()),
                role: None,
                company: None,
                location: None,
                bio: None,
                notes: None,
                links: Vec::new(),
                photo_data: None,
                favorite: false,
                addresses: vec![email.into()],
                birthday: None,
                keep_in_touch: None,
            })
            .unwrap()
            .id
    }

    #[test]
    fn groups_round_trip_with_members_listed_by_name() {
        let database = Database::open_memory();
        let zed = contact(&database, "Zed", "zed@example.com");
        let ada = contact(&database, "Ada", "ada@example.com");
        let write = database.create_contact_group("  Board  ", &[zed.clone(), ada.clone()], &[]).unwrap();
        assert_eq!(write.group.name, "Board");
        assert_eq!(write.group.member_ids, vec![ada.clone(), zed.clone()]);
        assert!(write.fields.contains("name") && write.fields.contains("createdAt") && write.fields.contains("id"));
        assert!(write.fields.contains(&member_field(&ada)) && write.fields.contains(&member_field(&zed)));
        database.create_contact_group("alpha", &[], &[]).unwrap();
        let names = database.list_contact_groups().unwrap().into_iter().map(|group| group.name).collect::<Vec<_>>();
        assert_eq!(names, vec!["alpha", "Board"]);
    }

    #[test]
    fn group_names_are_bounded_and_unique_ignoring_case() {
        let database = Database::open_memory();
        assert!(database.create_contact_group(" ", &[], &[]).is_err());
        assert!(database.create_contact_group(&"x".repeat(MAX_GROUP_NAME), &[], &[]).is_ok());
        assert!(database.create_contact_group(&"y".repeat(MAX_GROUP_NAME + 1), &[], &[]).is_err());
        assert!(database.create_contact_group("Bad\nname", &[], &[]).is_err());
        let board = database.create_contact_group("Board", &[], &[]).unwrap().group;
        assert!(database.create_contact_group("BOARD", &[], &[]).unwrap_err().to_string().contains("already exists"));
        let other = database.create_contact_group("Family", &[], &[]).unwrap().group;
        assert!(database.rename_contact_group(&other.id, "board").is_err());
        // Renaming a group to a different case of its own name is fine.
        let renamed = database.rename_contact_group(&board.id, "board").unwrap();
        assert_eq!(renamed.group.name, "board");
        assert_eq!(renamed.fields, BTreeSet::from(["name".to_string()]));
    }

    #[test]
    fn typed_addresses_reuse_an_owner_or_become_new_saved_contacts() {
        let database = Database::open_memory();
        let ada = contact(&database, "Ada", "ada@example.com");
        let group = database.create_contact_group("Board", &[], &[]).unwrap().group;
        let write = database
            .add_contact_group_members(&group.id, &[], &[" ADA@example.com ".into(), "new@example.com".into()])
            .unwrap();
        assert_eq!(write.saved_contacts.len(), 1);
        assert_eq!(write.saved_contacts[0].addresses, vec!["new@example.com".to_string()]);
        assert_eq!(write.group.member_ids.len(), 2);
        assert!(write.group.member_ids.contains(&ada));
        assert!(database.add_contact_group_members(&group.id, &[], &["not an address".into()]).is_err());
        // Re-adding an existing member changes nothing to replicate.
        let again = database.add_contact_group_members(&group.id, &[ada.clone()], &[]).unwrap();
        assert!(again.fields.is_empty());
    }

    #[test]
    fn membership_is_bounded_at_the_limit() {
        let database = Database::open_memory();
        let group = database.create_contact_group("Everyone", &[], &[]).unwrap().group;
        let emails = (0..MAX_GROUP_MEMBERS).map(|index| format!("person{index}@example.com")).collect::<Vec<_>>();
        let write = database.add_contact_group_members(&group.id, &[], &emails).unwrap();
        assert_eq!(write.group.member_ids.len(), MAX_GROUP_MEMBERS);
        assert!(database.add_contact_group_members(&group.id, &[], &["one-more@example.com".into()]).is_err());
        let too_many = (0..=MAX_GROUP_MEMBERS).map(|index| format!("x{index}@example.com")).collect::<Vec<_>>();
        let other = database.create_contact_group("Too many", &[], &[]).unwrap().group;
        assert!(database.add_contact_group_members(&other.id, &[], &too_many).is_err());
    }

    #[test]
    fn removing_members_and_deleting_contacts_or_groups_keeps_the_rest() {
        let database = Database::open_memory();
        let ada = contact(&database, "Ada", "ada@example.com");
        let bob = contact(&database, "Bob", "bob@example.com");
        let group = database.create_contact_group("Board", &[ada.clone(), bob.clone()], &[]).unwrap().group;
        let removed = database.remove_contact_group_members(&group.id, &[bob.clone(), "missing".into()]).unwrap();
        assert_eq!(removed.fields, BTreeSet::from([member_field(&bob)]));
        assert_eq!(removed.group.member_ids, vec![ada.clone()]);
        assert_eq!(database.contact_group_ids_for_contact(&ada).unwrap(), vec![group.id.clone()]);
        database.delete_contact_profile(&ada).unwrap();
        assert!(database.get_contact_group(&group.id).unwrap().unwrap().member_ids.is_empty());
        assert!(database.contact_group_ids_for_contact(&ada).unwrap().is_empty());
        database.delete_contact_group(&group.id).unwrap();
        assert!(database.get_contact_group(&group.id).unwrap().is_none());
        assert!(database.get_contact_profile(&bob).unwrap().is_some());
    }

    #[test]
    fn group_recipients_send_to_each_members_primary_address() {
        let database = Database::open_memory();
        let ada = contact(&database, "Ada", "ada@work.example");
        let mut request = database.get_contact_profile(&ada).unwrap().unwrap();
        request.addresses = vec!["ada@home.example".into(), "ada@work.example".into()];
        database
            .save_contact_profile(&SaveContactRequest {
                id: Some(request.id.clone()), display_name: request.display_name, role: None, company: None, location: None,
                bio: None, notes: None, links: Vec::new(), photo_data: None, favorite: false, addresses: request.addresses,
                birthday: None, keep_in_touch: None,
            })
            .unwrap();
        let bob = contact(&database, "Bob", "bob@example.com");
        let group = database.create_contact_group("Board", &[ada.clone(), bob.clone()], &[]).unwrap().group;
        // A member whose contact hasn't synced here is left out.
        let mut record = database.contact_group_record(&group.id).unwrap().unwrap();
        record[member_field("contact:later")] = Value::Bool(true);
        database.upsert_synced_contact_group(&group.id, &record).unwrap();
        database.create_contact_group("Empty", &[], &[]).unwrap();
        let directory = database.list_contact_group_recipients().unwrap();
        assert_eq!(directory.iter().map(|group| group.name.as_str()).collect::<Vec<_>>(), vec!["Board", "Empty"]);
        assert_eq!(
            directory[0].members,
            vec![
                GroupRecipient { contact_id: ada, display_name: Some("Ada".into()), email: "ada@home.example".into(), addresses: vec!["ada@home.example".into(), "ada@work.example".into()] },
                GroupRecipient { contact_id: bob, display_name: Some("Bob".into()), email: "bob@example.com".into(), addresses: vec!["bob@example.com".into()] },
            ]
        );
        assert!(directory[1].members.is_empty());
    }

    #[test]
    fn sync_records_carry_every_stored_member_and_materialize_back() {
        let database = Database::open_memory();
        let ada = contact(&database, "Ada", "ada@example.com");
        let group = database.create_contact_group("Board", &[ada.clone()], &[]).unwrap().group;
        let record = database.contact_group_record(&group.id).unwrap().unwrap();
        assert_eq!(record["name"], "Board");
        assert_eq!(record[member_field(&ada)], true);
        assert!(record.get("updatedAt").is_none());

        // A member whose contact hasn't arrived yet is stored but not listed.
        let mut incoming = record.clone();
        incoming["name"] = Value::String("Directors".into());
        incoming[member_field("contact:later")] = Value::Bool(true);
        database.upsert_synced_contact_group(&group.id, &incoming).unwrap();
        let synced = database.get_contact_group(&group.id).unwrap().unwrap();
        assert_eq!((synced.name.as_str(), synced.member_ids.clone()), ("Directors", vec![ada.clone()]));
        assert_eq!(database.contact_group_record(&group.id).unwrap().unwrap()[member_field("contact:later")], true);

        // A record without the member drops it; a duplicate name is accepted from sync.
        database.create_contact_group("Family", &[], &[]).unwrap();
        let mut dropped = incoming.clone();
        dropped.as_object_mut().unwrap().remove(&member_field(&ada));
        dropped["name"] = Value::String("family".into());
        database.upsert_synced_contact_group(&group.id, &dropped).unwrap();
        let synced = database.get_contact_group(&group.id).unwrap().unwrap();
        assert_eq!(synced.name, "family");
        assert!(synced.member_ids.is_empty());
    }
}
