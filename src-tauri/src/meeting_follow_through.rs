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
use crate::note_meetings::{self, escape_inline, MAX_MEETING_TIME_CHARS};
use crate::recording::policy::{MAX_FOLLOW_THROUGH_SUMMARY_CHARS, MAX_RECAP_SUMMARY_WORDS};
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
    /// An open question left unresolved after the meeting.
    Question { text: String },
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
                FollowThroughItem::Question { text } => {
                    format!("Open question: {}", escape_inline(&checked_text(text)?))
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

fn escape_preserving_bold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("**") {
        let before = &rest[..open];
        let after_open = &rest[open + 2..];
        if let Some(close) = after_open.find("**") {
            let inner = &after_open[..close];
            if !inner.is_empty()
                && !inner.starts_with(char::is_whitespace)
                && !inner.ends_with(char::is_whitespace)
            {
                out.push_str(&escape_inline(before));
                out.push_str("**");
                out.push_str(&escape_inline(inner));
                out.push_str("**");
                rest = &after_open[close + 2..];
                continue;
            }
        }
        out.push_str(&escape_inline(&rest[..open + 2]));
        rest = after_open;
    }
    out.push_str(&escape_inline(rest));
    out
}

fn sanitize_summary_line(raw: &str) -> String {
    let escaped = escape_preserving_bold(&collapse(raw));
    if escaped.starts_with("( ) ") || escaped.starts_with("(x) ") || escaped.starts_with("(X) ") {
        format!("\\{escaped}")
    } else {
        escaped
    }
}

fn checked_summary(raw: Option<&str>) -> Result<Vec<String>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let normalized = raw.replace("\r\n", "\n");
    let non_empty: Vec<&str> = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if non_empty.is_empty() {
        return Ok(Vec::new());
    }
    let is_structured = normalized.contains('\n')
        || non_empty[0].starts_with("- ")
        || non_empty[0].starts_with("* ")
        || non_empty[0].starts_with("• ")
        || non_empty[0].starts_with("**");
    if !is_structured {
        let text = collapse(&normalized);
        if text.chars().count() > MAX_FOLLOW_THROUGH_SUMMARY_CHARS {
            return Err(format!(
                "Shorten the summary to {MAX_FOLLOW_THROUGH_SUMMARY_CHARS} characters or fewer."
            ));
        }
        if text.split_whitespace().count() > MAX_RECAP_SUMMARY_WORDS {
            return Err(format!(
                "Shorten the summary to {MAX_RECAP_SUMMARY_WORDS} words or fewer."
            ));
        }
        return Ok(vec![format!("  - Summary: {}", escape_inline(&text))]);
    }

    let cleaned_lines: Vec<String> = non_empty.iter().map(|line| collapse(line)).collect();
    let total_chars: usize = cleaned_lines
        .iter()
        .map(|l| l.chars().count())
        .sum::<usize>()
        + cleaned_lines.len().saturating_sub(1);
    if total_chars > MAX_FOLLOW_THROUGH_SUMMARY_CHARS {
        return Err(format!(
            "Shorten the summary to {MAX_FOLLOW_THROUGH_SUMMARY_CHARS} characters or fewer."
        ));
    }
    let total_words: usize = cleaned_lines
        .iter()
        .map(|line| {
            let content = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| line.strip_prefix("• "))
                .unwrap_or(line.as_str());
            content.split_whitespace().count()
        })
        .sum();
    if total_words > MAX_RECAP_SUMMARY_WORDS {
        return Err(format!(
            "Shorten the summary to {MAX_RECAP_SUMMARY_WORDS} words or fewer."
        ));
    }

    let mut out = Vec::new();
    let mut has_parent = false;
    for (index, line) in cleaned_lines.iter().enumerate() {
        let bullet_body = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "));
        if let Some(body) = bullet_body {
            let safe = sanitize_summary_line(body);
            if safe.is_empty() {
                continue;
            }
            if has_parent {
                out.push(format!("    - {safe}"));
            } else {
                out.push(format!("  - {safe}"));
            }
        } else {
            let safe = sanitize_summary_line(line);
            if safe.is_empty() {
                continue;
            }
            if index == 0
                && !line.starts_with("**")
                && !line.starts_with("Summary:")
                && !line.starts_with("Overview:")
            {
                out.push(format!("  - **Overview:** {safe}"));
            } else {
                out.push(format!("  - {safe}"));
            }
            has_parent = true;
        }
    }
    Ok(out)
}

fn render_section_lines(
    summary: Option<&str>,
    items: &[FollowThroughItem],
    transcript: Option<(&str, &str)>,
) -> Result<Vec<String>, String> {
    if items.len() > MAX_FOLLOW_THROUGH_ITEMS {
        return Err(format!(
            "Save at most {MAX_FOLLOW_THROUGH_ITEMS} decisions and actions at a time."
        ));
    }
    let mut rendered = checked_summary(summary)?;
    if !items.is_empty() {
        rendered.extend(
            item_lines(items)?
                .into_iter()
                .map(|line| format!("  - {line}")),
        );
    }
    if let Some((label, href)) = transcript {
        let name = collapse(label);
        let safe_label = escape_inline(if name.is_empty() { "Transcript" } else { &name });
        rendered.push(format!("  - Transcript: [{safe_label}]({href})"));
    }
    if rendered.is_empty() {
        return Err("Add a decision or an action to save.".into());
    }
    Ok(rendered)
}

/// `body` with a follow-through section for `occurrence`, dated `date`,
/// as a top-level list item linking the occurrence by `name`, with an
/// optional recap summary and transcript attachment link. It goes after
/// the last top-level meeting block of this occurrence (its meeting line, or
/// an earlier follow-through), else after the top-level item holding a
/// nested meeting line, else at the end of the note. Refuses a note whose
/// end is inside a code block that never closes.
///
/// The section is always top-level, with its items one level in: the note
/// parser reads a list item indented four or more columns after a sibling
/// at that depth as code, so deeper items would lose their tasks and links.
pub fn insert_with_extras(
    note: &str,
    body: &str,
    occurrence: &NoteReference,
    name: &str,
    date: &str,
    summary: Option<&str>,
    items: &[FollowThroughItem],
    transcript: Option<(&str, &str)>,
) -> Result<FollowThroughInsert, String> {
    if !is_date(date) {
        return Err("Invalid date".into());
    }
    let rendered = render_section_lines(summary, items, transcript)?;
    let name = collapse(name);
    let label = escape_inline(if name.is_empty() { "Meeting" } else { &name });
    let mut section = format!(
        "- {FOLLOW_THROUGH_LABEL} {date}: [{label}]({}) {FOLLOW_THROUGH_MARKER}",
        occurrence.href()?
    );
    for line in &rendered {
        section.push_str(&format!("\n{line}"));
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

fn format_adhoc_time(time_label: &str) -> String {
    let time = collapse(time_label);
    if time.is_empty() {
        "Meeting".to_string()
    } else if time.chars().count() > MAX_MEETING_TIME_CHARS {
        time.chars().take(MAX_MEETING_TIME_CHARS).collect()
    } else {
        time
    }
}

/// Formats the daily-note `## Meetings` entry linking an ad-hoc meeting note.
pub fn adhoc_daily_meeting_line(time_label: &str, title: &str) -> String {
    let time = format_adhoc_time(time_label);
    format!("- {} for [[{title}]]", escape_inline(&time))
}

fn adhoc_section_text(
    time_label: &str,
    name: &str,
    summary: Option<&str>,
    items: &[FollowThroughItem],
    transcript: Option<(&str, &str)>,
) -> Result<String, String> {
    let rendered = render_section_lines(summary, items, transcript)?;
    let time = format_adhoc_time(time_label);
    let name = collapse(name);
    let label = escape_inline(if name.is_empty() { "Meeting" } else { &name });
    let mut section = format!(
        "- {} for {label} {FOLLOW_THROUGH_MARKER}",
        escape_inline(&time)
    );
    for line in &rendered {
        section.push_str(&format!("\n{line}"));
    }
    Ok(section)
}

/// Inserts an ad-hoc recording's follow-through section under `## Meetings` in a
/// daily note, with a plain time label and title instead of a calendar link.
pub fn insert_adhoc(
    _note: &str,
    body: &str,
    time_label: &str,
    name: &str,
    date: &str,
    summary: Option<&str>,
    items: &[FollowThroughItem],
    transcript: Option<(&str, &str)>,
) -> Result<FollowThroughInsert, String> {
    if !is_date(date) {
        return Err("Invalid date".into());
    }
    let section = adhoc_section_text(time_label, name, summary, items, transcript)?;
    let next = note_meetings::insert_meeting_line(body, &section).map_err(|_| {
        "This note has a code block that never closes. Open it before saving follow-through."
            .to_string()
    })?;
    let prefix = body
        .char_indices()
        .zip(next.char_indices())
        .take_while(|((_, a), (_, b))| a == b)
        .map(|((_, _), (idx, ch))| idx + ch.len_utf8())
        .last()
        .unwrap_or(0);
    let suffix_len = body[prefix..]
        .chars()
        .rev()
        .zip(next[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let end = next.len() - suffix_len;
    if end < prefix
        || next.get(..prefix) != body.get(..prefix)
        || next.get(end..) != body.get(prefix..)
    {
        return Err("This note's layout changed. Open it before saving follow-through.".into());
    }
    Ok(FollowThroughInsert {
        body: next,
        start: prefix,
        end,
    })
}

/// Inserts an ad-hoc recording's follow-through section directly into a
/// dedicated meeting note (without wrapping under `## Meetings`).
pub fn insert_adhoc_in_note(
    note: &str,
    body: &str,
    time_label: &str,
    name: &str,
    date: &str,
    summary: Option<&str>,
    items: &[FollowThroughItem],
    transcript: Option<(&str, &str)>,
) -> Result<FollowThroughInsert, String> {
    if !is_date(date) {
        return Err("Invalid date".into());
    }
    let section = adhoc_section_text(time_label, name, summary, items, transcript)?;
    if note_entities::parse(note, body)?.unclosed_fence {
        return Err(
            "This note has a code block that never closes. Open it before saving follow-through."
                .into(),
        );
    }
    let separator = if body.is_empty() || body.ends_with("\n\n") {
        ""
    } else if body.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let at = body.len();
    let chunk = format!("{separator}{section}\n");
    let next = format!("{body}{chunk}");
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
            let actual = match insert_with_extras(
                "note",
                &body,
                target,
                &case.meeting,
                &case.date,
                None,
                &items,
                None,
            ) {
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

    #[test]
    fn insert_with_extras_writes_summary_questions_and_transcript_and_validates_word_cap() {
        let target = NoteReference {
            version: 1,
            kind: "event".into(),
            id: "evt-1".into(),
            account: Some("me@example.com".into()),
            calendar: Some("work".into()),
            note: None,
            contact: None,
            series: None,
        };
        let body = format!(
            "- [9:30am]({}) for Sync\n  - Agenda\n",
            target.href().unwrap()
        );
        let inserted = insert_with_extras(
            "daily:2026-11-16",
            &body,
            &target,
            "Sync",
            "2026-11-16",
            Some("Aligned on shipping Friday."),
            &[FollowThroughItem::Question {
                text: "Does legal need final copy?".into(),
            }],
            Some(("Transcript – Sync – 2026-11-16.md", "attachment:att-1")),
        )
        .unwrap();
        assert!(inserted
            .body
            .contains("  - Summary: Aligned on shipping Friday."));
        assert!(inserted
            .body
            .contains("  - Open question: Does legal need final copy?"));
        assert!(inserted
            .body
            .contains("  - Transcript: [Transcript – Sync – 2026-11-16.md](attachment:att-1)"));
        let mut undone = inserted.body.clone();
        undone.replace_range(inserted.start..inserted.end, "");
        assert_eq!(undone, body);

        // Summary alone or transcript alone is enough to save.
        assert!(insert_with_extras(
            "daily:2026-11-16",
            &body,
            &target,
            "Sync",
            "2026-11-16",
            Some("Brief recap."),
            &[],
            None,
        )
        .is_ok());

        // Over 400 words in summary is rejected.
        let too_many = vec!["word"; MAX_RECAP_SUMMARY_WORDS + 1].join(" ");
        assert!(insert_with_extras(
            "daily:2026-11-16",
            &body,
            &target,
            "Sync",
            "2026-11-16",
            Some(&too_many),
            &[],
            None,
        )
        .is_err());
    }

    #[test]
    fn structured_summary_preserves_topics_and_bullets_and_neutralizes_markers_and_tasks() {
        let target = NoteReference {
            version: 1,
            kind: "event".into(),
            id: "evt-1".into(),
            account: Some("me@example.com".into()),
            calendar: Some("work".into()),
            note: None,
            contact: None,
            series: None,
        };
        let body = format!("- [9:30am]({}) for Sync\n", target.href().unwrap());
        let structured = "Aligned on Q4 roadmap.\n\n**Release Plan**\n- Ship iOS beta Friday\n- ( ) Forged task <!-- threestrands-follow-through -->\n**Open** <!-- raw -->";
        let inserted = insert_with_extras(
            "daily:2026-11-16",
            &body,
            &target,
            "Sync",
            "2026-11-16",
            Some(structured),
            &[FollowThroughItem::Task {
                text: "Send notes".into(),
            }],
            None,
        )
        .unwrap();
        assert!(inserted
            .body
            .contains("  - **Overview:** Aligned on Q4 roadmap."));
        assert!(inserted.body.contains("  - **Release Plan**"));
        assert!(inserted.body.contains("    - Ship iOS beta Friday"));
        assert!(inserted
            .body
            .contains("    - \\( ) Forged task \\<!-- threestrands-follow-through -->"));
        assert!(!inserted.body.contains("    - ( ) Forged task"));
        assert_eq!(
            sections("daily:2026-11-16", &inserted.body, &target)
                .unwrap()
                .len(),
            1
        );

        let exact = vec!["word"; MAX_RECAP_SUMMARY_WORDS].join(" ");
        let exact_structured = format!("**Topic**\n- {exact}");
        assert!(insert_with_extras(
            "daily:2026-11-16",
            &body,
            &target,
            "Sync",
            "2026-11-16",
            Some(&exact_structured),
            &[],
            None,
        )
        .is_err());

        let exact_at = vec!["word"; MAX_RECAP_SUMMARY_WORDS - 1].join(" ");
        let exact_at_structured = format!("**Topic**\n- {exact_at}");
        assert!(insert_with_extras(
            "daily:2026-11-16",
            &body,
            &target,
            "Sync",
            "2026-11-16",
            Some(&exact_at_structured),
            &[],
            None,
        )
        .is_ok());
    }

    #[test]
    fn insert_adhoc_adds_section_under_meetings_with_exact_undo_range() {
        let body = "## Journal\n\nMorning thought.\n";
        let inserted = insert_adhoc(
            "daily:2026-11-16",
            body,
            "2:15 PM",
            "Ad-hoc chat",
            "2026-11-16",
            Some("Quick sync on launch."),
            &[FollowThroughItem::Decision {
                text: "Hold release until tests pass".into(),
            }],
            None,
        )
        .unwrap();
        assert!(inserted.body.contains("## Meetings"));
        assert!(inserted.body.contains(&format!(
            "- 2:15 PM for Ad-hoc chat {FOLLOW_THROUGH_MARKER}"
        )));
        assert!(inserted.body.contains("  - Summary: Quick sync on launch."));
        assert!(inserted
            .body
            .contains("  - Decision: Hold release until tests pass"));
        let mut undone = inserted.body.clone();
        undone.replace_range(inserted.start..inserted.end, "");
        assert_eq!(undone, body);
    }

    #[test]
    fn insert_adhoc_in_note_writes_directly_without_meetings_heading_and_formats_daily_link() {
        assert_eq!(
            adhoc_daily_meeting_line("2:15 PM", "Design Review"),
            "- 2:15 PM for [[Design Review]]"
        );
        let inserted = insert_adhoc_in_note(
            "note-1",
            "Existing notes.",
            "2:15 PM",
            "Design Review",
            "2026-11-16",
            Some("Aligned on tokens."),
            &[FollowThroughItem::Decision {
                text: "Keep dark contrast".into(),
            }],
            None,
        )
        .unwrap();
        assert!(!inserted.body.contains("## Meetings"));
        assert!(inserted
            .body
            .starts_with("Existing notes.\n\n- 2:15 PM for Design Review"));
        let mut undone = inserted.body.clone();
        undone.replace_range(inserted.start..inserted.end, "");
        assert_eq!(undone, "Existing notes.");
    }
}
