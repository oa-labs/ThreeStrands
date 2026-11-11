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
    for line in prose_lines(body) {
        for span in line_links(line) {
            if links.len() == MAX_LINKS_PER_NOTE {
                return links;
            }
            links.push(NoteLink {
                key: title_key(&span.title),
                title: span.title,
                // As text, so a backlink never shows link targets or syntax.
                context: line_context(&plain_text(line)),
            });
        }
    }
    links
}

/// `body` with every link to the note whose title key is `old_key` pointed
/// at `new_title` instead, keeping any `|shown text`; `None` when no link
/// changes. Code and escaped links stay as written, as in [`extract_links`].
pub(crate) fn rewrite_links(body: &str, old_key: &str, new_title: &str) -> Option<String> {
    let mut changed = false;
    let mut fence: Option<(char, usize)> = None;
    let mut out = String::with_capacity(body.len());
    for piece in body.split_inclusive('\n') {
        let line = piece.trim_end_matches('\n').trim_end_matches('\r');
        let ending = &piece[line.len()..];
        if !in_prose(line, &mut fence) {
            out.push_str(piece);
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut next = 0;
        for span in line_links(line) {
            if title_key(&span.title) != old_key {
                continue;
            }
            out.extend(&chars[next..span.target_start]);
            out.push_str(new_title);
            next = span.target_end;
            changed = true;
        }
        out.extend(&chars[next..]);
        out.push_str(ending);
    }
    changed.then_some(out)
}

/// The lines of `body` outside fenced code blocks.
fn prose_lines(body: &str) -> impl Iterator<Item = &str> {
    let mut fence: Option<(char, usize)> = None;
    body.lines().filter(move |line| in_prose(line, &mut fence))
}

/// Whether `line` is prose, tracking fenced code blocks through `fence`.
/// Fence lines themselves aren't prose.
fn in_prose(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    if let Some(marker) = fence_marker(line) {
        match *fence {
            None => {
                *fence = Some(marker);
                return false;
            }
            Some((ch, length)) if marker.0 == ch && marker.1 >= length => {
                *fence = None;
                return false;
            }
            Some(_) => {}
        }
    }
    fence.is_none()
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

/// One link in a line: its title, and the character range of the target as
/// written (between `[[` and `|` or `]]`).
struct LinkSpan {
    title: String,
    target_start: usize,
    target_end: usize,
}

fn line_links(line: &str) -> Vec<LinkSpan> {
    let chars: Vec<char> = line.chars().collect();
    let mut links = Vec::new();
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
            if let Some((link, end)) = link_at(&chars, index + 2) {
                links.push(link);
                index = end;
                continue;
            }
        }
        index += 1;
    }
    links
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

/// The link starting at `start` (just past `[[`), and the index just past
/// its `]]`.
fn link_at(chars: &[char], start: usize) -> Option<(LinkSpan, usize)> {
    let mut index = start;
    while index + 1 < chars.len() {
        match chars[index] {
            '[' => return None,
            ']' if chars[index + 1] == ']' => {
                let target_end = chars[start..index]
                    .iter()
                    .position(|ch| *ch == '|')
                    .map_or(index, |offset| start + offset);
                let target: String = chars[start..target_end].iter().collect();
                let title = target.split_whitespace().collect::<Vec<_>>().join(" ");
                if title.is_empty() || title.chars().count() > MAX_NOTE_TITLE_CHARS {
                    return None;
                }
                let link = LinkSpan {
                    title,
                    target_start: start,
                    target_end,
                };
                return Some((link, index + 2));
            }
            ']' => return None,
            _ => index += 1,
        }
    }
    None
}

pub(crate) fn line_context(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() <= MAX_LINK_CONTEXT_CHARS {
        return line.to_string();
    }
    let mut context: String = line.chars().take(MAX_LINK_CONTEXT_CHARS - 1).collect();
    context.push('…');
    context
}

/// A note's Markdown as the plain text a list shows under its title:
/// front matter, fences, rules, and table dividers dropped; heading,
/// quote, list, and task markers removed; links shown as their text; and
/// emphasis, code, and escape characters removed. Lines are joined with
/// single spaces.
pub(crate) fn plain_text(body: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut lines = body.lines().peekable();
    if lines.peek().map(|line| line.trim_end()) == Some("---") {
        lines.next();
        for line in lines.by_ref() {
            if matches!(line.trim_end(), "---" | "...") {
                break;
            }
        }
    }
    for line in lines {
        let line = line.trim();
        if fence_marker(line).is_some() || is_rule(line) || is_table_divider(line) {
            continue;
        }
        let text = inline_plain(strip_block_markers(line));
        words.extend(text.split_whitespace().map(str::to_string));
    }
    words.join(" ")
}

/// `---`, `***`, or `___` (three or more, spaces allowed): a horizontal rule.
fn is_rule(line: &str) -> bool {
    let marks: String = line.chars().filter(|ch| !ch.is_whitespace()).collect();
    marks.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|mark| marks.chars().all(|ch| ch.to_string() == *mark))
}

/// A table's `| --- | :-: |` row.
fn is_table_divider(line: &str) -> bool {
    line.contains('-')
        && line.contains('|')
        && line.chars().all(|ch| matches!(ch, '|' | '-' | ':' | ' '))
}

/// The line without its quote, heading, list, and task markers.
fn strip_block_markers(mut line: &str) -> &str {
    loop {
        let before = line;
        line = line.trim_start();
        if let Some(rest) = line.strip_prefix('>') {
            line = rest;
        }
        let hashes = line.chars().take_while(|ch| *ch == '#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            line = &line[hashes..];
        }
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = line.strip_prefix(marker) {
                line = rest;
            }
        }
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") ")) {
            line = &line[digits + 2..];
        }
        for task in ["[ ] ", "[x] ", "[X] ", "[] ", "( ) ", "(x) ", "(X) ", "() "] {
            if let Some(rest) = line.strip_prefix(task) {
                line = rest;
            }
        }
        if line == before {
            return line;
        }
    }
}

/// Next matching punctuation at each character, so malformed, bracket-heavy
/// source remains linear rather than rescanning the rest of a long line.
fn next_positions(chars: &[char], needle: &[char]) -> Vec<usize> {
    let mut positions = vec![chars.len(); chars.len() + 1];
    let mut next = chars.len();
    for i in (0..chars.len()).rev() {
        if chars[i..].starts_with(needle) {
            next = i;
        }
        positions[i] = next;
    }
    positions
}

/// Inline Markdown as its visible text.
pub(crate) fn inline_plain(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let squares = next_positions(&chars, &[']']);
    let parens = next_positions(&chars, &[')']);
    let angles = next_positions(&chars, &['>']);
    let protocols = next_positions(&chars, &[':', '/', '/']);
    let mut out = String::with_capacity(line.len());
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        match ch {
            '\\' if chars
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_punctuation()) =>
            {
                out.push(chars[index + 1]);
                index += 2;
            }
            '[' if chars.get(index + 1) == Some(&'[') => {
                if let Some((span, end)) = link_at(&chars, index + 2) {
                    let inner: String = chars[index + 2..end - 2].iter().collect();
                    let shown = inner
                        .split_once('|')
                        .map_or(span.title.clone(), |(_, shown)| shown.trim().to_string());
                    out.push_str(if shown.is_empty() {
                        &span.title
                    } else {
                        &shown
                    });
                    index = end;
                } else {
                    out.push(ch);
                    index += 1;
                }
            }
            '!' if chars.get(index + 1) == Some(&'[') => index += 1,
            '[' => match markdown_link(&chars, index, &squares, &parens) {
                Some((text, end)) => {
                    out.push_str(&inline_plain(&text));
                    index = end;
                }
                None => {
                    out.push(ch);
                    index += 1;
                }
            },
            // An HTML comment, such as the size the editor keeps after a
            // resized image, isn't text.
            '<' if chars[index..].starts_with(&['<', '!', '-', '-']) => {
                index = (index + 4..chars.len().saturating_sub(2))
                    .find(|at| chars[*at..].starts_with(&['-', '-', '>']))
                    .map_or(chars.len(), |at| at + 3);
            }
            '<' if protocols[index + 1] < angles[index + 1] => {
                let end = angles[index + 1];
                out.extend(&chars[index + 1..end]);
                index = (end + 1).min(chars.len());
            }
            '*' | '~' | '=' | '`'
                if chars.get(index + 1) == Some(&ch) || matches!(ch, '*' | '`') =>
            {
                index += chars[index..].iter().take_while(|c| **c == ch).count();
            }
            '_' => {
                // Underscores mark emphasis at word edges; inside a word (snake_case) they're text.
                let run = chars[index..].iter().take_while(|c| **c == '_').count();
                let before = index.checked_sub(1).map(|at| chars[at]);
                let after = chars.get(index + run).copied();
                let in_word = before.is_some_and(char::is_alphanumeric)
                    && after.is_some_and(char::is_alphanumeric);
                if in_word {
                    out.extend(&chars[index..index + run]);
                }
                index += run;
            }
            '|' => {
                out.push(' ');
                index += 1;
            }
            _ => {
                out.push(ch);
                index += 1;
            }
        }
    }
    out
}

/// `[text](destination)` starting at `start`: the text, and the index past `)`.
fn markdown_link(
    chars: &[char],
    start: usize,
    squares: &[usize],
    parens: &[usize],
) -> Option<(String, usize)> {
    let close = squares[start + 1];
    if close == chars.len() || chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = parens[close + 2];
    if end == chars.len() {
        return None;
    }
    Some((chars[start + 1..close].iter().collect(), end + 1))
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
        assert_eq!(links[0].context, "Met Ada Lovelace about the plan.");
        let meeting = extract_links(
            "- [3:00 pm](threestrands:event/%7B%22id%22%3A%22e%22%7D) met with [Dana](threestrands:contact/%7B%7D) for [[OA Monthly]]",
        );
        assert_eq!(meeting[0].context, "3:00 pm met with Dana for OA Monthly");
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
        // The limit counts the text shown, not the Markdown behind it.
        let exact = "x".repeat(MAX_LINK_CONTEXT_CHARS - 2) + " [[a]]";
        assert_eq!(
            extract_links(&exact)[0].context,
            "x".repeat(MAX_LINK_CONTEXT_CHARS - 2) + " a"
        );
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
    fn plain_text_drops_markdown_syntax() {
        let body = "---\ntitle: x\n---\n## The Basics\n- **Dates (LOCKED):** Friday\n  * _Lodging_ ~~old~~ ==new== `code` see [VRBO](https://vrbo.com) <https://a.b>\n> quoted\n1. [ ] task [[Ada Lovelace|Ada]] [[Plan]] ![photo](x.png)\n---\n```js\nlet snake_case = 1;\n```\n| a | b |\n| --- | :-: |\n| 1 | 2 |\n\\*literal\\* 3 * 4";
        assert_eq!(
            plain_text(body),
            "The Basics Dates (LOCKED): Friday Lodging old new code see VRBO https://a.b quoted task Ada Plan photo let snake_case = 1; a b 1 2 *literal* 3 4"
        );
        assert_eq!(
            plain_text("#hashtag and # heading"),
            "#hashtag and # heading"
        );
        assert_eq!(plain_text(""), "");
        assert_eq!(
            plain_text("- () Round task\n- (X) Done\n- [] Checklist"),
            "Round task Done Checklist"
        );
        assert_eq!(
            plain_text("See ![](attachment:0123456789abcdef0123456789abcdef)<!-- {\"width\":320} --> and [Plan.pdf](attachment:0123456789abcdef0123456789abcdef) <!-- open"),
            "See and Plan.pdf"
        );
    }

    // The same cases as src/noteWikiLinks.test.ts, which the browser demo follows.
    #[test]
    fn inline_code_ends_only_at_a_backtick_run_of_the_same_length() {
        let body = "x ```code ````` [[Inside]] `` [[Still]] ``` [[After]]\n`` a ` [[Also inside]] `` [[Out]]";
        assert_eq!(titles(body), ["After", "Out"]);
        assert_eq!(titles("`` never closed [[Seen]]"), ["Seen"]);
    }

    #[test]
    fn title_keys_fold_unicode_whitespace_and_case() {
        assert_eq!(title_key("  Café\u{85}PLAN\u{3000}x "), "café plan x");
        assert_eq!(title_key("a\u{feff}b"), "a\u{feff}b");
        assert_eq!(title_key("INBOX Ideas"), "inbox ideas");
    }

    #[test]
    fn rewriting_links_follows_a_rename_and_keeps_shown_text() {
        let body = "See [[Project  plan]] and [[project plan|the plan]].\r\nNot [[Other]] or `[[Project plan]]`, nor \\[[Project plan]].\n```\n[[Project plan]]\n```\nLast [[PROJECT PLAN]]";
        assert_eq!(
            rewrite_links(body, &title_key("Project plan"), "Roadmap").as_deref(),
            Some("See [[Roadmap]] and [[Roadmap|the plan]].\r\nNot [[Other]] or `[[Project plan]]`, nor \\[[Project plan]].\n```\n[[Project plan]]\n```\nLast [[Roadmap]]")
        );
        assert_eq!(rewrite_links(body, &title_key("Nothing"), "Roadmap"), None);
        assert_eq!(
            rewrite_links("Über [[Café]] ✓\n", &title_key("café"), "Bar").as_deref(),
            Some("Über [[Bar]] ✓\n")
        );
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
