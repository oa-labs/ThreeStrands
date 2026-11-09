//! Note rules that don't touch storage: titles, daily-note identity, and the
//! `[[wiki links]]` a note's Markdown contains. `db/notes.rs` stores notes
//! and the link index built from these.

use chrono::NaiveDate;
use threestrands_sync_protocol::{is_daily_note_date, DAILY_NOTE_ID_PREFIX, MAX_NOTE_TITLE_CHARS};

/// Most links one note's index keeps. A body at the size limit could hold
/// thousands of tiny links; past this many, later ones aren't indexed.
pub(crate) const MAX_LINKS_PER_NOTE: usize = 500;
/// Most characters of the surrounding line kept with a link, for showing
/// a backlink in context.
pub(crate) const MAX_LINK_CONTEXT_CHARS: usize = 160;

/// One `[[link]]` in a note: the title as written and its lookup key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteLink {
    pub title: String,
    pub key: String,
    pub context: String,
}

/// The key a title is matched by: whitespace collapsed and case folded, so
/// `[[project  plan]]` finds "Project Plan".
pub(crate) fn title_key(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `value` is a real calendar date written `YYYY-MM-DD`.
pub(crate) fn is_daily_date(value: &str) -> bool {
    is_daily_note_date(value) && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

/// The entity id of the daily note for `date`.
pub(crate) fn daily_note_id(date: &str) -> String {
    format!("{DAILY_NOTE_ID_PREFIX}{date}")
}

/// The title a regular note is saved with, or why it can't be used. Titles
/// can't contain the characters that end or split a `[[link]]`, and can't be
/// a date, which names that day's daily note.
pub(crate) fn clean_note_title(title: &str) -> Result<String, &'static str> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return Err("A note needs a title");
    }
    if title.chars().count() > MAX_NOTE_TITLE_CHARS {
        return Err("A note title can be at most 200 characters");
    }
    if title
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '[' | ']' | '|'))
    {
        return Err("A note title can't contain [, ], or |");
    }
    if is_daily_note_date(&title) {
        return Err("A date names that day's daily note; choose another title");
    }
    Ok(title)
}

/// Every `[[link]]` in `body`, in order, up to [`MAX_LINKS_PER_NOTE`].
/// `[[Title|shown text]]` links to Title. Links inside code (fenced blocks
/// and inline code spans) and escaped ones (`\[[`) are not links.
pub(crate) fn extract_links(body: &str) -> Vec<NoteLink> {
    let mut links = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        if let Some(marker) = fence_marker(line) {
            match fence {
                None => {
                    fence = Some(marker);
                    continue;
                }
                Some((ch, length)) if marker.0 == ch && marker.1 >= length => {
                    fence = None;
                    continue;
                }
                Some(_) => {}
            }
        }
        if fence.is_some() {
            continue;
        }
        for title in line_links(line) {
            if links.len() == MAX_LINKS_PER_NOTE {
                return links;
            }
            links.push(NoteLink {
                key: title_key(&title),
                title,
                context: line_context(line),
            });
        }
    }
    links
}

/// The fence character and length when `line` opens or closes a fenced code
/// block: at most three spaces, then three or more backticks or tildes.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = rest.chars().next().filter(|ch| matches!(ch, '`' | '~'))?;
    let length = rest.chars().take_while(|c| *c == ch).count();
    (length >= 3).then_some((ch, length))
}

fn line_links(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut titles = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '`' {
            let run = chars[index..].iter().take_while(|c| **c == '`').count();
            match closing_backticks(&chars, index + run, run) {
                Some(end) => index = end,
                None => index += run,
            }
            continue;
        }
        let escaped = index > 0 && chars[index - 1] == '\\';
        if !escaped && chars[index] == '[' && chars.get(index + 1) == Some(&'[') {
            if let Some((title, end)) = link_at(&chars, index + 2) {
                titles.push(title);
                index = end;
                continue;
            }
        }
        index += 1;
    }
    titles
}

/// The index just past the backtick run of exactly `run` that closes an
/// inline code span opened before `from`.
fn closing_backticks(chars: &[char], from: usize, run: usize) -> Option<usize> {
    let mut index = from;
    while index < chars.len() {
        if chars[index] == '`' {
            let length = chars[index..].iter().take_while(|c| **c == '`').count();
            if length == run {
                return Some(index + length);
            }
            index += length;
        } else {
            index += 1;
        }
    }
    None
}

/// The link title starting at `start` (just past `[[`), and the index just
/// past its `]]`.
fn link_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut index = start;
    while index + 1 < chars.len() {
        match chars[index] {
            '[' => return None,
            ']' if chars[index + 1] == ']' => {
                let inner: String = chars[start..index].iter().collect();
                let target = inner.split('|').next().unwrap_or_default();
                let title = target.split_whitespace().collect::<Vec<_>>().join(" ");
                if title.is_empty() || title.chars().count() > MAX_NOTE_TITLE_CHARS {
                    return None;
                }
                return Some((title, index + 2));
            }
            ']' => return None,
            _ => index += 1,
        }
    }
    None
}

fn line_context(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() <= MAX_LINK_CONTEXT_CHARS {
        return line.to_string();
    }
    let mut context: String = line.chars().take(MAX_LINK_CONTEXT_CHARS - 1).collect();
    context.push('…');
    context
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(body: &str) -> Vec<String> {
        extract_links(body)
            .into_iter()
            .map(|link| link.title)
            .collect()
    }

    #[test]
    fn finds_links_with_aliases_and_matches_titles_by_key() {
        let links = extract_links("Met [[Ada  Lovelace]] about [[Project Plan|the plan]].");
        assert_eq!(
            links.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["Ada Lovelace", "Project Plan"]
        );
        assert_eq!(links[1].key, "project plan");
        assert_eq!(title_key("  PROJECT \t plan "), "project plan");
        assert_eq!(
            links[0].context,
            "Met [[Ada  Lovelace]] about [[Project Plan|the plan]]."
        );
    }

    #[test]
    fn skips_code_escapes_and_malformed_links() {
        let body = "```\n[[In fence]]\n```\n~~~~\n[[Tilde]]\n```\nstill fenced [[No]]\n~~~~\n\
            `[[inline]]` and ``code ` [[double]]`` but [[Yes]]\n\
            \\[[Escaped]] [[]] [[ ]] [[a[b]] [[unclosed\n[[After]]";
        assert_eq!(titles(body), ["Yes", "After"]);
    }

    #[test]
    fn an_unclosed_inline_code_span_doesnt_hide_later_links() {
        assert_eq!(titles("a ` b [[Seen]]"), ["Seen"]);
    }

    #[test]
    fn link_count_and_context_are_bounded_at_their_limits() {
        let body = "[[a]] ".repeat(MAX_LINKS_PER_NOTE + 1);
        assert_eq!(extract_links(&body).len(), MAX_LINKS_PER_NOTE);
        let exact = "x".repeat(MAX_LINK_CONTEXT_CHARS - 5) + "[[a]]";
        assert_eq!(extract_links(&exact)[0].context, exact);
        let long = "x".repeat(MAX_LINK_CONTEXT_CHARS) + "[[a]]";
        let context = &extract_links(&long)[0].context;
        assert_eq!(context.chars().count(), MAX_LINK_CONTEXT_CHARS);
        assert!(context.ends_with('…'));
        let title = "t".repeat(MAX_NOTE_TITLE_CHARS);
        assert_eq!(
            titles(&format!("[[{title}]]")),
            std::slice::from_ref(&title)
        );
        assert!(titles(&format!("[[{title}t]]")).is_empty());
    }

    #[test]
    fn titles_are_cleaned_and_refuse_link_syntax_and_dates() {
        assert_eq!(clean_note_title("  Q4   plan ").unwrap(), "Q4 plan");
        assert_eq!(clean_note_title("C# notes").unwrap(), "C# notes");
        for bad in ["", "   ", "a[b", "a]b", "a|b", "2026-11-09"] {
            assert!(clean_note_title(bad).is_err(), "{bad:?}");
        }
        let at_limit = "t".repeat(MAX_NOTE_TITLE_CHARS);
        assert!(clean_note_title(&at_limit).is_ok());
        assert!(clean_note_title(&format!("{at_limit}t")).is_err());
    }

    #[test]
    fn daily_dates_must_be_real_days() {
        assert!(is_daily_date("2028-02-29"));
        assert!(!is_daily_date("2026-02-29"));
        assert!(!is_daily_date("2026-13-01"));
        assert!(!is_daily_date("26-11-09"));
        assert_eq!(daily_note_id("2026-11-09"), "daily:2026-11-09");
    }
}
