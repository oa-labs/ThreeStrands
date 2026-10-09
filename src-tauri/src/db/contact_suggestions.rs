//! Mail-derived and pinned contact suggestion queries.

use super::{Database, DbResult};
use crate::models::ContactSuggestion;
use rusqlite::params;
use std::collections::HashMap;

impl Database {
    /// Ranks past correspondents for compose autocomplete. Deliberately
    /// doesn't import Google's address book: every suggestion is mined from
    /// this account's own cached `messages` (who it sent to, who it heard
    /// from), so results are inherently people the user has actually
    /// corresponded with, plus anything explicitly pinned. A sender is
    /// excluded from the "heard from" side when its message carries
    /// unsubscribe metadata (List-Unsubscribe/one-click) — that marks
    /// bulk/automated mail, not a real correspondent — unless the account
    /// also sent that address mail directly or pinned it. Bounded to the
    /// most recent messages so a large mailbox can't make every keystroke
    /// re-parse years of history.
    pub fn list_contact_suggestions(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
    ) -> DbResult<Vec<ContactSuggestion>> {
        self.contact_suggestions(account_id, query, limit, true)
    }

    pub(crate) fn contact_suggestions_including_suppressed(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
    ) -> DbResult<Vec<ContactSuggestion>> {
        self.contact_suggestions(account_id, query, limit, false)
    }

    fn contact_suggestions(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
        hide_suppressed: bool,
    ) -> DbResult<Vec<ContactSuggestion>> {
        let limit = limit.clamp(1, 5_000);
        self.with_connection(|connection| {
            struct Agg {
                display_name: Option<String>,
                sent_count: i64,
                received_count: i64,
                last_interacted_at: String,
                pinned: bool,
            }
            let mut by_email: HashMap<String, Agg> = HashMap::new();

            let mut statement = connection.prepare(
                "SELECT i.email,
                    (SELECT recent.display_name FROM contact_interactions recent
                     WHERE recent.account_id=i.account_id AND recent.email=i.email
                       AND recent.display_name IS NOT NULL AND recent.display_name<>''
                     ORDER BY recent.sent_at DESC,recent.message_id DESC LIMIT 1),
                    SUM(i.direction='sent'), SUM(i.direction='received'), MAX(i.sent_at)
                 FROM contact_interactions i WHERE i.account_id=?1 GROUP BY i.account_id,i.email",
            )?;
            let rows = statement.query_map(params![account_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, String>(4)?))
            })?;
            for row in rows {
                let (email,display_name,sent_count,received_count,last_interacted_at)=row?;
                by_email.insert(email,Agg{display_name,sent_count,received_count,last_interacted_at,pinned:false});
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
                });
                entry.pinned = true;
                if entry.display_name.is_none() {
                    entry.display_name = display_name;
                }
            }

            let mut profile_statement=connection.prepare("SELECT a.email,c.display_name,c.favorite,c.updated_at FROM contact_addresses a JOIN contacts c ON c.id=a.contact_id")?;
            let profile_rows=profile_statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,bool>(2)?,row.get::<_,String>(3)?)))?;
            for row in profile_rows {let(email,name,favorite,updated)=row?;let email=email.to_ascii_lowercase();let entry=by_email.entry(email).or_insert_with(||Agg{display_name:name.clone(),sent_count:0,received_count:0,last_interacted_at:updated,pinned:false});if entry.display_name.is_none(){entry.display_name=name;}entry.pinned|=favorite;}

            let mut suppressed_statement = connection.prepare("SELECT email FROM contact_suggestion_suppressions")?;
            let suppressed = suppressed_statement.query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<std::collections::HashSet<_>, _>>()?;
            let needle = query.trim().to_ascii_lowercase();
            let mut suggestions: Vec<ContactSuggestion> = by_email
                .into_iter()
                .filter(|(email, agg)| {
                    if hide_suppressed && suppressed.contains(email) { return false; }
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
                })
                .map(|(email, agg)| ContactSuggestion {
                    email,
                    display_name: agg.display_name,
                    sent_count: agg.sent_count,
                    received_count: agg.received_count,
                    last_interacted_at: agg.last_interacted_at,
                    pinned: agg.pinned,
                })
                .collect();
            suggestions.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then(b.sent_count.cmp(&a.sent_count))
                    .then(b.received_count.cmp(&a.received_count))
                    .then(b.last_interacted_at.cmp(&a.last_interacted_at))
            });
            suggestions.truncate(limit);
            Ok(suggestions)
        })
    }
}

#[cfg(test)]
#[path = "tests/contact_suggestions.rs"]
mod tests;
