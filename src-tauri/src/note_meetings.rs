//! The line **Add Note** writes about a calendar event into a daily note, and
//! where it goes. Pure text functions over the stored Markdown; the database
//! layer owns versions, conflicts, and saving. `src/noteMeetings.ts` mirrors
//! these for the demo client, and `fixtures/note-meetings.json` holds both to
//! the same output.
use crate::note_entities::NoteReference;
use serde::Deserialize;
use threestrands_sync_protocol::{is_daily_note_date, MAX_NOTE_TITLE_CHARS};

/// The heading meeting lines are collected under.
pub const MEETINGS_HEADING: &str = "Meetings";
/// Most attendees one line names.
pub const MAX_MEETING_ATTENDEES: usize = 50;
/// Longest time label, such as "9:30am" or "All day", in characters.
pub const MAX_MEETING_TIME_CHARS: usize = 32;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeetingAttendee {
    pub label: String,
    pub reference: NoteReference,
}

/// What the user chose in the Add Note dialog.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeetingEntry {
    pub event: NoteReference,
    /// The start time as the agenda shows it, or "All day".
    pub time: String,
    pub attendees: Vec<MeetingAttendee>,
    /// The meeting name, which may differ from the event's title.
    pub name: String,
    /// Link a note named after the meeting rather than writing its name.
    pub link_note: bool,
}

/// Escapes Markdown punctuation in link labels and plain text, with the same
/// characters as the page's `markdownEntityLink`, and turns line breaks into
/// spaces.
pub fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\r' | '\n' => out.push(' '),
            '\\' | '`' | '*' | '_' | '[' | ']' | '{' | '}' | '(' | ')' | '#' | '+' | '.' | '!'
            | '|' | '<' | '>' | '~' | '-' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn link(label: &str, reference: &NoteReference) -> Result<String, String> {
    Ok(format!("[{}]({})", escape_inline(label), reference.href()?))
}

/// The title of the note a meeting links: the meeting name made valid as a
/// note title. `[`, `]`, and `|` become `-`, spacing collapses, the title is
/// cut to the title limit, a date gets " (meeting)", and an empty name is
/// "Meeting".
pub fn meeting_note_title(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|ch| match ch {
            '[' | ']' | '|' => '-',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(MAX_NOTE_TITLE_CHARS).collect();
    let title = cut.trim_end().to_string();
    if title.is_empty() {
        "Meeting".into()
    } else if is_daily_note_date(&title) {
        format!("{title} (meeting)")
    } else {
        title
    }
}

/// The meeting's line, without its list marker's trailing newline:
/// `- [9:30am](event) met with [Pete](contact), [Ann](contact) for Name`.
/// `title` is the meeting note's title when the name links one.
pub fn meeting_line(entry: &MeetingEntry, title: Option<&str>) -> Result<String, String> {
    if entry.event.kind != "event" || !entry.event.validate() {
        return Err("Invalid event reference".into());
    }
    let time = entry.time.split_whitespace().collect::<Vec<_>>().join(" ");
    if time.is_empty() || time.chars().count() > MAX_MEETING_TIME_CHARS {
        return Err("Invalid meeting time".into());
    }
    if entry.attendees.len() > MAX_MEETING_ATTENDEES {
        return Err(format!(
            "A meeting line can name at most {MAX_MEETING_ATTENDEES} attendees"
        ));
    }
    let mut line = format!("- {}", link(&time, &entry.event)?);
    let mut people = Vec::with_capacity(entry.attendees.len());
    for attendee in &entry.attendees {
        if attendee.reference.kind != "contact" || !attendee.reference.validate() {
            return Err("Invalid attendee".into());
        }
        let label = attendee
            .label
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let label = if label.is_empty() {
            attendee.reference.id.clone()
        } else {
            label
        };
        people.push(link(&label, &attendee.reference)?);
    }
    if !people.is_empty() {
        line.push_str(" met with ");
        line.push_str(&people.join(", "));
    }
    line.push_str(" for ");
    match title {
        Some(title) => line.push_str(&format!("[[{title}]]")),
        None => {
            let name = entry.name.split_whitespace().collect::<Vec<_>>().join(" ");
            line.push_str(&escape_inline(if name.is_empty() {
                "Meeting"
            } else {
                &name
            }));
        }
    }
    Ok(line)
}

/// A fenced code block's opening run, when `line` opens or closes one.
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let text = &line[indent..];
    let ch = text.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let size = text.chars().take_while(|c| *c == ch).count();
    (size >= 3).then(|| (ch, size, &text[size..]))
}

/// An ATX heading's level and normalized text.
fn heading(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let text = &line[indent..];
    let level = text.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &text[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut content = rest.trim();
    // A closing run of `#` preceded by a space is not part of the text.
    let without = content.trim_end_matches('#');
    if without.len() < content.len() && (without.is_empty() || without.ends_with([' ', '\t'])) {
        content = without.trim_end();
    }
    Some((
        level,
        content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
    ))
}

/// Why a meeting line can't be placed.
#[derive(Debug, PartialEq, Eq)]
pub enum InsertError {
    /// The insertion point is inside a code block that never closes.
    UnclosedFence,
}

/// `body` with `line` added at the end of its `## Meetings` section (the
/// first level-2 heading with that text outside code, up to the next heading
/// of level 1 or 2), after the section's last non-blank line. Without such a
/// heading, the heading and line are appended at the end of the note,
/// separated from earlier text by a blank line.
pub fn insert_meeting_line(body: &str, line: &str) -> Result<String, InsertError> {
    let key = MEETINGS_HEADING.to_lowercase();
    let mut fence: Option<(char, usize)> = None;
    let mut offset = 0;
    // (end of the last non-blank line in the section, whether it was the
    // heading, fence state after it)
    let mut section: Option<(usize, bool, bool)> = None;
    for piece in body.split_inclusive('\n') {
        let text = piece.trim_end_matches(['\n', '\r']);
        let end = offset + piece.len();
        offset = end;
        if let Some((ch, size, rest)) = fence_run(text) {
            match fence {
                None => fence = Some((ch, size)),
                Some((open, open_size))
                    if open == ch && size >= open_size && rest.trim().is_empty() =>
                {
                    fence = None
                }
                Some(_) => {}
            }
            if let Some(found) = section.as_mut() {
                *found = (end, false, fence.is_some());
            }
            continue;
        }
        if fence.is_none() {
            if let Some((level, content)) = heading(text) {
                if section.is_some() && level <= 2 {
                    break;
                }
                if section.is_none() && level == 2 && content == key {
                    section = Some((end, true, false));
                    continue;
                }
            }
        }
        if let Some(found) = section.as_mut() {
            if !text.trim().is_empty() {
                *found = (end, false, fence.is_some());
            }
        }
    }
    match section {
        Some((_, _, true)) => Err(InsertError::UnclosedFence),
        Some((at, after_heading, false)) => {
            let (before, after) = body.split_at(at);
            let newline = if before.ends_with('\n') { "" } else { "\n" };
            let gap = if after_heading { "\n" } else { "" };
            Ok(format!("{before}{newline}{gap}{line}\n{after}"))
        }
        None if fence.is_some() => Err(InsertError::UnclosedFence),
        None => {
            let separator = if body.is_empty() || body.ends_with("\n\n") {
                ""
            } else if body.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            Ok(format!(
                "{body}{separator}## {MEETINGS_HEADING}\n\n{line}\n"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixtures {
        insert: Vec<InsertFixture>,
        titles: Vec<(String, String)>,
        lines: Vec<LineFixture>,
    }
    #[derive(Deserialize)]
    struct InsertFixture {
        name: String,
        body: String,
        expected: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LineFixture {
        name: String,
        entry: MeetingEntry,
        title: Option<String>,
        expected: Option<String>,
    }
    fn fixtures() -> Fixtures {
        serde_json::from_str(include_str!("../../fixtures/note-meetings.json")).unwrap()
    }

    #[test]
    fn shared_insert_fixtures() {
        for fixture in fixtures().insert {
            let result = insert_meeting_line(&fixture.body, "- NEW").ok();
            assert_eq!(result, fixture.expected, "{}", fixture.name);
        }
    }
    #[test]
    fn shared_title_fixtures() {
        for (name, title) in fixtures().titles {
            assert_eq!(meeting_note_title(&name), title, "{name:?}");
            assert!(crate::notes::clean_note_title(&title).is_ok(), "{title:?}");
        }
    }
    #[test]
    fn shared_line_fixtures() {
        for fixture in fixtures().lines {
            let line = meeting_line(&fixture.entry, fixture.title.as_deref()).ok();
            assert_eq!(line, fixture.expected, "{}", fixture.name);
        }
    }
    #[test]
    fn attendee_count_boundary() {
        let attendee = |i: usize| MeetingAttendee {
            label: format!("P{i}"),
            reference: NoteReference {
                version: 1,
                kind: "contact".into(),
                id: format!("p{i}@example.com"),
                account: None,
                calendar: None,
                note: None,
                contact: None,
            },
        };
        let entry = |count: usize| MeetingEntry {
            event: NoteReference {
                version: 1,
                kind: "event".into(),
                id: "e".into(),
                account: Some("you@example.com".into()),
                calendar: Some("cal".into()),
                note: None,
                contact: None,
            },
            time: "9:30am".into(),
            attendees: (0..count).map(attendee).collect(),
            name: "Sync".into(),
            link_note: false,
        };
        assert!(meeting_line(&entry(MAX_MEETING_ATTENDEES - 1), None).is_ok());
        assert!(meeting_line(&entry(MAX_MEETING_ATTENDEES), None).is_ok());
        assert!(meeting_line(&entry(MAX_MEETING_ATTENDEES + 1), None).is_err());
        let mut long = entry(0);
        long.time = "x".repeat(MAX_MEETING_TIME_CHARS);
        assert!(meeting_line(&long, None).is_ok());
        long.time.push('x');
        assert!(meeting_line(&long, None).is_err());
    }
}
