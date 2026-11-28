//! Derived note-task and entity-link indexes. Mutations reuse the note write
//! transaction, conflict guard and sync budget, never a second task entity.
use super::{Database, DatabaseError, DbResult};
use crate::{
    meeting_excerpt::{self, PrepInsert},
    models::{Note, NoteBacklink},
    note_entities::{self, EntityLink, NoteItem, NoteReference},
    note_meetings::{self, MeetingEntry},
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDocument {
    pub items: Vec<NoteItem>,
    pub links: Vec<EntityLink>,
    pub warning: Option<String>,
}
impl NoteDocument {
    pub(crate) fn from_note(note: &Note) -> Self {
        match note_entities::parse(&note.id, &note.body) {
            Ok(parsed) => Self {
                items: parsed.items,
                links: parsed.links,
                warning: None,
            },
            Err(error) => Self {
                items: vec![],
                links: vec![],
                warning: Some(error),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteTask {
    pub id: String,
    pub marker_id: String,
    pub note_id: String,
    pub note_title: String,
    pub title: String,
    pub checked: bool,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub conflicted: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingCapture {
    pub note: Note,
    pub added: bool,
    pub previous_body: String,
    /// The note the meeting's name links, when it links one.
    pub meeting_note: Option<Note>,
    /// Whether this action created `meeting_note`, so Undo may remove it.
    pub created_meeting_note: bool,
}

/// What a refused insert into a daily note says, naming that note.
pub(super) struct DailyInsertMessages {
    pub(super) changed: &'static str,
    pub(super) conflict: &'static str,
    pub(super) fence: &'static str,
}

/// A daily note checked for adding to: still the version the action began
/// from, not in Trash, and without a body conflict.
pub(super) struct DailyInsertStart {
    pub(super) note: Option<Note>,
    pub(super) previous_body: String,
    pub(super) parsed: note_entities::ParsedNote,
}

pub(super) fn begin_daily_insert(
    tx: &Transaction<'_>,
    date: &str,
    expected: Option<&str>,
    messages: &DailyInsertMessages,
) -> DbResult<DailyInsertStart> {
    if !crate::notes::is_daily_date(date) {
        return Err(DatabaseError::invalid("Invalid date"));
    }
    begin_note_insert(tx, &crate::notes::daily_note_id(date), expected, messages)
}

/// Checks that note `id` is still the version the action began from, not in
/// Trash, and without a body conflict, and parses its body.
pub(super) fn begin_note_insert(
    tx: &Transaction<'_>,
    id: &str,
    expected: Option<&str>,
    messages: &DailyInsertMessages,
) -> DbResult<DailyInsertStart> {
    let note = super::notes::get(tx, id)?;
    if !super::notes::is_current(note.as_ref(), expected) {
        return Err(DatabaseError::invalid(messages.changed));
    }
    let conflicted: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=?1 GROUP BY field HAVING count(DISTINCT value)>1)",
        [id], |r| r.get(0),
    )?;
    if conflicted {
        return Err(DatabaseError::invalid(messages.conflict));
    }
    let previous_body = note.as_ref().map(|n| n.body.clone()).unwrap_or_default();
    let parsed = note_entities::parse(id, &previous_body).map_err(DatabaseError::invalid)?;
    Ok(DailyInsertStart {
        note,
        previous_body,
        parsed,
    })
}

pub(crate) fn index(tx: &Transaction<'_>, note: &Note) -> DbResult<()> {
    // Keep completion observation across unchanged projections on this device.
    let previous = {
        let mut q = tx.prepare("SELECT marker_id,completed_at FROM note_tasks WHERE note_id=?1")?;
        let rows = q
            .query_map([&note.id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
        rows
    };
    tx.execute("DELETE FROM note_tasks WHERE note_id=?1", [&note.id])?;
    tx.execute("DELETE FROM note_entity_links WHERE note_id=?1", [&note.id])?;
    tx.execute(
        "DELETE FROM note_index_warnings WHERE note_id=?1",
        [&note.id],
    )?;
    let parsed = match note_entities::parse(&note.id, &note.body) {
        Ok(parsed) => parsed,
        Err(error) => {
            tx.execute(
                "INSERT INTO note_index_warnings VALUES (?1,?2)",
                params![note.id, error],
            )?;
            return Ok(());
        }
    };
    for item in parsed.items.iter().filter(|item| item.task) {
        let completed = item.checked.then(|| {
            previous
                .get(&item.id)
                .cloned()
                .flatten()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339())
        });
        tx.execute("INSERT INTO note_tasks(note_id,marker_id,title,checked,completed_at) VALUES (?1,?2,?3,?4,?5)",params![note.id,item.id,item.title,item.checked,completed])?;
    }
    for (position, link) in parsed.links.iter().enumerate() {
        let target = link.reference.target().map_err(DatabaseError::invalid)?;
        let series = series_target(tx, &link.reference)?;
        tx.execute("INSERT INTO note_entity_links(note_id,position,href,target,series,context,start) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![note.id,position as i64,link.href,target,series,link.context,link.start as i64])?;
    }
    Ok(())
}
/// The recurring series an event link counts toward: the one the link names,
/// else the one this device's event cache knows for that occurrence.
fn series_target(tx: &Transaction<'_>, reference: &NoteReference) -> DbResult<Option<String>> {
    if reference.kind != "event" {
        return Ok(None);
    }
    if reference.series.is_some() {
        return reference.series_target().map_err(DatabaseError::invalid);
    }
    let cached: Option<String> = tx
        .query_row(
            "SELECT json_extract(body,'$.seriesId') FROM note_event_cache WHERE account=?1 AND calendar=?2 AND id=?3",
            params![reference.account, reference.calendar, reference.id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    match cached.filter(|series| *series != reference.id) {
        Some(series) => NoteReference {
            series: Some(series),
            ..reference.clone()
        }
        .series_target()
        .map_err(DatabaseError::invalid),
        None => Ok(None),
    }
}
/// The reference, subject, and link of conversation (thread) `thread`.
pub(super) fn conversation_reference(
    tx: &Transaction<'_>,
    thread: &str,
) -> DbResult<(NoteReference, String, String)> {
    let (account, provider, subject): (String, String, String) = tx.query_row(
        "SELECT account_id,provider_thread_id,subject FROM threads WHERE id=?1",
        [thread],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let reference = NoteReference {
        version: 1,
        kind: "conversation".into(),
        id: provider,
        account: Some(account),
        calendar: None,
        note: None,
        contact: None,
        series: None,
    };
    let href = reference.href().map_err(DatabaseError::invalid)?;
    Ok((reference, subject, href))
}

/// Whether a parsed note already links `reference`'s conversation, including
/// through a verified alias of its thread id.
pub(super) fn links_conversation(
    tx: &Transaction<'_>,
    parsed: &note_entities::ParsedNote,
    reference: &NoteReference,
) -> DbResult<bool> {
    for link in &parsed.links {
        let same_conversation = link.reference.kind == "conversation"
            && link.reference.account == reference.account
            && conversation_id(
                tx,
                reference.account.as_deref().unwrap(),
                &link.reference.id,
            )?
            .as_ref()
                == Some(&reference.id);
        if &link.reference == reference || same_conversation {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Follow only verified, account-scoped provider aliases. A cycle or a
/// chain beyond the established provider bound is unavailable, never guessed.
pub(super) fn conversation_id(
    c: &rusqlite::Connection,
    account: &str,
    id: &str,
) -> DbResult<Option<String>> {
    let mut current = id.to_string();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..64 {
        if !seen.insert(current.clone()) {
            return Ok(None);
        }
        let next: Option<String> = c
            .query_row(
                "SELECT new_id FROM imap_thread_aliases WHERE account_id=?1 AND old_id=?2",
                params![account, current],
                |r| r.get(0),
            )
            .optional()?;
        match next {
            Some(next) => current = next,
            None => return Ok(Some(current)),
        }
    }
    Ok(None)
}

impl Database {
    pub fn note_document(&self, id: &str) -> DbResult<NoteDocument> {
        let note = self.get_note(id)?.ok_or(DatabaseError::NotFound("Note"))?;
        Ok(NoteDocument::from_note(&note))
    }
    pub fn list_note_tasks(&self) -> DbResult<Vec<NoteTask>> {
        self.with_connection(|c| {
            let mut q=c.prepare("SELECT t.marker_id,t.note_id,n.title,t.title,t.checked,n.updated_at,t.completed_at,
                EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=n.id GROUP BY field HAVING count(DISTINCT value)>1)
                FROM note_tasks t JOIN notes n ON n.id=t.note_id WHERE n.trashed=0 ORDER BY t.checked,t.completed_at DESC,t.note_id,t.marker_id")?;
            let rows = q.query_map([],|r|{let marker:String=r.get(0)?;let note:String=r.get(1)?; Ok(NoteTask{id:format!("note:{note}:{marker}"),marker_id:marker,note_id:note,note_title:r.get(2)?,title:r.get(3)?,checked:r.get(4)?,updated_at:r.get(5)?,completed_at:r.get(6)?,conflicted:r.get(7)?})})?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows)
        })
    }
    pub fn patch_note_item(
        &self,
        id: &str,
        marker: &str,
        expected: &str,
        checked: Option<bool>,
        task: Option<bool>,
    ) -> DbResult<Note> {
        self.with_transaction(|tx| {
            let note = super::notes::get(tx, id)?.ok_or(DatabaseError::NotFound("Note"))?;
            if !super::notes::is_current(Some(&note), Some(expected)) {
                return Err(DatabaseError::invalid(
                    "This note changed. Open it to resolve your changes.",
                ));
            }
            let conflicted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='note' AND entity_id=?1 GROUP BY field HAVING count(DISTINCT value)>1)", [id], |r|r.get(0))?;
            if conflicted { return Err(DatabaseError::invalid("Resolve this note's conflict before changing its tasks.")); }
            let body = note_entities::patch(id, &note.body, marker, checked, task)
                .map_err(DatabaseError::invalid)?;
            let title = note.title.clone();
            super::notes::edit(tx, note, title, &body)
        })
    }
    pub fn note_backlinks(&self, reference: &NoteReference) -> DbResult<Vec<NoteBacklink>> {
        reference.href().map_err(DatabaseError::invalid)?;
        self.with_connection(|c| {
            let mut references=vec![reference.clone()];
            if reference.kind=="conversation" {
                let account=reference.account.as_deref().unwrap();
                let target=conversation_id(c,account,&reference.id)?;
                if let Some(target)=target {
                    let mut q=c.prepare("SELECT old_id FROM imap_thread_aliases WHERE account_id=?1")?;
                    for old in q.query_map([account],|r|r.get::<_,String>(0))? {
                        let old=old?;
                        if conversation_id(c,account,&old)?.as_ref()==Some(&target) {
                            references.push(NoteReference{id:old,..reference.clone()});
                        }
                    }
                }
            }
            let mut rows=std::collections::BTreeMap::new();
            for reference in references {
                // Each note's first occurrence; its own href, so opening the
                // backlink can find that exact link in the note's text.
                let mut q=c.prepare("SELECT n.id,n.title,n.daily_date,l.context,l.href,MIN(l.position) FROM note_entity_links l JOIN notes n ON n.id=l.note_id WHERE (l.target=?1 OR l.series=?1) AND n.trashed=0 GROUP BY n.id ORDER BY n.title,n.id")?;
                for row in q.query_map([reference.target().map_err(DatabaseError::invalid)?],|r|{let href:String=r.get(4)?;Ok(NoteBacklink{reference:Some(NoteReference::parse(&href).unwrap_or_else(||reference.clone())),note_id:r.get(0)?,title:r.get(1)?,daily_date:r.get(2)?,context:r.get(3)?})})? {
                    let row=row?;rows.entry(row.note_id.clone()).or_insert(row);
                }
            }
            // Create backlinked note stores event -> daily meeting line ->
            // [[Meeting note]], not an event link in the meeting note itself.
            // Resolve that existing path at read time, including older notes.
            if reference.kind == "event" {
                let target = reference.target().map_err(DatabaseError::invalid)?;
                let daily_ids: Vec<_> = rows.values().filter(|row| row.daily_date.is_some()).map(|row| row.note_id.clone()).collect();
                for id in daily_ids {
                    let body: String = c.query_row("SELECT body FROM notes WHERE id=?1", [&id], |r| r.get(0))?;
                    let blocks = meeting_excerpt::meeting_blocks(&id, &body, |link| {
                        link.kind == "event" && (link.target().ok().as_ref() == Some(&target) || link.series_target().ok().flatten().as_ref() == Some(&target))
                    }).map_err(DatabaseError::invalid)?;
                    for block in blocks.into_iter().filter(|block| block.kind == meeting_excerpt::MeetingBlockKind::Excerpt) {
                        // Only the meeting's own line, never a nested task or
                        // a neighboring meeting. Ambiguous lines stay daily.
                        let line = body[block.start..block.end].lines().next().unwrap_or_default();
                        let links = crate::notes::extract_links(line);
                        if links.len() != 1 { continue; }
                        let linked = c.query_row(
                            "SELECT id,title FROM notes WHERE title_key=?1 AND trashed=0 AND daily_date IS NULL ORDER BY id LIMIT 1",
                            [&links[0].key], |r| Ok(NoteBacklink { note_id: r.get(0)?, title: r.get(1)?, daily_date: None, context: crate::notes::line_context(line), reference: None }),
                        ).optional()?;
                        if let Some(linked) = linked { rows.entry(linked.note_id.clone()).or_insert(linked); }
                    }
                }
            }
            Ok(rows.into_values().collect())
        })
    }
    /// Account+provider identity is portable; never resolve by subject or headers.
    pub fn resolve_note_conversation(&self, reference: &NoteReference) -> DbResult<Option<String>> {
        if !reference.validate() || reference.kind != "conversation" {
            return Err(DatabaseError::invalid("Invalid conversation reference"));
        }
        self.with_connection(|c| {
            let Some(id) =
                conversation_id(c, reference.account.as_deref().unwrap(), &reference.id)?
            else {
                return Ok(None);
            };
            Ok(c.query_row(
                "SELECT id FROM threads WHERE account_id=?1 AND provider_thread_id=?2",
                params![reference.account, id],
                |r| r.get(0),
            )
            .optional()?)
        })
    }
    /// The contact a contact reference opens: its saved contact while that
    /// still exists, else the saved contact that now owns the address (a
    /// merge moves addresses to the contact it keeps), else the address's
    /// contact derived from mail. Never creates a contact.
    pub fn resolve_note_contact(&self, reference: &NoteReference) -> DbResult<Option<String>> {
        if !reference.validate() || reference.kind != "contact" {
            return Err(DatabaseError::invalid("Invalid contact reference"));
        }
        let saved = self.with_connection(|c| {
            if let Some(id) = &reference.contact {
                let exists: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM contacts WHERE id=?1)",
                    [id],
                    |r| r.get(0),
                )?;
                if exists {
                    return Ok(Some(id.clone()));
                }
            }
            Ok(c.query_row(
                "SELECT contact_id FROM contact_addresses WHERE email=?1",
                [&reference.id],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        if saved.is_some() {
            return Ok(saved);
        }
        let derived = format!("derived:{}", reference.id);
        Ok(self
            .get_contact_profile(&derived)?
            .map(|profile| profile.id))
    }
    /// Writes the meeting's line into the `## Meetings` section of `date`'s
    /// daily note and, when the name links a note, creates that note if no
    /// note outside Trash has its title. Adds nothing when the note already
    /// links this event occurrence. All in one transaction: a refusal writes
    /// neither note.
    pub fn add_meeting_to_daily_note(
        &self,
        date: &str,
        expected: Option<&str>,
        entry: &MeetingEntry,
    ) -> DbResult<MeetingCapture> {
        const MESSAGES: DailyInsertMessages = DailyInsertMessages {
            changed: "That day's note changed. Open it before adding this meeting.",
            conflict: "Resolve that day's note conflict before adding this meeting.",
            fence: "That day's note has a code block that never closes where the meeting would go. Open it before adding this meeting.",
        };
        let title = entry
            .link_note
            .then(|| note_meetings::meeting_note_title(&entry.name));
        let line =
            note_meetings::meeting_line(entry, title.as_deref()).map_err(DatabaseError::invalid)?;
        let event = entry.event.target().map_err(DatabaseError::invalid)?;
        self.with_transaction(|tx| {
            let start = begin_daily_insert(tx, date, expected, &MESSAGES)?;
            let linked = start.parsed.links.iter().any(|link| link.reference.kind == "event" && link.reference.target().ok().as_ref() == Some(&event));
            if linked {
                return Ok(MeetingCapture { note: start.note.unwrap(), added: false, previous_body: start.previous_body, meeting_note: None, created_meeting_note: false });
            }
            let body = note_meetings::insert_meeting_line(&start.previous_body, &line)
                .map_err(|_| DatabaseError::invalid(MESSAGES.fence))?;
            let (meeting_note, created_meeting_note) = match &title {
                None => (None, false),
                Some(title) => {
                    let existing: Option<String> = tx.query_row(
                        "SELECT id FROM notes WHERE title_key=?1 AND trashed=0 AND daily_date IS NULL ORDER BY id LIMIT 1",
                        [crate::notes::title_key(title)], |r| r.get(0),
                    ).optional()?;
                    match existing {
                        Some(id) => (super::notes::get(tx, &id)?, false),
                        None => {
                            let now = chrono::Utc::now().to_rfc3339();
                            let note = Note {
                                id: uuid::Uuid::new_v4().to_string(),
                                title: crate::notes::clean_note_title(title).map_err(DatabaseError::invalid)?,
                                body: String::new(),
                                daily_date: None,
                                created_at: now.clone(),
                                updated_at: now,
                                trashed: false,
                                starred: false,
                                exclude_from_ai: false,
                                trashed_at: None,
                                authored_title: None,
                            };
                            super::notes::check_title_free(tx, &note.id, &note.title)?;
                            (Some(super::notes::create(tx, note)?), true)
                        }
                    }
                }
            };
            let saved = super::notes::save_daily_in(tx, date, &body)?.ok_or(DatabaseError::NotFound("Note"))?;
            Ok(MeetingCapture { note: saved, added: true, previous_body: start.previous_body, meeting_note, created_meeting_note })
        })
    }
}

/// What Reserve Prep Time did to the meeting's day note.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepReservation {
    pub note: Note,
    /// False when the meeting already had a prep task; nothing was written.
    pub added: bool,
    /// Whether this action also wrote the meeting's line.
    pub added_meeting_line: bool,
    pub previous_body: String,
    /// The prep task's id as Tasks lists it (`note:<note>:<marker>`).
    pub task_id: Option<String>,
}

impl Database {
    /// Adds a round task "Prepare for <name>" under the occurrence's line in
    /// `date`'s daily note, writing that line first when the note has none.
    /// When the line already holds a prep task, writes nothing and returns
    /// it, so repeating the action, or retrying after a lost response, adds
    /// no second task. One version-checked transaction, as Add Note.
    pub fn reserve_meeting_prep(
        &self,
        date: &str,
        expected: Option<&str>,
        entry: &MeetingEntry,
    ) -> DbResult<PrepReservation> {
        const MESSAGES: DailyInsertMessages = DailyInsertMessages {
            changed: "That day's note changed. Open it before reserving prep time.",
            conflict: "Resolve that day's note conflict before reserving prep time.",
            fence: "That day's note has a code block that never closes where the meeting would go. Open it before reserving prep time.",
        };
        let line = note_meetings::meeting_line(entry, None).map_err(DatabaseError::invalid)?;
        let event = entry.event.target().map_err(DatabaseError::invalid)?;
        let note_id = crate::notes::daily_note_id(date);
        self.with_transaction(|tx| {
            let start = begin_daily_insert(tx, date, expected, &MESSAGES)?;
            let linked = start.parsed.links.iter().any(|link| {
                link.reference.kind == "event" && link.reference.target().ok().as_ref() == Some(&event)
            });
            let mut body = start.previous_body.clone();
            let mut added_meeting_line = false;
            let insert = |body: &str| {
                meeting_excerpt::insert_prep_task(&note_id, body, &entry.event, &entry.name)
                    .map_err(DatabaseError::invalid)
            };
            let mut result = insert(&body)?;
            if result == PrepInsert::NoMeetingLine {
                if linked {
                    // Linked only from prose or a block that can't be bounded.
                    return Err(DatabaseError::invalid(
                        "This meeting is mentioned in that day's note, but not as a meeting line. Open the note to add prep time.",
                    ));
                }
                body = note_meetings::insert_meeting_line(&body, &line)
                    .map_err(|_| DatabaseError::invalid(MESSAGES.fence))?;
                added_meeting_line = true;
                result = insert(&body)?;
            }
            let (saved, added) = match result {
                PrepInsert::Inserted(next) => (
                    super::notes::save_daily_in(tx, date, &next)?.ok_or(DatabaseError::NotFound("Note"))?,
                    true,
                ),
                PrepInsert::Existing => (start.note.clone().ok_or(DatabaseError::NotFound("Note"))?, false),
                PrepInsert::NoMeetingLine => {
                    return Err(DatabaseError::invalid("The meeting's line couldn't be found after adding it"))
                }
            };
            let task_id = prep_task_id(&saved, &entry.event)?;
            Ok(PrepReservation { note: saved, added, added_meeting_line, previous_body: start.previous_body, task_id })
        })
    }
}

/// The prep task under the occurrence's line in `note`, by its Tasks id.
fn prep_task_id(note: &Note, occurrence: &NoteReference) -> DbResult<Option<String>> {
    let blocks = meeting_excerpt::occurrence_blocks(&note.id, &note.body, occurrence)
        .map_err(DatabaseError::invalid)?;
    let Some(block) = blocks
        .into_iter()
        .find(|block| block.kind == meeting_excerpt::MeetingBlockKind::Excerpt)
    else {
        return Ok(None);
    };
    let parsed = note_entities::parse(&note.id, &note.body).map_err(DatabaseError::invalid)?;
    let prefix = meeting_excerpt::PREP_TASK_PREFIX.trim_end().to_lowercase();
    Ok(parsed
        .items
        .iter()
        .find(|item| {
            item.task
                && block.start < item.start
                && item.start < block.end
                && item.title.to_lowercase().starts_with(&prefix)
        })
        .map(|item| format!("note:{}:{}", note.id, item.id)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedMeeting {
    pub event_ref: NoteReference,
    pub title: String,
    pub attendees: Vec<String>,
    pub start: std::time::SystemTime,
    pub end: std::time::SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMeeting {
    pub event_ref: NoteReference,
    pub title: String,
    pub start: std::time::SystemTime,
    pub end: std::time::SystemTime,
    pub attendees: Vec<String>,
    pub join_url: Option<String>,
    pub join_provider: Option<String>,
    pub is_tomorrow: bool,
}

fn is_non_meeting_calendar(calendar_id: &str) -> bool {
    let lower = calendar_id.to_ascii_lowercase();
    lower == "tasks"
        || lower.ends_with("#tasks@group.v.calendar.google.com")
        || lower.ends_with("#contacts@group.v.calendar.google.com")
        || lower.ends_with("#holiday@group.v.calendar.google.com")
}

fn is_ordinary_event_type(event: &crate::models::ScheduleEvent) -> bool {
    event
        .event_type
        .as_deref()
        .is_none_or(|kind| kind.is_empty() || kind == "default")
}

fn is_meeting_event(
    event: &crate::models::ScheduleEvent,
    join_link: Option<&crate::recording::meeting_links::JoinLink>,
) -> bool {
    !event.all_day
        && event.response_status.as_deref() != Some("declined")
        && is_ordinary_event_type(event)
        && event.working_location_type.is_none()
        && event.source_task_id.is_none()
        && !is_non_meeting_calendar(&event.calendar_id)
        && (!event.attendees.is_empty() || join_link.is_some())
}

fn event_reference_for(event: &crate::models::ScheduleEvent) -> NoteReference {
    NoteReference {
        version: 1,
        kind: "event".into(),
        series: event
            .series_id
            .as_ref()
            .filter(|series| *series != &event.id)
            .cloned(),
        id: event.id.clone(),
        account: Some(event.account_id.clone()),
        calendar: Some(event.calendar_id.clone()),
        note: None,
        contact: None,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteLinkChoice {
    pub label: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    pub reference: NoteReference,
}
impl Database {
    pub fn cache_note_events(
        &self,
        account: &str,
        calendars: &[String],
        min: &str,
        max: &str,
        events: &[crate::models::ScheduleEvent],
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            for calendar in calendars { tx.execute("DELETE FROM note_event_cache WHERE account=?1 AND calendar=?2 AND julianday(start)<julianday(?4) AND julianday(end)>julianday(?3)",params![account,calendar,min,max])?; }
            for event in events {
                tx.execute("INSERT INTO note_event_cache VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(account,calendar,id) DO UPDATE SET start=excluded.start,end=excluded.end,body=excluded.body",params![event.account_id,event.calendar_id,event.id,event.start,event.end,serde_json::to_string(event).map_err(super::serialization_error)?])?;
                // Links written before their series was known count toward it
                // once this device learns which series the occurrence is in.
                if let Some(series) = event.series_id.as_ref().filter(|series| **series != event.id) {
                    let occurrence = NoteReference { version: 1, kind: "event".into(), id: event.id.clone(), account: Some(event.account_id.clone()), calendar: Some(event.calendar_id.clone()), note: None, contact: None, series: Some(series.clone()) };
                    if let (Ok(target), Ok(Some(series))) = (occurrence.target(), occurrence.series_target()) {
                        tx.execute("UPDATE note_entity_links SET series=?2 WHERE target=?1 AND series IS NULL", params![target, series])?;
                    }
                }
            }
            Ok(())
        })
    }
    pub fn resolve_note_event(
        &self,
        reference: &NoteReference,
    ) -> DbResult<Option<crate::models::ScheduleEvent>> {
        if !reference.validate() || reference.kind != "event" {
            return Err(DatabaseError::invalid("Invalid event reference"));
        }
        self.with_connection(|c| {
            let mut q=c.prepare("SELECT body FROM note_event_cache WHERE account=?1 AND calendar=?2 AND EXISTS(SELECT 1 FROM calendar_accounts WHERE email=?1 AND status='connected') ORDER BY start")?;
            let rows=q.query_map(params![reference.account,reference.calendar],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            for body in rows { let event:crate::models::ScheduleEvent=serde_json::from_str(&body).map_err(super::serialization_error)?; if event.id==reference.id || event.series_id.as_ref()==Some(&reference.id) { return Ok(Some(event)); } }
            Ok(None)
        })
    }

    pub fn cached_event_end(
        &self,
        reference: &NoteReference,
    ) -> DbResult<Option<std::time::SystemTime>> {
        let Some(event) = self.resolve_note_event(reference)? else {
            return Ok(None);
        };
        let end = chrono::DateTime::parse_from_rfc3339(&event.end)
            .ok()
            .map(std::time::SystemTime::from);
        Ok(end)
    }

    /// The timed, undeclined meeting under way at `now` or starting within
    /// `lead` in any connected calendar account, preferring an in-progress
    /// meeting over an upcoming one.
    pub fn active_or_upcoming_meeting(
        &self,
        now: std::time::SystemTime,
        lead: std::time::Duration,
    ) -> DbResult<Option<OfferedMeeting>> {
        let bodies = self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT body FROM note_event_cache WHERE EXISTS(SELECT 1 FROM calendar_accounts WHERE email=account AND status='connected')",
            )?;
            let rows = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let horizon = now + lead;
        let mut best: Option<OfferedMeeting> = None;
        for body in bodies {
            let Ok(event) = serde_json::from_str::<crate::models::ScheduleEvent>(&body) else {
                continue;
            };
            if event.all_day
                || event.response_status.as_deref() == Some("declined")
                || !is_ordinary_event_type(&event)
                || event.working_location_type.is_some()
                || event.source_task_id.is_some()
                || is_non_meeting_calendar(&event.calendar_id)
            {
                continue;
            }
            let (Ok(start), Ok(end)) = (
                chrono::DateTime::parse_from_rfc3339(&event.start),
                chrono::DateTime::parse_from_rfc3339(&event.end),
            ) else {
                continue;
            };
            let start = std::time::SystemTime::from(start);
            let end = std::time::SystemTime::from(end);
            if now >= end || horizon < start {
                continue;
            }
            let candidate = OfferedMeeting {
                event_ref: event_reference_for(&event),
                title: event.title,
                attendees: event.attendees,
                start,
                end,
            };
            let replace = match &best {
                None => true,
                Some(current) => {
                    let cand_live = candidate.start <= now;
                    let curr_live = current.start <= now;
                    match (cand_live, curr_live) {
                        (true, false) => true,
                        (false, true) => false,
                        (true, true) => {
                            (
                                candidate.end,
                                std::cmp::Reverse(candidate.start),
                                &candidate.event_ref.id,
                            ) < (
                                current.end,
                                std::cmp::Reverse(current.start),
                                &current.event_ref.id,
                            )
                        }
                        (false, false) => {
                            (candidate.start, &candidate.event_ref.id)
                                < (current.start, &current.event_ref.id)
                        }
                    }
                }
            };
            if replace {
                best = Some(candidate);
            }
        }
        Ok(best)
    }

    /// Remaining timed, undeclined meetings for today (capped at `max_today`),
    /// or tomorrow's first meeting when today has none left. Excludes all-day
    /// events, declined invites, focus/OOO/working-location blocks, tasks, and
    /// solo events without attendees or a conferencing link.
    pub fn upcoming_tray_meetings<Tz: chrono::TimeZone>(
        &self,
        now: chrono::DateTime<Tz>,
        max_today: usize,
    ) -> DbResult<Vec<TrayMeeting>> {
        let bodies = self.with_connection(|c| {
            let mut q = c.prepare(
                "SELECT body FROM note_event_cache WHERE EXISTS(SELECT 1 FROM calendar_accounts WHERE email=account AND status='connected')",
            )?;
            let rows = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let now_sys = std::time::SystemTime::from(now.with_timezone(&chrono::Utc));
        let today = now.date_naive();
        let tomorrow = today.succ_opt();
        let mut today_meetings = Vec::new();
        let mut tomorrow_meetings = Vec::new();
        for body in bodies {
            let Ok(event) = serde_json::from_str::<crate::models::ScheduleEvent>(&body) else {
                continue;
            };
            let join_link = crate::recording::meeting_links::extract_join_link(
                event.conference_url.as_deref(),
                event.location.as_deref(),
                event.description.as_deref(),
            );
            if !is_meeting_event(&event, join_link.as_ref()) {
                continue;
            }
            let (Ok(start_dt), Ok(end_dt)) = (
                chrono::DateTime::parse_from_rfc3339(&event.start),
                chrono::DateTime::parse_from_rfc3339(&event.end),
            ) else {
                continue;
            };
            let start = std::time::SystemTime::from(start_dt);
            let end = std::time::SystemTime::from(end_dt);
            if end <= now_sys {
                continue;
            }
            let start_local_date = start_dt.with_timezone(&now.timezone()).date_naive();
            let is_active = start <= now_sys;
            let is_today = is_active || start_local_date == today;
            let is_tomorrow = !is_today && Some(start_local_date) == tomorrow;
            if !is_today && !is_tomorrow {
                continue;
            }
            let (join_url, join_provider) = match join_link {
                Some(link) => (Some(link.url), link.provider.map(str::to_string)),
                None => (None, None),
            };
            let meeting = TrayMeeting {
                event_ref: event_reference_for(&event),
                title: event.title,
                start,
                end,
                attendees: event.attendees,
                join_url,
                join_provider,
                is_tomorrow,
            };
            if is_today {
                today_meetings.push(meeting);
            } else {
                tomorrow_meetings.push(meeting);
            }
        }
        let cmp = |a: &TrayMeeting, b: &TrayMeeting| {
            (a.start, a.end, &a.event_ref.id).cmp(&(b.start, b.end, &b.event_ref.id))
        };
        today_meetings.sort_by(cmp);
        tomorrow_meetings.sort_by(cmp);
        if !today_meetings.is_empty() {
            today_meetings.truncate(max_today);
            return Ok(today_meetings);
        }
        Ok(tomorrow_meetings.into_iter().next().into_iter().collect())
    }
    pub fn note_link_choices(&self, kind: &str, query: &str) -> DbResult<Vec<NoteLinkChoice>> {
        // Reuse Contacts' saved and mail-derived profiles outside the connection lock.
        if kind == "contact" {
            return Ok(self
                .list_contact_profiles(query, 50)?
                .into_iter()
                .filter_map(|profile| {
                    let address = profile
                        .addresses
                        .iter()
                        .map(|address| note_entities::normalize_contact_address(address))
                        .find(|address| {
                            let reference = NoteReference {
                                version: 1,
                                kind: "contact".into(),
                                id: address.clone(),
                                account: None,
                                calendar: None,
                                note: None,
                                contact: None,
                                series: None,
                            };
                            reference.validate()
                        })?;
                    let reference = NoteReference {
                        version: 1,
                        kind: "contact".into(),
                        id: address.clone(),
                        account: None,
                        calendar: None,
                        note: None,
                        contact: (!profile.id.starts_with("derived:")).then_some(profile.id),
                        series: None,
                    };
                    Some(NoteLinkChoice {
                        label: profile
                            .display_name
                            .filter(|name| !name.trim().is_empty())
                            .unwrap_or_else(|| address.clone()),
                        detail: profile.addresses.join(" · "),
                        sent_at: None,
                        reference,
                    })
                })
                .collect());
        }
        self.with_connection(|c| {
            let mut choices=Vec::new();
            match kind {
                "conversation" => {
                    let mut q=c.prepare("SELECT t.subject,t.account_id,t.provider_thread_id,(SELECT json_group_array(person) FROM (SELECT value AS person FROM json_each(t.participants_json) UNION SELECT CASE WHEN coalesce(display_name,'')='' THEN email ELSE display_name || ' <' || email || '>' END AS person FROM contact_interactions WHERE thread_id=t.id)),t.last_message_at FROM threads t WHERE instr(lower(t.subject || ' ' || t.participants_json),lower(?1))>0 OR EXISTS(SELECT 1 FROM contact_interactions ci WHERE ci.thread_id=t.id AND instr(lower(ci.email || ' ' || coalesce(ci.display_name,'')),lower(?1))>0) ORDER BY t.last_message_at DESC,t.id LIMIT 50")?;
                    for r in q.query_map([query],|r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))? {
                        let (label,account,id,participants,sent_at)=r?;
                        let participants: Vec<String> = serde_json::from_str(&participants).map_err(super::serialization_error)?;
                        choices.push(NoteLinkChoice{label:if label.is_empty(){"(No subject)".into()}else{label},detail:format!("{} · {}",participants.join(", "),account),sent_at:Some(sent_at),reference:NoteReference{version:1,kind:kind.into(),id,account:Some(account),calendar:None,note:None,contact:None,series:None}});
                    }
                }
                "task" => {
                    let mut q=c.prepare("SELECT title,account_id,id FROM tasks WHERE instr(lower(title),lower(?1))>0 ORDER BY updated_at DESC LIMIT 50")?;
                    for r in q.query_map([query],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))? {
                        let (label,detail,id)=r?; choices.push(NoteLinkChoice{label,detail,sent_at:None,reference:NoteReference{version:1,kind:kind.into(),id,account:None,calendar:None,note:None,contact:None,series:None}});
                    }
                    let mut q=c.prepare("SELECT t.title,n.title,t.marker_id,n.id FROM note_tasks t JOIN notes n ON n.id=t.note_id WHERE n.trashed=0 AND instr(lower(t.title),lower(?1))>0 LIMIT 50")?;
                    for r in q.query_map([query],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))? {
                        let (label,detail,id,note)=r?; choices.push(NoteLinkChoice{label,detail:format!("Note · {detail}"),sent_at:None,reference:NoteReference{version:1,kind:kind.into(),id,account:None,calendar:None,note:Some(note),contact:None,series:None}});
                    }
                }
                "event" => {
                    let mut q=c.prepare("SELECT body FROM note_event_cache WHERE EXISTS(SELECT 1 FROM calendar_accounts WHERE email=account AND status='connected') ORDER BY start")?;
                    for r in q.query_map([],|r|r.get::<_,String>(0))? {
                        let event:crate::models::ScheduleEvent=serde_json::from_str(&r?).map_err(super::serialization_error)?;
                        if !event.title.to_lowercase().contains(&query.to_lowercase()){continue;}
                        let reference=NoteReference{version:1,kind:kind.into(),id:event.id.clone(),account:Some(event.account_id.clone()),calendar:Some(event.calendar_id.clone()),note:None,contact:None,series:event.series_id.clone().filter(|series|*series!=event.id)};
                        let detail=format!("{} · {} · {}",event.account_id,event.calendar_id,event.start);
                        choices.push(NoteLinkChoice{label:event.title.clone(),detail:format!("{detail}{}",if event.series_id.is_some(){" · This occurrence"}else{""}),sent_at:None,reference:reference.clone()});
                        if let Some(series)=event.series_id { if !choices.iter().any(|c|c.reference.id==series && c.reference.account==reference.account && c.reference.calendar==reference.calendar) {choices.push(NoteLinkChoice{label:event.title,detail:format!("{detail} · Entire series"),sent_at:None,reference:NoteReference{id:series,series:None,..reference}});} }
                    }
                }
                _ => return Err(DatabaseError::invalid("Unknown note link kind")),
            }
            choices.truncate(50);
            Ok(choices)
        })
    }
}

impl Database {
    pub fn forget_note_event(&self, account: &str, calendar: &str, id: &str) -> DbResult<()> {
        self.with_connection(|c| {
            c.execute(
                "DELETE FROM note_event_cache WHERE account=?1 AND calendar=?2 AND id=?3",
                params![account, calendar, id],
            )?;
            Ok(())
        })
    }
    pub fn note_task_warnings(&self) -> DbResult<Vec<(String, String, String)>> {
        self.with_connection(|c| {
            let mut q=c.prepare("SELECT n.id,n.title,w.warning FROM note_index_warnings w JOIN notes n ON n.id=w.note_id WHERE n.trashed=0 ORDER BY n.id")?;
            let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows)
        })
    }
}
