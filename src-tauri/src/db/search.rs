//! Full-text queries, preview encoding, and bounded search-index repair.

use super::messages::resolve_body;
use super::threads::THREAD_COLUMNS;
use super::{decode_json, Database, DbResult};
use crate::mime::readable_body_text;
use crate::models::{SearchThreadsRequest, Thread};
use rusqlite::{params, OptionalExtension};

impl Database {
    /// `account_id` merges every account when `None` — the unified inbox —
    /// or scopes to just that account when set, same as [`Self::list_threads`].
    pub fn search_threads(
        &self,
        request: &SearchThreadsRequest,
        account_id: Option<&str>,
    ) -> DbResult<Vec<Thread>> {
        if request.query.trim().is_empty() {
            return self.list_threads(account_id);
        }
        self.with_connection(|connection| {
            let limit = request.limit.unwrap_or(50).min(200) as i64;
            let offset = request.offset.unwrap_or(0) as i64;
            let query = fts_query(&request.query);
            if query.trim().is_empty() {
                return Ok(Vec::new());
            }
            // Trashed threads are hidden alongside archived ones by default; the
            // same "include archived" search toggle reveals both, since neither
            // belongs in the everyday inbox view.
            let archived_filter = if request.include_archived.unwrap_or(false) {
                ""
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
            let map_row = |row: &rusqlite::Row<'_>| {
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
            };
            let rows = match account_id {
                Some(id) => statement.query_map(params![query, limit, offset, id], map_row)?,
                None => statement.query_map(params![query, limit, offset], map_row)?,
            };
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Rewrites a bounded batch of threads queued by schema v46 so their
    /// search `body` drops quoted history repeated within the thread, and
    /// both the search `snippet` and the inbox preview (`threads.snippet`)
    /// show the latest message without its quote (see
    /// [`crate::quoted_history::searchable_thread_text`]). Returns the number
    /// of queued threads handled; call repeatedly until it returns 0. A queued
    /// thread that no longer has a search row is simply dequeued.
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
                    let bodies: Vec<String> = transaction
                        .prepare(
                            "SELECT body_text, body_text_z FROM messages
                             WHERE thread_id = ?1 ORDER BY sent_at, id",
                        )?
                        .query_map([thread_id], |row| {
                            Ok(readable_body_text(resolve_body(
                                0,
                                row.get(0)?,
                                row.get(1)?,
                            )?))
                        })?
                        .collect::<Result<_, _>>()?;
                    // The stored preview is still the provider's until this
                    // rewrite; it remains the fallback for a body without text.
                    let provider_snippet: String = transaction.query_row(
                        "SELECT snippet FROM threads WHERE id = ?1",
                        [thread_id],
                        |row| row.get(0),
                    )?;
                    let text = crate::quoted_history::searchable_thread_text(
                        bodies.iter().map(String::as_str),
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
        return provider_snippet.to_string();
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
