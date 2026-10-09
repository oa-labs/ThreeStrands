//! Address book profiles and mail-derived contacts.

use super::{Database, DbResult};
use crate::mime::NormalizedMessage;
use chrono::Utc;
use rusqlite::params;
use std::collections::HashMap;
use uuid::Uuid;
use crate::models::{
    ContactActivity, ContactFile, ContactFiles, ContactProfile, ContactTimelineItem, DomainContext,
    DomainPerson, KeepInTouch, SaveContactRequest,
};
use chrono::{DateTime, Duration, NaiveDate};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

const MAX_CONTACTS: usize = 5_000;
/// Arrivals kept for estimating how often someone writes.
const MAX_ACTIVITY_ARRIVALS: usize = 24;
pub(crate) const MAX_CONTACT_FILES: usize = 100;
pub(crate) const MAX_DOMAIN_CONTEXT: usize = 20;
/// Longest keep-in-touch interval: two years.
pub(crate) const MAX_KEEP_IN_TOUCH_DAYS: i64 = 730;
/// Furthest a keep-in-touch reminder can be snoozed: two years.
const MAX_KEEP_IN_TOUCH_SNOOZE_DAYS: i64 = 730;
/// Birthday, interval, and keep-in-touch timestamps, read by [`contact_extras`].
const CONTACT_EXTRA_COLUMNS: &str =
    "c.birthday,c.kit_interval_days,c.kit_started_at,c.kit_snoozed_until,c.kit_snoozed_at,c.kit_last_touch_at";

pub(super) fn index_contact_message(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    thread_id: &str,
    message: &NormalizedMessage,
) -> DbResult<()> {
    index_contact_addresses(
        tx,
        account_id,
        &message.id,
        thread_id,
        &message.from,
        &message.to,
        message.unsubscribe.is_some(),
        &message.date,
    )
}

fn index_contact_addresses(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    message_id: &str,
    thread_id: &str,
    sender: &str,
    recipients: &[String],
    automated: bool,
    sent_at: &str,
) -> DbResult<()> {
    let owner = account_id.trim().to_ascii_lowercase();
    let from = crate::correspondence::stored_addresses(sender);
    if let Some((name, email)) = from.into_iter().next() {
        let email = email.to_ascii_lowercase();
        if email == owner {
            for raw in recipients {
                for (name, address) in crate::correspondence::stored_addresses(raw) {
                    let address = address.to_ascii_lowercase();
                    if address.is_empty() || address == owner {
                        continue;
                    }
                    tx.execute("INSERT OR IGNORE INTO contact_interactions(message_id,thread_id,account_id,email,display_name,direction,sent_at) VALUES(?1,?2,?3,?4,?5,'sent',?6)",params![message_id,thread_id,owner,address,(!name.is_empty()&&!name.eq_ignore_ascii_case(&address)).then_some(name),sent_at])?;
                }
            }
        } else if !email.is_empty() && !automated {
            tx.execute("INSERT OR IGNORE INTO contact_interactions(message_id,thread_id,account_id,email,display_name,direction,sent_at) VALUES(?1,?2,?3,?4,?5,'received',?6)",params![message_id,thread_id,owner,email,(!name.is_empty()&&!name.eq_ignore_ascii_case(&email)).then_some(name),sent_at])?;
        }
    }
    Ok(())
}

impl Database {
    pub fn rebuild_contact_interactions(&self) -> DbResult<()> {
        self.with_transaction(|tx|{
            tx.execute("DELETE FROM contact_interactions",[])?;
            let mut statement=tx.prepare("SELECT m.id,m.thread_id,t.account_id,m.sender,m.recipients_json,m.unsubscribe_json,m.sent_at FROM messages m JOIN threads t ON t.id=m.thread_id")?;
            let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,Option<String>>(5)?.is_some(),row.get::<_,String>(6)?)))?;
            let collected=rows.collect::<Result<Vec<_>,_>>()?;drop(statement);
            for (message_id,thread_id,account_id,sender,recipients_json,automated,sent_at) in collected {let recipients=serde_json::from_str::<Vec<String>>(&recipients_json).unwrap_or_default();index_contact_addresses(tx,&account_id,&message_id,&thread_id,&sender,&recipients,automated,&sent_at)?;}
            Ok(())
        })
    }
    pub fn pin_contact(
        &self,
        account_id: &str,
        email: &str,
        display_name: Option<&str>,
    ) -> DbResult<()> {
        let email = email.trim().to_ascii_lowercase();
        if !email.contains('@') || email.len() > 320 {
            return Err("Enter a valid email address".into());
        }
        self.with_connection(|connection|{connection.execute("INSERT INTO pinned_contacts(account_id,email,display_name,pinned_at) VALUES(?1,?2,?3,?4) ON CONFLICT(account_id,email) DO UPDATE SET display_name=excluded.display_name",params![account_id,email,display_name,Utc::now().to_rfc3339()])?;Ok(())})?;
        let owner: Option<String> = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT contact_id FROM contact_addresses WHERE email=?1",
                    [&email],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        if let Some(id) = owner {
            self.with_connection(|connection| {
                connection.execute("UPDATE contacts SET favorite=1 WHERE id=?1", [id])?;
                Ok(())
            })?;
        } else {
            let _ = self.save_contact_profile(&SaveContactRequest {
                id: None,
                display_name: display_name.map(ToOwned::to_owned),
                role: None,
                company: None,
                location: None,
                bio: None,
                notes: None,
                links: Vec::new(),
                photo_data: None,
                favorite: true,
                addresses: vec![email],
                birthday: None,
                keep_in_touch: None,
            })?;
        }
        Ok(())
    }

    pub fn unpin_contact(&self, account_id: &str, email: &str) -> DbResult<()> {
        let normalized = email.trim().to_ascii_lowercase();
        self.with_connection(|connection|{connection.execute("DELETE FROM pinned_contacts WHERE account_id=?1 AND email=?2",params![account_id,normalized])?;let remaining:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM pinned_contacts WHERE email=?1)",[&normalized],|row|row.get(0))?;if !remaining{connection.execute("UPDATE contacts SET favorite=0 WHERE id IN (SELECT contact_id FROM contact_addresses WHERE email=?1)",[&normalized])?;}Ok(())})
    }

    /// Returns every user-saved profile, without merging derived mail
    /// suggestions or applying the interactive contact-list result limit.
    /// Export and sync repair must enumerate the persisted entity set exactly.
    pub fn list_saved_contact_profiles(&self) -> DbResult<Vec<ContactProfile>> {
        self.with_connection(|connection|{
            let mut statement=connection.prepare(
                &format!("SELECT c.id,c.display_name,c.role,c.company,c.location,c.bio,c.notes,c.links_json,c.photo_data,c.favorite,
                    COALESCE((SELECT json_group_array(email) FROM (SELECT a.email FROM contact_addresses a WHERE a.contact_id=c.id ORDER BY a.position,a.email)),'[]'),
                    COALESCE(stats.sent_count,0),COALESCE(stats.received_count,0),stats.last_interacted_at,{CONTACT_EXTRA_COLUMNS}
                 FROM contacts c
                 LEFT JOIN (
                    SELECT ca.contact_id,
                        COUNT(DISTINCT CASE WHEN ci.direction='sent' THEN ci.message_id END) AS sent_count,
                        COUNT(DISTINCT CASE WHEN ci.direction='received' THEN ci.message_id END) AS received_count,
                        MAX(ci.sent_at) AS last_interacted_at
                    FROM contact_addresses ca LEFT JOIN contact_interactions ci ON ci.email=ca.email
                    GROUP BY ca.contact_id
                 ) stats ON stats.contact_id=c.id
                 ORDER BY c.favorite DESC,c.updated_at DESC"))?;
            let rows=statement.query_map([],|row|Ok((
                row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?,
                row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<String>>(5)?,
                row.get::<_,Option<String>>(6)?,row.get::<_,String>(7)?,row.get::<_,Option<String>>(8)?,
                row.get::<_,bool>(9)?,row.get::<_,String>(10)?,row.get::<_,i64>(11)?,row.get::<_,i64>(12)?,
                row.get::<_,Option<String>>(13)?,contact_extras(row,14)?,
            )))?;
            rows.map(|row|{
                let(id,display_name,role,company,location,bio,notes,links,photo_data,favorite,addresses,sent_count,received_count,last_interacted_at,(birthday,keep_in_touch))=row?;
                let keep_in_touch_due_at=keep_in_touch_due_at(&keep_in_touch,last_interacted_at.as_deref());
                Ok(ContactProfile{id,display_name,role,company,location,bio,notes,links:serde_json::from_str(&links).unwrap_or_default(),photo_data,favorite,addresses:serde_json::from_str(&addresses).unwrap_or_default(),sent_count,received_count,last_interacted_at,birthday,keep_in_touch,keep_in_touch_due_at})
            }).collect()
        })
    }

    pub fn list_contact_profiles(
        &self,
        query: &str,
        limit: usize,
    ) -> DbResult<Vec<ContactProfile>> {
        self.list_contact_profiles_for_account(query, limit, None)
    }

    pub fn list_contact_profiles_for_account(
        &self,
        query: &str,
        limit: usize,
        account_id: Option<&str>,
    ) -> DbResult<Vec<ContactProfile>> {
        let needle = query.trim().to_ascii_lowercase();
        let limit = limit.clamp(1, MAX_CONTACTS);
        let saved = self.with_connection(|connection| {
            // A saved profile is shared across accounts. In a scoped view it
            // appears where it has mail history; profiles with no history stay
            // available everywhere. Filter before LIMIT to avoid hiding rows.
            let mut statement = connection.prepare(&format!(
                "SELECT c.id,c.display_name,c.role,c.company,c.location,c.bio,c.notes,c.links_json,
                        c.photo_data,c.favorite,{CONTACT_EXTRA_COLUMNS}
                 FROM contacts c
                 WHERE (?3 IS NULL
                   OR EXISTS(SELECT 1 FROM contact_addresses a JOIN contact_interactions ci ON ci.email=a.email WHERE a.contact_id=c.id AND ci.account_id=?3)
                   OR NOT EXISTS(SELECT 1 FROM contact_addresses a JOIN contact_interactions ci ON ci.email=a.email WHERE a.contact_id=c.id))
                   AND (?2='' OR lower(coalesce(c.display_name,'') || ' ' || coalesce(c.role,'') || ' ' || coalesce(c.company,'') || ' ' || coalesce(c.location,'') || ' ' || coalesce(c.bio,'') || ' ' || coalesce(c.notes,'') || ' ' || coalesce(c.links_json,'') || ' ' || coalesce((SELECT group_concat(email,' ') FROM contact_addresses a WHERE a.contact_id=c.id),'')) LIKE '%' || ?2 || '%')
                 ORDER BY c.favorite DESC,c.updated_at DESC LIMIT ?1"))?;
            let rows = statement.query_map(params![limit as i64,needle,account_id], |row| {
                Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?,
                    row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<String>>(5)?,
                    row.get::<_,Option<String>>(6)?,row.get::<_,String>(7)?,row.get::<_,Option<String>>(8)?,
                    row.get::<_,bool>(9)?,contact_extras(row,10)?))
            })?;
            rows.collect::<Result<Vec<_>,_>>().map_err(Into::into)
        })?;
        let mut mail_history = Vec::new();
        for account in self.list_accounts()?.into_iter().filter(|account| account_id.is_none_or(|selected| selected == account.email)) {
            mail_history.extend(self.contact_suggestions_including_suppressed(&account.email, "", MAX_CONTACTS)?);
        }
        let mut stats = HashMap::<String, (i64, i64, Option<String>)>::new();
        for item in &mail_history {
            let entry = stats
                .entry(item.email.to_ascii_lowercase())
                .or_insert((0, 0, None));
            entry.0 += item.sent_count;
            entry.1 += item.received_count;
            if entry
                .2
                .as_deref()
                .is_none_or(|last| item.last_interacted_at.as_str() > last)
            {
                entry.2 = Some(item.last_interacted_at.clone());
            }
        }
        let mut profiles = Vec::new();
        for (id, name, role, company, location, bio, notes, links, photo, favorite, (birthday, keep_in_touch)) in saved {
            let addresses = self.contact_addresses(&id)?;
            let query_text = format!(
                "{} {} {} {} {} {} {} {}",
                name.as_deref().unwrap_or(""),
                role.as_deref().unwrap_or(""),
                company.as_deref().unwrap_or(""),
                location.as_deref().unwrap_or(""),
                bio.as_deref().unwrap_or(""),
                notes.as_deref().unwrap_or(""),
                links,
                addresses.join(" ")
            )
            .to_ascii_lowercase();
            if !needle.is_empty() && !query_text.contains(&needle) {
                continue;
            }
            let mut sent = 0;
            let mut received = 0;
            let mut last = None::<String>;
            for email in &addresses {
                if let Some((s, r, date)) = stats.get(&email.to_ascii_lowercase()) {
                    sent += s;
                    received += r;
                    if date
                        .as_deref()
                        .is_some_and(|value| last.as_deref().is_none_or(|old| value > old))
                    {
                        last = date.clone();
                    }
                }
            }
            // A scoped view counts only this account's mail, but a reminder
            // is satisfied by mail from any account.
            let keep_in_touch_due_at = if keep_in_touch.interval_days.is_some() {
                let (_, _, latest) = self.contact_interaction_summary(&addresses)?;
                keep_in_touch_due_at(&keep_in_touch, latest.as_deref())
            } else {
                None
            };
            profiles.push(ContactProfile {
                id,
                display_name: name,
                role,
                company,
                location,
                bio,
                notes,
                links: serde_json::from_str(&links).unwrap_or_default(),
                photo_data: photo,
                favorite,
                addresses,
                sent_count: sent,
                received_count: received,
                last_interacted_at: last,
                birthday,
                keep_in_touch,
                keep_in_touch_due_at,
            });
        }
        // The existing ranked suggestion query supplies mail-derived entries and
        // intentionally retains per-account ranking behavior for compose.
        let mut derived = HashMap::<String, ContactProfile>::new();
        for item in mail_history {
            if item.sent_count == 0 {
                continue;
            }
            let email = item.email.to_ascii_lowercase();
            if profiles.iter().any(|profile| {
                profile
                    .addresses
                    .iter()
                    .any(|address| address.eq_ignore_ascii_case(&email))
            }) {
                continue;
            }
            let profile = derived.entry(email.clone()).or_insert(ContactProfile {
                id: format!("derived:{email}"),
                display_name: item.display_name.clone(),
                role: None,
                company: None,
                location: None,
                bio: None,
                notes: None,
                links: Vec::new(),
                photo_data: None,
                favorite: item.pinned,
                addresses: vec![email],
                sent_count: 0,
                received_count: 0,
                last_interacted_at: None,
                birthday: None,
                keep_in_touch: KeepInTouch::default(),
                keep_in_touch_due_at: None,
            });
            profile.sent_count += item.sent_count;
            profile.received_count += item.received_count;
            if profile.display_name.is_none() {
                profile.display_name = item.display_name;
            }
            if profile
                .last_interacted_at
                .as_deref()
                .is_none_or(|last| item.last_interacted_at.as_str() > last)
            {
                profile.last_interacted_at = Some(item.last_interacted_at);
            }
            profile.favorite |= item.pinned;
        }
        profiles.extend(derived.into_values().filter(|profile| {
            needle.is_empty()
                || profile
                    .display_name
                    .as_deref()
                    .is_some_and(|name| name.to_ascii_lowercase().contains(&needle))
                || profile.addresses[0].contains(&needle)
        }));
        profiles.sort_by(|a, b| {
            b.favorite
                .cmp(&a.favorite)
                .then_with(|| b.last_interacted_at.cmp(&a.last_interacted_at))
                .then_with(|| a.display_name.cmp(&b.display_name))
        });
        profiles.truncate(limit);
        Ok(profiles)
    }

    pub fn get_contact_profile(&self, id: &str) -> DbResult<Option<ContactProfile>> {
        if let Some(email) = id.strip_prefix("derived:") {
            let mut suggestions = Vec::new();
            for account in self.list_accounts()? {
                suggestions.extend(self.contact_suggestions_including_suppressed(
                    &account.email,
                    email,
                    MAX_CONTACTS,
                )?);
            }
            let matching = suggestions
                .into_iter()
                .filter(|item| item.email.eq_ignore_ascii_case(email) && item.sent_count > 0)
                .collect::<Vec<_>>();
            if matching.is_empty() {
                return Ok(None);
            }
            let (sent, received, last) = self.contact_interaction_summary(&[email.to_string()])?;
            return Ok(Some(ContactProfile {
                id: id.to_string(),
                display_name: matching.iter().find_map(|item| item.display_name.clone()),
                role: None,
                company: None,
                location: None,
                bio: None,
                notes: None,
                links: Vec::new(),
                photo_data: None,
                favorite: matching.iter().any(|item| item.pinned),
                addresses: vec![email.to_string()],
                sent_count: sent,
                received_count: received,
                last_interacted_at: last,
                birthday: None,
                keep_in_touch: KeepInTouch::default(),
                keep_in_touch_due_at: None,
            }));
        }
        self.with_connection(|connection| {
            let row = connection.query_row(
                &format!("SELECT c.id,c.display_name,c.role,c.company,c.location,c.bio,c.notes,c.links_json,c.photo_data,c.favorite,{CONTACT_EXTRA_COLUMNS} FROM contacts c WHERE c.id=?1"),[id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<String>>(5)?,row.get::<_,Option<String>>(6)?,row.get::<_,String>(7)?,row.get::<_,Option<String>>(8)?,row.get::<_,bool>(9)?,contact_extras(row,10)?))).optional()?;
            Ok(row)
        })?.map(|(id,display_name,role,company,location,bio,notes,links,photo_data,favorite,(birthday,keep_in_touch))| {
            let addresses=self.contact_addresses(&id)?;
            let (sent_count,received_count,last_interacted_at)=self.contact_interaction_summary(&addresses)?;
            let keep_in_touch_due_at=keep_in_touch_due_at(&keep_in_touch,last_interacted_at.as_deref());
            Ok(ContactProfile{id,display_name,role,company,location,bio,notes,links:serde_json::from_str(&links).unwrap_or_default(),photo_data,favorite,addresses,sent_count,received_count,last_interacted_at,birthday,keep_in_touch,keep_in_touch_due_at})
        }).transpose()
    }

    pub fn save_contact_profile(&self, request: &SaveContactRequest) -> DbResult<ContactProfile> {
        let id = self.with_transaction(|tx| save_contact_on(tx, request))?;
        self.get_contact_profile(&id)?
            .ok_or_else(|| "Saved contact could not be loaded".into())
    }

    /// Saved contacts with keep-in-touch reminders or a birthday, soonest
    /// reminder first. Contacts without a reminder follow, by name.
    pub fn list_keep_in_touch(&self) -> DbResult<Vec<ContactProfile>> {
        let mut profiles = self
            .list_saved_contact_profiles()?
            .into_iter()
            .filter(|profile| profile.keep_in_touch.interval_days.is_some() || profile.birthday.is_some())
            .collect::<Vec<_>>();
        profiles.sort_by(|a, b| {
            let due = |profile: &ContactProfile| profile.keep_in_touch_due_at.as_deref().and_then(parse_instant);
            match (due(a), due(b)) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then_with(|| a.display_name.cmp(&b.display_name))
        });
        Ok(profiles)
    }

    /// Turns reminders on (or changes the interval) for every contact in
    /// `ids`, or off when `interval_days` is `None`. Mail-derived contacts
    /// are saved first, so the returned profiles carry their saved ids.
    pub fn set_keep_in_touch(&self, ids: &[String], interval_days: Option<i64>) -> DbResult<Vec<ContactProfile>> {
        if ids.is_empty() || ids.len() > MAX_CONTACTS {
            return Err(format!("Choose between 1 and {MAX_CONTACTS} contacts").into());
        }
        if let Some(days) = interval_days {
            validate_keep_in_touch_days(days)?;
        }
        let mut saved_ids = Vec::with_capacity(ids.len());
        for id in ids {
            let saved = self.ensure_saved_contact(id)?;
            if !saved_ids.contains(&saved) {
                saved_ids.push(saved);
            }
        }
        let now = Utc::now().to_rfc3339();
        self.with_transaction(|tx| {
            for id in &saved_ids {
                // SQLite evaluates every SET expression against the old row,
                // so `kit_interval_days IS NULL` means "was off until now".
                // Changing an interval keeps the original start; turning
                // reminders off also drops a pending snooze.
                tx.execute(
                    "UPDATE contacts SET
                        kit_started_at=CASE WHEN ?1 IS NULL THEN NULL WHEN kit_interval_days IS NULL THEN ?2 ELSE COALESCE(kit_started_at,?2) END,
                        kit_snoozed_until=CASE WHEN ?1 IS NULL THEN NULL ELSE kit_snoozed_until END,
                        kit_snoozed_at=CASE WHEN ?1 IS NULL THEN NULL ELSE kit_snoozed_at END,
                        kit_interval_days=?1,
                        updated_at=?2
                     WHERE id=?3",
                    params![interval_days, now, id],
                )?;
            }
            Ok(())
        })?;
        saved_ids
            .iter()
            .map(|id| self.get_contact_profile(id)?.ok_or_else(|| "Saved contact could not be loaded".into()))
            .collect()
    }

    /// Pushes the next reminder to `until`, or clears the snooze. A touch
    /// after the snooze was set supersedes it (see [`keep_in_touch_due_at`]).
    pub fn snooze_keep_in_touch(&self, id: &str, until: Option<&str>) -> DbResult<ContactProfile> {
        let now = Utc::now();
        let until = match until {
            Some(value) => {
                let instant = parse_instant(value).ok_or("Choose a valid snooze date")?;
                if instant <= now || instant > now + Duration::days(MAX_KEEP_IN_TOUCH_SNOOZE_DAYS) {
                    return Err("Choose a snooze date within the next two years".into());
                }
                Some(instant.to_rfc3339())
            }
            None => None,
        };
        let changed = self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE contacts SET kit_snoozed_until=?1,kit_snoozed_at=CASE WHEN ?1 IS NULL THEN NULL ELSE ?2 END,updated_at=?2 WHERE id=?3 AND kit_interval_days IS NOT NULL",
                params![until, now.to_rfc3339(), id],
            )?)
        })?;
        if changed == 0 {
            return Err("Turn on keep in touch for this contact before snoozing".into());
        }
        self.get_contact_profile(id)?.ok_or_else(|| "Saved contact could not be loaded".into())
    }

    /// Logs a touch outside email (a call, a coffee) at the current time,
    /// which also ends any snooze.
    pub fn mark_contacted(&self, id: &str) -> DbResult<ContactProfile> {
        let id = self.ensure_saved_contact(id)?;
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE contacts SET kit_last_touch_at=?1,kit_snoozed_until=NULL,kit_snoozed_at=NULL,updated_at=?1 WHERE id=?2",
                params![now, id],
            )?;
            Ok(())
        })?;
        self.get_contact_profile(&id)?.ok_or_else(|| "Saved contact could not be loaded".into())
    }

    /// Returns a saved contact id, saving a `derived:<email>` contact first.
    pub(super) fn ensure_saved_contact(&self, id: &str) -> DbResult<String> {
        let profile = self.get_contact_profile(id)?.ok_or("Contact not found")?;
        if !profile.id.starts_with("derived:") {
            return Ok(profile.id);
        }
        let saved = self.save_contact_profile(&SaveContactRequest {
            id: Some(profile.id),
            display_name: profile.display_name,
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: Vec::new(),
            photo_data: None,
            favorite: profile.favorite,
            addresses: profile.addresses,
            birthday: None,
            keep_in_touch: None,
        })?;
        Ok(saved.id)
    }

    pub fn delete_contact_profile(&self, id: &str) -> DbResult<()> {
        self.with_transaction(|tx| {
            let mut statement =
                tx.prepare("SELECT email FROM contact_addresses WHERE contact_id=?1")?;
            let rows = statement.query_map([id], |row| row.get::<_, String>(0))?;
            let addresses = rows.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            tx.execute("DELETE FROM contacts WHERE id=?1", [id])?;
            tx.execute("DELETE FROM contact_group_members WHERE contact_id=?1", [id])?;
            for email in addresses {
                tx.execute("DELETE FROM pinned_contacts WHERE email=?1", [email])?;
            }
            Ok(())
        })
    }

    #[cfg(test)]
    pub fn contact_timeline(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> DbResult<Vec<ContactTimelineItem>> {
        self.contact_timeline_for_account(id, offset, limit, None)
    }

    pub fn contact_timeline_for_account(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
        account_id: Option<&str>,
    ) -> DbResult<Vec<ContactTimelineItem>> {
        let addresses = self.contact_address_list(id)?;
        if addresses.is_empty() {
            return Ok(Vec::new());
        }
        let marks = std::iter::repeat("?").take(addresses.len()).collect::<Vec<_>>().join(",");
        let mut values = addresses
            .iter()
            .map(|email| rusqlite::types::Value::Text(email.to_ascii_lowercase()))
            .collect::<Vec<_>>();
        for _ in 0..2 {
            values.push(account_id.map(|id| rusqlite::types::Value::Text(id.to_string())).unwrap_or(rusqlite::types::Value::Null));
        }
        self.interaction_threads(&format!("ci.email IN ({marks}) AND (? IS NULL OR ci.account_id=?)"), values, offset, limit)
    }

    /// One row per conversation whose interactions match `filter`, newest
    /// matching interaction first.
    fn interaction_threads(
        &self,
        filter: &str,
        mut values: Vec<rusqlite::types::Value>,
        offset: usize,
        limit: usize,
    ) -> DbResult<Vec<ContactTimelineItem>> {
        self.with_connection(|connection|{
            let sql=format!("WITH matched AS (SELECT ci.thread_id,ci.account_id,ci.email,ci.sent_at,ROW_NUMBER() OVER (PARTITION BY ci.thread_id ORDER BY ci.sent_at DESC,ci.message_id DESC,ci.email) AS position FROM contact_interactions ci WHERE {filter}) SELECT t.id,t.account_id,matched.email,t.subject,t.snippet,matched.sent_at,t.labels_json FROM matched JOIN threads t ON t.id=matched.thread_id WHERE matched.position=1 ORDER BY matched.sent_at DESC LIMIT ? OFFSET ?");
            values.push(rusqlite::types::Value::Integer(limit.clamp(1,100) as i64));
            values.push(rusqlite::types::Value::Integer(offset.min(i64::MAX as usize) as i64));
            let mut statement=connection.prepare(&sql)?;
            let rows=statement.query_map(rusqlite::params_from_iter(values),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?)))?;
            rows.map(|row|{let(thread_id,account_id,contact_email,subject,snippet,sent_at,labels)=row?;Ok(ContactTimelineItem{thread_id,account_id,contact_email,subject,snippet,sent_at,labels:serde_json::from_str(&labels).unwrap_or_default()})}).collect()
        })
    }

    /// Counts, first contact, the user's latest message, and recent arrival
    /// times for a saved contact id or a `derived:<email>` id.
    pub fn contact_activity(&self, id: &str) -> DbResult<ContactActivity> {
        let addresses = self.contact_address_list(id)?;
        let empty = ContactActivity { sent_count: 0, received_count: 0, thread_count: 0, first_at: None, last_sent_at: None, recent_received_at: Vec::new() };
        if addresses.is_empty() {
            return Ok(empty);
        }
        self.with_connection(|connection|{
            let marks=std::iter::repeat("?").take(addresses.len()).collect::<Vec<_>>().join(",");
            let values=addresses.iter().map(|email|rusqlite::types::Value::Text(email.to_ascii_lowercase())).collect::<Vec<_>>();
            let (sent_count,received_count,thread_count,first_at,last_sent_at)=connection.query_row(
                &format!("SELECT COUNT(DISTINCT CASE WHEN direction='sent' THEN message_id END),COUNT(DISTINCT CASE WHEN direction='received' THEN message_id END),COUNT(DISTINCT thread_id),MIN(sent_at),MAX(CASE WHEN direction='sent' THEN sent_at END) FROM contact_interactions WHERE email IN ({marks})"),
                rusqlite::params_from_iter(values.iter()),
                |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
            )?;
            let mut statement=connection.prepare(&format!("SELECT MAX(sent_at) AS at FROM contact_interactions WHERE direction='received' AND email IN ({marks}) GROUP BY message_id ORDER BY at DESC LIMIT {MAX_ACTIVITY_ARRIVALS}"))?;
            let recent_received_at=statement.query_map(rusqlite::params_from_iter(values.iter()),|row|row.get(0))?.collect::<Result<Vec<String>,_>>()?;
            Ok(ContactActivity{sent_count,received_count,thread_count,first_at,last_sent_at,recent_received_at})
        })
    }

    /// Non-inline attachments on messages the person sent, newest first.
    /// Calendar invitations are left out; the meetings section covers them.
    /// One file (same name, ignoring case, and size) is listed once, from its
    /// newest message: a message delivered to several of the user's accounts
    /// is stored once per account, and people re-send the same file.
    pub fn contact_files(&self, id: &str, limit: usize) -> DbResult<ContactFiles> {
        let addresses = self.contact_address_list(id)?;
        if addresses.is_empty() {
            return Ok(ContactFiles { files: Vec::new(), total: 0 });
        }
        self.with_connection(|connection|{
            let marks=std::iter::repeat("?").take(addresses.len()).collect::<Vec<_>>().join(",");
            let values=addresses.iter().map(|email|rusqlite::types::Value::Text(email.to_ascii_lowercase())).collect::<Vec<_>>();
            let from=format!("FROM (SELECT DISTINCT message_id FROM contact_interactions WHERE direction='received' AND email IN ({marks})) sent JOIN messages m ON m.id=sent.message_id JOIN threads t ON t.id=m.thread_id, json_each(m.attachments_json) a WHERE COALESCE(json_extract(a.value,'$.inline'),0)=0 AND trim(lower(COALESCE(json_extract(a.value,'$.mimeType'),''))) NOT LIKE 'text/calendar%' AND lower(COALESCE(json_extract(a.value,'$.filename'),'')) NOT LIKE '%.ics'");
            let distinct=format!("FROM (SELECT m.id AS message_id,m.thread_id,t.subject,m.sent_at,a.key AS part,a.value AS attachment,ROW_NUMBER() OVER (PARTITION BY lower(trim(COALESCE(json_extract(a.value,'$.filename'),''))),COALESCE(json_extract(a.value,'$.size'),-1) ORDER BY m.sent_at DESC,m.id,a.key) AS copy {from}) WHERE copy=1");
            let total=connection.query_row(&format!("SELECT COUNT(*) {distinct}"),rusqlite::params_from_iter(values.iter()),|row|row.get(0))?;
            let mut statement=connection.prepare(&format!("SELECT message_id,thread_id,subject,sent_at,attachment {distinct} ORDER BY sent_at DESC,message_id,part LIMIT {}",limit.clamp(1,MAX_CONTACT_FILES)))?;
            let rows=statement.query_map(rusqlite::params_from_iter(values.iter()),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?)))?;
            let mut files=Vec::new();
            for row in rows {
                let (message_id,thread_id,subject,sent_at,attachment)=row?;
                // A malformed stored attachment is skipped rather than failing the list.
                if let Ok(attachment)=serde_json::from_str(&attachment) {
                    files.push(ContactFile{message_id,thread_id,subject,sent_at,attachment});
                }
            }
            Ok(ContactFiles{files,total})
        })
    }

    /// Other correspondents at `domain` and their latest conversations,
    /// leaving out `exclude` (the selected person's own addresses).
    pub fn domain_context(&self, domain: &str, exclude: &[String], limit: usize) -> DbResult<DomainContext> {
        let domain = domain.trim().trim_start_matches('@').to_ascii_lowercase();
        if domain.is_empty() || !domain.contains('.') {
            return Ok(DomainContext { people: Vec::new(), threads: Vec::new() });
        }
        let suffix = format!("@{domain}");
        let excluded = exclude.iter().map(|email| email.trim().to_ascii_lowercase()).filter(|email| !email.is_empty()).collect::<Vec<_>>();
        let mut filter = "substr(ci.email,-?)=?".to_string();
        let mut values = vec![
            rusqlite::types::Value::Integer(suffix.len() as i64),
            rusqlite::types::Value::Text(suffix.clone()),
        ];
        if !excluded.is_empty() {
            filter.push_str(&format!(" AND ci.email NOT IN ({})", std::iter::repeat("?").take(excluded.len()).collect::<Vec<_>>().join(",")));
            values.extend(excluded.iter().map(|email| rusqlite::types::Value::Text(email.clone())));
        }
        let limit = limit.clamp(1, MAX_DOMAIN_CONTEXT);
        let people = self.with_connection(|connection|{
            let mut statement=connection.prepare(&format!("SELECT ci.email,(SELECT display_name FROM contact_interactions named WHERE named.email=ci.email AND named.display_name IS NOT NULL ORDER BY named.sent_at DESC LIMIT 1),MAX(ci.sent_at) AS last_at FROM contact_interactions ci WHERE {filter} GROUP BY ci.email ORDER BY last_at DESC LIMIT {limit}"))?;
            let rows=statement.query_map(rusqlite::params_from_iter(values.iter()),|row|Ok(DomainPerson{email:row.get(0)?,display_name:row.get(1)?,last_at:row.get(2)?}))?;
            rows.collect::<Result<Vec<_>,_>>().map_err(Into::into)
        })?;
        let threads = self.interaction_threads(&filter, values, 0, limit)?;
        Ok(DomainContext { people, threads })
    }

    /// Maps each address that belongs to a saved contact to that contact's id.
    /// Addresses without a saved owner are omitted.
    pub fn contact_ids_for_addresses(
        &self,
        emails: &[String],
    ) -> DbResult<std::collections::HashMap<String, String>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT contact_id FROM contact_addresses WHERE email=?1")?;
            let mut owners = std::collections::HashMap::new();
            for raw in emails {
                let email = raw.trim().to_ascii_lowercase();
                if email.is_empty() || owners.contains_key(&email) {
                    continue;
                }
                let owner: Option<String> =
                    statement.query_row([&email], |row| row.get(0)).optional()?;
                if let Some(owner) = owner {
                    owners.insert(email, owner);
                }
            }
            Ok(owners)
        })
    }

    /// Resolves a saved contact id or a `derived:<email>` id to its addresses.
    pub(crate) fn contact_address_list(&self, id: &str) -> DbResult<Vec<String>> {
        if let Some(email) = id.strip_prefix("derived:") {
            return Ok(vec![email.to_string()]);
        }
        self.contact_addresses(id)
    }

    fn contact_addresses(&self, id: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT email FROM contact_addresses WHERE contact_id=?1 ORDER BY position, email",
            )?;
            let rows = statement.query_map([id], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    fn contact_interaction_summary(
        &self,
        addresses: &[String],
    ) -> DbResult<(i64, i64, Option<String>)> {
        if addresses.is_empty() {
            return Ok((0, 0, None));
        }
        self.with_connection(|connection|{
            let marks=std::iter::repeat("?").take(addresses.len()).collect::<Vec<_>>().join(",");
            let sql=format!("SELECT COUNT(DISTINCT CASE WHEN direction='sent' THEN message_id END),COUNT(DISTINCT CASE WHEN direction='received' THEN message_id END),MAX(sent_at) FROM contact_interactions WHERE email IN ({marks})");
            let values=addresses.iter().map(|email|rusqlite::types::Value::Text(email.to_ascii_lowercase())).collect::<Vec<_>>();
            connection.query_row(&sql,rusqlite::params_from_iter(values),|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(Into::into)
        })
    }
}

fn clean_contact_text(value: Option<&str>, max: usize) -> DbResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err("Contact field is too long".into());
    }
    Ok(Some(value.to_string()))
}

fn contact_extras(row: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<(Option<String>, KeepInTouch)> {
    Ok((
        row.get(start)?,
        KeepInTouch {
            interval_days: row.get(start + 1)?,
            started_at: row.get(start + 2)?,
            snoozed_until: row.get(start + 3)?,
            snoozed_at: row.get(start + 4)?,
            last_touch_at: row.get(start + 5)?,
        },
    ))
}

fn parse_instant(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value).ok().map(|value| value.with_timezone(&Utc))
}

pub(crate) fn validate_keep_in_touch_days(days: i64) -> DbResult<()> {
    if (1..=MAX_KEEP_IN_TOUCH_DAYS).contains(&days) {
        Ok(())
    } else {
        Err(format!("Keep in touch every 1 to {MAX_KEEP_IN_TOUCH_DAYS} days").into())
    }
}

/// Checks settings that arrive whole, from sync or a settings import.
pub(crate) fn validate_keep_in_touch(value: &KeepInTouch) -> DbResult<()> {
    if let Some(days) = value.interval_days {
        validate_keep_in_touch_days(days)?;
    }
    for instant in [&value.started_at, &value.snoozed_until, &value.snoozed_at, &value.last_touch_at].into_iter().flatten() {
        if parse_instant(instant).is_none() {
            return Err("Keep-in-touch dates must be RFC 3339 timestamps".into());
        }
    }
    Ok(())
}

/// Accepts `MM-DD`, or `YYYY-MM-DD` when the year is known. February 29 is
/// allowed without a year.
pub(crate) fn normalize_birthday(value: Option<&str>) -> DbResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let valid = match value.len() {
        5 => NaiveDate::parse_from_str(&format!("2000-{value}"), "%Y-%m-%d").is_ok(),
        10 => NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok_and(|date| {
            use chrono::Datelike;
            (1900..=Utc::now().year()).contains(&date.year())
        }),
        _ => false,
    };
    if valid {
        Ok(Some(value.to_string()))
    } else {
        Err("Enter a birthday as MM-DD or YYYY-MM-DD".into())
    }
}

/// When the next keep-in-touch reminder falls due, or `None` when reminders
/// are off. A touch is the newest of any mail either way (`last_mail_at`)
/// and a hand-logged touch. Without any touch the interval counts from when
/// reminders were turned on. A snooze wins until a touch newer than it.
pub(crate) fn keep_in_touch_due_at(value: &KeepInTouch, last_mail_at: Option<&str>) -> Option<String> {
    let days = value.interval_days?;
    let last_touch = [last_mail_at, value.last_touch_at.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(parse_instant)
        .max();
    if let Some(until) = value.snoozed_until.as_deref().and_then(parse_instant) {
        let snoozed_at = value.snoozed_at.as_deref().and_then(parse_instant);
        let superseded = matches!((last_touch, snoozed_at), (Some(touch), Some(at)) if touch > at);
        if !superseded {
            return Some(until.to_rfc3339());
        }
    }
    let base = last_touch.or_else(|| value.started_at.as_deref().and_then(parse_instant))?;
    Some((base + Duration::days(days)).to_rfc3339())
}

/// Validates and writes a profile on an existing transaction. Import and merge
/// use this same validation so a rejected batch leaves no partial changes.
pub(super) fn save_contact_on(
    tx: &rusqlite::Transaction,
    request: &SaveContactRequest,
) -> DbResult<String> {
    let generated_id = request
        .id
        .as_deref()
        .is_none_or(|id| id.starts_with("derived:"));
    let mut id = request
        .id
        .as_deref()
        .filter(|id| !id.starts_with("derived:"))
        .map(ToOwned::to_owned)
        .or_else(|| {
            request.addresses.first().map(|email| {
                let digest = Sha256::digest(email.trim().to_ascii_lowercase().as_bytes());
                format!(
                    "contact:{}",
                    digest
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                )
            })
        })
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if id.trim().is_empty() || id.len() > 128 {
        return Err("Contact id is invalid".into());
    }
    let name = clean_contact_text(request.display_name.as_deref(), 200)?;
    let role = clean_contact_text(request.role.as_deref(), 200)?;
    let company = clean_contact_text(request.company.as_deref(), 200)?;
    let location = clean_contact_text(request.location.as_deref(), 200)?;
    let bio = clean_contact_text(request.bio.as_deref(), 4000)?;
    let notes = clean_contact_text(request.notes.as_deref(), 8000)?;
    let birthday = normalize_birthday(request.birthday.as_deref())?;
    if let Some(keep_in_touch) = &request.keep_in_touch {
        validate_keep_in_touch(keep_in_touch)?;
    }
    let replace_keep_in_touch = request.keep_in_touch.is_some();
    let keep_in_touch = request.keep_in_touch.clone().unwrap_or_default();
    let mut addresses = Vec::new();
    for raw in &request.addresses {
        let email = raw.trim().to_ascii_lowercase();
        if !email.contains('@') || email.len() > 320 || email.chars().any(char::is_whitespace) {
            return Err("Enter a valid email address".into());
        }
        if !addresses.iter().any(|value: &String| value == &email) {
            addresses.push(email);
        }
    }
    if addresses.is_empty() {
        return Err("A contact needs at least one email address".into());
    }
    if request.links.len() > 20 {
        return Err("A contact can have at most 20 links".into());
    }
    let mut links = Vec::new();
    for link in &request.links {
        let value = link.trim();
        let parsed = url::Url::parse(value).map_err(|_| "Enter a valid https link")?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() || value.len() > 2048 {
            return Err("Contact links must be valid https URLs".into());
        }
        links.push(value.to_string());
    }
    if let Some(photo) = request.photo_data.as_deref() {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, photo)
            .map_err(|_| "Contact photo is invalid")?;
        crate::image_format::validate_contact_photo(&bytes)?;
    }
    let links_json = serde_json::to_string(&links).map_err(|error| error.to_string())?;
    {
        // The address-derived ID is convenient for a first save, but an
        // address can later be removed and claimed by someone else. Keep
        // re-saving the same contact stable when it still owns an address;
        // otherwise avoid turning a hash collision into an upsert that
        // would replace the unrelated profile.
        if generated_id {
            let id_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM contacts WHERE id=?1)",
                [&id],
                |row| row.get(0),
            )?;
            if id_exists {
                let mut owns_requested_address = false;
                for email in &addresses {
                    let owner: Option<String> = tx
                        .query_row(
                            "SELECT contact_id FROM contact_addresses WHERE email=?1",
                            [email],
                            |row| row.get(0),
                        )
                        .optional()?;
                    if owner.as_deref() == Some(id.as_str()) {
                        owns_requested_address = true;
                        break;
                    }
                }
                if !owns_requested_address {
                    id = Uuid::new_v4().to_string();
                }
            }
        }
        for email in &addresses {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT contact_id FROM contact_addresses WHERE email=?1",
                    [email],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.as_deref().is_some_and(|owner| owner != id) {
                return Err("That address already belongs to another saved contact. Remove it there before linking it here.".into());
            }
        }
        tx.execute("INSERT INTO contacts(id,display_name,role,company,location,bio,notes,links_json,photo_data,favorite,updated_at,birthday,kit_interval_days,kit_started_at,kit_snoozed_until,kit_snoozed_at,kit_last_touch_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,role=excluded.role,company=excluded.company,location=excluded.location,bio=excluded.bio,notes=excluded.notes,links_json=excluded.links_json,photo_data=excluded.photo_data,favorite=excluded.favorite,updated_at=excluded.updated_at,birthday=excluded.birthday,
                kit_interval_days=CASE WHEN ?18 THEN excluded.kit_interval_days ELSE contacts.kit_interval_days END,
                kit_started_at=CASE WHEN ?18 THEN excluded.kit_started_at ELSE contacts.kit_started_at END,
                kit_snoozed_until=CASE WHEN ?18 THEN excluded.kit_snoozed_until ELSE contacts.kit_snoozed_until END,
                kit_snoozed_at=CASE WHEN ?18 THEN excluded.kit_snoozed_at ELSE contacts.kit_snoozed_at END,
                kit_last_touch_at=CASE WHEN ?18 THEN excluded.kit_last_touch_at ELSE contacts.kit_last_touch_at END",params![id,name,role,company,location,bio,notes,links_json,request.photo_data,request.favorite,Utc::now().to_rfc3339(),birthday,keep_in_touch.interval_days,keep_in_touch.started_at,keep_in_touch.snoozed_until,keep_in_touch.snoozed_at,keep_in_touch.last_touch_at,replace_keep_in_touch])?;
        tx.execute("DELETE FROM contact_addresses WHERE contact_id=?1", [&id])?;
        for (position, email) in addresses.iter().enumerate() {
            tx.execute(
                "INSERT INTO contact_addresses(contact_id,email,position) VALUES(?1,?2,?3)",
                params![id, email, position as i64],
            )?;
        }
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit(days: i64) -> KeepInTouch {
        KeepInTouch {
            interval_days: Some(days),
            started_at: Some("2026-09-01T00:00:00+00:00".into()),
            ..KeepInTouch::default()
        }
    }

    #[test]
    fn addresses_keep_their_saved_order_with_the_primary_first() {
        let database = Database::open_memory();
        let request = |id: Option<String>, addresses: &[&str]| SaveContactRequest {
            id, display_name: Some("Zed".into()), role: None, company: None, location: None, bio: None, notes: None,
            links: Vec::new(), photo_data: None, favorite: false, addresses: addresses.iter().map(|value| value.to_string()).collect(),
            birthday: None, keep_in_touch: None,
        };
        let saved = database.save_contact_profile(&request(None, &["zed@work.example", "Zed@Home.example"])).unwrap();
        assert_eq!(saved.addresses, vec!["zed@work.example", "zed@home.example"]);
        let saved = database.save_contact_profile(&request(Some(saved.id), &["zed@home.example", "zed@work.example"])).unwrap();
        assert_eq!(saved.addresses, vec!["zed@home.example", "zed@work.example"]);
        assert_eq!(database.get_contact_profile(&saved.id).unwrap().unwrap().addresses, saved.addresses);
        assert_eq!(database.list_saved_contact_profiles().unwrap()[0].addresses, saved.addresses);
        assert_eq!(database.list_contact_profiles("", 10).unwrap()[0].addresses, saved.addresses);
    }

    // New saved-contact ids are persisted and synced between devices, so the
    // SHA-256 they derive from must never drift across crate upgrades.
    #[test]
    fn a_new_contact_id_is_the_sha256_of_its_normalized_primary_address() {
        let database = Database::open_memory();
        let saved = database.save_contact_profile(&SaveContactRequest {
            id: None, display_name: Some("Jane".into()), role: None, company: None, location: None, bio: None, notes: None,
            links: Vec::new(), photo_data: None, favorite: false, addresses: vec!["  Jane@Example.com ".into()],
            birthday: None, keep_in_touch: None,
        }).unwrap();
        assert_eq!(saved.id, "contact:8c87b489ce35cf2e2f39f80e282cb2e804932a56a213983eeeb428407d43b52d");
    }

    #[test]
    fn due_date_is_off_without_an_interval() {
        let value = KeepInTouch { interval_days: None, ..kit(7) };
        assert_eq!(keep_in_touch_due_at(&value, Some("2026-09-20T00:00:00Z")), None);
    }

    #[test]
    fn due_date_counts_from_the_start_without_any_touch() {
        assert_eq!(keep_in_touch_due_at(&kit(14), None).as_deref(), Some("2026-09-15T00:00:00+00:00"));
    }

    #[test]
    fn due_date_counts_from_the_newest_mail_or_logged_touch() {
        let mut value = kit(7);
        assert_eq!(
            keep_in_touch_due_at(&value, Some("2026-09-20T10:00:00Z")).as_deref(),
            Some("2026-09-27T10:00:00+00:00")
        );
        value.last_touch_at = Some("2026-09-25T08:00:00+00:00".into());
        assert_eq!(
            keep_in_touch_due_at(&value, Some("2026-09-20T10:00:00Z")).as_deref(),
            Some("2026-10-02T08:00:00+00:00")
        );
        // An older logged touch does not pull the due date back.
        assert_eq!(
            keep_in_touch_due_at(&value, Some("2026-09-30T00:00:00Z")).as_deref(),
            Some("2026-10-07T00:00:00+00:00")
        );
    }

    #[test]
    fn snooze_holds_until_a_newer_touch_supersedes_it() {
        let mut value = kit(7);
        value.snoozed_at = Some("2026-09-21T00:00:00+00:00".into());
        value.snoozed_until = Some("2026-10-15T00:00:00+00:00".into());
        // Mail before the snooze leaves it in place.
        assert_eq!(
            keep_in_touch_due_at(&value, Some("2026-09-20T00:00:00Z")).as_deref(),
            Some("2026-10-15T00:00:00+00:00")
        );
        // Mail exactly at the snooze time is not newer.
        assert_eq!(
            keep_in_touch_due_at(&value, Some("2026-09-21T00:00:00Z")).as_deref(),
            Some("2026-10-15T00:00:00+00:00")
        );
        // Newer mail cancels the snooze and restarts the interval.
        assert_eq!(
            keep_in_touch_due_at(&value, Some("2026-09-22T00:00:00Z")).as_deref(),
            Some("2026-09-29T00:00:00+00:00")
        );
    }

    #[test]
    fn interval_limits_accept_the_bounds_and_reject_beyond_them() {
        assert!(validate_keep_in_touch_days(0).is_err());
        assert!(validate_keep_in_touch_days(1).is_ok());
        assert!(validate_keep_in_touch_days(MAX_KEEP_IN_TOUCH_DAYS - 1).is_ok());
        assert!(validate_keep_in_touch_days(MAX_KEEP_IN_TOUCH_DAYS).is_ok());
        assert!(validate_keep_in_touch_days(MAX_KEEP_IN_TOUCH_DAYS + 1).is_err());
    }

    #[test]
    fn keep_in_touch_settings_reject_malformed_timestamps() {
        let mut value = kit(30);
        validate_keep_in_touch(&value).unwrap();
        value.snoozed_until = Some("2026-10-15".into());
        assert!(validate_keep_in_touch(&value).is_err());
    }

    #[test]
    fn birthdays_accept_month_day_with_or_without_a_year() {
        assert_eq!(normalize_birthday(Some(" 12-09 ")).unwrap().as_deref(), Some("12-09"));
        assert_eq!(normalize_birthday(Some("1984-12-09")).unwrap().as_deref(), Some("1984-12-09"));
        assert_eq!(normalize_birthday(Some("02-29")).unwrap().as_deref(), Some("02-29"));
        assert_eq!(normalize_birthday(Some("")).unwrap(), None);
        assert_eq!(normalize_birthday(None).unwrap(), None);
        for invalid in ["13-01", "02-30", "2023-02-29", "1899-12-31", "9999-01-01", "Dec 9", "12/09"] {
            assert!(normalize_birthday(Some(invalid)).is_err(), "{invalid}");
        }
    }
}

#[cfg(test)]
#[path = "tests/contacts.rs"]
mod regression_tests;
