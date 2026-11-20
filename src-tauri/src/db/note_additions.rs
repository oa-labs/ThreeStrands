//! Add to Note: one guarded write that appends a passage of a message, a
//! link to its conversation, and copies of its files to a note, and the
//! undo that takes it back.

use serde::{Deserialize, Serialize};

use super::note_attachments::{clean_name, insert_attachment, stored_mime_type};
use super::note_entities::{
    begin_note_insert, conversation_reference, links_conversation, DailyInsertMessages,
};
use super::{Database, DatabaseError, DbResult};
use crate::image_format;
use crate::models::{Note, NoteAttachment};
use crate::note_additions::{self, AddedLink};
use crate::replicated_sync::{encode_id, random_id};

/// Where an addition goes: a note that exists (or a daily note that may not
/// yet), or a new note named by the user.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AddTarget {
    Note { id: String },
    New { title: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddToNoteRequest {
    pub target: AddTarget,
    /// The note's `updatedAt` the addition began from, or null for a daily
    /// note that doesn't exist yet and for a new note.
    pub expected: Option<String>,
    pub thread: String,
    pub message: String,
    /// The passage to quote; absent adds a link to the conversation.
    pub quote: Option<String>,
    /// The message's short date, as the dialog showed it.
    pub date_label: String,
    /// Attachment part ids of `message` to copy into the note.
    pub files: Vec<String>,
}

impl AddToNoteRequest {
    pub fn check(&self) -> Result<(), String> {
        note_additions::check_limits(self.quote.as_deref(), self.files.len())
    }
}

/// A file read from the message, ready to store.
pub struct AddedFile {
    pub name: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteAddition {
    pub note: Note,
    /// False when the note already linked the conversation and nothing else
    /// was added; nothing was written.
    pub added: bool,
    pub previous_body: String,
    /// Whether this action created `note`, so Undo may move it to Trash.
    pub created_note: bool,
    /// The files this action stored in the note.
    pub attachments: Vec<NoteAttachment>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoAddToNote {
    pub note_id: String,
    /// The note's `updatedAt` right after the addition.
    pub expected: String,
    pub previous_body: String,
    pub created_note: bool,
    pub attachments: Vec<String>,
}

const MESSAGES: DailyInsertMessages = DailyInsertMessages {
    changed: "This note changed. Open it before adding to it.",
    conflict: "Resolve this note's conflict before adding to it.",
    fence: "This note ends inside a code block. Open it before adding to it.",
};

impl Database {
    /// Whether `message` is a message of conversation `thread`.
    pub fn message_in_thread(&self, message: &str, thread: &str) -> DbResult<bool> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND thread_id=?2)",
                [message, thread],
                |r| r.get(0),
            )?)
        })
    }

    /// Appends the addition to the end of the target note, in one
    /// transaction: the note's text, its new files, and (for a new note) the
    /// note itself are written together or not at all. Refuses a note that
    /// changed since `expected`, is in Trash or in conflict, or ends inside
    /// a code fence.
    pub fn add_to_note(
        &self,
        request: &AddToNoteRequest,
        files: Vec<AddedFile>,
    ) -> DbResult<NoteAddition> {
        request.check().map_err(DatabaseError::invalid)?;
        self.with_transaction(|tx| {
            let (reference, subject, href) = conversation_reference(tx, &request.thread)?;
            let (note_id, daily, new_title) = match &request.target {
                AddTarget::Note { id } => {
                    let daily = id
                        .strip_prefix(threestrands_sync_protocol::DAILY_NOTE_ID_PREFIX)
                        .filter(|date| crate::notes::is_daily_date(date))
                        .map(str::to_string);
                    (id.clone(), daily, None)
                }
                AddTarget::New { title } => {
                    if request.expected.is_some() {
                        return Err(DatabaseError::invalid("A new note has no earlier version"));
                    }
                    let title =
                        crate::notes::clean_note_title(title).map_err(DatabaseError::invalid)?;
                    (uuid::Uuid::new_v4().to_string(), None, Some(title))
                }
            };
            let start = if new_title.is_some() {
                None
            } else {
                Some(begin_note_insert(
                    tx,
                    &note_id,
                    request.expected.as_deref(),
                    &MESSAGES,
                )?)
            };
            if let Some(start) = &start {
                if start.note.is_none() && daily.is_none() {
                    return Err(DatabaseError::NotFound("Note"));
                }
                if start.parsed.unclosed_fence {
                    return Err(DatabaseError::invalid(MESSAGES.fence));
                }
            }
            let previous_body = start
                .as_ref()
                .map(|s| s.previous_body.clone())
                .unwrap_or_default();
            let link_present = match &start {
                Some(start) => links_conversation(tx, &start.parsed, &reference)?,
                None => false,
            };
            let ids: Vec<String> = files.iter().map(|_| encode_id(&random_id())).collect();
            let links: Vec<AddedLink> = files
                .iter()
                .zip(&ids)
                .map(|(file, id)| AddedLink {
                    id: id.clone(),
                    name: clean_name(&file.name),
                    image: image_format::sniff_raster_mime(&file.bytes).is_some(),
                })
                .collect();
            let Some(block) = note_additions::compose(
                request.quote.as_deref(),
                &subject,
                &href,
                &request.date_label,
                &links,
                link_present,
            ) else {
                let note = start
                    .and_then(|s| s.note)
                    .ok_or(DatabaseError::NotFound("Note"))?;
                return Ok(NoteAddition {
                    note,
                    added: false,
                    previous_body,
                    created_note: false,
                    attachments: Vec::new(),
                });
            };
            let body = note_additions::append_block(&previous_body, &block);
            let (note, created_note) = match (&request.target, &daily, new_title) {
                (_, _, Some(title)) => {
                    let now = chrono::Utc::now().to_rfc3339();
                    let note = Note {
                        id: note_id.clone(),
                        title,
                        body,
                        daily_date: None,
                        created_at: now.clone(),
                        updated_at: now,
                        trashed: false,
                        starred: false,
                        trashed_at: None,
                        authored_title: None,
                    };
                    super::notes::check_title_free(tx, &note.id, &note.title)?;
                    (super::notes::create(tx, note)?, true)
                }
                (_, Some(date), None) => (
                    super::notes::save_daily_in(tx, date, &body)?
                        .ok_or(DatabaseError::NotFound("Note"))?,
                    false,
                ),
                (_, None, None) => {
                    let current = start
                        .and_then(|s| s.note)
                        .ok_or(DatabaseError::NotFound("Note"))?;
                    let title = current.title.clone();
                    (super::notes::edit(tx, current, title, &body)?, false)
                }
            };
            let mut attachments = Vec::with_capacity(files.len());
            for ((file, id), link) in files.iter().zip(&ids).zip(&links) {
                insert_attachment(tx, id, &note.id, &file.name, &file.mime_type, &file.bytes)?;
                attachments.push(NoteAttachment {
                    id: id.clone(),
                    note_id: note.id.clone(),
                    name: link.name.clone(),
                    mime_type: stored_mime_type(&file.mime_type, &file.bytes),
                    size: file.bytes.len() as u64,
                    image: link.image,
                    available: true,
                });
            }
            Ok(NoteAddition {
                note,
                added: true,
                previous_body,
                created_note,
                attachments,
            })
        })
    }

    /// Restores the body an addition replaced and deletes the files it
    /// stored, only while the note is exactly as the addition left it. A
    /// note the addition created goes to Trash. Refuses, changing nothing,
    /// when anything edited the note since.
    pub fn undo_add_to_note(&self, request: &UndoAddToNote) -> DbResult<Note> {
        self.with_transaction(|tx| {
            let current = super::notes::get(tx, &request.note_id)?;
            if !super::notes::is_current(current.as_ref(), Some(&request.expected)) {
                return Err(DatabaseError::invalid(
                    "This note changed after it was added to. Open Note to remove the addition.",
                ));
            }
            let current = current.ok_or(DatabaseError::NotFound("Note"))?;
            let restored = match &current.daily_date {
                Some(date) => super::notes::save_daily_in(tx, date, &request.previous_body)?
                    .unwrap_or(current),
                None => {
                    let title = current.title.clone();
                    super::notes::edit(tx, current, title, &request.previous_body)?
                }
            };
            super::note_attachments::delete_attachments(tx, &restored.id, &request.attachments)?;
            if request.created_note {
                return super::notes::set_trashed(tx, &restored.id, true);
            }
            Ok(restored)
        })
    }
}

#[cfg(test)]
#[path = "tests/note_additions.rs"]
mod tests;
