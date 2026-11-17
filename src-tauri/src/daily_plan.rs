//! The plan for a day: a `## Plan` section in that day's daily note. Pure text
//! functions over the stored Markdown; the database layer owns versions,
//! conflicts, and saving. `src/dailyPlan.ts` mirrors these for the demo client,
//! and `fixtures/daily-plan.json` holds both to the same output.
//!
//! A plan item is a top-level list item in the section, outside its
//! `### Not today` subsection. It links an existing task, a note task, or a
//! contact, is a round task typed while planning, or is free text. Decisions
//! about unfinished work are inert trailing comments, such as
//! `<!-- threestrands-plan:deferred 2026-11-20 -->`, never live HTML.
use crate::meeting_excerpt::{self, Line};
use crate::note_entities::{self, NoteReference};
use crate::note_meetings::escape_inline;
use serde::{Deserialize, Serialize};

pub const PLAN_HEADING: &str = "Plan";
pub const NOT_TODAY_HEADING: &str = "Not today";
/// The subsection of the plan where Today's evening view appends notes on the day.
pub const SHUTDOWN_HEADING: &str = "Shutdown";
/// Longest note on the day one save appends, in characters; `MAX_SHUTDOWN_NOTE_CHARS` in `src/dailyPlan.ts`.
pub const MAX_SHUTDOWN_NOTE_CHARS: usize = 2000;
const MARKER: &str = "<!-- threestrands-plan:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanItemKind {
    /// A link to a mail or standalone task.
    Task,
    /// A link to a round task in another note.
    NoteTask,
    /// A round task written in the plan itself.
    NewTask,
    /// A link to a contact, for a check-in.
    Contact,
    /// Any other list item: shown, not tracked.
    Text,
}

/// A decision recorded on a plan line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "date")]
pub enum PlanMarker {
    /// Carried here from the plan of that date.
    Carried(String),
    /// Set aside until that date (in `### Not today`).
    Deferred(String),
    /// Removed from planning; the task itself is unchanged.
    Dropped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanLine {
    pub kind: PlanItemKind,
    /// What the item links; for a round task in the plan, the note task.
    pub reference: Option<NoteReference>,
    pub label: String,
    /// A round task's marker id (persisted or provisional) and state.
    pub marker_id: Option<String>,
    pub checked: bool,
    pub marker: Option<PlanMarker>,
    /// Byte range of the item and its nested content; `end` excludes the
    /// final line break.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSection {
    /// The heading line's start, and the end of the section's last line.
    pub start: usize,
    pub end: usize,
    pub items: Vec<PlanLine>,
    pub not_today: Vec<PlanLine>,
}

fn is_date(value: &str) -> bool {
    value.len() == 10 && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

/// An ATX heading's level and normalized text: indented at most three spaces.
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

/// Each line with whether it is inside fenced code, and its heading if any.
fn scan<'a>(lines: &'a [Line<'a>]) -> Vec<(bool, Option<(usize, String)>)> {
    let mut fence: Option<(u8, usize)> = None;
    lines
        .iter()
        .map(|line| {
            let indent = meeting_excerpt::indent(line.text);
            if let Some((mark, run, trailing)) = meeting_excerpt::fence_run(line.text) {
                match fence {
                    Some((ch, n)) if ch == mark && run >= n && !trailing => {
                        fence = None;
                        return (true, None);
                    }
                    Some(_) => return (true, None),
                    None if indent <= 3 => {
                        fence = Some((mark, run));
                        return (true, None);
                    }
                    None => {}
                }
            }
            if fence.is_some() {
                (true, None)
            } else {
                (false, heading(line.text))
            }
        })
        .collect()
}

fn marker(line: &str) -> Option<PlanMarker> {
    let at = line.rfind(MARKER)?;
    let rest = line[at + MARKER.len()..].strip_suffix(" -->")?;
    match rest.split_once(' ') {
        Some(("carried", date)) if is_date(date) => Some(PlanMarker::Carried(date.into())),
        Some(("deferred", date)) if is_date(date) => Some(PlanMarker::Deferred(date.into())),
        None if rest == "dropped" => Some(PlanMarker::Dropped),
        _ => None,
    }
}

/// The plan section of `body`, or `None` without a `## Plan` heading outside code.
pub fn parse_plan(note: &str, body: &str) -> Result<Option<PlanSection>, String> {
    let lines = meeting_excerpt::lines(body);
    let scanned = scan(&lines);
    let Some(head) = scanned
        .iter()
        .position(|(_, h)| matches!(h, Some((2, key)) if key == &PLAN_HEADING.to_lowercase()))
    else {
        return Ok(None);
    };
    let close = scanned[head + 1..]
        .iter()
        .position(|(_, h)| matches!(h, Some((level, _)) if *level <= 2))
        .map_or(lines.len(), |at| head + 1 + at);
    let not_today = scanned[head + 1..close]
        .iter()
        .position(|(_, h)| matches!(h, Some((3, key)) if key == &NOT_TODAY_HEADING.to_lowercase()))
        .map(|at| head + 1 + at);
    let not_today_close = not_today.map(|at| {
        scanned[at + 1..close]
            .iter()
            .position(|(_, h)| matches!(h, Some((level, _)) if *level <= 3))
            .map_or(close, |next| at + 1 + next)
    });
    let parsed = note_entities::parse(note, body)?;
    let collect = |from: usize, to: usize, skip: Option<(usize, usize)>| -> Vec<PlanLine> {
        let mut out = Vec::new();
        let mut at = from;
        while at < to {
            if skip.is_some_and(|(a, b)| at >= a && at < b) {
                at = skip.unwrap().1;
                continue;
            }
            let line = &lines[at];
            let (code, heading) = &scanned[at];
            let quoted = line.text.trim_start().starts_with('>');
            if *code
                || heading.is_some()
                || quoted
                || !meeting_excerpt::is_list_item(line.text)
                || meeting_excerpt::indent(line.text) > 3
            {
                at += 1;
                continue;
            }
            let end = meeting_excerpt::item_end(&lines, at)
                .unwrap_or_else(|| line.end())
                .min(lines[to - 1].end());
            let first = line.start..line.end();
            let link = parsed.links.iter().find(|link| first.contains(&link.start));
            let item = parsed.items.iter().find(|item| item.start == line.start);
            let (kind, reference, label) = match (link, item) {
                (Some(link), _)
                    if link.reference.kind == "task" && link.reference.note.is_some() =>
                {
                    (
                        PlanItemKind::NoteTask,
                        Some(link.reference.clone()),
                        link.label.clone(),
                    )
                }
                (Some(link), _) if link.reference.kind == "task" => (
                    PlanItemKind::Task,
                    Some(link.reference.clone()),
                    link.label.clone(),
                ),
                (Some(link), _) if link.reference.kind == "contact" => (
                    PlanItemKind::Contact,
                    Some(link.reference.clone()),
                    link.label.clone(),
                ),
                (_, Some(item)) if item.task => (
                    PlanItemKind::NewTask,
                    Some(NoteReference {
                        version: 1,
                        kind: "task".into(),
                        id: item.id.clone(),
                        account: None,
                        calendar: None,
                        note: Some(note.to_string()),
                        contact: None,
                        series: None,
                    }),
                    item.title.clone(),
                ),
                _ => {
                    let text = line.text.trim_start_matches([' ', '\t']);
                    let text = text.split_once([' ', '\t']).map_or("", |(_, rest)| rest);
                    let text = text.find(MARKER).map_or(text, |at| &text[..at]);
                    (PlanItemKind::Text, None, text.trim().to_string())
                }
            };
            out.push(PlanLine {
                kind,
                reference,
                label,
                marker_id: item.filter(|item| item.task).map(|item| item.id.clone()),
                checked: item.is_some_and(|item| item.checked),
                marker: marker(line.text),
                start: line.start,
                end,
            });
            // Nested lines belong to this item.
            while at < to && lines[at].start < end {
                at += 1;
            }
        }
        out
    };
    let skip = not_today.zip(not_today_close);
    let items = collect(head + 1, close, skip);
    let not_today_items = match (not_today, not_today_close) {
        (Some(at), Some(end)) => collect(at + 1, end, None),
        _ => Vec::new(),
    };
    Ok(Some(PlanSection {
        start: lines[head].start,
        end: lines[close - 1].end(),
        items,
        not_today: not_today_items,
    }))
}

/// What a plan line links or says.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum PlanAddition {
    /// A link to a task, note task, or contact, written with `label`.
    Link {
        reference: NoteReference,
        label: String,
    },
    /// A new round task in the plan.
    NewTask { title: String },
}

/// The Markdown list line for `addition`, with an optional decision marker.
pub fn plan_line(addition: &PlanAddition, marker: Option<&PlanMarker>) -> Result<String, String> {
    let mut line = match addition {
        PlanAddition::Link { reference, label } => {
            if !matches!(reference.kind.as_str(), "task" | "contact") {
                return Err("A plan can link only tasks and contacts".into());
            }
            let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
            format!(
                "- [{}]({})",
                escape_inline(if label.is_empty() {
                    &reference.id
                } else {
                    &label
                }),
                reference.href()?
            )
        }
        PlanAddition::NewTask { title } => {
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            if title.is_empty() {
                return Err("A new task needs a title".into());
            }
            format!("- ( ) {}", escape_inline(&title))
        }
    };
    match marker {
        Some(PlanMarker::Carried(date)) => line.push_str(&format!(" {MARKER}carried {date} -->")),
        Some(PlanMarker::Deferred(date)) => line.push_str(&format!(" {MARKER}deferred {date} -->")),
        Some(PlanMarker::Dropped) => line.push_str(&format!(" {MARKER}dropped -->")),
        None => {}
    }
    Ok(line)
}

/// The end of the last non-blank line in `lines[from..to]`, or `fallback`.
fn last_text(lines: &[Line<'_>], from: usize, to: usize, fallback: usize) -> usize {
    lines[from..to]
        .iter()
        .rev()
        .find(|line| !line.text.trim().is_empty())
        .map_or(fallback, |line| line.end())
}

/// `body` with a whole `## Plan` section (`block`, ending in a line break)
/// added before `## Meetings` when that exists, else at the end of the note.
fn add_plan_section(
    body: &str,
    lines: &[Line<'_>],
    scanned: &[(bool, Option<(usize, String)>)],
    block: &str,
) -> String {
    let meetings = scanned
        .iter()
        .position(|(_, h)| matches!(h, Some((2, key)) if key == "meetings"));
    match meetings {
        Some(at) => format!(
            "{}{block}\n{}",
            &body[..lines[at].start],
            &body[lines[at].start..]
        ),
        None => {
            let separator = if body.is_empty() || body.ends_with("\n\n") {
                ""
            } else if body.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            format!("{body}{separator}{block}")
        }
    }
}

/// `body` with `new_lines` added as plan items (or under `### Not today`),
/// after the existing ones. Without a `## Plan` section, the section is added
/// before `## Meetings` when that exists, else at the end of the note.
pub fn insert_plan_lines(
    note: &str,
    body: &str,
    new_lines: &[String],
    not_today: bool,
) -> Result<String, String> {
    if new_lines.is_empty() {
        return Ok(body.to_string());
    }
    if note_entities::parse(note, body)?.unclosed_fence {
        return Err("unclosed fence".into());
    }
    let joined = new_lines.join("\n");
    let lines = meeting_excerpt::lines(body);
    let scanned = scan(&lines);
    let Some(section) = parse_plan(note, body)? else {
        let mut block = format!("## {PLAN_HEADING}\n\n");
        if not_today {
            block.push_str(&format!("### {NOT_TODAY_HEADING}\n\n"));
        }
        block.push_str(&joined);
        block.push('\n');
        return Ok(add_plan_section(body, &lines, &scanned, &block));
    };
    let head = lines
        .iter()
        .position(|line| line.start == section.start)
        .unwrap_or(0);
    let close = lines
        .iter()
        .position(|line| line.start > section.end)
        .unwrap_or(lines.len());
    let not_today_head = (head + 1..close).find(
        |at| matches!(&scanned[*at].1, Some((3, key)) if key == &NOT_TODAY_HEADING.to_lowercase()),
    );
    let at = if not_today {
        match not_today_head {
            Some(heading_at) => section
                .not_today
                .last()
                .map_or(lines[heading_at].end(), |item| item.end),
            None => {
                let point = last_text(&lines, head, close, lines[head].end());
                return Ok(format!(
                    "{}\n\n### {NOT_TODAY_HEADING}\n\n{joined}{}",
                    &body[..point],
                    &body[point..]
                ));
            }
        }
    } else {
        match section.items.last() {
            Some(item) => item.end,
            None => {
                let point = lines[head].end();
                return Ok(format!("{}\n\n{joined}{}", &body[..point], &body[point..]));
            }
        }
    };
    let blank_first = not_today && section.not_today.is_empty();
    Ok(format!(
        "{}{}{joined}{}",
        &body[..at],
        if blank_first { "\n\n" } else { "\n" },
        &body[at..]
    ))
}

/// A note on the day as one Markdown paragraph: whitespace collapsed to
/// single spaces, inline syntax escaped as other inserted text is, and a
/// leading list, quote, or ordered-list marker escaped so it stays text.
/// Mirrored by `shutdownParagraph` in `src/dailyPlan.ts`.
pub fn shutdown_paragraph(text: &str) -> Result<String, String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return Err("Write something to add to the day's notes.".into());
    }
    if collapsed.chars().count() > MAX_SHUTDOWN_NOTE_CHARS {
        return Err(format!(
            "Shorten the notes to {MAX_SHUTDOWN_NOTE_CHARS} characters or fewer."
        ));
    }
    let escaped = escape_inline(&collapsed);
    let digits = escaped.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && escaped[digits..].starts_with(['.', ')']) {
        return Ok(format!("{}\\{}", &escaped[..digits], &escaped[digits..]));
    }
    Ok(if escaped.starts_with(['-', '+', '>']) {
        format!("\\{escaped}")
    } else {
        escaped
    })
}

/// `body` with a `### Shutdown` paragraph at the end of the Plan section: under
/// an existing `### Shutdown` subsection when there is one, else under a new
/// one after everything else in the section. A note without a `## Plan`
/// section gets one, placed as `insert_plan_lines` places it.
pub fn append_shutdown_note(note: &str, body: &str, text: &str) -> Result<String, String> {
    let paragraph = shutdown_paragraph(text)?;
    if note_entities::parse(note, body)?.unclosed_fence {
        return Err("unclosed fence".into());
    }
    let lines = meeting_excerpt::lines(body);
    let scanned = scan(&lines);
    let Some(section) = parse_plan(note, body)? else {
        let block = format!("## {PLAN_HEADING}\n\n### {SHUTDOWN_HEADING}\n\n{paragraph}\n");
        return Ok(add_plan_section(body, &lines, &scanned, &block));
    };
    let head = lines
        .iter()
        .position(|line| line.start == section.start)
        .unwrap_or(0);
    let close = lines
        .iter()
        .position(|line| line.start > section.end)
        .unwrap_or(lines.len());
    let shutdown = (head + 1..close).find(
        |at| matches!(&scanned[*at].1, Some((3, key)) if key == &SHUTDOWN_HEADING.to_lowercase()),
    );
    Ok(match shutdown {
        Some(at) => {
            let sub_close = (at + 1..close)
                .find(|next| matches!(&scanned[*next].1, Some((level, _)) if *level <= 3))
                .unwrap_or(close);
            let point = last_text(&lines, at, sub_close, lines[at].end());
            format!("{}\n\n{paragraph}{}", &body[..point], &body[point..])
        }
        None => {
            let point = last_text(&lines, head, close, lines[head].end());
            format!(
                "{}\n\n### {SHUTDOWN_HEADING}\n\n{paragraph}{}",
                &body[..point],
                &body[point..]
            )
        }
    })
}

/// One place in a rearranged plan: an existing item, by its index in
/// `PlanSection::items`, or a new line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Arranged {
    Existing(usize),
    New(String),
}

/// `body` with its plan items rearranged, item by item. The existing items
/// named in `order` fill, in that order, the places those same items held, so
/// text between items and items not named stay where they were; each item
/// moves with its nested content. A new line follows the item before it in
/// `order` (new lines first in `order` precede the first named item). Items
/// in `remove` are taken out with their nested content. Without a named item
/// left, new lines go after the remaining items, as `insert_plan_lines` puts
/// them.
pub fn arrange_plan_items(
    note: &str,
    body: &str,
    section: Option<&PlanSection>,
    order: &[Arranged],
    remove: &[usize],
) -> Result<String, String> {
    let empty = PlanSection {
        start: 0,
        end: 0,
        items: Vec::new(),
        not_today: Vec::new(),
    };
    let section = section.unwrap_or(&empty);
    let mut named: Vec<usize> = Vec::new();
    let mut leading: Vec<String> = Vec::new();
    let mut trailing: Vec<Vec<String>> = Vec::new();
    for entry in order {
        match entry {
            Arranged::Existing(index) => {
                if *index >= section.items.len() || named.contains(index) || remove.contains(index)
                {
                    return Err("That item is no longer in the plan".into());
                }
                named.push(*index);
                trailing.push(Vec::new());
            }
            Arranged::New(line) => match trailing.last_mut() {
                Some(after) => after.push(line.clone()),
                None => leading.push(line.clone()),
            },
        }
    }
    if remove.iter().any(|index| *index >= section.items.len()) {
        return Err("That item is no longer in the plan".into());
    }
    if named.is_empty() {
        let mut out = body.to_string();
        let mut gone: Vec<usize> = remove.to_vec();
        gone.sort_unstable();
        gone.dedup();
        // New lines take the first removed item's place, else follow the items left.
        let first = gone.first().copied().filter(|_| !leading.is_empty());
        for index in gone.into_iter().rev() {
            let item = &section.items[index];
            out = if Some(index) == first {
                format!(
                    "{}{}{}",
                    &out[..item.start],
                    leading.join("\n"),
                    &out[item.end..]
                )
            } else {
                remove_plan_line(&out, item.start, item.end)
            };
        }
        return if first.is_some() {
            Ok(out)
        } else {
            insert_plan_lines(note, &out, &leading, false)
        };
    }
    let mut slots: Vec<usize> = named.clone();
    slots.sort_unstable();
    // (start, end, replacement); `None` removes the item and its line break.
    let mut edits: Vec<(usize, usize, Option<String>)> = Vec::new();
    for (place, slot) in slots.iter().enumerate() {
        let moved = &section.items[named[place]];
        let mut parts: Vec<&str> = Vec::new();
        if place == 0 {
            parts.extend(leading.iter().map(String::as_str));
        }
        parts.push(&body[moved.start..moved.end]);
        parts.extend(trailing[place].iter().map(String::as_str));
        let item = &section.items[*slot];
        edits.push((item.start, item.end, Some(parts.join("\n"))));
    }
    let mut gone: Vec<usize> = remove.to_vec();
    gone.sort_unstable();
    gone.dedup();
    for index in gone {
        let item = &section.items[index];
        edits.push((item.start, item.end, None));
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut out = body.to_string();
    for (start, end, replacement) in edits {
        out = match replacement {
            Some(text) => format!("{}{text}{}", &out[..start], &out[end..]),
            None => remove_plan_line(&out, start, end),
        };
    }
    Ok(out)
}

/// `body` without the item at `start..end` and its line break.
pub fn remove_plan_line(body: &str, start: usize, end: usize) -> String {
    let after = if body[end..].starts_with('\n') {
        end + 1
    } else {
        end
    };
    let before = if after == end && start > 0 && body[..start].ends_with('\n') {
        start - 1
    } else {
        start
    };
    format!("{}{}", &body[..before], &body[after..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Fixtures {
        references: BTreeMap<String, NoteReference>,
        parse: Vec<ParseCase>,
        insert: Vec<InsertCase>,
        arrange: Vec<ArrangeCase>,
        shutdown: Vec<ShutdownCase>,
    }
    #[derive(Deserialize)]
    struct ShutdownCase {
        name: String,
        body: String,
        text: String,
        expected: String,
    }
    #[derive(Deserialize)]
    struct ArrangeCase {
        name: String,
        body: String,
        order: Vec<ArrangeEntry>,
        remove: Vec<usize>,
        expected: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum ArrangeEntry {
        Existing(usize),
        New(String),
    }
    #[derive(Deserialize)]
    struct ParseCase {
        name: String,
        body: String,
        /// `null` when the note has no plan section.
        expected: Option<Vec<ExpectedLine>>,
        #[serde(default, rename = "notToday")]
        not_today: Vec<ExpectedLine>,
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct ExpectedLine {
        kind: PlanItemKind,
        text: String,
        #[serde(default)]
        marker: Option<PlanMarker>,
        #[serde(default)]
        checked: bool,
    }
    #[derive(Deserialize)]
    struct InsertCase {
        name: String,
        body: String,
        lines: Vec<String>,
        #[serde(default, rename = "notToday")]
        not_today: bool,
        expected: String,
    }

    fn fill(text: &str, references: &BTreeMap<String, NoteReference>) -> String {
        let mut names: Vec<&String> = references.keys().collect();
        names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        names.into_iter().fold(text.to_string(), |text, name| {
            text.replace(&format!("{{{name}}}"), &references[name].href().unwrap())
        })
    }

    fn fixtures() -> Fixtures {
        serde_json::from_str(include_str!("../../fixtures/daily-plan.json")).unwrap()
    }

    #[test]
    fn shared_fixtures_parse_plan_items_and_decisions() {
        let fixtures = fixtures();
        for case in fixtures.parse {
            let body = fill(&case.body, &fixtures.references);
            let section = parse_plan("daily:2026-11-16", &body).unwrap();
            let lines = |items: &[PlanLine]| -> Vec<ExpectedLine> {
                items
                    .iter()
                    .map(|item| ExpectedLine {
                        kind: item.kind,
                        text: body[item.start..item.end].to_string(),
                        marker: item.marker.clone(),
                        checked: item.checked,
                    })
                    .collect()
            };
            let expected = case.expected.map(|items| {
                items
                    .into_iter()
                    .map(|line| ExpectedLine {
                        text: fill(&line.text, &fixtures.references),
                        ..line
                    })
                    .collect::<Vec<_>>()
            });
            assert_eq!(
                section.as_ref().map(|s| lines(&s.items)),
                expected,
                "{}",
                case.name
            );
            let not_today: Vec<ExpectedLine> = case
                .not_today
                .into_iter()
                .map(|line| ExpectedLine {
                    text: fill(&line.text, &fixtures.references),
                    ..line
                })
                .collect();
            assert_eq!(
                section.map(|s| lines(&s.not_today)).unwrap_or_default(),
                not_today,
                "{} (not today)",
                case.name
            );
        }
    }

    #[test]
    fn shared_fixtures_insert_plan_lines_without_touching_other_text() {
        let fixtures = fixtures();
        for case in fixtures.insert {
            let body = fill(&case.body, &fixtures.references);
            let lines: Vec<String> = case
                .lines
                .iter()
                .map(|line| fill(line, &fixtures.references))
                .collect();
            let actual = insert_plan_lines("daily:2026-11-16", &body, &lines, case.not_today)
                .unwrap_or_else(|error| format!("ERROR: {error}"));
            assert_eq!(
                actual,
                fill(&case.expected, &fixtures.references),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn shared_fixtures_arrange_plan_items_item_by_item() {
        let fixtures = fixtures();
        for case in fixtures.arrange {
            let body = fill(&case.body, &fixtures.references);
            let section = parse_plan("daily:2026-11-16", &body).unwrap();
            let order: Vec<Arranged> = case
                .order
                .into_iter()
                .map(|entry| match entry {
                    ArrangeEntry::Existing(at) => Arranged::Existing(at),
                    ArrangeEntry::New(line) => Arranged::New(fill(&line, &fixtures.references)),
                })
                .collect();
            let actual = arrange_plan_items(
                "daily:2026-11-16",
                &body,
                section.as_ref(),
                &order,
                &case.remove,
            )
            .map_or_else(|_| "ERROR".to_string(), |body| body);
            assert_eq!(
                actual,
                fill(&case.expected, &fixtures.references),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn notes_on_the_day_match_the_shared_fixtures() {
        for case in fixtures().shutdown {
            let actual = append_shutdown_note("daily:2026-11-16", &case.body, &case.text)
                .unwrap_or_else(|_| "ERROR".to_string());
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn a_note_on_the_day_is_bounded_and_never_starts_a_list() {
        let longest = "x".repeat(MAX_SHUTDOWN_NOTE_CHARS);
        assert_eq!(shutdown_paragraph(&longest).unwrap(), longest);
        assert!(shutdown_paragraph(&format!("{longest}x")).is_err());
        assert_eq!(shutdown_paragraph("+ plus").unwrap(), "\\+ plus");
        assert_eq!(shutdown_paragraph("> quoted").unwrap(), "\\> quoted");
        assert_eq!(shutdown_paragraph("12) twelve").unwrap(), "12\\) twelve");
        assert_eq!(
            shutdown_paragraph("2026 was fine").unwrap(),
            "2026 was fine"
        );
        // Appending twice leaves the plan's own items alone.
        let once = append_shutdown_note("n", "## Plan\n\n- a\n", "One.").unwrap();
        let twice = append_shutdown_note("n", &once, "Two.").unwrap();
        let section = parse_plan("n", &twice).unwrap().unwrap();
        assert_eq!(section.items.len(), 1);
        assert!(section.not_today.is_empty());
    }

    #[test]
    fn plan_lines_link_escape_and_mark() {
        let task = NoteReference {
            version: 1,
            kind: "task".into(),
            id: "t1".into(),
            account: None,
            calendar: None,
            note: None,
            contact: None,
            series: None,
        };
        let line = plan_line(
            &PlanAddition::Link {
                reference: task.clone(),
                label: "Send *Q3* deck".into(),
            },
            Some(&PlanMarker::Carried("2026-11-15".into())),
        )
        .unwrap();
        assert_eq!(
            line,
            format!(
                "- [Send \\*Q3\\* deck]({}) <!-- threestrands-plan:carried 2026-11-15 -->",
                task.href().unwrap()
            )
        );
        assert_eq!(
            plan_line(
                &PlanAddition::NewTask {
                    title: " Draft   memo ".into()
                },
                None
            )
            .unwrap(),
            "- ( ) Draft memo"
        );
        assert!(plan_line(&PlanAddition::NewTask { title: "  ".into() }, None).is_err());
        let event = NoteReference {
            kind: "event".into(),
            account: Some("a@b.c".into()),
            calendar: Some("c".into()),
            ..task
        };
        assert!(plan_line(
            &PlanAddition::Link {
                reference: event,
                label: "x".into()
            },
            None
        )
        .is_err());
    }

    #[test]
    fn removing_a_plan_line_takes_only_that_item() {
        let body = "## Plan\n\n- a\n  - nested\n- b\n";
        let section = parse_plan("n", body).unwrap().unwrap();
        let first = &section.items[0];
        assert_eq!(
            remove_plan_line(body, first.start, first.end),
            "## Plan\n\n- b\n"
        );
        let last = &section.items[1];
        assert_eq!(
            remove_plan_line(body, last.start, last.end),
            "## Plan\n\n- a\n  - nested\n"
        );
        assert_eq!(remove_plan_line("## Plan\n\n- only", 9, 15), "## Plan\n");
    }
}
