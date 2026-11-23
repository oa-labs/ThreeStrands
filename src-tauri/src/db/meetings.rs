//! Local, read-only context for preparing a meeting: the notes about this
//! occurrence and earlier ones in its series, related cached mail, and open
//! commitments. Nothing here writes, contacts a provider, or loads message
//! bodies. Each section fails on its own, so one broken source never hides
//! the others, and every list says how much it left out.
//!
//! Identity is the full occurrence reference (account, calendar, event id).
//! A matching title or iCalendar UID never makes two events one meeting, and
//! a note about another occurrence is "previous" only when it is in the same
//! series and earlier.
use super::{normalize_sender, today::list_open_promises_on, Database, DatabaseError, DbResult};
use crate::{
    meeting_excerpt::{self, MeetingBlock, MeetingBlockKind},
    meeting_policy::{self as policy, MeetingLimits},
    models::ScheduleEvent,
    note_entities::{self, normalize_contact_address, NoteReference},
};
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SectionStatus {
    /// Everything that matched is listed.
    Ready,
    /// More matched than the limit allows; `omitted` says how many.
    Limited,
    /// The user must choose which mail accounts to look in first.
    ScopeRequired,
    /// This section couldn't be read; `message` says why.
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSection<T> {
    pub status: SectionStatus,
    pub items: Vec<T>,
    pub omitted: usize,
    pub message: Option<String>,
}

impl<T> MeetingSection<T> {
    fn bounded(mut items: Vec<T>, limit: usize) -> Self {
        let omitted = items.len().saturating_sub(limit);
        items.truncate(limit);
        Self {
            status: if omitted > 0 {
                SectionStatus::Limited
            } else {
                SectionStatus::Ready
            },
            items,
            omitted,
            message: None,
        }
    }
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: SectionStatus::Unavailable,
            items: Vec::new(),
            omitted: 0,
            message: Some(message.into()),
        }
    }
}

fn section<T>(result: DbResult<MeetingSection<T>>) -> MeetingSection<T> {
    result.unwrap_or_else(|error| {
        log::warn!("Meeting context section failed: {error}");
        MeetingSection::unavailable("This section couldn't be read on this device.")
    })
}

/// The event as this device last cached it, with the user's own addresses
/// left out of its attendees.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingEventSnapshot {
    pub title: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub series_id: Option<String>,
    pub attendees: Vec<String>,
    /// More attendees than the lookup limit; the rest weren't used for mail.
    pub attendees_omitted: usize,
    pub source_message_id: Option<String>,
    pub response_status: Option<String>,
    /// Cleaned plain-text description/agenda from the calendar invite, if any.
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingSourceKind {
    /// A meeting block: the list item linking the meeting and its nested content.
    Excerpt,
    /// A link outside a meeting block: only its own line, to open the note.
    Reference,
    /// A reusable note the block links with `[[Title]]`. It may cover other
    /// occurrences too, so only a preview is shown.
    LinkedNote,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingNoteSource {
    pub note_id: String,
    pub title: String,
    pub daily_date: Option<String>,
    /// The note's version when read, to notice later changes.
    pub revision: String,
    pub kind: MeetingSourceKind,
    pub text: String,
    pub truncated: bool,
    /// Byte range of `text` in the note body, to select it when opened.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingOccurrence {
    pub reference: NoteReference,
    /// Its start when this device has the event cached.
    pub start: Option<String>,
    /// Otherwise the date of the daily note that mentions it.
    pub date: Option<String>,
    pub sources: Vec<MeetingNoteSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationReason {
    /// The message the event was created from.
    Origin,
    /// A recent conversation whose subject names the meeting, such as its
    /// invitation and replies to it.
    Title,
    /// Recent mail with one or more attendees.
    Attendees,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingConversation {
    pub thread_id: String,
    pub account_id: String,
    pub subject: String,
    pub last_message_at: String,
    pub reason: ConversationReason,
    /// The attendees this conversation was found through.
    pub attendees: Vec<String>,
    /// Likely about this meeting: its origin, a subject naming it or sharing a
    /// distinctive word with its title, mail with all of its attendees, or mail
    /// with at least two attendees of a group meeting.
    pub related: bool,
    /// Short preview from the thread's cached snippet, never loading bodies.
    pub snippet: Option<String>,
    pub unread: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommitmentOrigin {
    /// A round task or task link inside this meeting's own notes.
    MeetingNote,
    /// An unfinished round task or task link carried over from an earlier occurrence's notes.
    PreviousMeetingNote,
    /// A task on the conversation the event came from.
    OriginConversation,
    /// A task on a conversation whose subject names the meeting.
    TitleConversation,
    /// A task on a conversation with an attendee: possibly related.
    AttendeeConversation,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingCommitment {
    /// The task's id as Tasks lists it (`note:<note>:<marker>` for a note task).
    pub id: String,
    pub title: String,
    pub status: String,
    /// `"action"` or `"waiting_for"`.
    pub kind: String,
    pub note_id: Option<String>,
    pub thread_id: Option<String>,
    pub account_id: Option<String>,
    pub origin: CommitmentOrigin,
    /// Not shown to belong to this meeting; listed as a possibility.
    pub candidate: bool,
}

/// An open promise the user made in a recent sent email to an attendee.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPromise {
    pub message_id: String,
    pub thread_id: String,
    pub account_id: String,
    pub subject: String,
    pub recipients: Vec<String>,
    pub excerpt: String,
    pub sent_at: String,
}

/// A file attached to a meeting-related email or one of the meeting's notes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingFile {
    /// `"mail"` or `"note"`.
    pub source: String,
    pub account_id: Option<String>,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub note_id: Option<String>,
    pub attachment_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    pub sent_at: String,
    pub starred: bool,
    /// Subject of the email or title of the note holding the file.
    pub context_label: String,
}

/// A follow-through section saved for this occurrence.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowThroughRecord {
    pub note_id: String,
    pub title: String,
    pub daily_date: Option<String>,
    pub revision: String,
    /// The review's date, from the section's line.
    pub date: Option<String>,
    pub text: String,
    pub truncated: bool,
    /// Byte range of the section in the note body.
    pub start: usize,
    pub end: usize,
}

/// Whether the meeting has been reviewed, from saved note content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FollowThroughState {
    /// A note holds a follow-through section linked to this occurrence.
    Reviewed,
    NotReviewed,
    /// The notes couldn't be read.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingContext {
    pub reference: NoteReference,
    /// `None` when this device has no cached copy of the event.
    pub event: Option<MeetingEventSnapshot>,
    /// The mail accounts looked in; empty when a choice is needed.
    pub mail_scope: Vec<String>,
    pub this_meeting: MeetingSection<MeetingNoteSource>,
    pub previous_meetings: MeetingSection<MeetingOccurrence>,
    pub series_notes: MeetingSection<MeetingNoteSource>,
    /// Other non-trashed notes mentioning the attendees or sharing a distinctive title word.
    pub related_notes: MeetingSection<MeetingNoteSource>,
    pub conversations: MeetingSection<MeetingConversation>,
    pub commitments: MeetingSection<MeetingCommitment>,
    /// Open email promises the user made to attendees in recent sent mail.
    pub promises: MeetingSection<MeetingPromise>,
    /// Relevant attachments from the meeting's mail and notes (excluding calendar invites unless starred).
    pub files: MeetingSection<MeetingFile>,
    /// Follow-through sections saved for this occurrence, in any note.
    pub follow_through: MeetingSection<FollowThroughRecord>,
    pub limits: MeetingLimits,
}

/// Counts for one meeting, for a day's list. Agrees with the full context.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingReadiness {
    pub reference: NoteReference,
    /// Whether this device has the event cached.
    pub event_available: bool,
    pub has_occurrence_note: bool,
    /// Earlier occurrences of the series with notes.
    pub prior_occurrences_with_notes: usize,
    pub series_notes: usize,
    pub open_commitments: usize,
    pub candidate_commitments: usize,
    pub latest_correspondence_at: Option<String>,
    pub scope_required: bool,
    pub follow_through: FollowThroughState,
    /// Sections that couldn't be read.
    pub unavailable_sections: Vec<String>,
    /// Why this meeting couldn't be checked at all; the counts are then zero.
    pub error: Option<String>,
}

#[derive(Clone)]
struct NoteRow {
    id: String,
    title: String,
    daily_date: Option<String>,
    body: String,
    updated_at: String,
}

fn note_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok(NoteRow {
        id: row.get(0)?,
        title: row.get(1)?,
        daily_date: row.get(2)?,
        body: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

/// Notes outside Trash with a link whose exact target is `target`.
fn notes_linking(c: &Connection, target: &str) -> DbResult<Vec<NoteRow>> {
    let mut q = c.prepare(
        "SELECT n.id,n.title,n.daily_date,n.body,n.updated_at FROM notes n
         WHERE n.trashed=0 AND EXISTS(SELECT 1 FROM note_entity_links l WHERE l.note_id=n.id AND l.target=?1)
         ORDER BY n.daily_date IS NULL, n.daily_date DESC, n.updated_at DESC, n.id",
    )?;
    let rows = q
        .query_map([target], note_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn cached_event(c: &Connection, reference: &NoteReference) -> DbResult<Option<ScheduleEvent>> {
    let body: Option<String> = c
        .query_row(
            "SELECT body FROM note_event_cache WHERE account=?1 AND calendar=?2 AND id=?3
             AND EXISTS(SELECT 1 FROM calendar_accounts WHERE email=?1 AND status='connected')",
            params![reference.account, reference.calendar, reference.id],
            |r| r.get(0),
        )
        .optional()?;
    body.map(|body| serde_json::from_str(&body).map_err(super::serialization_error))
        .transpose()
}

fn local_midnight(date: NaiveDate) -> Option<DateTime<Utc>> {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .earliest()
        .map(|time| time.with_timezone(&Utc))
}

/// When an event starts. An all-day event starts at local midnight of its date.
fn instant(start: &str) -> Option<DateTime<Utc>> {
    if let Ok(time) = DateTime::parse_from_rfc3339(start) {
        return Some(time.with_timezone(&Utc));
    }
    local_midnight(NaiveDate::parse_from_str(start, "%Y-%m-%d").ok()?)
}

/// The blocks of `note` that `is_target` accepts, as sources.
fn sources_in(
    note: &NoteRow,
    blocks: Vec<MeetingBlock>,
) -> Vec<(MeetingNoteSource, Option<MeetingBlock>)> {
    blocks
        .into_iter()
        .map(|block| {
            let (text, truncated) = meeting_excerpt::bounded_text(
                &note.body[block.start..block.end],
                policy::MAX_MEETING_EXCERPT_BYTES,
            );
            let source = MeetingNoteSource {
                note_id: note.id.clone(),
                title: note.title.clone(),
                daily_date: note.daily_date.clone(),
                revision: note.updated_at.clone(),
                kind: match block.kind {
                    MeetingBlockKind::Excerpt => MeetingSourceKind::Excerpt,
                    MeetingBlockKind::Reference => MeetingSourceKind::Reference,
                },
                text,
                truncated,
                start: block.start,
                end: block.end,
            };
            let excerpt = (block.kind == MeetingBlockKind::Excerpt).then_some(block);
            (source, excerpt)
        })
        .collect()
}

/// Reusable notes the excerpts link with `[[Title]]`, once each, as previews.
fn linked_notes(
    c: &Connection,
    excerpts: &[(NoteRow, MeetingBlock)],
    seen: &mut HashSet<String>,
) -> DbResult<Vec<MeetingNoteSource>> {
    let mut out = Vec::new();
    for (note, block) in excerpts {
        for link in crate::notes::extract_links(&note.body[block.start..block.end]) {
            let row = c
                .query_row(
                    "SELECT id,title,daily_date,body,updated_at FROM notes
                     WHERE title_key=?1 AND trashed=0 AND daily_date IS NULL ORDER BY id LIMIT 1",
                    [&link.key],
                    note_row,
                )
                .optional()?;
            let Some(row) = row else { continue };
            if !seen.insert(row.id.clone()) {
                continue;
            }
            let (text, truncated) =
                meeting_excerpt::bounded_text(&row.body, policy::MAX_LINKED_NOTE_PREVIEW_BYTES);
            out.push(MeetingNoteSource {
                end: text.len(),
                note_id: row.id,
                title: row.title,
                daily_date: None,
                revision: row.updated_at,
                kind: MeetingSourceKind::LinkedNote,
                text,
                truncated,
                start: 0,
            });
        }
    }
    Ok(out)
}

/// Where to look for mail about the meeting.
enum MailScope {
    Accounts(Vec<String>),
    Required,
}

/// What the context is built from, gathered before the connection is held.
struct Inputs<'a> {
    reference: &'a NoteReference,
    requested_scope: Option<&'a [String]>,
    own_addresses: &'a HashSet<String>,
    now: DateTime<Utc>,
    /// Full preparation inspects promises (which read the newest sent message body);
    /// batch readiness keeps zero message-body reads.
    include_promises: bool,
}

/// An excerpt block of this meeting, kept to find the commitments inside it.
type Excerpt = (NoteRow, MeetingBlock);

fn this_meeting(
    c: &Connection,
    reference: &NoteReference,
    target: &str,
    excerpts: &mut Vec<Excerpt>,
) -> DbResult<MeetingSection<MeetingNoteSource>> {
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for note in notes_linking(c, target)? {
        let blocks = meeting_excerpt::occurrence_blocks(&note.id, &note.body, reference)
            .map_err(DatabaseError::invalid)?;
        seen.insert(note.id.clone());
        for (source, excerpt) in sources_in(&note, blocks) {
            sources.push(source);
            if let Some(block) = excerpt {
                excerpts.push((note.clone(), block));
            }
        }
    }
    sources.extend(linked_notes(c, excerpts, &mut seen)?);
    Ok(MeetingSection::bounded(
        sources,
        policy::MAX_MEETING_NOTE_SOURCES,
    ))
}

/// The notes "This meeting" lists for the occurrence, which are the ones its
/// follow-through can be saved to.
pub(super) fn occurrence_note_ids(
    c: &Connection,
    reference: &NoteReference,
) -> DbResult<HashSet<String>> {
    let target = reference.target().map_err(DatabaseError::invalid)?;
    let mut excerpts = Vec::new();
    Ok(this_meeting(c, reference, &target, &mut excerpts)?
        .items
        .into_iter()
        .map(|source| source.note_id)
        .collect())
}

/// Follow-through sections for this occurrence in every note linking it.
fn follow_through(
    c: &Connection,
    reference: &NoteReference,
    target: &str,
) -> DbResult<MeetingSection<FollowThroughRecord>> {
    let mut records = Vec::new();
    for note in notes_linking(c, target)? {
        for section in crate::meeting_follow_through::sections(&note.id, &note.body, reference)
            .map_err(DatabaseError::invalid)?
        {
            let (text, truncated) = meeting_excerpt::bounded_text(
                &note.body[section.start..section.end],
                policy::MAX_MEETING_EXCERPT_BYTES,
            );
            records.push(FollowThroughRecord {
                note_id: note.id.clone(),
                title: note.title.clone(),
                daily_date: note.daily_date.clone(),
                revision: note.updated_at.clone(),
                date: section.date,
                text,
                truncated,
                start: section.start,
                end: section.end,
            });
        }
    }
    Ok(MeetingSection::bounded(
        records,
        policy::MAX_MEETING_NOTE_SOURCES,
    ))
}

fn series_notes(
    c: &Connection,
    series: Option<&NoteReference>,
) -> DbResult<MeetingSection<MeetingNoteSource>> {
    let Some(series) = series else {
        return Ok(MeetingSection::bounded(Vec::new(), 0));
    };
    let series_target = series.target().map_err(DatabaseError::invalid)?;
    let mut sources = Vec::new();
    for note in notes_linking(c, &series_target)? {
        let blocks = meeting_excerpt::meeting_blocks(&note.id, &note.body, |link| {
            link.kind == "event" && link.target().is_ok_and(|other| other == series_target)
        })
        .map_err(DatabaseError::invalid)?;
        sources.extend(
            sources_in(&note, blocks)
                .into_iter()
                .map(|(source, _)| source),
        );
    }
    Ok(MeetingSection::bounded(
        sources,
        policy::MAX_MEETING_NOTE_SOURCES,
    ))
}

/// Earlier occurrences of the series that notes mention, newest first. A
/// cached start decides the order; without one, only a daily note on an
/// earlier day does. A same-day note without a time can't be ordered against
/// this meeting and is left out, as is anything later.
fn previous_meetings(
    c: &Connection,
    target: &str,
    series: Option<&NoteReference>,
    event: Option<&ScheduleEvent>,
    prior_excerpts: &mut Vec<Excerpt>,
) -> DbResult<MeetingSection<MeetingOccurrence>> {
    let Some(series) = series else {
        return Ok(MeetingSection::bounded(Vec::new(), 0));
    };
    let Some(this_start) = event.and_then(|event| instant(&event.start)) else {
        return Ok(MeetingSection::unavailable(
            "Earlier meetings can't be ordered until this meeting's time is on this device.",
        ));
    };
    let this_date = this_start.with_timezone(&Local).date_naive();
    let series_target = series.target().map_err(DatabaseError::invalid)?;
    let mut q = c.prepare(
        "SELECT DISTINCT l.href, n.daily_date FROM note_entity_links l JOIN notes n ON n.id=l.note_id
         WHERE l.series=?1 AND l.target<>?2 AND n.trashed=0",
    )?;
    // Each other occurrence by its target, with the earliest daily note naming it.
    let mut occurrences: BTreeMap<String, (NoteReference, Option<NaiveDate>)> = BTreeMap::new();
    for row in q.query_map(params![series_target, target], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })? {
        let (href, daily) = row?;
        let Some(link) = NoteReference::parse(&href) else {
            continue;
        };
        let Ok(key) = link.target() else { continue };
        let daily = daily.and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok());
        let entry = occurrences.entry(key).or_insert((link, None));
        entry.1 = match (entry.1, daily) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    let mut prior = Vec::new();
    for (key, (link, daily)) in occurrences {
        let start = cached_event(c, &link)?.map(|event| event.start);
        let order = match start.as_deref().and_then(instant) {
            Some(time) if time < this_start => time,
            Some(_) => continue,
            None => match daily
                .filter(|date| *date < this_date)
                .and_then(local_midnight)
            {
                Some(time) => time,
                None => continue,
            },
        };
        prior.push((order, key, link, start, daily));
    }
    prior.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let omitted = prior.len().saturating_sub(policy::MAX_PRIOR_OCCURRENCES);
    let mut items = Vec::new();
    for (_, key, link, start, daily) in prior.into_iter().take(policy::MAX_PRIOR_OCCURRENCES) {
        let mut sources = Vec::new();
        for note in notes_linking(c, &key)? {
            let blocks = meeting_excerpt::occurrence_blocks(&note.id, &note.body, &link)
                .map_err(DatabaseError::invalid)?;
            for (source, excerpt) in sources_in(&note, blocks) {
                sources.push(source);
                if let Some(block) = excerpt {
                    prior_excerpts.push((note.clone(), block));
                }
            }
        }
        sources.truncate(policy::MAX_MEETING_NOTE_SOURCES);
        items.push(MeetingOccurrence {
            reference: link,
            start,
            date: daily.map(|date| date.format("%Y-%m-%d").to_string()),
            sources,
        });
    }
    let mut result = MeetingSection::bounded(items, policy::MAX_PRIOR_OCCURRENCES);
    if omitted > 0 {
        result.status = SectionStatus::Limited;
        result.omitted = omitted;
    }
    Ok(result)
}

fn account_known(c: &Connection, account: &str) -> DbResult<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE email=?1)",
        [account],
        |r| r.get(0),
    )?)
}

/// The accounts to look in: those requested, all of which must be on this
/// device, else the calendar's own account when it is also a mail account.
fn mail_scope(
    c: &Connection,
    reference: &NoteReference,
    requested: Option<&[String]>,
) -> DbResult<MailScope> {
    match requested {
        Some([]) => Ok(MailScope::Required),
        Some(accounts) => {
            for account in accounts {
                if !account_known(c, account)? {
                    return Err(DatabaseError::invalid(
                        "That mail account isn't on this device",
                    ));
                }
            }
            Ok(MailScope::Accounts(accounts.to_vec()))
        }
        None => {
            let account = reference.account.as_deref().unwrap_or_default();
            Ok(if account_known(c, account)? {
                MailScope::Accounts(vec![account.to_string()])
            } else {
                MailScope::Required
            })
        }
    }
}

/// The event's other people: normalized, once each, without the user's own
/// addresses.
fn external_attendees(event: &ScheduleEvent, own: &HashSet<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    event
        .attendees
        .iter()
        .map(|address| normalize_contact_address(address))
        .filter(|address| address.contains('@') && !own.contains(address))
        .filter(|address| seen.insert(address.clone()))
        .collect()
}

fn bounded_snippet(raw: Option<String>) -> Option<String> {
    let text = raw?;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut count = 0;
    for ch in collapsed.chars() {
        if count >= policy::MAX_CONVERSATION_SNIPPET_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
        count += 1;
    }
    Some(out)
}

/// Strips basic HTML tags from calendar invite descriptions and collapses blank space.
fn clean_event_description(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut text = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut tag_buf = String::new();
    for ch in raw.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_buf.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let tag = tag_buf
                    .trim()
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(tag.as_str(), "br" | "p" | "div" | "li" | "tr") {
                    text.push('\n');
                }
            }
            _ if in_tag => tag_buf.push(ch),
            _ => text.push(ch),
        }
    }
    let decoded = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let lines: Vec<String> = decoded
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        // Skip common auto-inserted meeting-link boilerplate dividers.
        .take_while(|line| !line.starts_with("-::~:~::~:~:~:~") && !line.starts_with("________"))
        .collect();
    if lines.is_empty() {
        return None;
    }
    let joined = lines.join("\n");
    let (bounded, truncated) =
        meeting_excerpt::bounded_text(&joined, policy::MAX_EVENT_DESCRIPTION_BYTES);
    Some(if truncated {
        format!("{bounded}…")
    } else {
        bounded
    })
}

/// The conversation the event was created from, when exactly one cached
/// message on a mail account here has its id.
fn origin_conversation(c: &Connection, message: &str) -> DbResult<Option<MeetingConversation>> {
    let mut q = c.prepare(
        "SELECT t.id,t.account_id,t.subject,t.last_message_at,t.snippet,t.unread FROM messages m JOIN threads t ON t.id=m.thread_id
         WHERE m.id=?1 AND t.trashed=0 AND EXISTS(SELECT 1 FROM accounts a WHERE a.email=t.account_id)",
    )?;
    let mut found = q
        .query_map([message], |r| {
            Ok(MeetingConversation {
                thread_id: r.get(0)?,
                account_id: r.get(1)?,
                subject: r.get(2)?,
                last_message_at: r.get(3)?,
                reason: ConversationReason::Origin,
                attendees: Vec::new(),
                related: true,
                snippet: bounded_snippet(r.get::<_, Option<String>>(4)?),
                unread: r.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // A message id cached in two accounts can't say which one it was.
    Ok(if found.len() == 1 { found.pop() } else { None })
}

/// Words too common in meeting titles and subjects to say two are about the
/// same thing.
const GENERIC_TITLE_WORDS: &[&str] = &[
    "about",
    "accepted",
    "call",
    "catch",
    "chat",
    "check",
    "daily",
    "declined",
    "from",
    "invitation",
    "meeting",
    "meetings",
    "monthly",
    "review",
    "standup",
    "sync",
    "team",
    "tentative",
    "update",
    "updated",
    "updates",
    "weekly",
    "with",
];

/// The distinctive words of a title or subject: ASCII-lowercased runs of
/// letters and digits of four or more characters that aren't generic.
fn title_words(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|word| word.chars().count() >= 4 && !GENERIC_TITLE_WORDS.contains(&word.as_str()))
        .collect()
}

/// Shortest meeting title matched against subjects; shorter ones, such as
/// "1:1", would match unrelated mail.
const MIN_TITLE_MATCH_CHARS: usize = 4;

/// Recent conversations in `accounts` whose subject contains the meeting's
/// title, newest first. Case is folded for ASCII only, as SQLite's `lower`.
fn title_conversations(
    c: &Connection,
    accounts: &[String],
    title: &str,
    since: DateTime<Utc>,
) -> DbResult<Vec<MeetingConversation>> {
    let key = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if key.chars().count() < MIN_TITLE_MATCH_CHARS {
        return Ok(Vec::new());
    }
    let mut q = c.prepare(
        "SELECT t.id,t.account_id,t.subject,t.last_message_at,t.snippet,t.unread FROM threads t
         WHERE t.account_id IN (SELECT value FROM json_each(?1)) AND t.trashed=0
           AND julianday(t.last_message_at)>=julianday(?3) AND instr(lower(t.subject),?2)>0
         ORDER BY t.last_message_at DESC, t.id LIMIT ?4",
    )?;
    let rows = q
        .query_map(
            params![
                serde_json::to_string(accounts).map_err(super::serialization_error)?,
                key,
                since.to_rfc3339(),
                (policy::MAX_MEETING_CONVERSATIONS + 2) as i64,
            ],
            |r| {
                Ok(MeetingConversation {
                    thread_id: r.get(0)?,
                    account_id: r.get(1)?,
                    subject: r.get(2)?,
                    last_message_at: r.get(3)?,
                    reason: ConversationReason::Title,
                    attendees: Vec::new(),
                    related: true,
                    snippet: bounded_snippet(r.get::<_, Option<String>>(4)?),
                    unread: r.get::<_, i64>(5)? != 0,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Whether `address` appears in `headers` as a whole address, not inside a
/// longer one (`ann@x.org` is not in `joann@x.org`).
fn mentions_address(headers: &str, address: &str) -> bool {
    let part = |ch: char| ch.is_ascii_alphanumeric() || "._%+-@".contains(ch);
    headers.match_indices(address).any(|(at, _)| {
        let before = headers[..at].chars().next_back();
        let mut after = headers[at + address.len()..].chars();
        // A following "." ends the address only when no more of it follows.
        let continues = match after.next() {
            Some('.') => after.next().is_some_and(|ch| ch.is_ascii_alphanumeric()),
            Some(ch) => part(ch),
            None => false,
        };
        !before.is_some_and(part) && !continues
    })
}

/// Recent conversations with the attendees in `accounts`, up to one more
/// than the limit so the section can say it was cut. A conversation lists
/// every attendee its messages' From, To, and Cc headers name. Related ones
/// (see [`MeetingConversation::related`]) come first, newest first within
/// each group. Message bodies are never read.
fn attendee_conversations(
    c: &Connection,
    accounts: &[String],
    attendees: &[String],
    title: &str,
    since: DateTime<Utc>,
) -> DbResult<Vec<MeetingConversation>> {
    if attendees.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = c.prepare(
        "SELECT ci.thread_id,t.account_id,t.subject,t.last_message_at,t.snippet,t.unread
         FROM contact_interactions ci JOIN threads t ON t.id=ci.thread_id
         WHERE ci.account_id IN (SELECT value FROM json_each(?1))
           AND ci.email IN (SELECT value FROM json_each(?2))
           AND julianday(ci.sent_at)>=julianday(?3) AND t.trashed=0
         GROUP BY ci.thread_id ORDER BY MAX(ci.sent_at) DESC, ci.thread_id LIMIT ?4",
    )?;
    let candidates = q
        .query_map(
            params![
                serde_json::to_string(accounts).map_err(super::serialization_error)?,
                serde_json::to_string(attendees).map_err(super::serialization_error)?,
                since.to_rfc3339(),
                policy::MAX_ATTENDEE_CANDIDATE_THREADS as i64,
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)? != 0,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut headers = c.prepare(
        "SELECT lower(sender || ' ' || recipients_json || ' ' || coalesce(cc_json,'')) FROM messages WHERE thread_id=?1",
    )?;
    let topic = title_words(title);
    let mut found = Vec::new();
    for (thread_id, account_id, subject, last_message_at, snippet, unread) in candidates {
        let text = headers
            .query_map([&thread_id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        let mut matched: Vec<String> = attendees
            .iter()
            .filter(|address| mentions_address(&text, address))
            .cloned()
            .collect();
        matched.sort();
        let related = matched.len() == attendees.len()
            || (attendees.len() >= 3 && matched.len() >= 2)
            || title_words(&subject)
                .iter()
                .any(|word| topic.contains(word));
        found.push(MeetingConversation {
            thread_id,
            account_id,
            subject,
            last_message_at,
            reason: ConversationReason::Attendees,
            attendees: matched,
            related,
            snippet: bounded_snippet(snippet),
            unread,
        });
    }
    // Stable: newest first within each group.
    found.sort_by_key(|item| !item.related);
    found.truncate(policy::MAX_MEETING_CONVERSATIONS + 2);
    Ok(found)
}

fn conversations(
    c: &Connection,
    event: Option<&ScheduleEvent>,
    scope: &MailScope,
    attendees: &[String],
    now: DateTime<Utc>,
) -> DbResult<MeetingSection<MeetingConversation>> {
    let Some(event) = event else {
        return Ok(MeetingSection::unavailable(
            "Related mail needs this meeting's attendees, which aren't on this device.",
        ));
    };
    let mut items: Vec<MeetingConversation> = match &event.source_message_id {
        Some(message) => origin_conversation(c, message)?.into_iter().collect(),
        None => Vec::new(),
    };
    let MailScope::Accounts(accounts) = scope else {
        let mut result = MeetingSection::bounded(items, policy::MAX_MEETING_CONVERSATIONS);
        result.status = SectionStatus::ScopeRequired;
        result.message = Some("Choose which mail accounts to look in for related mail.".into());
        return Ok(result);
    };
    let since = now - chrono::Duration::days(policy::MEETING_MAIL_LOOKBACK_DAYS);
    let related = title_conversations(c, accounts, &event.title, since)?
        .into_iter()
        .chain(attendee_conversations(
            c,
            accounts,
            attendees,
            &event.title,
            since,
        )?);
    for conversation in related {
        if !items
            .iter()
            .any(|item| item.thread_id == conversation.thread_id)
        {
            items.push(conversation);
        }
    }
    Ok(MeetingSection::bounded(
        items,
        policy::MAX_MEETING_CONVERSATIONS,
    ))
}

fn task_row(
    origin: CommitmentOrigin,
    candidate: bool,
) -> impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<MeetingCommitment> {
    move |r| {
        Ok(MeetingCommitment {
            id: r.get(0)?,
            title: r.get(1)?,
            status: r.get(2)?,
            kind: r
                .get::<_, Option<String>>(3)?
                .unwrap_or_else(|| "action".into()),
            note_id: None,
            thread_id: r.get(4)?,
            account_id: r.get(5)?,
            origin,
            candidate,
        })
    }
}

/// Open round tasks and task links inside meeting blocks (`MeetingNote` or `PreviousMeetingNote`).
fn note_commitments(
    c: &Connection,
    excerpts: &[Excerpt],
    origin: CommitmentOrigin,
) -> DbResult<Vec<MeetingCommitment>> {
    let mut items = Vec::new();
    for (note, block) in excerpts {
        let parsed = note_entities::parse(&note.id, &note.body).map_err(DatabaseError::invalid)?;
        let inside = |at: usize| block.start <= at && at < block.end;
        for item in parsed
            .items
            .iter()
            .filter(|item| item.task && !item.checked && inside(item.start))
        {
            items.push(MeetingCommitment {
                id: format!("note:{}:{}", note.id, item.id),
                title: item.title.clone(),
                status: "open".into(),
                kind: "action".into(),
                note_id: Some(note.id.clone()),
                thread_id: None,
                account_id: None,
                origin,
                candidate: false,
            });
        }
        for link in parsed
            .links
            .iter()
            .filter(|link| link.reference.kind == "task" && inside(link.start))
        {
            let task = &link.reference;
            let found = match &task.note {
                Some(owner) => c
                    .query_row(
                        "SELECT t.title FROM note_tasks t JOIN notes n ON n.id=t.note_id
                         WHERE t.note_id=?1 AND t.marker_id=?2 AND t.checked=0 AND n.trashed=0",
                        params![owner, task.id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?
                    .map(|title| MeetingCommitment {
                        id: format!("note:{owner}:{}", task.id),
                        title,
                        status: "open".into(),
                        kind: "action".into(),
                        note_id: Some(owner.clone()),
                        thread_id: None,
                        account_id: None,
                        origin,
                        candidate: false,
                    }),
                None => c
                    .query_row(
                        "SELECT id,title,status,kind,thread_id,account_id FROM tasks
                         WHERE id=?1 AND status IN ('open','in_progress')",
                        [&task.id],
                        task_row(origin, false),
                    )
                    .optional()?,
            };
            items.extend(found);
        }
    }
    Ok(items)
}

/// Commitments in this meeting's notes, earlier occurrences in the same series,
/// and on its conversations. Tasks on attendee conversations are candidates:
/// they involve these people, not necessarily this meeting.
fn commitments(
    c: &Connection,
    excerpts: &[Excerpt],
    prior_excerpts: &[Excerpt],
    conversations: &MeetingSection<MeetingConversation>,
    event_available: bool,
) -> DbResult<MeetingSection<MeetingCommitment>> {
    let mut items = note_commitments(c, excerpts, CommitmentOrigin::MeetingNote)?;
    items.extend(note_commitments(
        c,
        prior_excerpts,
        CommitmentOrigin::PreviousMeetingNote,
    )?);
    let mut q = c.prepare(
        "SELECT id,title,status,kind,thread_id,account_id FROM tasks
         WHERE thread_id=?1 AND status IN ('open','in_progress') ORDER BY updated_at DESC, id",
    )?;
    for thread in &conversations.items {
        let (origin, candidate) = match thread.reason {
            ConversationReason::Origin => (CommitmentOrigin::OriginConversation, false),
            ConversationReason::Title => (CommitmentOrigin::TitleConversation, false),
            ConversationReason::Attendees => (CommitmentOrigin::AttendeeConversation, true),
        };
        let rows = q
            .query_map([&thread.thread_id], task_row(origin, candidate))?
            .collect::<Result<Vec<_>, _>>()?;
        items.extend(rows);
    }
    let mut ids = HashSet::new();
    items.retain(|item| ids.insert(item.id.clone()));
    // Stable: definite commitments keep their order ahead of candidates.
    items.sort_by_key(|item| item.candidate);
    let mut result = MeetingSection::bounded(items, policy::MAX_MEETING_COMMITMENTS);
    if !event_available {
        result.message = Some("Only commitments in this meeting's notes are shown.".into());
    }
    Ok(result)
}

/// Open email promises the user made to at least one meeting attendee in recent mail.
fn meeting_promises(
    c: &Connection,
    scope: &MailScope,
    attendees: &[String],
    now: DateTime<Utc>,
) -> DbResult<MeetingSection<MeetingPromise>> {
    let MailScope::Accounts(accounts) = scope else {
        return Ok(MeetingSection::bounded(
            Vec::new(),
            policy::MAX_MEETING_PROMISES,
        ));
    };
    if attendees.is_empty() || accounts.is_empty() {
        return Ok(MeetingSection::bounded(
            Vec::new(),
            policy::MAX_MEETING_PROMISES,
        ));
    }
    let attendee_set: HashSet<&str> = attendees.iter().map(String::as_str).collect();
    let since = (now - chrono::Duration::days(policy::MEETING_MAIL_LOOKBACK_DAYS)).to_rfc3339();
    let mut found = Vec::new();
    let mut seen_threads = HashSet::new();
    for account in accounts {
        for promise in list_open_promises_on(c, Some(account), &since)? {
            let matches_attendee = promise
                .recipients
                .iter()
                .any(|recipient| attendee_set.contains(normalize_sender(recipient).0.as_str()));
            if !matches_attendee || !seen_threads.insert(promise.thread.id.clone()) {
                continue;
            }
            found.push(MeetingPromise {
                message_id: promise.message_id,
                thread_id: promise.thread.id,
                account_id: promise.thread.account_id,
                subject: promise.thread.subject,
                recipients: promise.recipients,
                excerpt: promise.excerpt,
                sent_at: promise.sent_at,
            });
        }
    }
    found.sort_by(|a, b| {
        b.sent_at
            .cmp(&a.sent_at)
            .then_with(|| a.thread_id.cmp(&b.thread_id))
    });
    Ok(MeetingSection::bounded(found, policy::MAX_MEETING_PROMISES))
}

/// Attachments from the meeting's mail threads and linked notes (excluding `.ics` invites unless starred).
fn meeting_files(
    c: &Connection,
    conversations: &MeetingSection<MeetingConversation>,
    note_ids: &HashSet<String>,
) -> DbResult<MeetingSection<MeetingFile>> {
    let mut items = Vec::new();
    let thread_ids: Vec<&str> = conversations
        .items
        .iter()
        .map(|conv| conv.thread_id.as_str())
        .collect();
    if !thread_ids.is_empty() {
        let related_threads: HashSet<&str> = conversations
            .items
            .iter()
            .filter(|conv| conv.related)
            .map(|conv| conv.thread_id.as_str())
            .collect();
        let mut q = c.prepare(
            "SELECT t.account_id,t.id,m.id,t.subject,m.sent_at,a.attachment_id,a.filename,a.mime_type,a.size,s.message_id IS NOT NULL
             FROM file_attachments a
             JOIN messages m ON m.id=a.message_id
             JOIN threads t ON t.id=m.thread_id
             LEFT JOIN file_stars s ON s.message_id=a.message_id AND s.attachment_id=a.attachment_id
             WHERE t.id IN (SELECT value FROM json_each(?1))
               AND t.trashed=0
               AND NOT EXISTS (SELECT 1 FROM json_each(t.labels_json) WHERE upper(value)='SPAM')
               AND (s.message_id IS NOT NULL OR NOT (a.mime_type LIKE 'text/calendar%' OR lower(a.filename) LIKE '%.ics'))
             ORDER BY s.message_id IS NULL, m.sent_at DESC, m.id, a.attachment_id
             LIMIT ?2",
        )?;
        let rows = q
            .query_map(
                params![
                    serde_json::to_string(&thread_ids).map_err(super::serialization_error)?,
                    (policy::MAX_MEETING_FILES * 2) as i64,
                ],
                |r| {
                    let thread_id: String = r.get(1)?;
                    let subject: String = r.get(3)?;
                    Ok(MeetingFile {
                        source: "mail".into(),
                        account_id: Some(r.get(0)?),
                        thread_id: Some(thread_id),
                        message_id: Some(r.get(2)?),
                        note_id: None,
                        attachment_id: r.get(5)?,
                        filename: r.get(6)?,
                        mime_type: r.get(7)?,
                        size: r.get::<_, i64>(8)?.max(0) as u64,
                        sent_at: r.get(4)?,
                        starred: r.get(9)?,
                        context_label: if subject.trim().is_empty() {
                            "(No subject)".into()
                        } else {
                            subject
                        },
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        // Related-thread files lead; starred and newest lead within each group.
        let mut mail_files = rows;
        mail_files.sort_by(|a, b| {
            let a_rel = a
                .thread_id
                .as_deref()
                .is_some_and(|id| related_threads.contains(id));
            let b_rel = b
                .thread_id
                .as_deref()
                .is_some_and(|id| related_threads.contains(id));
            b_rel
                .cmp(&a_rel)
                .then_with(|| b.starred.cmp(&a.starred))
                .then_with(|| b.sent_at.cmp(&a.sent_at))
        });
        items.extend(mail_files);
    }
    if !note_ids.is_empty() {
        let ids: Vec<&str> = note_ids.iter().map(String::as_str).collect();
        let mut q = c.prepare(
            "SELECT a.id,a.note_id,a.name,a.mime_type,a.size,a.created_at,a.starred,COALESCE(n.title, substr(a.note_id, 7))
             FROM note_attachments a
             LEFT JOIN notes n ON n.id=a.note_id
             WHERE a.note_id IN (SELECT value FROM json_each(?1)) AND COALESCE(n.trashed, 0)=0
             ORDER BY a.starred DESC, julianday(a.created_at) DESC, a.id
             LIMIT ?2",
        )?;
        let note_files = q
            .query_map(
                params![
                    serde_json::to_string(&ids).map_err(super::serialization_error)?,
                    policy::MAX_MEETING_FILES as i64,
                ],
                |r| {
                    Ok(MeetingFile {
                        source: "note".into(),
                        account_id: None,
                        thread_id: None,
                        message_id: None,
                        note_id: Some(r.get(1)?),
                        attachment_id: r.get(0)?,
                        filename: r.get(2)?,
                        mime_type: r.get(3)?,
                        size: r.get::<_, i64>(4)?.max(0) as u64,
                        sent_at: r.get(5)?,
                        starred: r.get::<_, i64>(6)? != 0,
                        context_label: r.get(7)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        items.extend(note_files);
    }
    items.sort_by(|a, b| {
        b.starred
            .cmp(&a.starred)
            .then_with(|| b.sent_at.cmp(&a.sent_at))
            .then_with(|| a.attachment_id.cmp(&b.attachment_id))
    });
    Ok(MeetingSection::bounded(items, policy::MAX_MEETING_FILES))
}

/// Non-trashed notes outside the meeting's own occurrence/series notes that
/// mention an attendee (contact link) or share a distinctive title word.
fn related_notes(
    c: &Connection,
    event: Option<&ScheduleEvent>,
    attendees: &[String],
    excluded_note_ids: &HashSet<String>,
) -> DbResult<MeetingSection<MeetingNoteSource>> {
    let Some(event) = event else {
        return Ok(MeetingSection::bounded(
            Vec::new(),
            policy::MAX_RELATED_NOTES,
        ));
    };
    let mut matched: BTreeMap<String, (usize, NoteRow)> = BTreeMap::new();
    // 1. Notes with contact links to meeting attendees.
    if !attendees.is_empty() {
        let targets: Vec<String> = attendees
            .iter()
            .filter_map(|address| {
                NoteReference {
                    version: 1,
                    kind: "contact".into(),
                    id: address.clone(),
                    account: None,
                    calendar: None,
                    note: None,
                    contact: None,
                    series: None,
                }
                .target()
                .ok()
            })
            .collect();
        if !targets.is_empty() {
            let mut q = c.prepare(
                "SELECT n.id,n.title,n.daily_date,n.body,n.updated_at,COUNT(DISTINCT l.target)
                 FROM note_entity_links l JOIN notes n ON n.id=l.note_id
                 WHERE l.target IN (SELECT value FROM json_each(?1)) AND n.trashed=0
                 GROUP BY n.id
                 ORDER BY COUNT(DISTINCT l.target) DESC, n.updated_at DESC, n.id
                 LIMIT ?2",
            )?;
            let rows = q
                .query_map(
                    params![
                        serde_json::to_string(&targets).map_err(super::serialization_error)?,
                        (policy::MAX_RELATED_NOTES * 3) as i64,
                    ],
                    |r| Ok((note_row(r)?, r.get::<_, i64>(5)?.max(0) as usize)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            for (note, count) in rows {
                if !excluded_note_ids.contains(&note.id) {
                    matched.insert(note.id.clone(), (count * 2, note));
                }
            }
        }
    }
    // 2. Non-daily notes whose title shares a distinctive word with the meeting title.
    let topic = title_words(&event.title);
    if !topic.is_empty() {
        let mut q = c.prepare(
            "SELECT id,title,daily_date,body,updated_at FROM notes
             WHERE trashed=0 AND daily_date IS NULL
             ORDER BY updated_at DESC, id LIMIT 200",
        )?;
        let rows = q.query_map([], note_row)?.collect::<Result<Vec<_>, _>>()?;
        for note in rows {
            if excluded_note_ids.contains(&note.id) {
                continue;
            }
            let overlap = title_words(&note.title)
                .iter()
                .filter(|word| topic.contains(*word))
                .count();
            if overlap > 0 {
                matched
                    .entry(note.id.clone())
                    .and_modify(|(score, _)| *score += overlap * 3)
                    .or_insert((overlap * 3, note));
            }
        }
    }
    let mut ranked: Vec<(usize, NoteRow)> = matched.into_values().collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    let sources: Vec<MeetingNoteSource> = ranked
        .into_iter()
        .map(|(_, note)| {
            let (text, truncated) =
                meeting_excerpt::bounded_text(&note.body, policy::MAX_LINKED_NOTE_PREVIEW_BYTES);
            MeetingNoteSource {
                end: text.len(),
                note_id: note.id,
                title: note.title,
                daily_date: note.daily_date,
                revision: note.updated_at,
                kind: MeetingSourceKind::LinkedNote,
                text,
                truncated,
                start: 0,
            }
        })
        .collect();
    Ok(MeetingSection::bounded(sources, policy::MAX_RELATED_NOTES))
}

/// Everything one meeting's context needs, read in one connection.
fn build(c: &Connection, inputs: &Inputs<'_>) -> DbResult<MeetingContext> {
    let reference = inputs.reference;
    let target = reference.target().map_err(DatabaseError::invalid)?;
    let event = cached_event(c, reference)?;
    let scope = mail_scope(c, reference, inputs.requested_scope)?;

    let mut excerpts = Vec::new();
    let this_meeting = section(this_meeting(c, reference, &target, &mut excerpts));

    let series_id = event
        .as_ref()
        .and_then(|event| event.series_id.clone())
        .or_else(|| reference.series.clone())
        .filter(|series| *series != reference.id);
    let series_reference = series_id.as_ref().map(|series| NoteReference {
        id: series.clone(),
        series: None,
        contact: None,
        ..reference.clone()
    });
    let series_notes = section(series_notes(c, series_reference.as_ref()));
    let mut prior_excerpts = Vec::new();
    let previous_meetings = section(previous_meetings(
        c,
        &target,
        series_reference.as_ref(),
        event.as_ref(),
        &mut prior_excerpts,
    ));

    let attendees = event
        .as_ref()
        .map(|event| external_attendees(event, inputs.own_addresses))
        .unwrap_or_default();
    let attendees_omitted = attendees
        .len()
        .saturating_sub(policy::MAX_MEETING_ATTENDEES);
    let lookup = &attendees[..attendees.len().min(policy::MAX_MEETING_ATTENDEES)];
    let conversations = section(conversations(c, event.as_ref(), &scope, lookup, inputs.now));
    let commitments = section(commitments(
        c,
        &excerpts,
        &prior_excerpts,
        &conversations,
        event.is_some(),
    ));
    let promises = if inputs.include_promises {
        section(meeting_promises(c, &scope, lookup, inputs.now))
    } else {
        MeetingSection::bounded(Vec::new(), policy::MAX_MEETING_PROMISES)
    };
    let mut direct_note_ids: HashSet<String> = this_meeting
        .items
        .iter()
        .map(|source| source.note_id.clone())
        .collect();
    for occurrence in &previous_meetings.items {
        for source in &occurrence.sources {
            direct_note_ids.insert(source.note_id.clone());
        }
    }
    let files = section(meeting_files(c, &conversations, &direct_note_ids));
    for source in &series_notes.items {
        direct_note_ids.insert(source.note_id.clone());
    }
    let related_notes = section(related_notes(c, event.as_ref(), lookup, &direct_note_ids));
    let follow_through = section(follow_through(c, reference, &target));

    Ok(MeetingContext {
        reference: reference.clone(),
        mail_scope: match scope {
            MailScope::Accounts(accounts) => accounts,
            MailScope::Required => Vec::new(),
        },
        event: event.map(|event| MeetingEventSnapshot {
            description: clean_event_description(event.description.as_deref()),
            title: event.title,
            start: event.start,
            end: event.end,
            all_day: event.all_day,
            series_id,
            attendees,
            attendees_omitted,
            source_message_id: event.source_message_id,
            response_status: event.response_status,
        }),
        this_meeting,
        previous_meetings,
        series_notes,
        related_notes,
        conversations,
        commitments,
        promises,
        files,
        follow_through,
        limits: policy::LIMITS,
    })
}

fn readiness(context: &MeetingContext) -> MeetingReadiness {
    let mut unavailable = Vec::new();
    for (name, status) in [
        ("thisMeeting", context.this_meeting.status),
        ("previousMeetings", context.previous_meetings.status),
        ("seriesNotes", context.series_notes.status),
        ("conversations", context.conversations.status),
        ("commitments", context.commitments.status),
        ("followThrough", context.follow_through.status),
    ] {
        if status == SectionStatus::Unavailable {
            unavailable.push(name.to_string());
        }
    }
    let commitments = &context.commitments.items;
    MeetingReadiness {
        reference: context.reference.clone(),
        event_available: context.event.is_some(),
        has_occurrence_note: context
            .this_meeting
            .items
            .iter()
            .any(|source| source.kind != MeetingSourceKind::Reference),
        prior_occurrences_with_notes: context
            .previous_meetings
            .items
            .iter()
            .filter(|occurrence| !occurrence.sources.is_empty())
            .count()
            + context.previous_meetings.omitted,
        series_notes: context.series_notes.items.len() + context.series_notes.omitted,
        open_commitments: commitments.iter().filter(|item| !item.candidate).count(),
        candidate_commitments: commitments.iter().filter(|item| item.candidate).count(),
        latest_correspondence_at: context
            .conversations
            .items
            .iter()
            .map(|conversation| conversation.last_message_at.clone())
            .max(),
        scope_required: context.conversations.status == SectionStatus::ScopeRequired,
        follow_through: match context.follow_through.status {
            SectionStatus::Unavailable => FollowThroughState::Unknown,
            _ if context.follow_through.items.is_empty() => FollowThroughState::NotReviewed,
            _ => FollowThroughState::Reviewed,
        },
        unavailable_sections: unavailable,
        error: None,
    }
}

fn unreadable(reference: &NoteReference, error: &DatabaseError) -> MeetingReadiness {
    log::warn!("Meeting readiness failed: {error}");
    MeetingReadiness {
        reference: reference.clone(),
        event_available: false,
        has_occurrence_note: false,
        prior_occurrences_with_notes: 0,
        series_notes: 0,
        open_commitments: 0,
        candidate_commitments: 0,
        latest_correspondence_at: None,
        scope_required: false,
        follow_through: FollowThroughState::Unknown,
        unavailable_sections: Vec::new(),
        error: Some("This meeting couldn't be checked on this device.".into()),
    }
}

impl Database {
    /// Every address the user sends or is invited as: mail accounts, their
    /// sending identities, and calendar accounts.
    fn meeting_own_addresses(&self) -> DbResult<HashSet<String>> {
        let mut own = HashSet::new();
        for account in self.list_accounts()? {
            own.insert(normalize_contact_address(&account.email));
            for identity in self.account_sending_identities(&account.email)? {
                own.insert(normalize_contact_address(&identity.address));
            }
        }
        self.with_connection(|c| {
            let mut q = c.prepare("SELECT email FROM calendar_accounts")?;
            for email in q.query_map([], |r| r.get::<_, String>(0))? {
                own.insert(normalize_contact_address(&email?));
            }
            Ok(())
        })?;
        Ok(own)
    }

    fn checked_occurrence(reference: &NoteReference) -> DbResult<()> {
        if reference.kind != "event" || !reference.validate() {
            return Err(DatabaseError::invalid("Invalid event reference"));
        }
        Ok(())
    }

    /// The local context for preparing one meeting occurrence. `mail_scope`
    /// names the mail accounts to look in; `None` uses the calendar's own
    /// account when it is also a mail account.
    pub fn meeting_context(
        &self,
        reference: &NoteReference,
        mail_scope: Option<&[String]>,
        now: DateTime<Utc>,
    ) -> DbResult<MeetingContext> {
        Self::checked_occurrence(reference)?;
        let own = self.meeting_own_addresses()?;
        self.with_connection(|c| {
            build(
                c,
                &Inputs {
                    reference,
                    requested_scope: mail_scope,
                    own_addresses: &own,
                    now,
                    include_promises: true,
                },
            )
        })
    }

    /// Counts for up to [`policy::MAX_READINESS_EVENTS`] meetings, such as a
    /// day's. Each event uses its calendar's own mail account unless
    /// `mail_scope` names accounts. One meeting that can't be read reports
    /// its own error without failing the others.
    pub fn meeting_readiness(
        &self,
        references: &[NoteReference],
        mail_scope: Option<&[String]>,
        now: DateTime<Utc>,
    ) -> DbResult<Vec<MeetingReadiness>> {
        if references.len() > policy::MAX_READINESS_EVENTS {
            return Err(DatabaseError::invalid(format!(
                "At most {} meetings can be checked at once",
                policy::MAX_READINESS_EVENTS
            )));
        }
        for reference in references {
            Self::checked_occurrence(reference)?;
        }
        let own = self.meeting_own_addresses()?;
        self.with_connection(|c| {
            Ok(references
                .iter()
                .map(|reference| {
                    let inputs = Inputs {
                        reference,
                        requested_scope: mail_scope,
                        own_addresses: &own,
                        now,
                        include_promises: false,
                    };
                    match build(c, &inputs) {
                        Ok(context) => readiness(&context),
                        Err(error) => unreadable(reference, &error),
                    }
                })
                .collect())
        })
    }
}

#[cfg(test)]
#[path = "tests/meetings.rs"]
mod tests;
