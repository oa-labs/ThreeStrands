//! Device-local attachment browsing. The projection is maintained by schema
//! triggers, so ingestion, retention, and account deletion use one contract.
use super::{messages::resolve_body, Database, DatabaseError, DbResult};
use crate::mime::RawMessage;
use crate::models::{
    ContextQuery, FilePage, FileQuery, MessageAttachment, NoteFileSource, WorkspaceConversation,
    WorkspaceFile,
};
use crate::search_query::like_contains_pattern;
use rusqlite::{functions::FunctionFlags, params, types::Value, Connection, OptionalExtension};
use std::collections::{BTreeSet, HashMap};
use threestrands_sync_protocol::DAILY_NOTE_ID_PREFIX;

const MAX_FILE_PAGE: usize = 100;
/// Calendar invitations, matched the same way as the Files "calendar" type.
const CALENDAR: &str = "(a.mime_type LIKE 'text/calendar%' OR lower(a.filename) LIKE '%.ics')";
const DIRECTION: &str = "CASE WHEN EXISTS(SELECT 1 FROM contact_interactions ci WHERE ci.message_id=m.id AND ci.account_id=t.account_id AND ci.direction='sent') OR lower(trim(m.sender))=lower(t.account_id) OR lower(trim(m.sender)) LIKE '%<'||lower(t.account_id)||'>' THEN 'sent' ELSE 'received' END";

/// SQLite's `LIKE` and `lower()` fold ASCII only, so `fold_case(column) LIKE
/// fold(value)` is how Files and context matching ignore case in any script.
pub(super) fn register_functions(connection: &Connection) -> rusqlite::Result<()> {
    connection.create_scalar_function(
        "fold_case",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            Ok(context
                .get::<Option<String>>(0)?
                .map(|value| value.to_lowercase()))
        },
    )
}

fn folded_pattern(value: &str) -> String {
    like_contains_pattern(&value.to_lowercase())
}

/// A note's title as Files shows it; today's note before it is first saved
/// has no row yet, and is named by its date.
const NOTE_TITLE: &str = "COALESCE(n.title, substr(a.note_id, 7))";

/// The SQL condition for a Files type, over `a.mime_type` and `a.filename`.
fn type_condition(kind: &str) -> DbResult<&'static str> {
    Ok(match kind {
        "image" => "a.mime_type LIKE 'image/%'",
        "audio" => "a.mime_type LIKE 'audio/%'",
        "video" => "a.mime_type LIKE 'video/%'",
        "calendar" => CALENDAR,
        "spreadsheet" => "a.mime_type LIKE '%spreadsheet%' OR a.mime_type LIKE '%excel%' OR lower(a.filename) GLOB '*.xls*' OR lower(a.filename) LIKE '%.csv' OR lower(a.filename) LIKE '%.ods'",
        "presentation" => "a.mime_type LIKE '%presentation%' OR a.mime_type LIKE '%powerpoint%' OR lower(a.filename) GLOB '*.ppt*' OR lower(a.filename) LIKE '%.odp'",
        "document" => "a.mime_type='application/pdf' OR a.mime_type LIKE '%word%' OR a.mime_type='text/plain' OR lower(a.filename) LIKE '%.odt' OR lower(a.filename) LIKE '%.rtf'",
        "archive" => "a.mime_type LIKE '%zip%' OR a.mime_type LIKE '%compressed%' OR lower(a.filename) LIKE '%.tar' OR lower(a.filename) LIKE '%.gz'",
        _ => return Err(DatabaseError::invalid("Invalid file type")),
    })
}

/// Whether a query lists notes' files: asked for, and not narrowed by
/// anything only mail has (a conversation, sender, direction, the
/// calendar type, or a message to put first).
fn notes_included(query: &FileQuery) -> bool {
    query.include_notes == Some(true)
        && query.thread_id.is_none()
        && query.thread_ids.is_none()
        && query.priority_message_id.is_none()
        && query.sender.as_deref().unwrap_or_default().is_empty()
        && query.direction.is_none()
        && query.r#type.as_deref() != Some("calendar")
}

/// When a file was sent or added, for ordering mail and note files
/// together; unreadable times sort last.
fn instant(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value).map_or(i64::MIN, |time| time.timestamp_millis())
}

impl Database {
    /// Files for Files and the context rail: mail attachments, and with
    /// `include_notes` the files attached to notes outside Trash, newest
    /// first together.
    pub fn list_files(&self, query: &FileQuery) -> DbResult<FilePage> {
        let limit = query.limit.unwrap_or(50);
        if limit == 0 || limit > MAX_FILE_PAGE || query.account_ids.len() > 100 {
            return Err(DatabaseError::invalid("Invalid file query limits"));
        }
        let offset = query.offset.unwrap_or(0);
        if !notes_included(query) {
            let mut page = self.mail_files(query, offset, limit)?;
            self.fill_mail_availability(&mut page.files)?;
            return Ok(page);
        }
        // Both lists are read up to the end of this page and merged; the
        // page is the window of the merged order.
        let window = offset.saturating_add(limit);
        let mut page = self.mail_files(query, 0, window)?;
        let (note_total, notes) = self.note_files(query, window)?;
        page.files.extend(notes);
        // Stable, so files added at the same instant keep their own order.
        page.files
            .sort_by_key(|file| std::cmp::Reverse(instant(&file.sent_at)));
        page.files = page.files.into_iter().skip(offset).take(limit).collect();
        page.total += note_total;
        self.fill_mail_availability(&mut page.files)?;
        Ok(page)
    }

    fn mail_files(&self, query: &FileQuery, offset: usize, limit: usize) -> DbResult<FilePage> {
        if query.account_ids.is_empty() {
            return Ok(FilePage {
                files: vec![],
                total: 0,
                hidden_calendar: 0,
            });
        }
        let mut filters = vec![
            format!(
                "t.account_id IN ({})",
                vec!["?"; query.account_ids.len()].join(",")
            ),
            "t.trashed=0".into(),
            "NOT EXISTS (SELECT 1 FROM json_each(t.labels_json) WHERE upper(value)='SPAM')".into(),
        ];
        let mut values: Vec<Value> = query.account_ids.iter().cloned().map(Value::Text).collect();
        if let Some(ids) = &query.thread_ids {
            if ids.is_empty() {
                return Ok(FilePage {
                    files: vec![],
                    total: 0,
                    hidden_calendar: 0,
                });
            }
            if ids.len() > 20 {
                return Err(DatabaseError::invalid("Too many file threads"));
            }
            filters.push(format!("t.id IN ({})", vec!["?"; ids.len()].join(",")));
            values.extend(ids.iter().cloned().map(Value::Text));
        }
        // Invitations are listed by meeting title, so search matches that title too.
        // People often remember who sent a file rather than its name, so the sender matches as well.
        if let Some(search) = query.search.as_ref().filter(|value| !value.is_empty()) {
            filters.push(format!("(fold_case(a.filename) LIKE ? ESCAPE '\\' OR fold_case(m.sender) LIKE ? ESCAPE '\\' OR ({CALENDAR} AND fold_case(t.subject) LIKE ? ESCAPE '\\'))"));
            values.extend(std::iter::repeat_n(Value::Text(folded_pattern(search)), 3));
        }
        for (column, value, like) in [
            ("t.id", &query.thread_id, false),
            ("m.sender", &query.sender, true),
        ] {
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                filters.push(if like {
                    format!("fold_case({column}) LIKE ? ESCAPE '\\'")
                } else {
                    format!("{column}=?")
                });
                values.push(Value::Text(if like {
                    folded_pattern(value)
                } else {
                    value.clone()
                }));
            }
        }
        for (operator, value) in [(">=", &query.after), ("<", &query.before)] {
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                if chrono::DateTime::parse_from_rfc3339(value).is_err() {
                    return Err(DatabaseError::invalid("Invalid file date"));
                }
                filters.push(format!("julianday(m.sent_at){operator}julianday(?)"));
                values.push(Value::Text(value.clone()));
            }
        }
        if let Some(direction) = &query.direction {
            if direction != "received" && direction != "sent" {
                return Err(DatabaseError::invalid("Invalid file direction"));
            }
            filters.push(format!("({DIRECTION})=?"));
            values.push(Value::Text(direction.clone()));
        }
        if query.starred == Some(true) {
            filters.push("s.message_id IS NOT NULL".into());
        }
        if let Some(kind) = query.r#type.as_ref().filter(|kind| !kind.is_empty()) {
            filters.push(format!("({})", type_condition(kind)?));
        }
        let joins = "FROM file_attachments a JOIN messages m ON m.id=a.message_id JOIN threads t ON t.id=m.thread_id LEFT JOIN file_stars s ON s.message_id=a.message_id AND s.attachment_id=a.attachment_id";
        let matched = filters.join(" AND ");
        let exclude_calendar = query.exclude_calendar == Some(true);
        let from = if exclude_calendar {
            format!("{joins} WHERE {matched} AND NOT {CALENDAR}")
        } else {
            format!("{joins} WHERE {matched}")
        };
        self.with_connection(|connection| {
            let total = connection.query_row(&format!("SELECT COUNT(*) {from}"), rusqlite::params_from_iter(&values), |row| row.get::<_,i64>(0))? as usize;
            let hidden_calendar = if exclude_calendar { connection.query_row(&format!("SELECT COUNT(*) {joins} WHERE {matched} AND {CALENDAR}"), rusqlite::params_from_iter(&values), |row| row.get::<_,i64>(0))? as usize } else { 0 };
            let priority = if let Some(id)=&query.priority_message_id { values.push(Value::Text(id.clone())); "CASE WHEN m.id=? THEN 0 ELSE 1 END," } else { "" };
            let importance = if query.prioritize_starred==Some(true) { "s.message_id IS NULL," } else { "" };
            let sql = format!("SELECT t.account_id,m.id,t.id,t.subject,m.sent_at,m.sender,({DIRECTION}),a.attachment_id,a.filename,a.mime_type,a.size,s.message_id IS NOT NULL {from} ORDER BY {priority}{importance}m.sent_at DESC,m.id,a.attachment_id LIMIT ? OFFSET ?");
            values.push(Value::Integer(limit.min(i64::MAX as usize) as i64)); values.push(Value::Integer(offset.min(i64::MAX as usize) as i64));
            let mut statement = connection.prepare(&sql)?;
            let files = statement.query_map(rusqlite::params_from_iter(&values), |row| Ok(WorkspaceFile {
                account_id: row.get(0)?, message_id: row.get(1)?, thread_id: row.get(2)?, subject: row.get(3)?, sent_at: row.get(4)?, sender: row.get(5)?, direction: row.get(6)?,
                attachment: MessageAttachment { id: row.get(7)?, filename: row.get(8)?, mime_type: row.get(9)?, size: row.get::<_, i64>(10)?.max(0) as u64, content_id: None, inline: false },
                starred: row.get(11)?, availability: "unknown".into(), note: None,
            }))?.collect::<Result<Vec<_>, _>>()?;
            Ok(FilePage { files, total, hidden_calendar })
        })
    }

    /// Marks each listed mail file `offline` or `download`, from locally
    /// cached MIME metadata only; bytes are never downloaded or decoded.
    /// Missing metadata stays `unknown` rather than promising availability.
    fn fill_mail_availability(&self, files: &mut [WorkspaceFile]) -> DbResult<()> {
        // One read per source message, however many of its files are listed.
        let ids: BTreeSet<String> = files
            .iter()
            .filter(|file| file.note.is_none())
            .map(|file| file.message_id.clone())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let payloads = self.with_connection(|connection| {
            let mut payloads = Vec::with_capacity(ids.len());
            let mut statement = connection.prepare(&format!(
                "SELECT id,payload,payload_z FROM message_metadata WHERE id IN ({})",
                vec!["?"; ids.len()].join(",")
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(&ids), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })?;
            for row in rows {
                payloads.push(row?);
            }
            Ok(payloads)
        })?;
        // Payloads are decoded after the connection is released.
        let messages: HashMap<String, RawMessage> = payloads
            .into_iter()
            .filter_map(|(id, legacy, compressed)| {
                let payload = resolve_body(1, legacy, compressed).ok()?;
                Some((id, serde_json::from_str(&payload).ok()?))
            })
            .collect();
        for file in files.iter_mut().filter(|file| file.note.is_none()) {
            if let Some(message) = messages.get(&file.message_id) {
                if let Ok(local) =
                    crate::mime::attachment_payload_available(message, &file.attachment.id)
                {
                    file.availability = if local { "offline" } else { "download" }.into();
                }
            }
        }
        Ok(())
    }

    /// Files attached to notes outside Trash that match `query`, newest
    /// first, up to `limit`, and how many match in all. A file in today's
    /// note before the note is first saved is listed under the day's date.
    fn note_files(&self, query: &FileQuery, limit: usize) -> DbResult<(usize, Vec<WorkspaceFile>)> {
        let mut filters = vec![
            "COALESCE(n.trashed, 0)=0".to_string(),
            format!("(n.id IS NOT NULL OR a.note_id LIKE '{DAILY_NOTE_ID_PREFIX}%')"),
        ];
        let mut values: Vec<Value> = Vec::new();
        if let Some(search) = query.search.as_ref().filter(|value| !value.is_empty()) {
            filters.push(format!("(fold_case(a.filename) LIKE ? ESCAPE '\\' OR fold_case({NOTE_TITLE}) LIKE ? ESCAPE '\\')"));
            values.extend(std::iter::repeat_n(Value::Text(folded_pattern(search)), 2));
        }
        for (operator, value) in [(">=", &query.after), ("<", &query.before)] {
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                if chrono::DateTime::parse_from_rfc3339(value).is_err() {
                    return Err(DatabaseError::invalid("Invalid file date"));
                }
                filters.push(format!("julianday(a.created_at){operator}julianday(?)"));
                values.push(Value::Text(value.clone()));
            }
        }
        if query.starred == Some(true) {
            filters.push("a.starred=1".into());
        }
        if let Some(kind) = query.r#type.as_ref().filter(|kind| !kind.is_empty()) {
            filters.push(format!("({})", type_condition(kind)?));
        }
        let from = format!(
            "FROM (SELECT *, name AS filename FROM note_attachments) a LEFT JOIN notes n ON n.id=a.note_id WHERE {}",
            filters.join(" AND ")
        );
        self.with_connection(|connection| {
            let total = connection.query_row(&format!("SELECT COUNT(*) {from}"), rusqlite::params_from_iter(&values), |row| row.get::<_, i64>(0))? as usize;
            values.push(Value::Integer(limit.min(i64::MAX as usize) as i64));
            let mut statement = connection.prepare(&format!(
                "SELECT a.id, a.filename, a.mime_type, a.size, a.created_at, a.starred, a.note_id, {NOTE_TITLE},
                        CASE WHEN a.note_id LIKE '{DAILY_NOTE_ID_PREFIX}%' THEN substr(a.note_id, {}) END,
                        EXISTS(SELECT 1 FROM note_attachment_data d WHERE d.id=a.id)
                 {from} ORDER BY julianday(a.created_at) DESC, a.id LIMIT ?",
                DAILY_NOTE_ID_PREFIX.len() + 1
            ))?;
            let files = statement
                .query_map(rusqlite::params_from_iter(&values), |row| {
                    let title: String = row.get(7)?;
                    Ok(WorkspaceFile {
                        account_id: String::new(),
                        message_id: String::new(),
                        thread_id: String::new(),
                        subject: title.clone(),
                        sent_at: row.get(4)?,
                        sender: String::new(),
                        direction: "received".into(),
                        attachment: MessageAttachment {
                            id: row.get(0)?,
                            filename: row.get(1)?,
                            mime_type: row.get(2)?,
                            size: row.get::<_, i64>(3)?.max(0) as u64,
                            content_id: None,
                            inline: false,
                        },
                        starred: row.get(5)?,
                        availability: if row.get::<_, bool>(9)? { "offline" } else { "syncing" }.into(),
                        note: Some(NoteFileSource { id: row.get(6)?, title, daily_date: row.get(8)? }),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((total, files))
        })
    }

    /// Stars or unstars a note's file in Files, on this device.
    pub fn set_note_attachment_star(&self, id: &str, starred: bool) -> DbResult<()> {
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE note_attachments SET starred=?2 WHERE id=?1",
                params![id, starred],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Attachment"));
            }
            Ok(())
        })
    }

    pub fn context_candidates(&self, query: &ContextQuery) -> DbResult<Vec<WorkspaceConversation>> {
        if query.account_ids.is_empty() {
            return Ok(vec![]);
        }
        if query.account_ids.len() > 100 || query.terms.len() > 12 || query.attendees.len() > 100 {
            return Err(DatabaseError::invalid("Invalid context limits"));
        }
        let mut values: Vec<Value> = query.account_ids.iter().cloned().map(Value::Text).collect();
        // Mirrors rankContext: a conversation other than the source needs a
        // subject topic, and for a meeting an attendee too. Requiring both here
        // keeps evidence rankContext would discard from filling the limit.
        let mut evidence = vec![];
        if let Some(id) = &query.thread_id {
            evidence.push("t.id=?".to_string());
            values.push(Value::Text(id.clone()));
        }
        if !query.terms.is_empty() {
            let mut related = format!(
                "({})",
                vec!["fold_case(t.subject) LIKE ? ESCAPE '\\'"; query.terms.len()].join(" OR ")
            );
            values.extend(
                query
                    .terms
                    .iter()
                    .map(|term| Value::Text(folded_pattern(term))),
            );
            if !query.attendees.is_empty() {
                related.push_str(&format!(" AND EXISTS(SELECT 1 FROM contact_interactions ci WHERE ci.thread_id=t.id AND lower(ci.email) IN ({}))", vec!["?"; query.attendees.len()].join(",")));
                values.extend(
                    query
                        .attendees
                        .iter()
                        .map(|email| Value::Text(email.to_ascii_lowercase())),
                );
            }
            evidence.push(related);
        }
        if evidence.is_empty() {
            return Ok(vec![]);
        }
        let mut filter=format!("t.account_id IN ({}) AND t.trashed=0 AND NOT EXISTS(SELECT 1 FROM json_each(t.labels_json) WHERE upper(value)='SPAM') AND ({})",vec!["?";query.account_ids.len()].join(","),evidence.join(" OR "));
        if query.thread_id.is_none() {
            for (operator, value) in [(">=", &query.after), ("<", &query.before)] {
                if let Some(value) = value {
                    if chrono::DateTime::parse_from_rfc3339(value).is_err() {
                        return Err(DatabaseError::invalid("Invalid context date"));
                    }
                    filter.push_str(&format!(
                        " AND julianday(t.last_message_at){operator}julianday(?)"
                    ));
                    values.push(Value::Text(value.clone()));
                }
            }
        }
        let ordering = if let Some(id) = &query.thread_id {
            values.push(Value::Text(id.clone()));
            "CASE WHEN t.id=? THEN 0 ELSE 1 END,"
        } else {
            ""
        };
        self.with_connection(|c| {
            let mut statement=c.prepare(&format!("SELECT t.id,t.account_id,t.subject,t.last_message_at,(SELECT json_group_array(person) FROM (SELECT value AS person FROM json_each(t.participants_json) UNION SELECT email AS person FROM contact_interactions WHERE thread_id=t.id)) FROM threads t WHERE {filter} ORDER BY {ordering} t.last_message_at DESC,t.id LIMIT 120"))?;
            let result=statement.query_map(rusqlite::params_from_iter(&values),|row| Ok(WorkspaceConversation { thread_id:row.get(0)?,account_id:row.get(1)?,subject:row.get(2)?,sent_at:row.get(3)?,participants:serde_json::from_str(&row.get::<_,String>(4)?).unwrap_or_default() }))?.collect::<Result<Vec<_>,_>>()?;
            Ok(result)
        })
    }

    pub fn file_stars(&self, message_ids: &[String]) -> DbResult<Vec<(String, String)>> {
        if message_ids.is_empty() {
            return Ok(vec![]);
        }
        if message_ids.len() > 100 {
            return Err(DatabaseError::invalid("Too many star sources"));
        }
        self.with_connection(|c| {
            let mut statement = c.prepare(&format!(
                "SELECT message_id,attachment_id FROM file_stars WHERE message_id IN ({})",
                vec!["?"; message_ids.len()].join(",")
            ))?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(message_ids), |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// The account that owns a stored message, for star requests that name none.
    pub fn message_account(&self, message_id: &str) -> DbResult<String> {
        self.with_connection(|c| {
            c.query_row(
                "SELECT t.account_id FROM messages m JOIN threads t ON t.id=m.thread_id WHERE m.id=?1",
                [message_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(DatabaseError::NotFound("Message"))
        })
    }

    pub fn set_file_star(
        &self,
        account_id: &str,
        message_id: &str,
        attachment_id: &str,
        starred: bool,
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM file_attachments a JOIN messages m ON m.id=a.message_id JOIN threads t ON t.id=m.thread_id WHERE t.account_id=?1 AND m.id=?2 AND a.attachment_id=?3)", params![account_id,message_id,attachment_id], |row| row.get(0))?;
            if !exists { return Err(DatabaseError::NotFound("Attachment")); }
            if starred { tx.execute("INSERT OR IGNORE INTO file_stars VALUES (?1,?2)", params![message_id,attachment_id])?; }
            else { tx.execute("DELETE FROM file_stars WHERE message_id=?1 AND attachment_id=?2", params![message_id,attachment_id])?; }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{database, message};
    fn query(account: &str) -> FileQuery {
        FileQuery {
            account_ids: vec![account.into()],
            ..Default::default()
        }
    }
    fn seed(db: &Database, account: &str, id: &str) {
        let mut mail = message(id, id, "2026-10-01T10:00:00Z", "budget");
        mail.attachments = vec![MessageAttachment {
            id: "part:0.1".into(),
            filename: "budget.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 100,
            content_id: None,
            inline: false,
        }];
        db.upsert_thread(account, &[mail]).unwrap();
    }
    #[test]
    fn files_scope_occurrences_paginate_and_preserve_stars_on_refresh() {
        let db = database();
        seed(&db, "you@example.com", "first");
        seed(&db, "other@example.com", "second");
        seed(&db, "you@example.com", "third");
        let mut q = query("you@example.com");
        q.limit = Some(1);
        let first = db.list_files(&q).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.files.len(), 1);
        q.offset = Some(1);
        assert_ne!(
            first.files[0].message_id,
            db.list_files(&q).unwrap().files[0].message_id
        );
        db.set_file_star("you@example.com", "first", "part:0.1", true)
            .unwrap();
        assert!(db
            .set_file_star("other@example.com", "first", "part:0.1", true)
            .is_err());
        seed(&db, "you@example.com", "first");
        q.offset = Some(0);
        q.starred = Some(true);
        assert_eq!(db.list_files(&q).unwrap().files[0].message_id, "first");
        db.with_connection(|c| {
            c.execute(
                "UPDATE threads SET archived=1 WHERE provider_thread_id='first'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.list_files(&q).unwrap().total, 1);
        db.with_connection(|c| {
            c.execute("DELETE FROM messages WHERE id='first'", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.list_files(&q).unwrap().total, 0);
    }
    fn seed_invitation(
        db: &Database,
        account: &str,
        id: &str,
        subject: &str,
        filename: &str,
        mime_type: &str,
    ) {
        let mut mail = message(id, id, "2026-10-02T10:00:00Z", "body");
        mail.subject = subject.into();
        mail.attachments = vec![MessageAttachment {
            id: "part:0.2".into(),
            filename: filename.into(),
            mime_type: mime_type.into(),
            size: 900,
            content_id: None,
            inline: false,
        }];
        db.upsert_thread(account, &[mail]).unwrap();
    }
    #[test]
    fn files_can_hide_calendar_invitations_and_count_them() {
        let db = database();
        seed(&db, "you@example.com", "report");
        seed_invitation(
            &db,
            "you@example.com",
            "invite",
            "Invitation: Weekly sync @ Tue Oct 14",
            "invite.ics",
            "application/octet-stream",
        );
        seed_invitation(
            &db,
            "you@example.com",
            "meeting",
            "Quarterly planning",
            "meeting",
            "text/calendar; method=REQUEST",
        );
        let mut q = query("you@example.com");
        let all = db.list_files(&q).unwrap();
        assert_eq!((all.total, all.hidden_calendar), (3, 0));
        q.exclude_calendar = Some(true);
        let page = db.list_files(&q).unwrap();
        assert_eq!((page.total, page.hidden_calendar), (1, 2));
        assert_eq!(page.files[0].attachment.filename, "budget.pdf");
        // The hidden count follows the other filters.
        q.search = Some("budget".into());
        assert_eq!(db.list_files(&q).unwrap().hidden_calendar, 0);
    }
    #[test]
    fn files_search_matches_invitation_titles_but_not_other_subjects() {
        let db = database();
        // A document's subject is not its title, so only its filename is searched.
        seed_invitation(
            &db,
            "you@example.com",
            "notes",
            "Weekly sync notes",
            "notes.pdf",
            "application/pdf",
        );
        seed_invitation(
            &db,
            "you@example.com",
            "invite",
            "Invitation: Weekly SYNC @ Tue Oct 14",
            "invite.ics",
            "text/calendar",
        );
        let mut q = query("you@example.com");
        q.search = Some("weekly sync".into());
        let page = db.list_files(&q).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.files[0].message_id, "invite");
        q.search = Some("notes.pdf".into());
        assert_eq!(db.list_files(&q).unwrap().files[0].message_id, "notes");
        q.search = Some("%".into());
        assert_eq!(db.list_files(&q).unwrap().total, 0);
    }
    #[test]
    fn files_exclude_inline_trash_spam_and_literal_search_is_safe() {
        let db = database();
        seed(&db, "you@example.com", "file");
        let mut q = query("you@example.com");
        q.search = Some("%".into());
        assert_eq!(db.list_files(&q).unwrap().total, 0);
        q.search = None;
        q.r#type = Some("document".into());
        assert_eq!(db.list_files(&q).unwrap().total, 1);
        db.with_connection(|c| {
            c.execute(
                "UPDATE threads SET labels_json='[\"SPAM\"]' WHERE provider_thread_id='file'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.list_files(&q).unwrap().total, 0);
        db.with_connection(|c| {
            c.execute(
                "UPDATE threads SET labels_json='[]',trashed=1 WHERE provider_thread_id='file'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.list_files(&q).unwrap().total, 0);
        db.with_connection(|c| { c.execute("UPDATE threads SET trashed=0 WHERE provider_thread_id='file'",[])?; c.execute("UPDATE messages SET attachments_json='[{\"id\":\"inline\",\"filename\":\"logo.png\",\"inline\":true}]' WHERE id='file'",[])?; Ok(()) }).unwrap();
        assert_eq!(db.list_files(&q).unwrap().total, 0);
        assert_eq!(db.list_files(&FileQuery::default()).unwrap().total, 0);
    }
    #[test]
    fn stars_survive_reopening_and_removed_parts_and_threads_remove_annotations() {
        let path = crate::db::test_support::TempDbPath::new();
        {
            let db = Database::open(&path.path).unwrap();
            seed(&db, "you@example.com", "saved");
            db.set_file_star("you@example.com", "saved", "part:0.1", true)
                .unwrap();
        }
        let db = Database::open(&path.path).unwrap();
        assert_eq!(
            db.file_stars(&["saved".into()]).unwrap(),
            vec![("saved".into(), "part:0.1".into())]
        );
        db.with_connection(|c| {
            c.execute(
                "UPDATE messages SET attachments_json='[]' WHERE id='saved'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db.file_stars(&["saved".into()]).unwrap().is_empty());
        seed(&db, "you@example.com", "saved");
        db.set_file_star("you@example.com", "saved", "part:0.1", true)
            .unwrap();
        db.with_connection(|c| {
            c.execute("DELETE FROM threads WHERE provider_thread_id='saved'", [])?;
            Ok(())
        })
        .unwrap();
        assert!(db.file_stars(&["saved".into()]).unwrap().is_empty());
        assert_eq!(db.list_files(&query("you@example.com")).unwrap().total, 0);
    }
    #[test]
    fn files_test_exact_date_boundaries_and_query_limits() {
        let db = database();
        seed(&db, "you@example.com", "file");
        let mut q = query("you@example.com");
        q.after = Some("2026-10-01T10:00:00Z".into());
        assert_eq!(db.list_files(&q).unwrap().total, 1);
        q.before = q.after.clone();
        assert_eq!(db.list_files(&q).unwrap().total, 0);
        q.before = Some("2026-10-01T10:00:01Z".into());
        assert_eq!(db.list_files(&q).unwrap().total, 1);
        for limit in [99, 100] {
            q.limit = Some(limit);
            assert!(db.list_files(&q).is_ok());
        }
        q.limit = Some(101);
        assert!(db.list_files(&q).is_err());
        q.limit = Some(0);
        assert!(db.list_files(&q).is_err());
    }
    #[test]
    fn source_and_star_priority_apply_before_the_file_page_limit() {
        let db = database();
        seed(&db, "you@example.com", "a");
        seed(&db, "you@example.com", "z");
        db.set_file_star("you@example.com", "a", "part:0.1", true)
            .unwrap();
        let mut q = query("you@example.com");
        q.limit = Some(1);
        q.prioritize_starred = Some(true);
        q.priority_message_id = Some("z".into());
        assert_eq!(db.list_files(&q).unwrap().files[0].message_id, "z");
        q.priority_message_id = None;
        assert_eq!(db.list_files(&q).unwrap().files[0].message_id, "a");
    }
    #[test]
    fn context_candidates_are_scoped_and_include_outgoing_recipients() {
        let db = database();
        for (id, account) in [
            ("work", "you@example.com"),
            ("private", "private@example.com"),
        ] {
            let mut mail = message(id, id, "2026-10-01T10:00:00Z", "Atlas forecast");
            mail.subject = "Atlas budget".into();
            mail.from = account.into();
            mail.to = vec!["Alice <alice@example.com>".into()];
            db.upsert_thread(account, &[mail]).unwrap();
        }
        let q = ContextQuery {
            account_ids: vec!["you@example.com".into()],
            thread_id: None,
            terms: vec!["Atlas".into()],
            attendees: vec!["alice@example.com".into()],
            after: Some("2026-09-01T00:00:00Z".into()),
            before: None,
        };
        let result = db.context_candidates(&q).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].account_id, "you@example.com");
        assert!(result[0].participants.contains(&"alice@example.com".into()));
        assert!(db
            .context_candidates(&ContextQuery {
                account_ids: vec![],
                ..q
            })
            .unwrap()
            .is_empty());
    }
    #[test]
    fn availability_reads_each_source_message_once_and_reports_each_part() {
        let db = database();
        let mut mail = message("both", "both", "2026-10-01T10:00:00Z", "budget");
        mail.metadata_json = r#"{"id":"both","threadId":"both","payload":{"mimeType":"multipart/mixed","parts":[{"filename":"local.pdf","body":{"data":"AA"}},{"filename":"remote.pdf","body":{"attachmentId":"provider-1"}}]}}"#.into();
        mail.attachments = ["part:0.0", "part:0.1", "part:0.9"]
            .into_iter()
            .map(|id| MessageAttachment {
                id: id.into(),
                filename: format!("{id}.pdf"),
                mime_type: "application/pdf".into(),
                size: 1,
                content_id: None,
                inline: false,
            })
            .collect();
        db.upsert_thread("you@example.com", &[mail]).unwrap();
        seed(&db, "you@example.com", "unparsed");
        let mut availability: Vec<(String, String)> = db
            .list_files(&query("you@example.com"))
            .unwrap()
            .files
            .into_iter()
            .map(|file| {
                (
                    format!("{}/{}", file.message_id, file.attachment.id),
                    file.availability,
                )
            })
            .collect();
        availability.sort();
        assert_eq!(
            availability,
            [
                ("both/part:0.0", "offline"),
                ("both/part:0.1", "download"),
                ("both/part:0.9", "unknown"),
                ("unparsed/part:0.1", "unknown"),
            ]
            .map(|(file, state)| (file.to_string(), state.to_string()))
        );
    }
    #[test]
    fn file_search_folds_case_beyond_ascii() {
        let db = database();
        let mut mail = message("report", "report", "2026-10-01T10:00:00Z", "budget");
        mail.from = "Ågot Ødegård <agot@example.com>".into();
        mail.attachments = vec![MessageAttachment {
            id: "part:0.1".into(),
            filename: "Überprüfung.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 1,
            content_id: None,
            inline: false,
        }];
        db.upsert_thread("you@example.com", &[mail]).unwrap();
        let mut q = query("you@example.com");
        for search in ["überprüfung", "ÜBERPRÜFUNG", "Überprüfung"] {
            q.search = Some(search.into());
            assert_eq!(db.list_files(&q).unwrap().total, 1, "{search}");
        }
        q.search = Some("prüfung_".into());
        assert_eq!(db.list_files(&q).unwrap().total, 0);
        // Search also finds the file by who sent it, by name or address.
        for search in ["ØDEGÅRD", "agot@example"] {
            q.search = Some(search.into());
            assert_eq!(db.list_files(&q).unwrap().total, 1, "{search}");
        }
        q.search = None;
        q.sender = Some("øDEGÅRD".into());
        assert_eq!(db.list_files(&q).unwrap().total, 1);
    }
    #[test]
    fn context_topics_fold_case_beyond_ascii() {
        let db = database();
        let mut mail = message("review", "review", "2026-10-01T10:00:00Z", "notes");
        mail.subject = "Überprüfung Q3".into();
        db.upsert_thread("you@example.com", &[mail]).unwrap();
        let result = db
            .context_candidates(&ContextQuery {
                account_ids: vec!["you@example.com".into()],
                thread_id: None,
                terms: vec!["überprüfung".into()],
                attendees: vec![],
                after: None,
                before: None,
            })
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].subject, "Überprüfung Q3");
    }
    #[test]
    fn attendee_only_threads_cannot_crowd_topic_matches_out_of_the_candidate_limit() {
        let db = database();
        let mut older = message("atlas", "atlas", "2026-09-01T10:00:00Z", "numbers");
        older.subject = "Atlas budget".into();
        older.from = "Alice <alice@example.com>".into();
        db.upsert_thread("you@example.com", &[older]).unwrap();
        for index in 0..150 {
            let id = format!("chat-{index}");
            let mut mail = message(
                &id,
                &id,
                &format!("2026-10-01T10:{:02}:{:02}Z", index / 60, index % 60),
                "hello",
            );
            mail.subject = "Lunch".into();
            mail.from = "Alice <alice@example.com>".into();
            db.upsert_thread("you@example.com", &[mail]).unwrap();
        }
        let mut snippet_only = message(
            "snippet",
            "snippet",
            "2026-10-02T10:00:00Z",
            "atlas mentioned in passing",
        );
        snippet_only.from = "Alice <alice@example.com>".into();
        db.upsert_thread("you@example.com", &[snippet_only])
            .unwrap();
        let result = db
            .context_candidates(&ContextQuery {
                account_ids: vec!["you@example.com".into()],
                thread_id: None,
                terms: vec!["atlas".into()],
                attendees: vec!["alice@example.com".into()],
                after: Some("2026-08-01T00:00:00Z".into()),
                before: None,
            })
            .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|item| item.subject.as_str())
                .collect::<Vec<_>>(),
            ["Atlas budget"]
        );
    }
    #[test]
    fn star_requests_without_an_account_resolve_the_owning_account() {
        let db = database();
        seed(&db, "you@example.com", "mine");
        assert_eq!(db.message_account("mine").unwrap(), "you@example.com");
        assert!(matches!(
            db.message_account("missing"),
            Err(DatabaseError::NotFound(_))
        ));
    }
    #[test]
    fn thread_merges_preserve_stars_and_cross_account_message_replacement_is_rejected() {
        let db = database();
        seed(&db, "you@example.com", "moving");
        db.set_file_star("you@example.com", "moving", "part:0.1", true)
            .unwrap();
        let mut moved = message("moving", "survivor", "2026-10-01T10:00:00Z", "budget");
        moved.attachments = db
            .list_files(&query("you@example.com"))
            .unwrap()
            .files
            .into_iter()
            .map(|file| file.attachment)
            .collect();
        db.apply_ingested_threads_with_aliases(
            "you@example.com",
            &[("survivor".into(), vec![moved.clone()], vec![])],
            &[("moving".into(), "survivor".into())],
        )
        .unwrap();
        assert_eq!(db.file_stars(&["moving".into()]).unwrap().len(), 1);
        assert!(db
            .upsert_thread("other@example.com", &[moved.clone()])
            .is_err());
        let mut conflicting = moved;
        conflicting.thread_id = "another-thread".into();
        assert!(db.upsert_thread("you@example.com", &[conflicting]).is_err());
        assert_eq!(db.list_files(&query("you@example.com")).unwrap().total, 1);
        assert_eq!(db.list_files(&query("other@example.com")).unwrap().total, 0);
    }

    /// A file attached to `note_id`, dated `added_at`.
    fn note_file(db: &Database, note_id: &str, name: &str, bytes: &[u8], added_at: &str) -> String {
        let file = db.add_note_attachment(note_id, name, "", bytes).unwrap();
        db.with_connection(|c| {
            c.execute(
                "UPDATE note_attachments SET created_at=?2 WHERE id=?1",
                params![file.id, added_at],
            )?;
            Ok(())
        })
        .unwrap();
        file.id
    }

    fn names(page: &FilePage) -> Vec<String> {
        page.files
            .iter()
            .map(|file| file.attachment.filename.clone())
            .collect()
    }

    #[test]
    fn files_list_note_files_with_mail_newest_first_and_page_across_both() {
        let db = database();
        seed(&db, "you@example.com", "mail"); // 2026-10-01T10:00:00Z
        let plan = db.create_note("Floor plans", "").unwrap();
        note_file(
            &db,
            &plan.id,
            "newer.png",
            b"\x89PNG\r\n\x1a\nx",
            "2026-10-03T10:00:00Z",
        );
        note_file(&db, &plan.id, "older.txt", b"text", "2026-09-01T10:00:00Z");
        let today = note_file(
            &db,
            "daily:2026-10-02",
            "today.txt",
            b"day",
            "2026-10-02T10:00:00Z",
        );

        let mut q = query("you@example.com");
        assert_eq!(db.list_files(&q).unwrap().total, 1, "only when asked for");
        q.include_notes = Some(true);
        let page = db.list_files(&q).unwrap();
        assert_eq!(page.total, 4);
        assert_eq!(
            names(&page),
            ["newer.png", "today.txt", "budget.pdf", "older.txt"]
        );
        let newer = &page.files[0];
        let source = newer.note.as_ref().unwrap();
        assert_eq!(
            (source.id.as_str(), source.title.as_str()),
            (plan.id.as_str(), "Floor plans")
        );
        assert_eq!(
            (newer.availability.as_str(), newer.message_id.as_str()),
            ("offline", "")
        );
        assert_eq!(
            page.files[1].note.as_ref().unwrap().daily_date.as_deref(),
            Some("2026-10-02")
        );
        assert_eq!(page.files[1].attachment.id, today);
        assert!(page.files[2].note.is_none());

        q.limit = Some(2);
        q.offset = Some(1);
        assert_eq!(
            names(&db.list_files(&q).unwrap()),
            ["today.txt", "budget.pdf"]
        );
        q.offset = Some(3);
        assert_eq!(names(&db.list_files(&q).unwrap()), ["older.txt"]);
    }

    #[test]
    fn files_filter_note_files_and_leave_them_out_of_mail_only_filters() {
        let db = database();
        seed(&db, "you@example.com", "mail");
        let plan = db.create_note("Floor plans", "").unwrap();
        let image = note_file(
            &db,
            &plan.id,
            "kitchen.png",
            b"\x89PNG\r\n\x1a\nx",
            "2026-10-03T10:00:00Z",
        );
        note_file(&db, &plan.id, "notes.txt", b"text", "2026-10-04T10:00:00Z");
        let trashed = db.create_note("Old", "").unwrap();
        note_file(&db, &trashed.id, "gone.txt", b"x", "2026-10-05T10:00:00Z");
        db.trash_note(&trashed.id).unwrap();
        let mut q = query("you@example.com");
        q.include_notes = Some(true);
        assert_eq!(
            db.list_files(&q).unwrap().total,
            3,
            "a note in Trash keeps its files out"
        );

        // The note's title finds its files, as a sender finds mail.
        q.search = Some("FLOOR".into());
        assert_eq!(
            names(&db.list_files(&q).unwrap()),
            ["notes.txt", "kitchen.png"]
        );
        q.search = None;
        q.r#type = Some("image".into());
        assert_eq!(names(&db.list_files(&q).unwrap()), ["kitchen.png"]);
        q.r#type = None;
        q.after = Some("2026-10-04T00:00:00Z".into());
        assert_eq!(names(&db.list_files(&q).unwrap()), ["notes.txt"]);
        q.after = None;
        db.set_note_attachment_star(&image, true).unwrap();
        assert!(db.set_note_attachment_star("missing", true).is_err());
        q.starred = Some(true);
        assert_eq!(names(&db.list_files(&q).unwrap()), ["kitchen.png"]);
        q.starred = None;

        for narrow in [
            FileQuery {
                sender: Some("sender".into()),
                ..q.clone()
            },
            FileQuery {
                direction: Some("received".into()),
                ..q.clone()
            },
            FileQuery {
                r#type: Some("calendar".into()),
                ..q.clone()
            },
            FileQuery {
                thread_ids: Some(vec!["t".into()]),
                ..q.clone()
            },
        ] {
            assert!(db
                .list_files(&narrow)
                .unwrap()
                .files
                .iter()
                .all(|file| file.note.is_none()));
        }
        // With no mail account, notes' files still list.
        let mut none = q.clone();
        none.account_ids.clear();
        assert_eq!(db.list_files(&none).unwrap().total, 2);
    }

    #[test]
    fn a_file_another_device_added_lists_as_syncing_until_it_arrives() {
        let db = database();
        let plan = db.create_note("Plan", "").unwrap();
        let id = note_file(&db, &plan.id, "a.txt", b"a", "2026-10-03T10:00:00Z");
        db.with_connection(|c| {
            c.execute("DELETE FROM note_attachment_data WHERE id=?1", [&id])?;
            Ok(())
        })
        .unwrap();
        let mut q = query("you@example.com");
        q.include_notes = Some(true);
        assert_eq!(db.list_files(&q).unwrap().files[0].availability, "syncing");
    }
}
