//! What Add to Note writes: a quoted passage, a link to its conversation,
//! and links to copied files, as Markdown appended to a note. Pure text
//! work; `db::note_additions` owns versions, conflicts, and saving, and
//! `src/addToNotePolicy.ts` mirrors the limits for the dialog.

use crate::note_meetings::escape_inline;

/// Most characters of quoted text one addition holds. A passage longer than
/// this is refused rather than cut, so a quote is never silently shortened.
pub const MAX_ADDITION_QUOTE_CHARS: usize = 20_000;
/// Most files one addition copies.
pub const MAX_ADDITION_FILES: usize = 20;

/// A file copied into the note, as its Markdown names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedLink {
    pub id: String,
    pub name: String,
    pub image: bool,
}

/// Refuses a quote or file list over its limit.
pub fn check_limits(quote: Option<&str>, files: usize) -> Result<(), String> {
    if quote.is_some_and(|text| text.chars().count() > MAX_ADDITION_QUOTE_CHARS) {
        return Err(format!(
            "That passage is longer than {MAX_ADDITION_QUOTE_CHARS} characters. Select less of the message."
        ));
    }
    if files > MAX_ADDITION_FILES {
        return Err(format!(
            "A note can take at most {MAX_ADDITION_FILES} files at a time."
        ));
    }
    Ok(())
}

/// `text` as a Markdown block quote: every line prefixed, blank lines kept
/// as bare markers, and no line able to open or close a code fence inside it.
/// A `>` that starts a line of the text nests as a quote inside this one.
pub fn format_quote(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.trim_matches('\n').split('\n').collect();
    let mut out = String::new();
    for line in lines {
        let line = line.trim_end();
        let fenced = line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
        if line.is_empty() {
            out.push_str(">\n");
        } else if fenced {
            let indent = line.len() - line.trim_start().len();
            out.push_str(&format!("> {}\\{}\n", &line[..indent], &line[indent..]));
        } else {
            out.push_str(&format!("> {line}\n"));
        }
    }
    out
}

/// A copied file as Markdown: an image shows in place, any other file is a link.
pub fn file_markdown(file: &AddedLink) -> String {
    let label = escape_inline(&file.name);
    let bang = if file.image { "!" } else { "" };
    format!("{bang}[{label}](attachment:{})", file.id)
}

/// The link to the conversation, labelled by its subject.
pub fn conversation_link(subject: &str, href: &str) -> String {
    let subject = subject.trim();
    let subject = if subject.is_empty() {
        "(No subject)"
    } else {
        subject
    };
    format!("[{}]({href})", escape_inline(subject))
}

/// The Markdown one addition writes, ending in a newline. `None` when there
/// is nothing to write: no passage, no files, and a link the note has.
///
/// - With a passage: the quote, then `> — [Subject](link) · date`.
/// - Without one: a bullet with the link (as Add to Today's Note wrote it),
///   or nothing when `link_present` and no file is added.
/// - Files follow after a blank line, on one line.
pub fn compose(
    quote: Option<&str>,
    subject: &str,
    href: &str,
    date_label: &str,
    files: &[AddedLink],
    link_present: bool,
) -> Option<String> {
    let link = conversation_link(subject, href);
    let mut block = String::new();
    match quote.filter(|text| !text.trim().is_empty()) {
        Some(text) => {
            block.push_str(&format_quote(text));
            let date = date_label.trim();
            let source = if date.is_empty() {
                format!("> — {link}\n")
            } else {
                format!("> — {link} · {}\n", escape_inline(date))
            };
            block.push_str(&source);
        }
        None if !link_present => block.push_str(&format!("- {link}\n")),
        None => {}
    }
    if !files.is_empty() {
        if !block.is_empty() {
            block.push('\n');
        }
        let line: Vec<String> = files.iter().map(file_markdown).collect();
        block.push_str(&line.join(" "));
        block.push('\n');
    }
    (!block.is_empty()).then_some(block)
}

/// `previous` with `block` at its end. A bullet follows the last line
/// directly, as Add to Today's Note appended it; anything else follows a
/// blank line so a quote never joins the paragraph above it.
pub fn append_block(previous: &str, block: &str) -> String {
    if previous.is_empty() {
        return block.to_string();
    }
    let mut body = previous.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    if !block.starts_with("- ") && !body.ends_with("\n\n") {
        body.push('\n');
    }
    body.push_str(block);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    const HREF: &str = "threestrands:conversation/x";

    fn file(name: &str, image: bool) -> AddedLink {
        AddedLink {
            id: "a".repeat(32),
            name: name.into(),
            image,
        }
    }

    #[test]
    fn quotes_every_line_and_keeps_blank_lines_and_inner_markers() {
        assert_eq!(format_quote("one\ntwo"), "> one\n> two\n");
        assert_eq!(format_quote("one\r\n\r\ntwo\n\n"), "> one\n>\n> two\n");
        assert_eq!(
            format_quote("a > b\n> already\n>> deeper"),
            "> a > b\n> > already\n> >> deeper\n"
        );
    }

    #[test]
    fn a_quoted_fence_cannot_open_a_code_block() {
        assert_eq!(format_quote("```\ncode\n~~~"), "> \\```\n> code\n> \\~~~\n");
    }

    #[test]
    fn writes_the_quote_source_line_and_files() {
        let block = compose(
            Some("Hello\nthere"),
            "Q3 [plan]",
            HREF,
            "Oct 30",
            &[file("a.png", true), file("r.pdf", false)],
            false,
        )
        .unwrap();
        assert_eq!(
            block,
            format!("> Hello\n> there\n> — [Q3 \\[plan\\]]({HREF}) · Oct 30\n\n![a.png](attachment:{id}) [r.pdf](attachment:{id})\n", id = "a".repeat(32))
        );
    }

    #[test]
    fn a_link_alone_is_a_bullet_and_is_skipped_when_present() {
        assert_eq!(
            compose(None, "", HREF, "Oct 30", &[], false).unwrap(),
            format!("- [(No subject)]({HREF})\n")
        );
        assert_eq!(compose(None, "Plan", HREF, "Oct 30", &[], true), None);
        let files_only = compose(None, "Plan", HREF, "", &[file("r.pdf", false)], true).unwrap();
        assert!(files_only.starts_with("[r.pdf]"));
    }

    #[test]
    fn a_blank_passage_is_the_link_alone() {
        assert_eq!(
            compose(Some("  \n "), "Plan", HREF, "", &[], false).unwrap(),
            format!("- [Plan]({HREF})\n")
        );
    }

    #[test]
    fn appends_after_a_blank_line_except_for_a_bullet() {
        assert_eq!(append_block("", "> q\n"), "> q\n");
        assert_eq!(append_block("text", "> q\n"), "text\n\n> q\n");
        assert_eq!(append_block("text\n", "> q\n"), "text\n\n> q\n");
        assert_eq!(append_block("text\n\n", "> q\n"), "text\n\n> q\n");
        assert_eq!(append_block("- a", "- [l](x)\n"), "- a\n- [l](x)\n");
    }

    #[test]
    fn limits_hold_below_at_and_above() {
        let quote = |n: usize| "é".repeat(n);
        assert!(check_limits(Some(&quote(MAX_ADDITION_QUOTE_CHARS - 1)), 0).is_ok());
        assert!(check_limits(Some(&quote(MAX_ADDITION_QUOTE_CHARS)), 0).is_ok());
        assert!(check_limits(Some(&quote(MAX_ADDITION_QUOTE_CHARS + 1)), 0).is_err());
        assert!(check_limits(None, MAX_ADDITION_FILES - 1).is_ok());
        assert!(check_limits(None, MAX_ADDITION_FILES).is_ok());
        assert!(check_limits(None, MAX_ADDITION_FILES + 1).is_err());
    }
}
