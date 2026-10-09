//! Message payload storage, attachments, and unsubscribe attempts.
//! Storage helpers reuse the caller's connection without locking or committing.

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::mime::{RawMessage, UnsubscribeMetadata};
use crate::models::{UnsubscribeMethod, UnsubscribeTarget};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

impl Database {
    pub fn attachment_message(&self, message_id: &str) -> DbResult<(String, RawMessage)> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT t.account_id, mm.payload, mm.payload_z
                     FROM messages m
                     JOIN threads t ON t.id = m.thread_id
                     JOIN message_metadata mm ON mm.id = m.id
                     WHERE m.id = ?1",
                    [message_id],
                    |row| {
                        let account_id: String = row.get(0)?;
                        let payload = resolve_body(1, row.get(1)?, row.get(2)?)?;
                        let message: RawMessage =
                            serde_json::from_str(&payload).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    payload.len(),
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?;
                        Ok((account_id, message))
                    },
                )
                .optional()?
                .ok_or(DatabaseError::NotFound("Attachment source"))
        })
    }

    /// Resolves the unsubscribe URL from locally cached message metadata and
    /// records the attempt before any external side effect occurs. The
    /// webview supplies only the stable message ID, never an arbitrary URL.
    pub fn begin_unsubscribe(&self, message_id: &str) -> DbResult<UnsubscribeTarget> {
        self.with_transaction(|transaction| {
            let (thread_id, metadata_json): (String, Option<String>) = transaction
                .query_row(
                    "SELECT thread_id, unsubscribe_json FROM messages WHERE id = ?1",
                    [message_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or(DatabaseError::NotFound("Message"))?;
            let metadata = metadata_json
                .ok_or_else(|| {
                    DatabaseError::invalid("This message has no unsubscribe option")
                })
                .and_then(|value| {
                    serde_json::from_str::<UnsubscribeMetadata>(&value).map_err(serialization_error)
                })?;
            let (method, url) = if let Some(url) = metadata.one_click_url {
                (UnsubscribeMethod::OneClick, url)
            } else if let Some(url) = metadata.mailto_url {
                (UnsubscribeMethod::Mailto, url)
            } else if let Some(url) = metadata.web_url {
                (UnsubscribeMethod::Web, url)
            } else {
                return Err(DatabaseError::invalid("This message has no usable unsubscribe option"));
            };
            let request_id = Uuid::new_v4().to_string();
            transaction.execute(
                "INSERT INTO unsubscribe_requests(
                        id, message_id, thread_id, method, state, created_at
                     ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params![
                    request_id,
                    message_id,
                    thread_id,
                    unsubscribe_method_name(&method),
                    Utc::now().to_rfc3339(),
                ],
            )?;
            Ok(UnsubscribeTarget {
                request_id,
                method,
                url,
            })
        })
    }

    pub fn finish_unsubscribe(
        &self,
        request_id: &str,
        state: &str,
        http_status: Option<u16>,
        error: Option<&str>,
    ) -> DbResult<()> {
        if !matches!(state, "succeeded" | "opened" | "failed") {
            return Err(DatabaseError::invalid("Invalid unsubscribe request state"));
        }
        let changed = self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE unsubscribe_requests
                 SET state = ?1, http_status = ?2, completed_at = ?3, last_error = ?4
                 WHERE id = ?5 AND state = 'pending'",
                params![
                    state,
                    http_status,
                    Utc::now().to_rfc3339(),
                    error,
                    request_id
                ],
            )?)
        })?;
        if changed == 0 {
            return Err(DatabaseError::invalid("Unsubscribe request was not pending"));
        }
        Ok(())
    }

    pub fn message_ids_for_thread(&self, thread_id: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id FROM messages WHERE thread_id = ?1 ORDER BY sent_at")?;
            let ids = statement
                .query_map([thread_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

    /// The raw provider payload cached for `id`, if any.
    pub(crate) fn message_metadata(&self, id: &str) -> DbResult<Option<String>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT payload, payload_z FROM message_metadata WHERE id = ?1",
                    [id],
                    |row| resolve_body(0, row.get(0)?, row.get(1)?),
                )
                .optional()?)
        })
    }

    pub(crate) fn put_message_metadata(&self, id: &str, payload: &str) -> DbResult<()> {
        self.with_connection(|connection| store_message_metadata(connection, id, payload))
    }
}

fn unsubscribe_method_name(method: &UnsubscribeMethod) -> &'static str {
    match method {
        UnsubscribeMethod::OneClick => "oneClick",
        UnsubscribeMethod::Mailto => "mailto",
        UnsubscribeMethod::Web => "web",
    }
}

/// zstd-compresses message body text for the `body_html_z`/`body_text_z`
/// columns. Encoding an in-memory byte slice cannot meaningfully fail.
pub(super) fn compress_body(text: &str) -> Vec<u8> {
    zstd::stream::encode_all(text.as_bytes(), 3)
        .expect("zstd encoding of an in-memory byte slice cannot fail")
}

/// Writes a raw provider payload compressed, clearing any legacy plaintext.
pub(super) fn store_message_metadata(
    connection: &Connection,
    id: &str,
    payload: &str,
) -> DbResult<()> {
    connection.execute(
        "INSERT INTO message_metadata(id, payload, payload_z) VALUES (?1, '', ?2)
         ON CONFLICT(id) DO UPDATE SET payload = '', payload_z = excluded.payload_z",
        params![id, compress_body(payload)],
    )?;
    Ok(())
}

/// Prefers the compressed column when present (every row written after the
/// body-compression migration); falls back to the legacy plaintext column
/// for rows synced before it.
pub(super) fn resolve_body(
    column_index: usize,
    legacy: String,
    compressed: Option<Vec<u8>>,
) -> rusqlite::Result<String> {
    match compressed {
        Some(bytes) => zstd::stream::decode_all(bytes.as_slice())
            .ok()
            .and_then(|buf| String::from_utf8(buf).ok())
            .ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    column_index,
                    rusqlite::types::Type::Blob,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "failed to decompress message body",
                    )),
                )
            }),
        None => Ok(legacy),
    }
}

#[cfg(test)]
#[path = "tests/messages.rs"]
mod tests;
