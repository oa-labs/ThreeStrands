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
    use super::readable_preview;

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
}
