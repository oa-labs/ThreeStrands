//! Display cleanup for one-line message previews. Senders' plain-text parts
//! often carry Markdown (`[label](url)`, `**bold**`) and long raw URLs, which
//! read as noise in an inbox row. This only shapes preview text; the stored
//! body and the search index keep every character.


/// `text` with Markdown link and emphasis syntax reduced to its words and
/// URLs removed. Line structure is kept for the caller to collapse.
pub fn readable_preview(text: &str) -> String {
    text.lines()
        .map(|line| tidy(&strip_emphasis(&strip_links(strip_heading(line)))))
        .collect::<Vec<_>>()
        .join("\n")
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

/// `[label](target)` → label, `![alt](target)` → alt, and raw URLs (bare or in
/// `<...>`) removed. A label that is itself a URL is removed too.
fn strip_links(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let at_word_start = i == 0 || !chars[i - 1].is_alphanumeric();
        let bracket = if chars[i] == '[' { Some(i) } else if chars[i] == '!' && chars.get(i + 1) == Some(&'[') { Some(i + 1) } else { None };
        if let Some(open) = bracket {
            if let Some((label, next)) = markdown_link(&chars, open) {
                if !is_url_start(&label.chars().collect::<Vec<_>>()) {
                    out.push_str(&label);
                }
                i = next;
                continue;
            }
        }
        if at_word_start && is_url_start(&chars[i..]) {
            i = url_end(&chars, i);
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
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
    use super::{collapse_preview, decode_entity_references, readable_preview, with_preheader};
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
        readable_preview(text).split_whitespace().collect::<Vec<_>>().join(" ")
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
}
