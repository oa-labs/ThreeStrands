//! Address book profiles and mail-derived contacts.

use super::*;
use crate::models::{ContactProfile, ContactTimelineItem, SaveContactRequest};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

const MAX_CONTACTS: usize = 5_000;

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
    let from = crate::correspondence::addresses(sender).unwrap_or_default();
    if let Some((name, email)) = from.into_iter().next() {
        let email = email.to_ascii_lowercase();
        if email == owner {
            for raw in recipients {
                for (name, address) in crate::correspondence::addresses(raw).unwrap_or_default() {
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
                "SELECT c.id,c.display_name,c.role,c.company,c.location,c.bio,c.notes,c.links_json,c.photo_data,c.favorite,
                    COALESCE((SELECT json_group_array(a.email) FROM contact_addresses a WHERE a.contact_id=c.id),'[]'),
                    COALESCE(stats.sent_count,0),COALESCE(stats.received_count,0),stats.last_interacted_at
                 FROM contacts c
                 LEFT JOIN (
                    SELECT ca.contact_id,
                        COUNT(DISTINCT CASE WHEN ci.direction='sent' THEN ci.message_id END) AS sent_count,
                        COUNT(DISTINCT CASE WHEN ci.direction='received' THEN ci.message_id END) AS received_count,
                        MAX(ci.sent_at) AS last_interacted_at
                    FROM contact_addresses ca LEFT JOIN contact_interactions ci ON ci.email=ca.email
                    GROUP BY ca.contact_id
                 ) stats ON stats.contact_id=c.id
                 ORDER BY c.favorite DESC,c.updated_at DESC")?;
            let rows=statement.query_map([],|row|Ok((
                row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?,
                row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<String>>(5)?,
                row.get::<_,Option<String>>(6)?,row.get::<_,String>(7)?,row.get::<_,Option<String>>(8)?,
                row.get::<_,bool>(9)?,row.get::<_,String>(10)?,row.get::<_,i64>(11)?,row.get::<_,i64>(12)?,
                row.get::<_,Option<String>>(13)?,
            )))?;
            rows.map(|row|{
                let(id,display_name,role,company,location,bio,notes,links,photo_data,favorite,addresses,sent_count,received_count,last_interacted_at)=row?;
                Ok(ContactProfile{id,display_name,role,company,location,bio,notes,links:serde_json::from_str(&links).unwrap_or_default(),photo_data,favorite,addresses:serde_json::from_str(&addresses).unwrap_or_default(),sent_count,received_count,last_interacted_at})
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
            let mut statement = connection.prepare(
                "SELECT c.id,c.display_name,c.role,c.company,c.location,c.bio,c.notes,c.links_json,
                        c.photo_data,c.favorite,c.updated_at
                 FROM contacts c
                 WHERE (?3 IS NULL
                   OR EXISTS(SELECT 1 FROM contact_addresses a JOIN contact_interactions ci ON ci.email=a.email WHERE a.contact_id=c.id AND ci.account_id=?3)
                   OR NOT EXISTS(SELECT 1 FROM contact_addresses a JOIN contact_interactions ci ON ci.email=a.email WHERE a.contact_id=c.id))
                   AND (?2='' OR lower(coalesce(c.display_name,'') || ' ' || coalesce(c.role,'') || ' ' || coalesce(c.company,'') || ' ' || coalesce(c.location,'') || ' ' || coalesce(c.bio,'') || ' ' || coalesce(c.notes,'') || ' ' || coalesce(c.links_json,'') || ' ' || coalesce((SELECT group_concat(email,' ') FROM contact_addresses a WHERE a.contact_id=c.id),'')) LIKE '%' || ?2 || '%')
                 ORDER BY c.favorite DESC,c.updated_at DESC LIMIT ?1")?;
            let rows = statement.query_map(params![limit as i64,needle,account_id], |row| {
                Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?,
                    row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<String>>(5)?,
                    row.get::<_,Option<String>>(6)?,row.get::<_,String>(7)?,row.get::<_,Option<String>>(8)?,
                    row.get::<_,bool>(9)?))
            })?;
            rows.collect::<Result<Vec<_>,_>>().map_err(Into::into)
        })?;
        let mut mail_history = Vec::new();
        for account in self.list_accounts()?.into_iter().filter(|account| account_id.is_none_or(|selected| selected == account.email)) {
            mail_history.extend(self.list_contact_suggestions(&account.email, "", MAX_CONTACTS)?);
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
        for (id, name, role, company, location, bio, notes, links, photo, favorite) in saved {
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
                suggestions.extend(self.list_contact_suggestions(
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
            }));
        }
        self.with_connection(|connection| {
            let row = connection.query_row(
                "SELECT id,display_name,role,company,location,bio,notes,links_json,photo_data,favorite FROM contacts WHERE id=?1",[id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<String>>(5)?,row.get::<_,Option<String>>(6)?,row.get::<_,String>(7)?,row.get::<_,Option<String>>(8)?,row.get::<_,bool>(9)?))).optional()?;
            Ok(row)
        })?.map(|(id,display_name,role,company,location,bio,notes,links,photo_data,favorite)| {
            let addresses=self.contact_addresses(&id)?;
            let (sent_count,received_count,last_interacted_at)=self.contact_interaction_summary(&addresses)?;
            Ok(ContactProfile{id,display_name,role,company,location,bio,notes,links:serde_json::from_str(&links).unwrap_or_default(),photo_data,favorite,addresses,sent_count,received_count,last_interacted_at})
        }).transpose()
    }

    pub fn save_contact_profile(&self, request: &SaveContactRequest) -> DbResult<ContactProfile> {
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
        self.with_transaction(|tx| {
            // The address-derived ID is convenient for a first save, but an
            // address can later be removed and claimed by someone else. Keep
            // re-saving the same contact stable when it still owns an address;
            // otherwise avoid turning a hash collision into an upsert that
            // would replace the unrelated profile.
            if generated_id {
                let id_exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM contacts WHERE id=?1)",[&id],|row|row.get(0))?;
                if id_exists {
                    let mut owns_requested_address=false;
                    for email in &addresses {
                        let owner:Option<String>=tx.query_row("SELECT contact_id FROM contact_addresses WHERE email=?1",[email],|row|row.get(0)).optional()?;
                        if owner.as_deref()==Some(id.as_str()) {owns_requested_address=true;break;}
                    }
                    if !owns_requested_address { id=Uuid::new_v4().to_string(); }
                }
            }
            for email in &addresses {
                let existing:Option<String>=tx.query_row("SELECT contact_id FROM contact_addresses WHERE email=?1",[email],|row|row.get(0)).optional()?;
                if existing.as_deref().is_some_and(|owner|owner!=id) { return Err("That address already belongs to another saved contact. Remove it there before linking it here.".into()); }
            }
            tx.execute("INSERT INTO contacts(id,display_name,role,company,location,bio,notes,links_json,photo_data,favorite,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,role=excluded.role,company=excluded.company,location=excluded.location,bio=excluded.bio,notes=excluded.notes,links_json=excluded.links_json,photo_data=excluded.photo_data,favorite=excluded.favorite,updated_at=excluded.updated_at",params![id,name,role,company,location,bio,notes,links_json,request.photo_data,request.favorite,Utc::now().to_rfc3339()])?;
            tx.execute("DELETE FROM contact_addresses WHERE contact_id=?1",[&id])?;
            for email in &addresses { tx.execute("INSERT INTO contact_addresses(contact_id,email) VALUES(?1,?2)",params![id,email])?; }
            Ok(())
        })?;
        self.get_contact_profile(&id)?
            .ok_or_else(|| "Saved contact could not be loaded".into())
    }

    pub fn delete_contact_profile(&self, id: &str) -> DbResult<()> {
        self.with_transaction(|tx| {
            let mut statement =
                tx.prepare("SELECT email FROM contact_addresses WHERE contact_id=?1")?;
            let rows = statement.query_map([id], |row| row.get::<_, String>(0))?;
            let addresses = rows.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            tx.execute("DELETE FROM contacts WHERE id=?1", [id])?;
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
        self.with_connection(|connection|{
            let marks=std::iter::repeat("?").take(addresses.len()).collect::<Vec<_>>().join(",");
            let sql=format!("SELECT t.id,t.account_id,t.subject,t.snippet,MAX(ci.sent_at),t.labels_json FROM contact_interactions ci JOIN threads t ON t.id=ci.thread_id WHERE ci.email IN ({marks}) AND (? IS NULL OR ci.account_id=?) GROUP BY t.id ORDER BY MAX(ci.sent_at) DESC LIMIT ? OFFSET ?");
            let mut values=addresses.iter().map(|email|rusqlite::types::Value::Text(email.to_ascii_lowercase())).collect::<Vec<_>>();
            for _ in 0..2 { values.push(account_id.map(|id| rusqlite::types::Value::Text(id.to_string())).unwrap_or(rusqlite::types::Value::Null)); }
            values.push(rusqlite::types::Value::Integer(limit.clamp(1,100) as i64));
            values.push(rusqlite::types::Value::Integer(offset.min(i64::MAX as usize) as i64));
            let mut statement=connection.prepare(&sql)?;
            let rows=statement.query_map(rusqlite::params_from_iter(values),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?)))?;
            rows.map(|row|{let(thread_id,account_id,subject,snippet,sent_at,labels)=row?;Ok(ContactTimelineItem{thread_id,account_id,subject,snippet,sent_at,labels:serde_json::from_str(&labels).unwrap_or_default()})}).collect()
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
                "SELECT email FROM contact_addresses WHERE contact_id=?1 ORDER BY email",
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
