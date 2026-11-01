//! Device-local provider cache, separate from saved/synchronized profiles.
use super::{Database, DatabaseError, DbResult};
use crate::{
    models::{ContactProfile, KeepInTouch},
    system_contacts::{ContactConnection, SystemContact, MAX_SYSTEM_CONTACTS},
};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

pub(super) fn read_cache(connection: &rusqlite::Connection) -> DbResult<Vec<SystemContact>> {
    let mut statement =
        connection.prepare("SELECT data_json FROM system_contact_cache ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|row| serde_json::from_str(&row?).map_err(super::serialization_error))
        .collect()
}
impl Database {
    pub(crate) fn system_contact_connection(&self) -> DbResult<ContactConnection> {
        self.with_connection(|c| {
            let json: Option<String> = c
                .query_row(
                    "SELECT connection_json FROM system_contact_connection WHERE id=1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            json.map(|s| serde_json::from_str(&s).map_err(super::serialization_error))
                .transpose()
                .map(|v| v.unwrap_or_default())
        })
    }
    pub(crate) fn store_system_contact_connection(
        &self,
        connection: &ContactConnection,
    ) -> DbResult<()> {
        self.with_connection(|c| { c.execute("INSERT INTO system_contact_connection(id,connection_json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET connection_json=excluded.connection_json", [serde_json::to_string(connection).map_err(super::serialization_error)?])?; Ok(()) })
    }
    pub(crate) fn system_contact_count(&self) -> DbResult<usize> {
        self.with_connection(|c| {
            c.query_row("SELECT COUNT(*) FROM system_contact_cache", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|v| v as usize)
            .map_err(Into::into)
        })
    }
    pub(crate) fn clear_system_contact_cache(&self) -> DbResult<()> {
        self.with_transaction(|tx| {
            tx.execute("DELETE FROM system_contact_addresses", [])?;
            tx.execute("DELETE FROM system_contact_cache", [])?;
            Ok(())
        })
    }
    pub(crate) fn disconnect_system_contacts(&self) -> DbResult<()> {
        self.with_transaction(|tx| {
            tx.execute("DELETE FROM system_contact_addresses", [])?;
            tx.execute("DELETE FROM system_contact_cache", [])?;
            tx.execute("DELETE FROM system_contact_connection", [])?;
            Ok(())
        })
    }
    pub(crate) fn replace_system_contacts(
        &self,
        connection: &ContactConnection,
        contacts: &[SystemContact],
    ) -> DbResult<()> {
        if contacts.len() > MAX_SYSTEM_CONTACTS {
            return Err(DatabaseError::invalid("Too many Mac contacts"));
        }
        self.with_transaction(|tx| {
            tx.execute("DELETE FROM system_contact_addresses", [])?;
            tx.execute("DELETE FROM system_contact_cache", [])?;
            for contact in contacts {
                if !connection.selected_sources.contains(&contact.source_id) { return Err(DatabaseError::invalid("Contact is outside selected address books")); }
                let mut contact = contact.clone();
                contact.addresses = contact.addresses.iter().filter_map(|a| {
                    let a = a.trim().to_ascii_lowercase();
                    crate::provider::imap::validate_envelope_address(&a).ok().map(|_| a)
                }).collect();
                let mut seen = std::collections::HashSet::new();
                contact.addresses.retain(|a| seen.insert(a.clone()));
                if contact.addresses.is_empty() { continue; }
                // Keyed by source as macOS raw identifiers can be linked across containers.
                contact.id = format!("derived:mac:{}", Sha256::digest(format!("{}\0{}", contact.source_id, contact.id).as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>());
                for text in [&mut contact.display_name, &mut contact.company, &mut contact.role] {
                    *text = text.take().map(|s| s.chars().filter(|c| !c.is_control()).take(1000).collect::<String>()).filter(|s| !s.trim().is_empty());
                }
                tx.execute("INSERT INTO system_contact_cache(id,data_json) VALUES(?1,?2)", params![contact.id, serde_json::to_string(&contact).map_err(super::serialization_error)?])?;
                for email in &contact.addresses { tx.execute("INSERT INTO system_contact_addresses(contact_id,email,display_name) VALUES(?1,?2,?3)", params![contact.id, email, contact.display_name])?; }
            }
            tx.execute("INSERT INTO system_contact_connection(id,connection_json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET connection_json=excluded.connection_json", [serde_json::to_string(connection).map_err(super::serialization_error)?])?;
            Ok(())
        })
    }
    pub(crate) fn resolve_contact_ids_with_system(
        &self,
        emails: &[String],
    ) -> DbResult<std::collections::HashMap<String, String>> {
        let mut owners = self.contact_ids_for_addresses(emails)?;
        let requested: std::collections::HashSet<_> = emails
            .iter()
            .map(|e| e.trim().to_ascii_lowercase())
            .collect();
        for profile in self.system_contact_profiles("")? {
            for email in profile.addresses {
                if requested.contains(&email) {
                    owners.entry(email).or_insert_with(|| profile.id.clone());
                }
            }
        }
        Ok(owners)
    }
    pub(crate) fn system_contact_profiles(&self, query: &str) -> DbResult<Vec<ContactProfile>> {
        self.system_contact_profiles_for_account(query, None)
    }
    pub(crate) fn system_contact_profiles_for_account(
        &self,
        query: &str,
        account: Option<&str>,
    ) -> DbResult<Vec<ContactProfile>> {
        let (contacts, owned, stats) = self.with_connection(|c| {
            let contacts = read_cache(c)?;
            let mut statement = c.prepare("SELECT lower(email) FROM contact_addresses")?;
            let owned = statement.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<std::collections::HashSet<_>, _>>()?;
            let mut statement = c.prepare("SELECT ci.email,SUM(CASE WHEN ?1 IS NULL OR ci.account_id=?1 THEN ci.direction='sent' ELSE 0 END),SUM(CASE WHEN ?1 IS NULL OR ci.account_id=?1 THEN ci.direction='received' ELSE 0 END),MAX(CASE WHEN ?1 IS NULL OR ci.account_id=?1 THEN ci.sent_at END) FROM contact_interactions ci JOIN (SELECT DISTINCT email FROM system_contact_addresses) a ON a.email=ci.email GROUP BY ci.email")?;
            let stats = statement.query_map([account], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?))))?.collect::<Result<std::collections::HashMap<_, _>, _>>()?;
            Ok((contacts, owned, stats))
        })?;
        let needle = query.trim().to_ascii_lowercase();
        // Saved profiles win. Do not silently merge source records into local
        // profiles, or combine two people solely by their display names.
        let mut profiles = Vec::new();
        for c in contacts {
            if c.addresses.iter().any(|a| owned.contains(a)) {
                continue;
            }
            if !needle.is_empty()
                && !format!(
                    "{} {} {} {}",
                    c.display_name.as_deref().unwrap_or(""),
                    c.company.as_deref().unwrap_or(""),
                    c.role.as_deref().unwrap_or(""),
                    c.addresses.join(" ")
                )
                .to_ascii_lowercase()
                .contains(&needle)
            {
                continue;
            }
            let mut sent_count = 0;
            let mut received_count = 0;
            let mut last_interacted_at: Option<String> = None;
            for email in &c.addresses {
                if let Some((sent, received, last)) = stats.get(email) {
                    sent_count += sent;
                    received_count += received;
                    if let Some(last) = last {
                        if last_interacted_at.as_ref().is_none_or(|old| last > old) {
                            last_interacted_at = Some(last.clone());
                        }
                    }
                }
            }
            if account.is_some()
                && sent_count == 0
                && received_count == 0
                && c.addresses.iter().any(|a| stats.contains_key(a))
            {
                continue;
            }
            profiles.push(ContactProfile {
                id: c.id,
                display_name: c.display_name,
                role: c.role,
                company: c.company,
                location: None,
                bio: None,
                notes: None,
                links: vec![],
                photo_data: None,
                favorite: false,
                addresses: c.addresses,
                sent_count,
                received_count,
                last_interacted_at,
                birthday: None,
                keep_in_touch: KeepInTouch::default(),
                keep_in_touch_due_at: None,
            });
        }
        Ok(profiles)
    }
}
#[cfg(test)]
#[path = "tests/system_contacts.rs"]
mod tests;
