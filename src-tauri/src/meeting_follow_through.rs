//! A meeting's follow-through: the decisions and actions the user reviewed
//! after it, saved as a dated list item in a note about that occurrence.
//! Pure text functions over stored Markdown; the database layer owns
//! versions, receipts, and saving. `src/meetingFollowThrough.ts` mirrors
//! these for the demo client, and `fixtures/meeting-follow-through.json`
//! holds both to the same output.
//!
//! The section is one list item whose line ends with the inert comment
//! [`FOLLOW_THROUGH_MARKER`], with one nested item per decision or action:
//!
//! ```text
//! - [9:30am](threestrands:event/…) for Sync
//!   - Agenda
//! - Follow-through 2026-11-16: [Sync](threestrands:event/…) <!-- threestrands-follow-through -->
//!   - Decision: Ship on Friday
//!   - ( ) Send the deck
//!   - [ ] Book the room
//!   - Action: Dana checks pricing
//!   - [Review the contract](threestrands:task/…)
//! ```
//!
//! The section's own line links the occurrence, so a note holding several
//! meetings still says which one it is about, and the section is a meeting
//! block of that occurrence. A section marker inside one of the
//! occurrence's meeting blocks is what makes the meeting count as reviewed:
//! saved note content, the same on every synced device. One the user moved
//! under the meeting's line still counts.
use crate::meeting_excerpt::{
    self, fence_run, indent, is_list_item, item_end, lines, MeetingBlockKind,
};
use crate::meeting_policy::{MAX_FOLLOW_THROUGH_ITEMS, MAX_FOLLOW_THROUGH_ITEM_CHARS};
use crate::note_entities::{self, NoteReference};
use crate::note_meetings::escape_inline;
use serde::{Deserialize, Serialize};

/// Ends the section's first line. Inert: an HTML comment the editor keeps.
pub const FOLLOW_THROUGH_MARKER: &str = "<!-- threestrands-follow-through -->";
/// The words the section's line starts with, before its date.
pub const FOLLOW_THROUGH_LABEL: &str = "Follow-through";

/// One decision or action the user accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum FollowThroughItem {
    /// Written as `Decision: …`, ordinary text.
    Decision { text: String },
    /// A round task, which Tasks lists.
    Task { text: String },
    /// A square checklist item, which stays in the note.
    Checklist { text: String },
    /// An action kept as information, such as someone else's commitment:
    /// written as `Action: …`, not tracked.
    Action { text: String },
    /// A link to an existing task instead of a new one.
    Link { task: NoteReference, title: String },
}

/// A follow-through section found in a note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowThroughSection {
    /// The date on its line, when it has one.
    pub date: Option<String>,
    /// Byte range of the item and its nested content; `end` excludes the
    /// final line break.
    pub start: usize,
    pub end: usize,
}

/// Where a section was added: the new body, and the byte range of the
/// inserted text (with the line break or blank line before it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowThroughInsert {
    pub body: String,
    pub start: usize,
    pub end: usize,
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The item's text, spaces collapsed, checked against the limits.
fn checked_text(text: &str) -> Result<String, String> {
    let text = collapse(text);
    if text.is_empty() {
        return Err("Each decision and action needs some text.".into());
    }
    if text.chars().count() > MAX_FOLLOW_THROUGH_ITEM_CHARS {
        return Err(format!(
            "Shorten each decision and action to {MAX_FOLLOW_THROUGH_ITEM_CHARS} characters or fewer."
        ));
    }
    Ok(text)
}

/// Checks the items against the limits and returns each one's Markdown,
/// without a list marker.
pub fn item_lines(items: &[FollowThroughItem]) -> Result<Vec<String>, String> {
    if items.is_empty() {
        return Err("Add a decision or an action to save.".into());
    }
    if items.len() > MAX_FOLLOW_THROUGH_ITEMS {
        return Err(format!(
            "Save at most {MAX_FOLLOW_THROUGH_ITEMS} decisions and actions at a time."
        ));
    }
    items
        .iter()
        .map(|item| {
            Ok(match item {
                FollowThroughItem::Decision { text } => {
                    format!("Decision: {}", escape_inline(&checked_text(text)?))
                }
                FollowThroughItem::Task { text } => {
                    format!("( ) {}", escape_inline(&checked_text(text)?))
                }
                FollowThroughItem::Checklist { text } => {
                    format!("[ ] {}", escape_inline(&checked_text(text)?))
                }
                FollowThroughItem::Action { text } => {
                    format!("Action: {}", escape_inline(&checked_text(text)?))
                }
                FollowThroughItem::Link { task, title } => {
                    if task.kind != "task" {
                        return Err("Only a task can be linked as an action.".into());
                    }
                    let title = collapse(title);
                    let label = if title.is_empty() {
                        "Task".into()
                    } else {
                        title
                    };
                    format!("[{}]({})", escape_inline(&label), task.href()?)
                }
            })
        })
        .collect()
}

fn is_date(value: &str) -> bool {
    value.len() == 10 && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

/// Whether `line` is a follow-through section's first line.
pub fn is_section_line(line: &str) -> bool {
    is_list_item(line)
        && line
            .trim_end()
            .strip_suffix(FOLLOW_THROUGH_MARKER)
            .is_some_and(|before| !before.ends_with('\\'))
}

/// The date written after the label, if any.
fn section_date(line: &str) -> Option<String> {
    let at = line.find(FOLLOW_THROUGH_LABEL)? + FOLLOW_THROUGH_LABEL.len();
    let candidate = line.get(at..)?.trim_start().get(..10)?;
    is_date(candidate).then(|| candidate.to_string())
}

/// The follow-through sections inside this occurrence's meeting blocks, in
/// text order. A marker in a code block, a quote, or outside every meeting
/// block of this occurrence doesn't count.
pub fn sections(
    note: &str,
    body: &str,
    occurrence: &NoteReference,
) -> Result<Vec<FollowThroughSection>, String> {
    let all = lines(body);
    let mut out: Vec<FollowThroughSection> = Vec::new();
    for block in meeting_excerpt::occurrence_blocks(note, body, occurrence)? {
        if block.kind != MeetingBlockKind::Excerpt {
            continue;
        }
        let Some(first) = all.iter().position(|line| line.start == block.start) else {
            continue;
        };
        let mut fence: Option<(u8, usize)> = None;
        for at in first..all.len() {
            let line = &all[at];
            if line.start >= block.end {
                break;
            }
            if let Some((mark, size)) = fence {
                if let Some((ch, run, trailing)) = fence_run(line.text) {
                    if ch == mark && run >= size && !trailing {
                        fence = None;
                    }
                }
                continue;
            }
            if let Some((mark, run, _)) = fence_run(line.text) {
                fence = Some((mark, run));
                continue;
            }
            if !is_section_line(line.text) || out.iter().any(|section| section.start == line.start)
            {
                continue;
            }
            if let Some(end) = item_end(&all, at) {
                out.push(FollowThroughSection {
                    date: section_date(line.text),
                    start: line.start,
                    end,
                });
            }
        }
    }
    out.sort_by_key(|section| section.start);
    Ok(out)
}

/// `body` with a follow-through section for `occurrence`, dated `date`,
/// as a top-level list item linking the occurrence by `name`. It goes after
/// the last top-level meeting block of this occurrence (its meeting line, or
/// an earlier follow-through), else after the top-level item holding a
/// nested meeting line, else at the end of the note. Refuses a note whose
/// end is inside a code block that never closes.
///
/// The section is always top-level, with its items one level in: the note
/// parser reads a list item indented four or more columns after a sibling
/// at that depth as code, so deeper items would lose their tasks and links.
pub fn insert(
    note: &str,
    body: &str,
    occurrence: &NoteReference,
    name: &str,
    date: &str,
    items: &[FollowThroughItem],
) -> Result<FollowThroughInsert, String> {
    if !is_date(date) {
        return Err("Invalid date".into());
    }
    let rendered = item_lines(items)?;
    let name = collapse(name);
    let label = escape_inline(if name.is_empty() { "Meeting" } else { &name });
    let mut section = format!(
        "- {FOLLOW_THROUGH_LABEL} {date}: [{label}]({}) {FOLLOW_THROUGH_MARKER}",
        occurrence.href()?
    );
    for line in &rendered {
        section.push_str(&format!("\n  - {line}"));
    }
    let all = lines(body);
    let blocks: Vec<_> = meeting_excerpt::occurrence_blocks(note, body, occurrence)?
        .into_iter()
        .filter(|block| block.kind == MeetingBlockKind::Excerpt)
        .collect();
    let position = |block_start: usize| all.iter().position(|line| line.start == block_start);
    let after_top_level = blocks
        .iter()
        .rev()
        .find(|block| position(block.start).is_some_and(|at| indent(all[at].text) == 0))
        .map(|block| block.end)
        .or_else(|| {
            // A nested meeting line: after the top-level item holding it.
            let block = blocks.first()?;
            let at = position(block.start)?;
            let top = (0..at)
                .rev()
                .find(|&index| is_list_item(all[index].text) && indent(all[index].text) == 0)?;
            item_end(&all, top).filter(|end| *end >= block.end)
        });
    let (at, chunk) = match after_top_level {
        Some(end) => (end, format!("\n{section}")),
        None => {
            if note_entities::parse(note, body)?.unclosed_fence {
                return Err("This note has a code block that never closes. Open it before saving follow-through.".into());
            }
            let separator = if body.is_empty() || body.ends_with("\n\n") {
                ""
            } else if body.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            (body.len(), format!("{separator}{section}\n"))
        }
    };
    let next = format!("{}{chunk}{}", &body[..at], &body[at..]);
    // The new text must read back as exactly one more section of this
    // occurrence, or the note's structure isn't what it seemed.
    let before = sections(note, body, occurrence)?.len();
    if sections(note, &next, occurrence)?.len() != before + 1 {
        return Err("This note's layout changed. Open it before saving follow-through.".into());
    }
    Ok(FollowThroughInsert {
        body: next,
        start: at,
        end: at + chunk.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_policy::MAX_FOLLOW_THROUGH_ITEM_CHARS;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Fixtures {
        #[serde(rename = "references")]
        events: BTreeMap<String, NoteReference>,
        target: String,
        sections: Vec<SectionCase>,
        insert: Vec<InsertCase>,
    }
    #[derive(Deserialize)]
    struct SectionCase {
        name: String,
        body: String,
        sections: Vec<ExpectedSection>,
    }
    #[derive(Deserialize)]
    struct ExpectedSection {
        date: Option<String>,
        text: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct InsertCase {
        name: String,
        body: String,
        meeting: String,
        date: String,
        items: Vec<FollowThroughItem>,
        /// The new body, or "ERROR: <message>".
        expected: String,
    }

    fn fill(text: &str, events: &BTreeMap<String, NoteReference>) -> String {
        let mut names: Vec<&String> = events.keys().collect();
        names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        names.into_iter().fold(text.to_string(), |text, name| {
            text.replace(&format!("{{{name}}}"), &events[name].href().unwrap())
        })
    }

    fn fixtures() -> Fixtures {
        serde_json::from_str(include_str!("../../fixtures/meeting-follow-through.json")).unwrap()
    }

    #[test]
    fn shared_fixtures_find_only_this_occurrences_sections() {
        let fixtures = fixtures();
        let target = &fixtures.events[&fixtures.target];
        for case in fixtures.sections {
            let body = fill(&case.body, &fixtures.events);
            let actual: Vec<(Option<String>, String)> = sections("note", &body, target)
                .unwrap()
                .into_iter()
                .map(|section| (section.date, body[section.start..section.end].to_string()))
                .collect();
            let expected: Vec<(Option<String>, String)> = case
                .sections
                .iter()
                .map(|section| (section.date.clone(), fill(&section.text, &fixtures.events)))
                .collect();
            assert_eq!(actual, expected, "{}", case.name);
        }
    }

    #[test]
    fn shared_fixtures_insert_one_section_without_touching_other_text() {
        let fixtures = fixtures();
        let target = &fixtures.events[&fixtures.target];
        for case in fixtures.insert {
            let body = fill(&case.body, &fixtures.events);
            let items: Vec<FollowThroughItem> = serde_json::from_str(&fill(
                &serde_json::to_string(&case.items).unwrap(),
                &fixtures.events,
            ))
            .unwrap();
            let actual = match insert("note", &body, target, &case.meeting, &case.date, &items) {
                Ok(inserted) => {
                    // Removing the inserted range gives back the original.
                    let mut undone = inserted.body.clone();
                    undone.replace_range(inserted.start..inserted.end, "");
                    assert_eq!(undone, body, "{}", case.name);
                    inserted.body
                }
                Err(error) => format!("ERROR: {error}"),
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
    fn item_text_is_limited_below_at_and_above_the_limit() {
        let at = "a".repeat(MAX_FOLLOW_THROUGH_ITEM_CHARS);
        let above = "a".repeat(MAX_FOLLOW_THROUGH_ITEM_CHARS + 1);
        assert!(item_lines(&[FollowThroughItem::Task { text: "a".into() }]).is_ok());
        assert!(item_lines(&[FollowThroughItem::Task { text: at }]).is_ok());
        assert!(item_lines(&[FollowThroughItem::Task { text: above }]).is_err());
        // Spaces collapse before counting.
        let spaced = format!("{}   ", "a".repeat(MAX_FOLLOW_THROUGH_ITEM_CHARS));
        assert!(item_lines(&[FollowThroughItem::Decision { text: spaced }]).is_ok());
    }

    #[test]
    fn item_count_is_limited_below_at_and_above_the_limit() {
        let items = |n: usize| -> Vec<FollowThroughItem> {
            (0..n)
                .map(|i| FollowThroughItem::Checklist {
                    text: format!("item {i}"),
                })
                .collect()
        };
        assert!(item_lines(&items(0)).is_err());
        assert!(item_lines(&items(MAX_FOLLOW_THROUGH_ITEMS - 1)).is_ok());
        assert!(item_lines(&items(MAX_FOLLOW_THROUGH_ITEMS)).is_ok());
        assert!(item_lines(&items(MAX_FOLLOW_THROUGH_ITEMS + 1)).is_err());
    }

    #[test]
    fn typed_text_cannot_become_markup_a_marker_or_a_different_item() {
        let lines = item_lines(&[
            FollowThroughItem::Action {
                text: "x <!-- threestrands-follow-through -->".into(),
            },
            FollowThroughItem::Decision {
                text: "[link](threestrands:task/x)\nsecond line".into(),
            },
        ])
        .unwrap();
        assert_eq!(lines[0], "Action: x \\<!-- threestrands-follow-through -->");
        assert!(!is_section_line(&format!("- {}", lines[0])));
        assert_eq!(
            lines[1],
            "Decision: \\[link\\](threestrands:task/x) second line"
        );
    }

    #[test]
    fn only_tasks_can_be_linked() {
        let contact = NoteReference {
            version: 1,
            kind: "contact".into(),
            id: "ann@example.com".into(),
            account: None,
            calendar: None,
            note: None,
            contact: None,
            series: None,
        };
        assert!(item_lines(&[FollowThroughItem::Link {
            task: contact,
            title: "Ann".into()
        }])
        .is_err());
    }
}
