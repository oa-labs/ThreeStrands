//! Finds the part of a note that is about one meeting. A meeting line is a
//! list item linking the occurrence, and its block is that item with its
//! nested content: neighboring entries, headings, and other text are never
//! part of it. A link outside a list item, or one whose block can't be bounded
//! safely, is only a reference: it can be opened, but its surrounding text is
//! not treated as the meeting's.
//!
//! Pure text functions over stored Markdown. Links are found by
//! [`note_entities::parse`], so code, comments, and escaped links follow the
//! same rules as backlinks. `src/meetingExcerpt.ts` mirrors this for the demo
//! client, and `fixtures/meeting-excerpts.json` holds both to the same output.
use crate::note_entities::{self, NoteReference};
use serde::Serialize;

/// How much of a meeting block is kept for one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingBlockKind {
    /// A list item linking the meeting, with its nested content.
    Excerpt,
    /// A link whose surrounding text isn't bounded as a meeting block: only
    /// its own line, for navigation.
    Reference,
}

/// A byte range of the note body. `end` excludes the final line break.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingBlock {
    pub kind: MeetingBlockKind,
    pub start: usize,
    pub end: usize,
}

pub(crate) struct Line<'a> {
    pub(crate) start: usize,
    pub(crate) text: &'a str,
}

impl Line<'_> {
    pub(crate) fn end(&self) -> usize {
        self.start + self.text.len()
    }
}

pub(crate) fn lines(body: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut offset = 0;
    for piece in body.split_inclusive('\n') {
        out.push(Line {
            start: offset,
            text: piece.trim_end_matches(['\r', '\n']),
        });
        offset += piece.len();
    }
    out
}

/// Leading whitespace in columns, with tabs to the next multiple of four, as
/// the note parser counts it.
pub(crate) fn indent(line: &str) -> usize {
    line.chars()
        .take_while(|ch| matches!(ch, ' ' | '\t'))
        .fold(0, |column, ch| {
            if ch == '\t' {
                column + 4 - column % 4
            } else {
                column + 1
            }
        })
}

pub(crate) fn is_list_item(line: &str) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    let bytes = rest.as_bytes();
    let marker = if matches!(bytes.first(), Some(b'-' | b'+' | b'*')) {
        1
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
            return false;
        }
        digits + 1
    };
    matches!(bytes.get(marker), Some(b' ' | b'\t'))
}

/// A fence opening or closing on this line: its character and run length,
/// and whether anything but whitespace follows the run.
pub(crate) fn fence_run(line: &str) -> Option<(u8, usize, bool)> {
    let text = line.trim_start_matches([' ', '\t']);
    let mark = *text.as_bytes().first()?;
    if !matches!(mark, b'`' | b'~') {
        return None;
    }
    let run = text.bytes().take_while(|b| *b == mark).count();
    (run >= 3).then(|| (mark, run, !text[run..].trim().is_empty()))
}

/// The end of the block of the list item on line `at`, or `None` when a
/// nested fence never closes and the block can't be bounded.
pub(crate) fn item_end(lines: &[Line<'_>], at: usize) -> Option<usize> {
    let item_indent = indent(lines[at].text);
    let mut end = lines[at].end();
    let mut fence: Option<(u8, usize)> = None;
    for line in &lines[at + 1..] {
        if let Some((mark, size)) = fence {
            if let Some((ch, run, trailing)) = fence_run(line.text) {
                if ch == mark && run >= size && !trailing {
                    fence = None;
                }
            }
            end = line.end();
            continue;
        }
        if line.text.trim().is_empty() {
            continue;
        }
        if indent(line.text) <= item_indent {
            break;
        }
        if let Some((mark, run, _)) = fence_run(line.text) {
            fence = Some((mark, run));
        }
        end = line.end();
    }
    fence.is_none().then_some(end)
}

/// The meeting blocks in `body` for links that `is_target` accepts, in text
/// order. Each line gives at most one block, and a link inside an earlier
/// block of the same note adds none.
pub fn meeting_blocks(
    note: &str,
    body: &str,
    is_target: impl Fn(&NoteReference) -> bool,
) -> Result<Vec<MeetingBlock>, String> {
    let parsed = note_entities::parse(note, body)?;
    let lines = lines(body);
    let mut blocks: Vec<MeetingBlock> = Vec::new();
    for link in parsed
        .links
        .iter()
        .filter(|link| is_target(&link.reference))
    {
        if blocks
            .iter()
            .any(|block| block.start <= link.start && link.start < block.end)
        {
            continue;
        }
        let Some(at) = lines
            .iter()
            .position(|line| line.start <= link.start && link.start < line.end())
        else {
            continue;
        };
        let line = &lines[at];
        let quoted = line.text.trim_start_matches([' ', '\t']).starts_with('>');
        let block = match (!quoted && is_list_item(line.text))
            .then(|| item_end(&lines, at))
            .flatten()
        {
            Some(end) => MeetingBlock {
                kind: MeetingBlockKind::Excerpt,
                start: line.start,
                end,
            },
            None => MeetingBlock {
                kind: MeetingBlockKind::Reference,
                start: line.start,
                end: line.end(),
            },
        };
        blocks.push(block);
    }
    Ok(blocks)
}

/// Blocks naming exactly this occurrence: the same account, calendar, and
/// event id. A link to the whole series or another occurrence doesn't count.
pub fn occurrence_blocks(
    note: &str,
    body: &str,
    occurrence: &NoteReference,
) -> Result<Vec<MeetingBlock>, String> {
    let target = occurrence.target()?;
    meeting_blocks(note, body, |reference| {
        reference.kind == "event" && reference.target().is_ok_and(|other| other == target)
    })
}

/// `text` cut to at most `limit` bytes at a character boundary, and whether
/// it was cut.
pub fn bounded_text(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_string(), false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// The indent that nests a new list item under the list item starting at
/// byte `start`: aligned with that item's text, so it nests under any marker.
pub(crate) fn child_indent(body: &str, start: usize) -> Result<String, String> {
    let item_line = body[start..].split('\n').next().unwrap_or_default();
    let leading = &item_line[..item_line.len() - item_line.trim_start_matches([' ', '\t']).len()];
    let rest = &item_line[leading.len()..];
    let marker = rest
        .find([' ', '\t'])
        .ok_or_else(|| "Invalid meeting line".to_string())?;
    let gap = rest[marker..].len() - rest[marker..].trim_start_matches([' ', '\t']).len();
    Ok(format!("{leading}{}", " ".repeat(marker + gap)))
}

/// What [`insert_prep_task`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum PrepInsert {
    /// The body with the prep task added under the meeting's line.
    Inserted(String),
    /// The meeting's block already holds an open or done prep task.
    Existing,
    /// No list item in the body is this occurrence's meeting line.
    NoMeetingLine,
}

/// The words a prep task starts with. Repeating Reserve Prep Time finds a
/// task under the meeting's line that starts with them, so a prep task the
/// user renamed to something else is no longer recognized.
pub const PREP_TASK_PREFIX: &str = "Prepare for ";

/// `body` with a round task `( ) Prepare for <name>` nested under the first
/// list item linking `occurrence`, after the item's last nested line. The
/// task is indented to the item's text, so it nests under any list marker.
pub fn insert_prep_task(
    note: &str,
    body: &str,
    occurrence: &NoteReference,
    name: &str,
) -> Result<PrepInsert, String> {
    let Some(block) = occurrence_blocks(note, body, occurrence)?
        .into_iter()
        .find(|block| block.kind == MeetingBlockKind::Excerpt)
    else {
        return Ok(PrepInsert::NoMeetingLine);
    };
    let parsed = note_entities::parse(note, body)?;
    let prefix = PREP_TASK_PREFIX.trim_end().to_lowercase();
    if parsed.items.iter().any(|item| {
        item.task
            && block.start < item.start
            && item.start < block.end
            && item.title.to_lowercase().starts_with(&prefix)
    }) {
        return Ok(PrepInsert::Existing);
    }
    let indent = child_indent(body, block.start)?;
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let title =
        crate::note_meetings::escape_inline(if name.is_empty() { "Meeting" } else { &name });
    Ok(PrepInsert::Inserted(format!(
        "{}\n{indent}- ( ) {PREP_TASK_PREFIX}{title}{}",
        &body[..block.end],
        &body[block.end..]
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Fixtures {
        events: BTreeMap<String, NoteReference>,
        target: String,
        cases: Vec<Case>,
    }
    #[derive(Deserialize)]
    struct Case {
        name: String,
        body: String,
        blocks: Vec<Expected>,
    }
    #[derive(Deserialize)]
    struct Expected {
        kind: String,
        text: String,
    }

    fn fill(text: &str, events: &BTreeMap<String, NoteReference>) -> String {
        // Longest names first, so `{A}` doesn't match inside `{A_OTHER_CALENDAR}`.
        let mut names: Vec<&String> = events.keys().collect();
        names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        names.into_iter().fold(text.to_string(), |text, name| {
            text.replace(&format!("{{{name}}}"), &events[name].href().unwrap())
        })
    }

    #[test]
    fn shared_fixtures_extract_only_the_meetings_own_block() {
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("../../fixtures/meeting-excerpts.json")).unwrap();
        let target = &fixtures.events[&fixtures.target];
        for case in fixtures.cases {
            let body = fill(&case.body, &fixtures.events);
            let blocks = occurrence_blocks("note", &body, target).unwrap();
            let actual: Vec<(String, String)> = blocks
                .iter()
                .map(|block| {
                    let kind = match block.kind {
                        MeetingBlockKind::Excerpt => "excerpt",
                        MeetingBlockKind::Reference => "reference",
                    };
                    (kind.to_string(), body[block.start..block.end].to_string())
                })
                .collect();
            let expected: Vec<(String, String)> = case
                .blocks
                .iter()
                .map(|block| (block.kind.clone(), fill(&block.text, &fixtures.events)))
                .collect();
            assert_eq!(actual, expected, "{}", case.name);
        }
    }

    #[derive(Deserialize)]
    struct PrepFixtures {
        events: BTreeMap<String, NoteReference>,
        target: String,
        prep: Vec<PrepCase>,
    }
    #[derive(Deserialize)]
    struct PrepCase {
        name: String,
        body: String,
        meeting: String,
        /// The new body, "EXISTING", or "NO_LINE".
        expected: String,
    }

    #[test]
    fn shared_fixtures_place_one_prep_task_under_the_meeting_line() {
        let fixtures: PrepFixtures =
            serde_json::from_str(include_str!("../../fixtures/meeting-excerpts.json")).unwrap();
        let target = &fixtures.events[&fixtures.target];
        for case in fixtures.prep {
            let body = fill(&case.body, &fixtures.events);
            let actual = match insert_prep_task("note", &body, target, &case.meeting).unwrap() {
                PrepInsert::Inserted(body) => body,
                PrepInsert::Existing => "EXISTING".into(),
                PrepInsert::NoMeetingLine => "NO_LINE".into(),
            };
            assert_eq!(
                actual,
                fill(&case.expected, &fixtures.events),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn a_series_link_is_not_this_occurrences_block() {
        let occurrence = NoteReference {
            version: 1,
            kind: "event".into(),
            id: "evt-a".into(),
            account: Some("you@example.com".into()),
            calendar: Some("work".into()),
            note: None,
            contact: None,
            series: Some("series".into()),
        };
        let series = NoteReference {
            id: "series".into(),
            series: None,
            ..occurrence.clone()
        };
        let body = format!(
            "- [Series]({})\n- [This one]({})\n",
            series.href().unwrap(),
            occurrence.href().unwrap()
        );
        let blocks = occurrence_blocks("note", &body, &occurrence).unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(body[blocks[0].start..blocks[0].end].starts_with("- [This one]"));
    }

    #[test]
    fn bounded_text_cuts_below_at_and_above_the_limit_on_character_boundaries() {
        assert_eq!(bounded_text("abc", 4), ("abc".into(), false));
        assert_eq!(bounded_text("abcd", 4), ("abcd".into(), false));
        assert_eq!(bounded_text("abcde", 4), ("abcd".into(), true));
        assert_eq!(bounded_text("aé", 2), ("a".into(), true));
    }
}
