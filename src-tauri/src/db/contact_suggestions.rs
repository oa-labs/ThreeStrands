//! Mail-derived and pinned contact suggestion queries.

use super::{Database, DbResult};
use crate::models::ContactSuggestion;
use rusqlite::params;
use std::collections::HashMap;

// A superset of matching addresses: historical names can nominate a candidate,
// but the final filter below still uses its current effective display name.
// Filtering before aggregation lets SQLite seek (account_id,email) for the
// few autocomplete candidates rather than group the entire message history.
const MATCHING_EMAILS: &str = "WITH matching_emails AS (
    SELECT email FROM contact_interactions WHERE account_id=?1 AND (
        instr(email,?2)=1 OR (instr(email,'@')>0 AND instr(substr(email,instr(email,'@')+1),?2)>0)
        OR instr(lower(display_name),?2)>0)
    UNION SELECT email FROM pinned_contacts WHERE account_id=?1 AND (
        instr(email,?2)=1 OR (instr(email,'@')>0 AND instr(substr(email,instr(email,'@')+1),?2)>0)
        OR instr(lower(display_name),?2)>0)
    UNION SELECT email FROM system_contact_addresses WHERE
        instr(email,?2)=1 OR instr(substr(email,instr(email,'@')+1),?2)>0 OR instr(lower(display_name),?2)>0
    UNION SELECT lower(a.email) FROM contact_addresses a JOIN contacts c ON c.id=a.contact_id WHERE
        instr(lower(a.email),?2)=1 OR (instr(a.email,'@')>0 AND instr(lower(substr(a.email,instr(a.email,'@')+1)),?2)>0)
        OR instr(lower(c.display_name),?2)>0
)";

impl Database {
    /// Ranks correspondents, saved profiles, and selected Mac address books
    /// for compose autocomplete. Mail history remains account-scoped. A sender is
    /// excluded from the "heard from" side when its message carries
    /// unsubscribe metadata (List-Unsubscribe/one-click) — that marks
    /// bulk/automated mail, not a real correspondent — unless the account
    /// also sent that address mail directly or pinned it. Autocomplete selects
    /// candidate addresses before aggregating their indexed interactions.
    pub fn list_contact_suggestions(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
    ) -> DbResult<Vec<ContactSuggestion>> {
        self.contact_suggestions(account_id, query, limit, true, &[], &[])
    }

    pub(crate) fn contact_suggestions_including_suppressed(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
    ) -> DbResult<Vec<ContactSuggestion>> {
        self.contact_suggestions(account_id, query, limit, false, &[], &[])
    }

    pub fn contextual_contact_suggestions(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
        seeds: &[String],
        excluded: &[String],
    ) -> DbResult<Vec<ContactSuggestion>> {
        if seeds.is_empty() && excluded.is_empty() {
            return self.list_contact_suggestions(account_id, query, limit);
        }
        self.contact_suggestions(account_id, query, limit, true, seeds, excluded)
    }

    /// Suggestions for the people invited to a calendar event. Mail history
    /// comes from `calendar_account` when it is also a mail account here, and
    /// otherwise from the primary mail account; saved contacts and selected
    /// Mac address books are shared by every account. The invitees seed
    /// "often emailed with" ranking and are never suggested again, and
    /// neither is the calendar account itself, which organizes the event.
    pub fn invitee_suggestions(
        &self,
        calendar_account: Option<&str>,
        query: &str,
        limit: usize,
        invitees: &[String],
    ) -> DbResult<Vec<ContactSuggestion>> {
        let accounts = self.list_accounts()?;
        let mail_account = calendar_account
            .and_then(|calendar| {
                accounts
                    .iter()
                    .find(|account| account.email.eq_ignore_ascii_case(calendar))
            })
            .map(|account| account.email.clone())
            .unwrap_or_else(|| self.primary_account_id());
        let mut excluded = invitees.to_vec();
        excluded.extend(calendar_account.map(str::to_string));
        self.contact_suggestions(&mail_account, query, limit, true, invitees, &excluded)
    }

    fn contact_suggestions(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
        hide_suppressed: bool,
        seeds: &[String],
        excluded: &[String],
    ) -> DbResult<Vec<ContactSuggestion>> {
        let limit = limit.clamp(1, 5_000);
        let needle = query.trim().to_ascii_lowercase();
        self.with_connection(|connection| {
            struct Agg {
                display_name: Option<String>,
                sent_count: i64,
                received_count: i64,
                last_interacted_at: String,
                pinned: bool,
                /// The saved contact's name when this is a saved contact's address.
                /// Matching it as well finds a contact saved under a name that
                /// differs from the one their mail uses.
                saved: Option<Option<String>>,
            }
            let relationships = super::co_recipients::relationships(connection, Some(account_id), seeds, chrono::Utc::now())?;
            let excluded = super::co_recipients::normalize_addresses(excluded);
            let mine = super::sending_addresses_for_account(connection, account_id);
            let contextual = !seeds.is_empty() || !excluded.is_empty();
            let mut by_email: HashMap<String, Agg> = HashMap::new();
            for (email, relation) in &relationships {
                by_email.insert(email.clone(), Agg { display_name: relation.display_name.clone(),
                    sent_count: 0, received_count: 0, last_interacted_at: String::new(), pinned: false, saved: None });
            }

            let candidate_cte = if needle.is_empty() { "" } else { MATCHING_EMAILS };
            let candidate_filter = if needle.is_empty() { "" } else { "AND i.email IN (SELECT email FROM matching_emails)" };
            let mut statement = connection.prepare(&format!(
                "{candidate_cte} SELECT i.email,
                    (SELECT recent.display_name FROM contact_interactions recent
                     WHERE recent.account_id=i.account_id AND recent.email=i.email
                       AND recent.display_name IS NOT NULL AND recent.display_name<>''
                     ORDER BY recent.sent_at DESC,recent.message_id DESC LIMIT 1),
                    SUM(i.direction='sent'), SUM(i.direction='received'), MAX(i.sent_at)
                 FROM contact_interactions i WHERE i.account_id=?1 {candidate_filter} GROUP BY i.account_id,i.email",
            ))?;
            let parameters = if needle.is_empty() { vec![account_id] } else { vec![account_id, needle.as_str()] };
            let rows = statement.query_map(rusqlite::params_from_iter(parameters), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, String>(4)?))
            })?;
            for row in rows {
                let (email,display_name,sent_count,received_count,last_interacted_at)=row?;
                by_email.insert(email,Agg{display_name,sent_count,received_count,last_interacted_at,pinned:false,saved:None});
            }

            let mut pinned_statement = connection
                .prepare(
                    "SELECT email, display_name, pinned_at FROM pinned_contacts WHERE account_id = ?1",
                )?;
            let pinned_rows = pinned_statement
                .query_map(params![account_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
            for row in pinned_rows {
                let (email, display_name, pinned_at) = row?;
                let entry = by_email.entry(email).or_insert_with(|| Agg {
                    display_name: display_name.clone(),
                    sent_count: 0,
                    received_count: 0,
                    last_interacted_at: pinned_at,
                    pinned: false,
                    saved: None,
                });
                entry.pinned = true;
                if entry.display_name.is_none() {
                    entry.display_name = display_name;
                }
            }

            let mut profile_statement=connection.prepare("SELECT a.email,c.display_name,c.favorite,c.updated_at FROM contact_addresses a JOIN contacts c ON c.id=a.contact_id")?;
            let profile_rows=profile_statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,bool>(2)?,row.get::<_,String>(3)?)))?;
            for row in profile_rows {let(email,name,favorite,updated)=row?;let email=email.to_ascii_lowercase();let entry=by_email.entry(email).or_insert_with(||Agg{display_name:name.clone(),sent_count:0,received_count:0,last_interacted_at:updated,pinned:false,saved:None});if entry.display_name.is_none(){entry.display_name=name.clone();}entry.pinned|=favorite;entry.saved=Some(name);}

            // Provider-only suggestions rank after established correspondents;
            // a selected address book does not imply favorite/pinned status.
            let mut system_statement = connection.prepare("SELECT a.email,a.display_name FROM system_contact_addresses a WHERE NOT EXISTS(SELECT 1 FROM system_contact_addresses sibling JOIN contact_addresses local ON local.email=sibling.email WHERE sibling.contact_id=a.contact_id) ORDER BY a.contact_id")?;
            let system_rows = system_statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)))?;
            for row in system_rows {
                let (email, name) = row?;
                let entry = by_email.entry(email).or_insert_with(|| Agg { display_name: name.clone(), sent_count: 0, received_count: 0, last_interacted_at: String::new(), pinned: false, saved: None });
                if name.is_some() { entry.display_name = name; }
            }

            let mut suppressed_statement = connection.prepare("SELECT email FROM contact_suggestion_suppressions")?;
            let suppressed = suppressed_statement.query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<std::collections::HashSet<_>, _>>()?;
            // While the user types, someone they saved as a contact outranks
            // people only in their mail history, so a contact never emailed
            // is not crowded out by busier correspondents with the same name.
            // With nothing typed, the list stays about mail history and context.
            let saved_first: std::collections::HashSet<String> = if needle.is_empty() { Default::default() }
                else { by_email.iter().filter(|(_, agg)| agg.saved.is_some()).map(|(email, _)| email.clone()).collect() };
            let mut suggestions: Vec<ContactSuggestion> = by_email
                .into_iter()
                .filter(|(email, agg)| {
                    if hide_suppressed && suppressed.contains(email) { return false; }
                    if contextual && (excluded.contains(email) || mine.contains(email)) { return false; }
                    let domain_matches = email
                        .split_once('@')
                        .is_some_and(|(_, domain)| domain.contains(&needle));
                    needle.is_empty()
                        || email.starts_with(&needle)
                        || domain_matches
                        || agg
                            .display_name
                            .as_deref()
                            .is_some_and(|name| name.to_ascii_lowercase().contains(&needle))
                        || agg.saved.as_ref().and_then(Option::as_deref)
                            .is_some_and(|name| name.to_ascii_lowercase().contains(&needle))
                })
                .map(|(email, agg)| ContactSuggestion {
                    email: email.clone(),
                    display_name: agg.display_name,
                    sent_count: agg.sent_count,
                    received_count: agg.received_count,
                    last_interacted_at: agg.last_interacted_at,
                    related_to: relationships.get(&email).map(|r| r.seeds.iter().take(3).cloned().collect()).unwrap_or_default(),
                    pinned: agg.pinned,
                })
                .collect();
            suggestions.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then_with(|| super::co_recipients::compare(relationships.get(&a.email), relationships.get(&b.email)))
                    .then_with(|| saved_first.contains(&b.email).cmp(&saved_first.contains(&a.email)))
                    .then(b.sent_count.cmp(&a.sent_count))
                    .then(b.received_count.cmp(&a.received_count))
                    .then(b.last_interacted_at.cmp(&a.last_interacted_at))
                    .then(a.email.cmp(&b.email))
            });
            suggestions.truncate(limit);
            Ok(suggestions)
        })
    }
}

#[cfg(test)]
#[path = "tests/contact_suggestions.rs"]
mod tests;
