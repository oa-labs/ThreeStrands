//! Display cleanup for one-line message previews. Senders' plain-text parts
//! often carry Markdown (`[label](url)`, `**bold**`) and long raw URLs, which
//! read as noise in an inbox row. This only shapes preview text; the stored
//! body and the search index keep every character.

use std::collections::HashSet;

use crate::quoted_history::SEARCH_PREVIEW_CHARS;

/// What a preview knows about a message beyond its plain text.
#[derive(Default)]
pub struct PreviewContext {
    /// The thread's subject. An opening line that repeats it is dropped.
    pub subject: String,
    /// Words the message's HTML shows only as image alt or title text. A line
    /// of nothing else is a converter's spelled-out image, not message text.
    pub image_only_words: HashSet<String>,
}

/// Fewest words a subject needs before an opening line repeating it is
/// dropped, so a one- or two-word reply ("Thanks") is never mistaken for it.
pub const MIN_REPEATED_SUBJECT_WORDS: usize = 3;

/// Most words left on a line that ended in a raw URL for the line to count
/// as the URL's lead-in ("View this post on the web at", "View in browser").
pub const MAX_LINK_LEAD_IN_WORDS: usize = 8;

/// Shortest run of one rule character (`-`, `_`, `=`, `*`, `~`, `—`) that is
/// drawn as a divider rather than written as punctuation.
pub const MIN_RULE_RUN: usize = 3;

/// One line of preview for `text`, a message before any quoted history:
/// entity references decoded, HTML comments removed, readable lines (see
/// [`readable_preview`]) on one line with invisible padding collapsed, and
/// capped at `SEARCH_PREVIEW_CHARS`. Text that cleans away to nothing, such
/// as a body that is only a link, previews as written.
pub fn message_preview(text: &str, context: &PreviewContext) -> String {
    // Decoded before collapsing and capping, so entity padding collapses
    // like the characters it stands for and the cap never splits a reference.
    let text = strip_html_comments(&decode_entity_references(text));
    match collapse_preview(&readable_preview(&text, context)) {
        cleaned if cleaned.is_empty() => collapse_preview(&text),
        cleaned => cleaned,
    }
    .chars()
    .take(SEARCH_PREVIEW_CHARS)
    .collect()
}

/// `text` with Markdown link and emphasis syntax reduced to its words, URLs,
/// spelled-out images, and divider rules removed, and lines dropped that
/// only describe the message around it:
///
/// - a line of nothing but words the HTML shows only as image text;
/// - an opening line that repeats the subject, when text follows it;
/// - a short lead-in to a raw URL that ended its line, when the rest of the
///   text still fills the preview, so a short note that shares a link
///   keeps saying so.
///
/// Line structure is kept for the caller to collapse.
pub fn readable_preview(text: &str, context: &PreviewContext) -> String {
    let lines: Vec<(String, bool)> = text
        .lines()
        .map(|line| {
            let (line, ends_with_link) = strip_links(strip_heading(line));
            (tidy(&strip_emphasis(&strip_rules(&line))), ends_with_link)
        })
        .collect();
    let mut kept: Vec<bool> = lines
        .iter()
        .map(|(line, _)| {
            let words = words(line);
            !line.trim().is_empty() && (words.is_empty() || !words.iter().all(|word| context.image_only_words.contains(word)))
        })
        .collect();
    let subject = words(subject_text(&context.subject));
    if subject.len() >= MIN_REPEATED_SUBJECT_WORDS {
        if let Some(first) = (0..lines.len()).find(|&index| kept[index] && !words(&lines[index].0).is_empty()) {
            if words(&lines[first].0) == subject && kept[first + 1..].contains(&true) {
                kept[first] = false;
            }
        }
    }
    // A list item's words are its content, even when a link ends it.
    let lead_in = |(line, ends_with_link): &(String, bool)| {
        *ends_with_link && words(line).len() <= MAX_LINK_LEAD_IN_WORDS && !is_list_item(line)
    };
    let rest: Vec<&str> = lines
        .iter()
        .zip(&kept)
        .filter(|(line, &kept)| kept && !lead_in(line))
        .map(|((line, _), _)| line.as_str())
        .collect();
    if collapse_preview(&rest.join("\n")).chars().count() >= SEARCH_PREVIEW_CHARS {
        for (keep, line) in kept.iter_mut().zip(&lines) {
            *keep &= !lead_in(line);
        }
    }
    lines
        .into_iter()
        .zip(kept)
        .filter_map(|((line, _), kept)| kept.then_some(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `line` opens like a list item: a bullet (`-`, `*`, `•`) or a
/// number or letter followed by `.` or `)`.
fn is_list_item(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with(['-', '*', '•']) {
        return true;
    }
    let marker = line.split_whitespace().next().unwrap_or("");
    marker.len() > 1
        && marker.ends_with(['.', ')'])
        && (marker[..marker.len() - 1].chars().all(|c| c.is_ascii_digit())
            || (marker.len() == 2 && marker.starts_with(|c: char| c.is_ascii_alphabetic())))
}

/// Lowercased runs of letters and digits.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// `subject` without its reply and forward prefixes (`Re:`, `Fwd:`,
/// `RE[2]:`), however many are stacked.
fn subject_text(subject: &str) -> &str {
    let mut rest = subject.trim_start();
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(prefix) = ["re", "fwd", "fw"].into_iter().find(|prefix| lower.starts_with(prefix)) else {
            return rest;
        };
        let after = &rest[prefix.len()..];
        let after = match after.strip_prefix('[') {
            Some(count) => match count.find(']') {
                Some(end) if count[..end].chars().all(|c| c.is_ascii_digit()) => &count[end + 1..],
                _ => return rest,
            },
            None => after,
        };
        match after.trim_start().strip_prefix(':') {
            Some(next) => rest = next.trim_start(),
            None => return rest,
        }
    }
}

/// `text` without complete HTML comments (`<!-- … -->`) or conditional
/// markers (`<![endif]-->`), which some senders' plain-text parts copy from
/// their HTML. An unterminated comment stays as written.
fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = if rest.starts_with("<!--") {
            rest[4..].find("-->").map(|end| 4 + end + 3)
        } else if rest.starts_with("<![") {
            rest.find(']').and_then(|close| {
                let after = &rest[close + 1..];
                if after.starts_with("-->") {
                    Some(close + 4)
                } else if after.starts_with('>') {
                    Some(close + 2)
                } else {
                    None
                }
            })
        } else {
            None
        };
        match end {
            Some(end) => rest = &rest[end..],
            None => {
                out.push_str("<!");
                rest = &rest[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `line` with runs of `MIN_RULE_RUN` or more of one rule character
/// replaced by a space.
fn strip_rules(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let run = chars[i..].iter().take_while(|&&c| c == chars[i]).count();
        if matches!(chars[i], '-' | '_' | '=' | '*' | '~' | '—') && run >= MIN_RULE_RUN {
            out.push(' ');
        } else {
            out.extend(&chars[i..i + run]);
        }
        i += run;
    }
    out
}

/// Replaces named and numeric HTML entity references (`&amp;`, `&#847;`,
/// `&#x2007;`) with their characters; an unknown name, a bare `&`, or a
/// reference without its closing `;` stays as written. Senders' plain-text
/// parts often carry them, like `&zwnj;&nbsp;` preheader padding.
pub fn decode_entity_references(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        rest = &rest[start..];
        let reference_len = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .filter(|&end| end > 0 && rest[1 + end..].starts_with(';'))
            .map(|end| end + 2);
        match reference_len {
            Some(len) => {
                mail_parser::decoders::html::add_html_token(&mut decoded, &rest.as_bytes()[..len], false);
                rest = &rest[len..];
            }
            None => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// Characters that draw nothing on their own. Senders repeat them between
/// spaces as preheader padding so inbox previews stop early.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{034F}' | '\u{00AD}')
}

/// `text` on one line: whitespace runs (non-breaking spaces included) become
/// one space, and invisible characters at a word's edges are dropped, so a
/// run of padding disappears. Inside a word they stay, where joiners shape
/// emoji sequences and scripts like Persian.
pub fn collapse_preview(text: &str) -> String {
    text.split_whitespace()
        .map(|word| word.trim_matches(is_invisible))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// How many of a preheader's first words, in order, mark it as already in a
/// preview. Enough to be a phrase, not just a shared brand name.
pub const PREHEADER_OPENING_WORDS: usize = 5;

/// `preview` led by the sender's `preheader` (the hidden line written for
/// inbox previews), unless the preview already holds the preheader's
/// opening phrase or every one of its words, so a copy that differs only in
/// punctuation or what follows is not shown twice. Capped at
/// `SEARCH_PREVIEW_CHARS` like the preview itself.
pub fn with_preheader(preview: &str, preheader: &str) -> String {
    let preheader = collapse_preview(preheader);
    let shown: String = preview.chars().take(crate::quoted_history::SEARCH_PREVIEW_CHARS).collect();
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let (wanted, shown_words) = (words(&preheader), words(&shown));
    if wanted.is_empty() {
        return shown;
    }
    let opening = &wanted[..wanted.len().min(PREHEADER_OPENING_WORDS)];
    let holds_opening = shown_words.windows(opening.len()).any(|run| run == opening);
    let holds_all = wanted.iter().all(|word| shown_words.contains(word));
    if holds_opening || holds_all {
        return shown;
    }
    format!("{preheader} {preview}")
        .trim_end()
        .chars()
        .take(crate::quoted_history::SEARCH_PREVIEW_CHARS)
        .collect()
}

fn strip_heading(line: &str) -> &str {
    let trimmed = line.trim_start();
    let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
    match trimmed[hashes..].strip_prefix(' ') {
        Some(rest) if (1..=6).contains(&hashes) => rest,
        _ => line,
    }
}

fn is_url_start(chars: &[char]) -> bool {
    let starts = |prefix: &str| {
        prefix.len() <= chars.len()
            && prefix.chars().zip(chars).all(|(a, b)| a.eq_ignore_ascii_case(b))
    };
    starts("https://") || starts("http://") || starts("www.")
}

/// Index just past the URL starting at `start`, leaving trailing sentence
/// punctuation and closing brackets outside it.
fn url_end(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while end < chars.len() && !chars[end].is_whitespace() && !matches!(chars[end], '<' | '>') {
        end += 1;
    }
    while end > start && matches!(chars[end - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '\'' | '"') {
        end -= 1;
    }
    end
}

/// `[label](target)` → label, `![alt](target)` → alt, raw URLs (bare or in
/// `<...>`) removed, and converters' spelled-out images (`[image: Logo]`,
/// `[image]`, `[cid:image001.png@…]`) removed. A label that is itself a URL
/// is removed too. Also returns whether a raw URL ended the line, with
/// nothing but punctuation after it; a Markdown label is the link's own
/// text (often a headline), not a lead-in to it.
fn strip_links(line: &str) -> (String, bool) {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut link_end = None;
    let mut i = 0;
    while i < chars.len() {
        let at_word_start = i == 0 || !chars[i - 1].is_alphanumeric();
        let bracket = if chars[i] == '[' { Some(i) } else if chars[i] == '!' && chars.get(i + 1) == Some(&'[') { Some(i + 1) } else { None };
        if let Some(open) = bracket {
            if let Some((label, next)) = markdown_link(&chars, open) {
                if is_url_start(&label.chars().collect::<Vec<_>>()) {
                    link_end = Some(out.len());
                } else {
                    out.push_str(&label);
                }
                i = next;
                continue;
            }
            if let Some(next) = image_placeholder(&chars, open) {
                i = next;
                continue;
            }
        }
        if at_word_start && is_url_start(&chars[i..]) {
            i = url_end(&chars, i);
            link_end = Some(out.len());
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    let ends_with_link = link_end.is_some_and(|end| !out[end..].chars().any(char::is_alphanumeric));
    (out, ends_with_link)
}

/// The index past `]` for a converter's image placeholder at `open`:
/// `[image]`, `[image: alt]`, or `[cid:…]`.
fn image_placeholder(chars: &[char], open: usize) -> Option<usize> {
    let close = open + 1 + chars[open + 1..].iter().position(|&c| c == ']' || c == '[')?;
    if chars[close] != ']' {
        return None;
    }
    let inner: String = chars[open + 1..close].iter().collect::<String>().trim().to_lowercase();
    (inner == "image" || inner.starts_with("image:") || inner.starts_with("cid:")).then_some(close + 1)
}

/// The label and the index past `)` for a `[label](target)` at `open`. The
/// target must be one unbroken token, so prose in brackets is left alone.
fn markdown_link(chars: &[char], open: usize) -> Option<(String, usize)> {
    let close = open + 1 + chars[open + 1..].iter().position(|&c| c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let target_start = close + 2;
    let target_end = target_start + chars[target_start..].iter().position(|&c| c == ')')?;
    if target_end == target_start || chars[target_start..target_end].iter().any(|c| c.is_whitespace()) {
        return None;
    }
    Some((chars[open + 1..close].iter().collect(), target_end + 1))
}

/// Removes paired `**`, `__`, `*`, `_`, and `` ` `` delimiters that open at a
/// word start and close at a word end, so `snake_case` and `2 * 3` survive.
fn strip_emphasis(line: &str) -> String {
    let mut chars: Vec<char> = line.chars().collect();
    for delimiter in ["**", "__", "*", "_", "`"] {
        let delimiter: Vec<char> = delimiter.chars().collect();
        let width = delimiter.len();
        let mut i = 0;
        while i + width <= chars.len() {
            let opens = chars[i..i + width] == delimiter[..]
                && (i == 0 || !chars[i - 1].is_alphanumeric())
                && chars.get(i + width).is_some_and(|c| !c.is_whitespace());
            if !opens {
                i += 1;
                continue;
            }
            let closer = (i + width + 1..=chars.len().saturating_sub(width)).find(|&j| {
                chars[j..j + width] == delimiter[..]
                    && !chars[j - 1].is_whitespace()
                    && chars.get(j + width).is_none_or(|c| !c.is_alphanumeric())
            });
            match closer {
                Some(j) => {
                    chars.drain(j..j + width);
                    chars.drain(i..i + width);
                    i = j - width;
                }
                None => i += 1,
            }
        }
    }
    chars.into_iter().collect()
}

/// Drops brackets a removed URL left empty and the space it left before
/// punctuation.
fn tidy(line: &str) -> String {
    let mut out = line.to_string();
    loop {
        let before = out.clone();
        for empty in ["()", "[]", "<>", "( )", "[ ]", "< >"] {
            out = out.replace(empty, "");
        }
        for mark in ['.', ',', ';', ':', '!', '?', ')'] {
            out = out.replace(&format!(" {mark}"), &mark.to_string());
        }
        if out == before {
            return out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        collapse_preview, decode_entity_references, message_preview, readable_preview, with_preheader, PreviewContext,
        MAX_LINK_LEAD_IN_WORDS, MIN_REPEATED_SUBJECT_WORDS, MIN_RULE_RUN,
    };
    use crate::quoted_history::SEARCH_PREVIEW_CHARS;

    #[test]
    fn a_preheader_leads_a_preview_that_does_not_show_it() {
        assert_eq!(
            with_preheader("Your Play Points \u{2022} Gold 146", "Available through\u{00A0}Thursday. \u{200C} \u{034F}"),
            "Available through Thursday. Your Play Points \u{2022} Gold 146"
        );
        assert_eq!(with_preheader("", "Delivered 1 item: Drinkware"), "Delivered 1 item: Drinkware");
        let long = "x".repeat(SEARCH_PREVIEW_CHARS);
        let led = with_preheader(&long, "Delivered 1 item");
        assert!(led.starts_with("Delivered 1 item x"));
        assert_eq!(led.chars().count(), SEARCH_PREVIEW_CHARS);
    }

    #[test]
    fn a_preview_already_showing_the_preheader_is_unchanged() {
        for (preview, preheader) in [
            // The preheader is the first line of the text, as converters write it.
            ("Available through Thursday. Your Play Points", "Available through Thursday."),
            // Same words, different punctuation and case.
            ("AVAILABLE through thursday \u{2014} your points", "Available through Thursday."),
            // A copy that differs by a joined word, behind a brand name.
            ("Senja Capital for small businesses within 48 hours.Visit our site", "Capital for small businesses within 48 hours. Visit our site"),
            ("Anything at all", ""),
        ] {
            assert_eq!(with_preheader(preview, preheader), preview);
        }
        // The opening phrase, one word short of, at, and past its length,
        // with the rest of the preheader missing from the preview.
        let preheader = "alpha bravo charlie delta echo foxtrot golf";
        for (preview, kept) in [
            ("Brand alpha bravo charlie delta zulu", false),
            ("Brand alpha bravo charlie delta echo zulu", true),
            ("Brand alpha bravo charlie delta echo foxtrot zulu", true),
        ] {
            assert_eq!(with_preheader(preview, preheader) == preview, kept, "{preview}");
        }
        // A copy interrupted by an extra line is still the same preheader.
        let preview = "Join our live Demo Day on October 21. Tomorrow: see it in action. Discover a smarter approach.";
        assert_eq!(with_preheader(preview, "Join our live Demo Day on October 21. Discover a smarter approach. Join us live."), preview);
        // Shared words later in the preview still lead with the preheader.
        assert!(with_preheader("Wealthfront Funds arrive tomorrow from your Cash account to Checking. Transfer complete.", "Transfer from Cash to Checking (account ending 42)").starts_with("Transfer from Cash"));
        // Words past what the preview shows do not count as shown.
        let preview = format!("{} Thursday", "x ".repeat(SEARCH_PREVIEW_CHARS / 2));
        assert!(with_preheader(&preview, "Available Thursday").starts_with("Available Thursday x"));
    }

    fn preview(text: &str) -> String {
        readable_preview(text, &PreviewContext::default()).split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn markdown_links_keep_their_label_and_drop_the_target() {
        assert_eq!(
            preview("Hey Joel I'm [Amit](https://www.example.com/in/amit-1234/) from **Acme**."),
            "Hey Joel I'm Amit from Acme."
        );
        assert_eq!(preview("See ![chart](https://example.com/c.png) and [https://example.com](https://example.com)"), "See chart and");
        assert_eq!(preview("# Weekly update\n## Agenda\n- `deploy` at _noon_"), "Weekly update Agenda - deploy at noon");
    }

    #[test]
    fn raw_urls_are_removed_in_every_wrapper() {
        assert_eq!(
            preview("Track your order ( https://track.example.com/?n=123 ) today."),
            "Track your order today."
        );
        assert_eq!(preview("Read more: <https://example.com/a?b=c>"), "Read more:");
        assert_eq!(preview("Details at www.example.org/page, then reply."), "Details at, then reply.");
        assert_eq!(preview("Join [https://meet.example.com/x] now"), "Join now");
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for text in [
            "Use snake_case_names and 2 * 3 = 6.",
            "On Mon, A wrote: > hello",
            "Prices [approx] (see attached) for Q3",
            "Email me at ann@example.com",
            "#1 priority: ship it",
            "The https-only rule applies",
        ] {
            assert_eq!(preview(text), text);
        }
    }

    #[test]
    fn entity_references_decode_and_incomplete_ones_stay_as_written() {
        assert_eq!(
            decode_entity_references("R&amp;D at AT&T &#x2014; &#847;&nbsp;&bogus; & &#; &zwnj"),
            "R&D at AT&T \u{2014} \u{34F}\u{A0}&bogus; & &#; &zwnj"
        );
    }

    #[test]
    fn padding_runs_collapse_away_and_joiners_inside_words_stay() {
        assert_eq!(
            collapse_preview("Shutterstock \u{200C}\u{A0}\u{200C}\u{A0} \u{34F} \u{200B}\u{FEFF}\u{AD} Free images"),
            "Shutterstock Free images"
        );
        assert_eq!(collapse_preview("\u{200C}Hello\u{2060}"), "Hello");
        for text in ["family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} photo", "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}"] {
            assert_eq!(collapse_preview(text), text);
        }
    }

    fn with_context(text: &str, context: &PreviewContext) -> String {
        message_preview(text, context)
    }

    #[test]
    fn html_comments_copied_into_plain_text_are_removed() {
        assert_eq!(
            preview_of("Keep track of your account\n<!--[if !mso]><!-->\n<!--[if false]><!-->\nYou signed in to Muse.\n<![endif]-->Done <![if mso]>here"),
            "Keep track of your account You signed in to Muse. Done here"
        );
        assert_eq!(preview_of("bot left a comment <!-- review --> Summary: 3 issues"), "bot left a comment Summary: 3 issues");
        // An unterminated comment, and markup that only looks like one, stay.
        assert_eq!(preview_of("Use <!-- for comments"), "Use <!-- for comments");
        assert_eq!(preview_of("a <!b> c <![x"), "a <!b> c <![x");
    }

    fn preview_of(text: &str) -> String {
        with_context(text, &PreviewContext::default())
    }

    #[test]
    fn divider_rules_are_removed_at_their_minimum_run() {
        assert_eq!(MIN_RULE_RUN, 3);
        assert_eq!(preview("Go to LinkedIn\n----------------------------------------\nThis email was sent"), "Go to LinkedIn This email was sent");
        assert_eq!(preview("**** Supabase Update ****\nHey there"), "Supabase Update Hey there");
        assert_eq!(preview("Review here: ____ You received this ===== ~~~ ——— done"), "Review here: You received this done");
        for (rule, kept) in [("--", true), ("---", false), ("----", false)] {
            assert_eq!(preview(&format!("a {rule} b")) == format!("a {rule} b"), kept, "{rule}");
        }
        // Runs of mixed characters, and other repeated punctuation, are prose.
        assert_eq!(preview("Wait... what?! -_- ok"), "Wait... what?! -_- ok");
    }

    #[test]
    fn spelled_out_images_are_removed() {
        assert_eq!(
            preview("Approved. [image: Logo] Frank Jackson [image: Icon] [cid:image001.png@01DD3AB9.CE674CA0] [IMAGE]"),
            "Approved. Frank Jackson"
        );
        assert_eq!(preview("Photos [image: image0.jpeg][image: image1.jpeg] attached"), "Photos attached");
        // Prose that mentions images in brackets stays.
        assert_eq!(preview("See [images of the site] and [image of plan] [image"), "See [images of the site] and [image of plan] [image");
    }

    #[test]
    fn lines_of_only_image_text_are_dropped() {
        let context = PreviewContext {
            image_only_words: ["logo", "facebook", "linkedin", "icon"].map(String::from).into(),
            ..Default::default()
        };
        // A wordmark line, and an icon row in a signature.
        assert_eq!(with_context("Nextdoor logo\nLOGO\nSee what your neighbors are saying", &context), "Nextdoor logo See what your neighbors are saying");
        assert_eq!(with_context("Thanks, Ann\nfacebook | linkedin\nIcon", &context), "Thanks, Ann");
        // Visible words on the same line keep it.
        assert_eq!(with_context("Follow us on facebook", &context), "Follow us on facebook");
        assert_eq!(with_context("Logo", &context), "Logo");
    }

    #[test]
    fn an_opening_line_that_repeats_the_subject_is_dropped() {
        assert_eq!(MIN_REPEATED_SUBJECT_WORDS, 3);
        let context = |subject: &str| PreviewContext { subject: subject.into(), ..Default::default() };
        let body = "WordPress upgrades were successful!\nDear customer, we upgraded your sites.";
        for subject in ["WordPress upgrades were successful", "Re: RE[2]: Fwd: wordpress upgrades — were successful", "FW:WordPress upgrades were successful"] {
            assert_eq!(with_context(body, &context(subject)), "Dear customer, we upgraded your sites.", "{subject}");
        }
        // Below, at, and past the subject minimum.
        for (subject, dropped) in [("Thanks Ann", false), ("Thanks Ann Lee", true), ("Thanks Ann Lee Smith", true)] {
            let preview = with_context(&format!("{subject}\nSee you Monday."), &context(subject));
            assert_eq!(preview == "See you Monday.", dropped, "{subject}");
        }
        // Lines without words are not dropped: an amount split across lines, or an emoji.
        assert_eq!(with_context("Thanks\n\u{1F642}", &context("Thanks so much")), "Thanks \u{1F642}");
        assert_eq!(with_context("Sue paid you\n$\n250\n.\n00", &context("Sue paid you")), "$ 250 . 00");
        // Not when it is the only text, not when the line says more, and only the first line.
        assert_eq!(with_context("Sue paid you $250.00", &context("Sue paid you $250.00")), "Sue paid you $250.00");
        assert_eq!(with_context("Sue paid you $250.00 for dinner\nThanks", &context("Sue paid you $250.00")), "Sue paid you $250.00 for dinner Thanks");
        assert_eq!(with_context("Hi Joel\nSue paid you $250.00\nThanks", &context("Sue paid you $250.00")), "Hi Joel Sue paid you $250.00 Thanks");
        assert_eq!(with_context("Rebate offer ends soon\nok", &context("Rebate offer ends soon")), "ok");
    }

    #[test]
    fn link_lead_ins_are_dropped_when_the_rest_fills_the_preview() {
        assert_eq!(MAX_LINK_LEAD_IN_WORDS, 8);
        let story = "The unintended consequences of policy, and why voters keep supporting it. ".repeat(3);
        let full = |lead_in: &str| with_context(&format!("{lead_in}\n{story}"), &PreviewContext::default());
        for lead_in in [
            "View this post on the web at https://example.substack.com/p/post",
            "View in Browser [ https://example.com/redirect/5d0e ]",
            "[https://example.com/web](https://example.com/web)",
            "Read more: <https://example.com/a?b=c>.",
        ] {
            assert!(full(lead_in).starts_with("The unintended"), "{lead_in}");
        }
        // Seven and eight words before the link are a lead-in; nine are a sentence.
        assert!(full("one two three four five six seven https://example.com/x").starts_with("The unintended"));
        assert!(full("one two three four five six seven eight https://example.com/x").starts_with("The unintended"));
        assert!(full("one two three four five six seven eight nine https://example.com/x").starts_with("one two"));
        // A link inside the line, a short note sharing a link, and a Markdown label mid-sentence stay.
        assert!(full("Details at https://example.com/page, then reply.").starts_with("Details at, then reply."));
        assert_eq!(preview_of("Here's the doc: https://docs.example.com/d/1\n\nJoel"), "Here's the doc: Joel");
        assert!(full("Hey Joel I'm [Amit](https://example.com/amit) from Acme.").starts_with("Hey Joel I'm Amit"));
        // A list item's words are content.
        for item in ["1. Consider OrbStack for fast containers - https://orbstack.dev", "2) Docs https://example.com/d", "- Slides: https://example.com/s", "a. Notes https://example.com/n"] {
            assert!(full(item).starts_with(&item[..5]), "{item}");
        }
        // A Markdown label is the link's own text, often a headline.
        assert!(full("[Don't Let Your Funds Go To Waste](https://example.com/fsa)").starts_with("Don't Let Your Funds"));
    }
}
