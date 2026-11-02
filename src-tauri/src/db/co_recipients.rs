//! Rebuildable, device-local visible-recipient evidence. No sync/export writes.
use super::{Database, DatabaseError, DbResult};
use crate::{mime::NormalizedMessage, models::ContactGroupSuggestion};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(super) const WINDOW_DAYS: i64 = 365;
pub(super) const HALF_LIFE_DAYS: f64 = 90.0;
pub(super) const MAX_RECIPIENTS: usize = 20;
pub(super) const MIN_SUPPORT: usize = 2;
const MAX_CONTEXT: usize = 100;
const MAX_EVIDENCE_ROWS: usize = 20_000;
const BACKFILL_CURSOR: &str = "local_co_recipient_cursor";

pub(super) fn normalize_addresses(addresses: &[String]) -> BTreeSet<String> {
    addresses
        .iter()
        .filter_map(|email| {
            let email = email.trim().to_ascii_lowercase();
            crate::provider::imap::validate_envelope_address(&email)
                .ok()
                .map(|_| email)
        })
        .collect()
}

/// Index sent-folder headers: eligibility uses CURRENT account identities at
/// query time. Changing an alias cannot leave old sender decisions.
/// Cc/Bcc's combined search field is deliberately never passed here.
pub(super) fn index_message(
    tx: &Transaction<'_>,
    account: &str,
    message: &NormalizedMessage,
) -> DbResult<()> {
    index_headers(
        tx,
        account,
        &message.id,
        &message.from,
        &message.to,
        &message.cc,
        &message.date,
        message.unsubscribe.is_some()
            || !message.labels.iter().any(|label| label == "SENT")
            || message.labels.iter().any(|label| label == "DRAFT")
            || serde_json::from_str::<RankingMetadata>(&message.metadata_json)
                .is_ok_and(|raw| bulk_metadata(&raw)),
    )
}

// Deserialize only labels and headers. serde skips MIME bodies and attachment
// payload fields instead of allocating them again for relationship indexing.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RankingMetadata {
    #[serde(default)]
    label_ids: Option<Vec<String>>,
    #[serde(default)]
    payload: RankingHeaders,
}
#[derive(Default, serde::Deserialize)]
struct RankingHeaders {
    #[serde(default)]
    headers: Vec<RankingHeader>,
}
#[derive(serde::Deserialize)]
struct RankingHeader {
    name: String,
    value: String,
}

fn bulk_metadata(raw: &RankingMetadata) -> bool {
    raw.payload.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("List-Id")
            || (h.name.eq_ignore_ascii_case("Precedence")
                && ["bulk", "list", "junk"]
                    .iter()
                    .any(|value| h.value.trim().eq_ignore_ascii_case(value)))
    })
}

#[allow(clippy::too_many_arguments)]
fn index_headers(
    tx: &Transaction<'_>,
    account: &str,
    id: &str,
    from: &str,
    to: &[String],
    cc: &[String],
    date: &str,
    ineligible: bool,
) -> DbResult<()> {
    tx.execute(
        "DELETE FROM co_recipient_addresses WHERE message_id=?1",
        [id],
    )?;
    if ineligible {
        return Ok(());
    }
    let Ok(date) = DateTime::parse_from_rfc3339(date) else {
        return Ok(());
    };
    let date = date.with_timezone(&Utc).to_rfc3339();
    let senders = crate::correspondence::stored_addresses(from);
    if senders.len() != 1 {
        return Ok(());
    }
    let sender = senders[0].1.to_ascii_lowercase();
    if normalize_addresses(std::slice::from_ref(&sender)).is_empty() {
        return Ok(());
    }
    let mut recipients = BTreeMap::new();
    for raw in to.iter().chain(cc) {
        for (name, email) in crate::correspondence::stored_addresses(raw) {
            let email = email.to_ascii_lowercase();
            if !normalize_addresses(std::slice::from_ref(&email)).is_empty() {
                recipients.entry(email).or_insert(name);
            }
        }
    }
    // Keep self addresses here so identity changes can be handled at lookup.
    // Oversized messages remain bounded by the provider's existing header limits.
    // Reject obvious bulk lists before writing many rows; allow room for identities.
    if recipients.len() > MAX_RECIPIENTS + MAX_CONTEXT {
        return Ok(());
    }
    for (email, name) in recipients {
        tx.execute("INSERT INTO co_recipient_addresses(message_id,account_id,email,display_name,sender,sent_at)
            VALUES(?1,?2,?3,?4,?5,?6)", params![id, account, email, (!name.is_empty() && name != email).then_some(name), sender, date])?;
    }
    Ok(())
}

#[derive(Default, Debug)]
pub(super) struct Relationship {
    pub seeds: Vec<String>,
    pub strength: f64,
    pub display_name: Option<String>,
}

pub(super) fn compare(a: Option<&Relationship>, b: Option<&Relationship>) -> std::cmp::Ordering {
    let key = |r: Option<&Relationship>| {
        (
            r.map_or(0, |r| r.seeds.len()),
            r.map_or(0.0, |r| r.strength),
        )
    };
    let (an, aw) = key(a);
    let (bn, bw) = key(b);
    bn.cmp(&an).then_with(|| bw.total_cmp(&aw))
}

struct Evidence {
    account: String,
    thread: String,
    date: DateTime<Utc>,
    recipients: BTreeMap<String, Option<String>>,
}

/// Indexed seed lookups, bounded before scoring. Hitting the work budget returns
/// no relationship boost, rather than a biased truncated relationship graph.
pub(super) fn relationships(
    connection: &Connection,
    account: Option<&str>,
    seeds: &[String],
    now: DateTime<Utc>,
) -> DbResult<HashMap<String, Relationship>> {
    if seeds.len() > MAX_CONTEXT {
        return Ok(HashMap::new());
    }
    let mut seeds = normalize_addresses(seeds);
    if seeds.is_empty() {
        return Ok(HashMap::new());
    }
    let accounts: Vec<String> = if let Some(account) = account {
        vec![account.to_owned()]
    } else {
        let mut statement = connection.prepare("SELECT email FROM accounts")?;
        let values = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        values
    };
    let identities: HashMap<_, _> = accounts
        .iter()
        .map(|a| {
            (
                a.clone(),
                super::sending_addresses_for_account(connection, a),
            )
        })
        .collect();
    let own: BTreeSet<_> = identities.values().flatten().cloned().collect();
    seeds.retain(|s| !own.contains(s));
    if seeds.is_empty() {
        return Ok(HashMap::new());
    }
    let mut evidence: BTreeMap<String, Evidence> = BTreeMap::new();
    let mut rows_read = 0;
    for seed in &seeds {
        // Separate SQL forms retain the leading account/email index in compose.
        let restriction = if account.is_some() {
            "s.account_id=?3 AND"
        } else {
            ""
        };
        let mut statement = connection.prepare(&format!(
            "SELECT s.message_id,s.account_id,m.thread_id,s.sender,s.sent_at,r.email,r.display_name
            FROM co_recipient_addresses s JOIN co_recipient_addresses r ON r.message_id=s.message_id
            JOIN messages m ON m.id=s.message_id WHERE {restriction} s.email=?1 AND s.sent_at>=?2
            LIMIT ?4"
        ))?;
        let cutoff = (now - Duration::days(WINDOW_DAYS)).to_rfc3339();
        let rows = statement.query_map(
            params![
                seed,
                cutoff,
                account,
                (MAX_EVIDENCE_ROWS - rows_read + 1) as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )?;
        for row in rows {
            rows_read += 1;
            if rows_read > MAX_EVIDENCE_ROWS {
                return Ok(HashMap::new());
            }
            let (id, account, thread, sender, date, email, name) = row?;
            let Some(mine) = identities.get(&account) else {
                continue;
            };
            if !mine.contains(&sender) {
                continue;
            }
            let Ok(date) = DateTime::parse_from_rfc3339(&date).map(|d| d.with_timezone(&Utc))
            else {
                continue;
            };
            if date < now - Duration::days(WINDOW_DAYS) {
                continue;
            }
            let group = evidence.entry(id).or_insert_with(|| Evidence {
                account,
                thread,
                date,
                recipients: BTreeMap::new(),
            });
            if !own.contains(&email) {
                group.recipients.insert(email, name);
            }
        }
    }
    Ok(score(
        &evidence.into_values().collect::<Vec<_>>(),
        &seeds,
        now,
    ))
}

/// Pure ranking with an explicit clock; only distinct messages establish support.
fn score(
    evidence: &[Evidence],
    seeds: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> HashMap<String, Relationship> {
    let mut result: HashMap<String, Relationship> = HashMap::new();
    for seed in seeds {
        let mut denominator = 0.0;
        let mut pairs: BTreeMap<
            String,
            (
                usize,
                BTreeMap<(String, String, chrono::NaiveDate), f64>,
                Option<String>,
            ),
        > = BTreeMap::new();
        for message in evidence {
            let size = message.recipients.len();
            if size < 2
                || size > MAX_RECIPIENTS
                || !message.recipients.contains_key(seed)
                || message.date < now - Duration::days(WINDOW_DAYS)
            {
                continue;
            }
            let age = (now - message.date).num_seconds().max(0) as f64 / 86_400.0;
            let recency = 2_f64.powf(-age / HALF_LIFE_DAYS);
            denominator += recency;
            for (email, name) in &message.recipients {
                if seeds.contains(email) {
                    continue;
                }
                let pair = pairs.entry(email.clone()).or_default();
                pair.0 += 1;
                if pair.2.is_none() {
                    pair.2 = name.clone();
                }
                let weight = recency / (size - 1) as f64;
                pair.1
                    .entry((
                        message.account.clone(),
                        message.thread.clone(),
                        message.date.date_naive(),
                    ))
                    .and_modify(|w| *w = w.max(weight))
                    .or_insert(weight);
            }
        }
        if denominator == 0.0 {
            continue;
        }
        for (email, (support, groups, name)) in pairs {
            if support < MIN_SUPPORT {
                continue;
            }
            let entry = result.entry(email).or_default();
            entry.seeds.push(seed.clone());
            entry.strength += groups.values().sum::<f64>() / denominator / seeds.len() as f64;
            if entry.display_name.is_none() {
                entry.display_name = name;
            }
        }
    }
    result
}

impl Database {
    /// Resumable primary-key walk, independent of normal ingestion. A short
    /// transaction commits both the cursor and the batch; deletion needs no log.
    pub(crate) fn backfill_co_recipients(&self, limit: usize) -> DbResult<usize> {
        self.with_transaction(|tx| {
            let cursor: String = tx.query_row("SELECT value FROM compose_settings WHERE key=?1", [BACKFILL_CURSOR], |row| row.get(0)).optional()?.unwrap_or_default();
            let mut statement = tx.prepare("SELECT m.id,t.account_id,m.sender,m.recipients_json,m.cc_json,m.sent_at,m.unsubscribe_json IS NOT NULL,
                t.labels_json FROM messages m JOIN threads t ON t.id=m.thread_id
                WHERE m.id>?1 ORDER BY m.id LIMIT ?2")?;
            let rows = statement.query_map(params![cursor,limit.clamp(1,500) as i64], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,bool>(6)?,r.get::<_,String>(7)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for (id, account, from, to, cc, date, bulk, thread_labels) in &rows {
                // Read one payload at a time, never an entire batch of MIME
                // metadata blobs. Header-only deserialization skips bodies.
                let source: Option<(String, Option<Vec<u8>>)> = tx.query_row(
                    "SELECT payload,payload_z FROM message_metadata WHERE id=?1", [id],
                    |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
                let (payload, compressed) = source.unwrap_or_default();
                let metadata = super::messages::resolve_body(0, payload, compressed).ok()
                    .and_then(|json| serde_json::from_str::<RankingMetadata>(&json).ok());
                // Old metadata that cannot establish individual draft status
                // falls back conservatively to the thread's draft flag.
                let ineligible = metadata.and_then(|m| m.label_ids.as_ref().map(|labels| !labels.iter().any(|l| l == "SENT") || labels.iter().any(|l| l == "DRAFT") || bulk_metadata(&m))).unwrap_or_else(||
                    serde_json::from_str::<Vec<String>>(thread_labels).map_or(true, |labels| !labels.iter().any(|l| l == "SENT") || labels.iter().any(|l| l == "DRAFT")));
                let to: Vec<String> = serde_json::from_str(to).unwrap_or_default();
                let cc: Vec<String> = serde_json::from_str(cc).unwrap_or_default();
                index_headers(tx, account, id, from, &to, &cc, date, *bulk || ineligible)?;
            }
            if let Some(last) = rows.last() {
                tx.execute("INSERT INTO compose_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![BACKFILL_CURSOR,last.0])?;
            }
            Ok(rows.len())
        })
    }

    pub(crate) fn rank_contact_group_candidates(
        &self,
        group_id: &str,
        selected_ids: &[String],
        emails: &[String],
    ) -> DbResult<Vec<ContactGroupSuggestion>> {
        if selected_ids.len() > MAX_CONTEXT || emails.len() > MAX_CONTEXT {
            return Ok(Vec::new());
        }
        let group = self
            .get_contact_group(group_id)?
            .ok_or(DatabaseError::NotFound("Contact group"))?;
        let profiles = self.list_contact_profiles("", 5_000)?;
        let mut seeds = emails.to_vec();
        for profile in &profiles {
            if group.member_ids.contains(&profile.id) || selected_ids.contains(&profile.id) {
                seeds.extend(profile.addresses.clone());
            }
        }
        self.with_connection(|connection| {
            let relationships = relationships(connection, None, &seeds, Utc::now())?;
            let mut statement =
                connection.prepare("SELECT email FROM contact_suggestion_suppressions")?;
            let hidden = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<BTreeSet<_>>>()?;
            let mut candidates: Vec<_> = profiles
                .iter()
                .filter(|p| !group.member_ids.contains(&p.id))
                .filter_map(|profile| {
                    let best = profile
                        .addresses
                        .iter()
                        .filter(|a| !hidden.contains(*a))
                        .filter_map(|a| relationships.get(a))
                        .min_by(|a, b| compare(Some(a), Some(b)));
                    best.map(|r| (profile, r))
                })
                .collect();
            candidates.sort_by(|(a, ar), (b, br)| {
                b.favorite
                    .cmp(&a.favorite)
                    .then_with(|| compare(Some(ar), Some(br)))
                    .then(a.id.cmp(&b.id))
            });
            Ok(candidates
                .into_iter()
                .map(|(p, r)| ContactGroupSuggestion {
                    id: p.id.clone(),
                    related_to: r.seeds.iter().take(3).cloned().collect(),
                })
                .collect())
        })
    }
}

#[cfg(test)]
#[path = "tests/co_recipients.rs"]
mod tests;
