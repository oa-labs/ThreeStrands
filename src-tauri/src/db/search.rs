//! Full-text queries, preview encoding, and bounded search-index repair.

use super::messages::{compress_body, resolve_body};
use super::threads::THREAD_COLUMNS;
use super::{decode_json, Database, DbResult};
use crate::db::DatabaseError;
use crate::mime::{normalize, readable_body_text, RawMessage};
use crate::models::{SearchThreadsRequest, Thread};
use crate::search_query::{
    check_local_search_limits, default_archived_trashed_filter, DateBound, DefaultFilter,
    LocalSearchPlan, MessageState, QueryTerm, StructuredLocalSearch,
};
use rusqlite::{params, types::Value, OptionalExtension};

impl Database {
    /// `account_id` merges every account when `None` — the unified inbox —
    /// or scopes to just that account when set, same as [`Self::list_threads`].
    ///
    /// Phase 5: a query that contains at least one recognised operator
    /// (`from:`/`to:`/`subject:`/`before:`/`after:`/`is:`/`has:attachment`/
    /// `label:`/`in:`) and no `Raw` term takes the OPERATOR-AWARE structured
    /// path ([`Self::structured_search_threads`]); every other query — free
    /// text only, and anything with unmodelled syntax (negation, `OR`,
    /// parentheses, an empty `label:`/`in:`) — takes the unchanged legacy FTS
    /// path below, byte-identical to before. The decision is the pure
    /// [`LocalSearchPlan`] in `search_query.rs`.
    pub fn search_threads(
        &self,
        request: &SearchThreadsRequest,
        account_id: Option<&str>,
    ) -> DbResult<Vec<Thread>> {
        if request.query.trim().is_empty() {
            return self.list_threads(account_id);
        }
        match LocalSearchPlan::parse(&request.query) {
            LocalSearchPlan::Structured(plan) => {
                self.structured_search_threads(&plan, request, account_id)
            }
            LocalSearchPlan::Legacy => self.legacy_search_threads(request, account_id),
        }
    }

    /// The pre-run-2a local search: hand the raw query string to `fts_query`
    /// and MATCH it against `thread_search`. Unchanged — a legacy query
    /// produces the same SQL and the same results as before run 2a.
    fn legacy_search_threads(
        &self,
        request: &SearchThreadsRequest,
        account_id: Option<&str>,
    ) -> DbResult<Vec<Thread>> {
        self.with_connection(|connection| {
            let limit = request.limit.unwrap_or(50).min(200) as i64;
            let offset = request.offset.unwrap_or(0) as i64;
            let query = fts_query(&request.query);
            if query.trim().is_empty() {
                return Ok(Vec::new());
            }
            // The inbox-only scope hides archived and trashed threads. The
            // all-mail scope (`include_archived`) reveals archived mail but,
            // like Gmail's default search, still hides Trash and Spam; this
            // free-text path has no `in:` term that could name them.
            let archived_filter = if request.include_archived.unwrap_or(false) {
                ALL_MAIL_LEGACY_FILTER
            } else {
                "AND t.archived = 0 AND t.trashed = 0"
            };
            let account_filter = if account_id.is_some() {
                "AND t.account_id = ?4"
            } else {
                ""
            };
            // -1 asks FTS5 to excerpt whichever column has the most matches, so a
            // hit on the body or a recipient still produces a relevant snippet.
            // The match itself is wrapped in \u{1}/\u{2} rather than HTML markup
            // so the frontend can highlight it without ever parsing untrusted HTML.
            let sql = format!(
                "SELECT {THREAD_COLUMNS},
                        snippet(thread_search, -1, '\u{1}', '\u{2}', '…', 12) AS match_snippet
                 FROM thread_search s
                 JOIN threads t ON t.id = s.thread_id
                 WHERE thread_search MATCH ?1 {archived_filter} {account_filter}
                 ORDER BY t.last_received_at DESC, rank
                 LIMIT ?2 OFFSET ?3"
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = match account_id {
                Some(id) => {
                    statement.query_map(params![query, limit, offset, id], map_thread_row)?
                }
                None => statement.query_map(params![query, limit, offset], map_thread_row)?,
            };
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Operator-aware local search. Builds a single query whose structured
    /// terms become `WHERE` predicates — `threads` columns directly, `messages`
    /// facts as `EXISTS` subqueries by `thread_id` (so a thread matching
    /// through several messages is returned ONCE, never duplicated as a JOIN
    /// would), and free text through the unchanged `fts_query`/`MATCH` only
    /// when present. All user text goes in as bound PARAMETERS; nothing is
    /// interpolated. Shares the legacy path's archived/trashed default
    /// exclusion, account filter, limit/offset and recency ordering.
    fn structured_search_threads(
        &self,
        plan: &StructuredLocalSearch,
        request: &SearchThreadsRequest,
        account_id: Option<&str>,
    ) -> DbResult<Vec<Thread>> {
        // Reject an over-limit query outright with a plain message — never
        // truncate and never fall back to the legacy path.
        if let Err(limit) = check_local_search_limits(plan) {
            return Err(DatabaseError::invalid(limit.message()));
        }

        self.with_connection(|connection| {
            let limit = request.limit.unwrap_or(50).min(200) as i64;
            let offset = request.offset.unwrap_or(0) as i64;

            // Bound parameters, in the order the `?N` placeholders below are
            // appended. Everything user-controlled is a parameter.
            let mut binds: Vec<Value> = Vec::new();
            let mut predicates: Vec<String> = Vec::new();

            // Free text, rebuilt exactly as the legacy path would see it and
            // passed through the SAME `fts_query`. Only present when the query
            // has free-text terms; with none, no `MATCH` is used at all.
            let fts = plan.free_text().map(|text| fts_query(&text));
            // An all-whitespace / empty FTS expression matches nothing, exactly
            // as the legacy path returns an empty result for one.
            let uses_match = fts.as_deref().is_some_and(|q| !q.trim().is_empty());
            if let Some(fts) = &fts {
                if !uses_match {
                    return Ok(Vec::new());
                }
                binds.push(Value::Text(fts.clone()));
                predicates.push("thread_search MATCH ?".to_string());
            }

            for term in &plan.terms {
                if let Some(predicate) = term_predicate(term, &mut binds, false)? {
                    predicates.push(predicate);
                }
            }

            // The archived/trashed default exclusion, relaxed for a query that
            // names a location (`label:`/`in:`). Computed by the pure
            // `default_archived_trashed_filter`; see its rules.
            let DefaultFilter {
                exclude_archived,
                exclude_trashed,
                exclude_spam,
            } = default_archived_trashed_filter(
                &plan.terms,
                request.include_archived.unwrap_or(false),
            );
            if exclude_archived {
                predicates.push("t.archived = 0".to_string());
            }
            if exclude_trashed {
                predicates.push("t.trashed = 0".to_string());
            }
            if exclude_spam {
                predicates.push(format!("NOT {}", thread_has_system_id("SPAM")));
            }
            if let Some(id) = account_id {
                binds.push(Value::Text(id.to_string()));
                predicates.push("t.account_id = ?".to_string());
            }

            // When MATCH is used, read from the FTS table joined to `threads`
            // exactly as the legacy path does: `snippet()` and `rank` only work
            // inside the MATCH query itself, so a correlated subquery cannot
            // supply them. With no MATCH there is no rank and no snippet, so
            // order on recency then a deterministic `t.id`, and return NULL.
            let (from_clause, snippet_select, order_by) = if uses_match {
                (
                    "thread_search s JOIN threads t ON t.id = s.thread_id",
                    "snippet(thread_search, -1, '\u{1}', '\u{2}', '…', 12) AS match_snippet",
                    "ORDER BY t.last_received_at DESC, rank, t.id",
                )
            } else {
                (
                    "threads t",
                    "NULL AS match_snippet",
                    "ORDER BY t.last_received_at DESC, t.id",
                )
            };

            // Rewrite the sequential `?` placeholders into positional `?N` so
            // the bind order is explicit and the limit/offset tail is stable.
            let where_clause = predicates.join(" AND ");
            let sql = number_placeholders(&format!(
                "SELECT {THREAD_COLUMNS}, {snippet_select}
                 FROM {from_clause}
                 WHERE {where_clause}
                 {order_by}
                 LIMIT ? OFFSET ?"
            ));
            binds.push(Value::Integer(limit));
            binds.push(Value::Integer(offset));

            let mut statement = connection.prepare(&sql)?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(binds.iter()), map_thread_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Rewrites a bounded batch of threads queued by schema migrations so their
    /// search `body` drops quoted history repeated within the thread, and
    /// both the search `snippet` and the inbox preview (`threads.snippet`)
    /// show the latest message without its quote (see
    /// [`crate::quoted_history::searchable_thread_text`]). Returns the number
    /// of queued threads handled; call repeatedly until it returns 0. A queued
    /// thread that no longer has a search row is simply dequeued. Re-normalize
    /// cached MIME payloads when available to repair HTML-derived text produced
    /// by older extractors, while preserving genuine text/plain alternatives.
    pub fn reindex_next_search_batch(&self, batch_size: usize) -> DbResult<usize> {
        self.with_transaction(|transaction| {
            let queued: Vec<String> = transaction
                .prepare("SELECT thread_id FROM pending_search_reindex LIMIT ?1")?
                .query_map(params![batch_size as i64], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            for thread_id in &queued {
                let rowid: Option<i64> = transaction
                    .query_row(
                        "SELECT rowid FROM thread_search WHERE thread_id = ?1",
                        [thread_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(rowid) = rowid {
                    let messages: Vec<(String, String, String, Option<Vec<String>>)> = transaction
                        .prepare(
                            "SELECT m.id, m.body_text, m.body_text_z,
                                    COALESCE(mm.payload, ''), mm.payload_z
                             FROM messages m LEFT JOIN message_metadata mm ON mm.id = m.id
                             WHERE m.thread_id = ?1 ORDER BY m.sent_at, m.id",
                        )?
                        .query_map([thread_id], |row| {
                            let stored = resolve_body(1, row.get(1)?, row.get(2)?)?;
                            let metadata = resolve_body(3, row.get(3)?, row.get(4)?)?;
                            let normalized = serde_json::from_str::<RawMessage>(&metadata)
                                .ok()
                                .and_then(|raw| normalize(&raw).ok());
                            let cc_bcc = normalized.as_ref().map(|message| message.cc_bcc.clone());
                            let corrected = normalized
                                .map(|message| message.body_text)
                                .unwrap_or_else(|| readable_body_text(stored.clone()));
                            Ok((row.get(0)?, stored, corrected, cc_bcc))
                        })?
                        .collect::<Result<_, _>>()?;
                    for (id, stored, corrected, cc_bcc) in &messages {
                        if stored != corrected {
                            transaction.execute(
                                "UPDATE messages SET body_text = '', body_text_z = ?1 WHERE id = ?2",
                                params![compress_body(corrected), id],
                            )?;
                        }
                        // Fills in Cc/Bcc for mail stored before they were
                        // kept; a message without a stored payload keeps `[]`.
                        if let Some(cc_bcc) = cc_bcc {
                            transaction.execute(
                                "UPDATE messages SET cc_bcc_json = ?1 WHERE id = ?2",
                                params![serde_json::to_string(cc_bcc).unwrap_or_else(|_| "[]".into()), id],
                            )?;
                        }
                    }
                    // The stored preview is still the provider's until this
                    // rewrite; it remains the fallback for a body without text.
                    let provider_snippet: String = transaction.query_row(
                        "SELECT snippet FROM threads WHERE id = ?1",
                        [thread_id],
                        |row| row.get(0),
                    )?;
                    let text = crate::quoted_history::searchable_thread_text(
                        messages.iter().map(|(_, _, text, _)| text.as_str()),
                    );
                    transaction.execute(
                        "UPDATE thread_search SET body = ?1, snippet = ?2 WHERE rowid = ?3",
                        params![text.body, search_preview(&text, &provider_snippet), rowid],
                    )?;
                    transaction.execute(
                        "UPDATE threads SET snippet = ?1 WHERE id = ?2",
                        params![list_preview(&text, &provider_snippet), thread_id],
                    )?;
                }
                transaction.execute(
                    "DELETE FROM pending_search_reindex WHERE thread_id = ?1",
                    [thread_id],
                )?;
            }
            Ok(queued.len())
        })
    }
}

/// Maps a thread result row (the `THREAD_COLUMNS` plus a trailing
/// `match_snippet`) into a [`Thread`]. Shared by the legacy and structured
/// local-search paths so both decode a row identically.
fn map_thread_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: row.get(0)?,
        provider_thread_id: row.get(1)?,
        subject: row.get(2)?,
        snippet: row.get(3)?,
        participants: decode_json(row.get::<_, String>(4)?)?,
        last_message_at: row.get(5)?,
        unread: row.get(6)?,
        starred: row.get(7)?,
        archived: row.get(8)?,
        labels: decode_json(row.get::<_, String>(9)?)?,
        trashed: row.get(10)?,
        account_id: row.get(11)?,
        summary: row.get(12)?,
        summary_generated_at: row.get(13)?,
        has_attachments: row.get(14)?,
        last_received_at: row.get(15)?,
        summary_revision: row.get(16)?,
        match_snippet: row.get(17)?,
    })
}

/// The SQL predicate that matches a thread whose `labels_json` holds any id
/// that is a label of the THREAD'S OWN account whose cached `name` equals the
/// bound NAME. Two placeholders: the first matches the name exactly, the
/// second lets a hyphen form (`label:my-label`) find a spaced name ("My
/// Label") — Gmail users type labels with hyphens for spaces. The CALLER binds
/// NAME to BOTH placeholders (two pushes), in order.
///
/// Matching is `COLLATE NOCASE`, i.e. ASCII case-insensitive ONLY (`a`==`A`,
/// but `é`/`É` are distinct) — the same limitation as the rest of the
/// structured path and `label_ids_named`. The comparison is `=`/NOCASE, not
/// `LIKE`, so SQL wildcards (`%`, `_`) and quotes in the name are matched
/// LITERALLY, never as patterns. Several labels sharing a name are all
/// accepted. A name nobody has yields an empty `EXISTS` — no match, not an
/// error. `EXISTS` (not a JOIN) so a thread matching through several labels is
/// returned ONCE.
///
/// CACHE CAVEAT: resolution is only as good as the `account_labels` cache. If
/// the cache for an account is empty or stale, `label:` and non-system `in:`
/// find NOTHING for that account. That is expected until the frontend's label
/// listing has run once for the account (it warms every connected account at
/// startup). This is a rebuildable cache, not a source of truth; there is
/// deliberately no provider call or migration here to work around a cold
/// cache.
fn label_name_sql() -> String {
    "EXISTS (SELECT 1 FROM account_labels al, json_each(t.labels_json) j \
     WHERE al.account_id = t.account_id AND al.label_id = j.value \
     AND (al.name = ? COLLATE NOCASE OR replace(al.name, ' ', '-') = ? COLLATE NOCASE))"
        .to_string()
}

/// The SQL predicate for one structured term, appending its bound values to
/// `binds` in placeholder order. Top-level free text returns `None`: it is
/// matched once by the query's own `MATCH`, which also supplies the rank and
/// snippet. Free text inside `Not`/`AnyOf` (`nested`) becomes a subquery on
/// the same index. User text is always bound, never interpolated.
fn term_predicate(term: &QueryTerm, binds: &mut Vec<Value>, nested: bool) -> DbResult<Option<String>> {
    let predicate = match term {
        QueryTerm::FreeText { .. } if !nested => return Ok(None),
        QueryTerm::FreeText { text, quoted } => {
            let fts = fts_query(&if *quoted { format!("\"{text}\"") } else { text.clone() });
            if fts.trim().is_empty() {
                // Nothing searchable (punctuation only): matches nothing, as
                // the top-level path returns no results for it.
                "0".to_string()
            } else {
                binds.push(Value::Text(fts));
                "t.id IN (SELECT thread_id FROM thread_search WHERE thread_search MATCH ?)".to_string()
            }
        }
        QueryTerm::From(value) => {
            binds.push(Value::Text(crate::search_query::like_contains_pattern(value)));
            "EXISTS (SELECT 1 FROM messages m WHERE m.thread_id = t.id \
             AND m.sender LIKE ? ESCAPE '\\')"
                .to_string()
        }
        QueryTerm::To(value) => {
            binds.push(Value::Text(crate::search_query::like_contains_pattern(value)));
            // Like Gmail, `to:` covers To, Cc and Bcc. Both columns are JSON
            // arrays, so each element is matched through `json_each`: a LIKE
            // over the raw text would also match the brackets, quotes and
            // commas of the encoding.
            "EXISTS (SELECT 1 FROM messages m WHERE m.thread_id = t.id AND EXISTS ( \
             SELECT 1 FROM (SELECT value FROM json_each(m.recipients_json) \
                            UNION ALL SELECT value FROM json_each(m.cc_bcc_json)) r \
             WHERE r.value LIKE ? ESCAPE '\\'))"
                .to_string()
        }
        QueryTerm::Subject(value) => {
            binds.push(Value::Text(crate::search_query::like_contains_pattern(value)));
            "t.subject LIKE ? ESCAPE '\\'".to_string()
        }
        QueryTerm::Date { bound, date } => {
            binds.push(Value::Text(crate::search_query::utc_day_start_rfc3339(*date)));
            let op = match bound {
                // `after:d` = on or after the start of day d (UTC).
                DateBound::After => ">=",
                // `before:d` = strictly before the start of day d.
                DateBound::Before => "<",
            };
            // Compare INSTANTS, not strings: `sent_at` is RFC 3339 in
            // whatever offset the message carried (a `Date:` header fallback
            // keeps the sender's offset), so a lexicographic compare against a
            // UTC bound is wrong by the offset. `datetime()` normalises both
            // to UTC and yields NULL (so no match) for an unparseable value.
            format!(
                "EXISTS (SELECT 1 FROM messages m WHERE m.thread_id = t.id \
                 AND datetime(m.sent_at) {op} datetime(?))"
            )
        }
        QueryTerm::State(MessageState::Unread) => "t.unread = 1".to_string(),
        QueryTerm::State(MessageState::Starred) => "t.starred = 1".to_string(),
        QueryTerm::HasAttachment => "t.has_attachments = 1".to_string(),
        QueryTerm::Label(value) => {
            // Resolve a label NAME to the ids in `t.labels_json` through the
            // per-account cache. See `label_name_sql`. The fragment has TWO
            // placeholders (exact name and hyphen-folded name), so NAME is
            // bound twice.
            binds.push(Value::Text(value.clone()));
            binds.push(Value::Text(value.clone()));
            label_name_sql()
        }
        QueryTerm::In(value) => {
            // A fixed set of system LOCATION words resolves without the cache
            // (system ids are provider-neutral); any other value resolves
            // exactly like a label NAME.
            match system_location_predicate(value) {
                Some(predicate) => predicate,
                None => {
                    binds.push(Value::Text(value.clone()));
                    binds.push(Value::Text(value.clone()));
                    label_name_sql()
                }
            }
        }
        QueryTerm::Not(inner) => {
            let inner = term_predicate(inner, binds, true)?.unwrap_or_else(|| "1".to_string());
            format!("NOT ({inner})")
        }
        QueryTerm::AnyOf(alternatives) => {
            let mut parts = Vec::with_capacity(alternatives.len());
            for alternative in alternatives {
                parts.push(term_predicate(alternative, binds, true)?.unwrap_or_else(|| "1".to_string()));
            }
            format!("(({}))", parts.join(") OR ("))
        }
        // Unreachable: a Raw term would have routed to legacy. Listed so a
        // new term fails to compile here.
        QueryTerm::Raw(_) => {
            return Err(DatabaseError::invalid(
                "internal: non-structured term reached the structured path",
            ));
        }
    };
    Ok(Some(predicate))
}

/// The legacy free-text path's all-mail scope: archived mail is searched,
/// Trash and Spam are not.
const ALL_MAIL_LEGACY_FILTER: &str = "AND t.trashed = 0 \
    AND NOT EXISTS (SELECT 1 FROM json_each(t.labels_json) j WHERE j.value = 'SPAM')";

/// Matches a thread whose `labels_json` holds the system label id `id`. `id`
/// is a hard-coded CONSTANT (never user text), so it is safe to inline into
/// the SQL — it binds no parameter and is not a `LIKE` pattern.
fn thread_has_system_id(id: &str) -> String {
    format!("EXISTS (SELECT 1 FROM json_each(t.labels_json) j WHERE j.value = '{id}')")
}

/// The predicate for a system `in:` LOCATION word, or `None` when the value is
/// not a system word (the caller then resolves it through the label cache like
/// a `label:` NAME). Case-insensitive on the word. These resolve WITHOUT the
/// cache because system ids are the same on every provider, so `in:inbox`,
/// `in:sent`, `in:trash`, `in:spam`/`junk`, `in:archive` and `in:anywhere`
/// work offline and with an empty cache.
///
/// `archive` is defined BY EXCLUSION (archived, not trashed, not spam): an
/// IMAP Archive mailbox contributes no label of its own, so an archived thread
/// cannot be told apart from other non-inbox mail by a positive id — it is
/// simply mail that is out of the inbox but not in trash or spam.
///
/// `anywhere` carries no predicate; the breadth it needs comes from the
/// default archived/trashed filter being fully relaxed for it (see
/// `default_archived_trashed_filter`).
fn system_location_predicate(value: &str) -> Option<String> {
    let word = value.to_ascii_lowercase();
    match word.as_str() {
        "inbox" => Some(thread_has_system_id("INBOX")),
        "sent" => Some(thread_has_system_id("SENT")),
        "trash" => Some("t.trashed = 1".to_string()),
        "spam" | "junk" => Some(thread_has_system_id("SPAM")),
        "archive" => Some(format!(
            "t.archived = 1 AND t.trashed = 0 AND NOT {}",
            thread_has_system_id("SPAM")
        )),
        // No positive predicate; the relaxed default filter does the work.
        "anywhere" => Some("1 = 1".to_string()),
        _ => None,
    }
}

/// Rewrites sequential anonymous `?` placeholders into positional `?1`, `?2`,
/// … left to right. The structured query is assembled from a variable number
/// of predicate fragments, so numbering them after assembly keeps each bound
/// value tied to its position without the fragments having to know their own
/// index. There are no literal `?` characters in the generated SQL other than
/// placeholders (user text is bound, never inlined), so a plain left-to-right
/// rewrite is unambiguous.
fn number_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 16);
    let mut index = 0u32;
    for ch in sql.chars() {
        if ch == '?' {
            index += 1;
            out.push('?');
            out.push_str(&index.to_string());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Builds an FTS5 MATCH expression, ANDing together every unquoted word and
/// every "quoted phrase" as a prefix match. `input.split('"')` alternates
/// unquoted segments (even indices) with quoted ones (odd indices); an
/// unterminated trailing quote is simply treated as still-quoted.
fn fts_query(input: &str) -> String {
    let mut terms: Vec<String> = Vec::new();
    for (index, segment) in input.split('"').enumerate() {
        if index % 2 == 0 {
            for word in segment.split_whitespace() {
                terms.push(format!("\"{}\"*", word));
            }
        } else {
            let words: Vec<&str> = segment.split_whitespace().collect();
            if words.is_empty() {
                continue;
            }
            terms.push(format!("\"{}\"*", words.join(" ")));
        }
    }
    terms.join(" AND ")
}

/// The search row's `snippet`: the latest message before any quoted history,
/// or the provider's snippet when that message has no body text.
pub(super) fn search_preview(
    text: &crate::quoted_history::ThreadSearchText,
    provider_snippet: &str,
) -> String {
    if text.latest_preview.is_empty() {
        provider_snippet.to_string()
    } else {
        text.latest_preview.clone()
    }
}

/// The inbox preview (`threads.snippet`): the latest message before any
/// quoted history, HTML-entity-encoded like the provider snippets the reader
/// decodes for display, or the provider's snippet when that message has no
/// body text. Entity references already in the text (senders' text/plain
/// parts often carry them, like `&#847;` preheader padding) are decoded
/// first, as the reader does for plain-text bodies, so they don't show
/// literally.
pub(super) fn list_preview(
    text: &crate::quoted_history::ThreadSearchText,
    provider_snippet: &str,
) -> String {
    if text.latest_preview.is_empty() {
        // Provider snippets can carry the same leaked stylesheet text.
        return crate::html_plain_text::after_leading_stylesheet(provider_snippet)
            .unwrap_or(provider_snippet)
            .to_string();
    }
    let decoded = decode_entity_references(&text.latest_preview);
    let mut encoded = String::with_capacity(decoded.len());
    for character in decoded.chars() {
        match character {
            '&' => encoded.push_str("&amp;"),
            '<' => encoded.push_str("&lt;"),
            '>' => encoded.push_str("&gt;"),
            '"' => encoded.push_str("&quot;"),
            '\'' => encoded.push_str("&#39;"),
            other => encoded.push(other),
        }
    }
    encoded
}

/// Replaces named and numeric HTML entity references (`&amp;`, `&#847;`,
/// `&#x2007;`) with their characters; an unknown name or a bare `&` stays as
/// written.
fn decode_entity_references(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        rest = &rest[start..];
        let reference_len = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .filter(|&end| end > 0 && rest[1 + end..].starts_with(';'))
            .map(|end| end + 2);
        match reference_len {
            Some(len) => {
                mail_parser::decoders::html::add_html_token(
                    &mut decoded,
                    rest[..len].as_bytes(),
                    false,
                );
                rest = &rest[len..];
            }
            None => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

#[cfg(test)]
#[path = "tests/search.rs"]
mod tests;
