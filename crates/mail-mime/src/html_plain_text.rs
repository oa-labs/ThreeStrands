//! Inert email HTML-to-text extraction. This is not the reader's sanitizer:
//! no markup is returned, no resources are fetched, and no code is executed.
//! Tokenization handles attributes, quoted `>`, entities, comments, and raw
//! style/script text without letting non-content blocks enter inbox previews.

use html5ever::tokenizer::{
    states::RawKind, BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer,
};
use html5ever::LocalName;
use std::cell::RefCell;
use std::collections::HashSet;

#[derive(Default)]
struct TextSink {
    state: RefCell<TextState>,
}

#[derive(Default)]
struct TextState {
    text: String,
    pending_space: bool,
    ignored: Vec<LocalName>,
    /// Alt and title text of images, which a reader sees only when an image
    /// fails to load.
    image_text: String,
}

impl TextState {
    fn line_break(&mut self) {
        self.pending_space = false;
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn append(&mut self, text: &str) {
        for character in text.chars() {
            if character.is_ascii_whitespace() {
                self.pending_space = true;
            } else {
                if self.pending_space && !self.text.is_empty() && !self.text.ends_with('\n') {
                    self.text.push(' ');
                }
                self.pending_space = false;
                self.text.push(character);
            }
        }
    }
}

impl TokenSink for TextSink {
    type Handle = ();

    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        let mut state = self.state.borrow_mut();
        match token {
            Token::TagToken(tag) => {
                let name = tag.name.as_ref();
                if tag.kind == TagKind::EndTag {
                    if let Some(index) = state.ignored.iter().rposition(|name| *name == tag.name) {
                        state.ignored.truncate(index);
                    }
                } else {
                    if is_non_content(name) {
                        state.ignored.push(tag.name.clone());
                    }
                    if let Some(raw) = raw_kind(name) {
                        return TokenSinkResult::RawData(raw);
                    }
                    if state.ignored.is_empty() && matches!(name, "img" | "area" | "input") {
                        for attribute in &tag.attrs {
                            if matches!(attribute.name.local.as_ref(), "alt" | "title") {
                                state.image_text.push(' ');
                                state.image_text.push_str(&attribute.value);
                            }
                        }
                    }
                }
                if state.ignored.is_empty()
                    && matches!(
                        name,
                        "address"
                            | "article"
                            | "aside"
                            | "blockquote"
                            | "br"
                            | "dd"
                            | "div"
                            | "dl"
                            | "dt"
                            | "figcaption"
                            | "figure"
                            | "footer"
                            | "h1"
                            | "h2"
                            | "h3"
                            | "h4"
                            | "h5"
                            | "h6"
                            | "header"
                            | "hr"
                            | "li"
                            | "ol"
                            | "p"
                            | "pre"
                            | "section"
                            | "table"
                            | "td"
                            | "th"
                            | "tr"
                            | "ul"
                    )
                {
                    state.line_break();
                }
            }
            Token::CharacterTokens(text) if state.ignored.is_empty() => state.append(&text),
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// Elements whose contents are never message text.
fn is_non_content(name: &str) -> bool {
    matches!(name, "head" | "style" | "script" | "template" | "noscript" | "title")
}

/// Raw-text states recognize only the matching end tag: markup-like CSS/JS
/// strings cannot end an ignored block.
fn raw_kind(name: &str) -> Option<RawKind> {
    match name {
        "script" => Some(RawKind::ScriptData),
        "style" | "noscript" | "xmp" | "iframe" | "noembed" => Some(RawKind::Rawtext),
        "title" | "textarea" => Some(RawKind::Rcdata),
        _ => None,
    }
}

pub fn html_to_text(html: &str) -> String {
    text_state(html).text.trim().to_string()
}

fn text_state(html: &str) -> TextState {
    let tokenizer = Tokenizer::new(TextSink::default(), Default::default());
    let input = BufferQueue::default();
    input.push_back(html.into());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.state.into_inner()
}

/// Words the HTML shows only as image alt or title text, never as visible
/// text: lowercased runs of letters and digits. Plain-text alternatives
/// often spell this text out (`Brand logo`), where it reads as message text.
pub fn image_only_words(html: &str) -> HashSet<String> {
    let state = text_state(html);
    let words = |text: &str| -> HashSet<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let visible = words(&state.text);
    words(&state.image_text).into_iter().filter(|word| !visible.contains(word)).collect()
}

/// Longest preheader kept, in characters once whitespace and invisible
/// padding are collapsed. Senders write a line or two; more than this means
/// an unclosed hidden element swallowed the message.
pub const MAX_PREHEADER_CHARS: usize = 300;

/// Fewest words, each with two or more letters, a preheader must hold, so a
/// hidden tracking token is not mistaken for one.
pub const MIN_PREHEADER_WORDS: usize = 3;

/// Elements without an end tag.
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr",
];

/// Characters that draw nothing; senders pad preheaders with them so inbox
/// previews stop early.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{034F}' | '\u{00AD}')
}

/// Whether an inline style keeps an element's text from being seen:
/// `display: none`, `visibility: hidden`, `opacity: 0`, a font size of at
/// most 1px, or a zero height or max-height with `overflow: hidden`.
fn style_hides(style: &str) -> bool {
    let number = |value: &str| value.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%').parse::<f64>().ok();
    let (mut zero_height, mut clipped) = (false, false);
    for declaration in style.split(';') {
        let Some((name, value)) = declaration.split_once(':') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let value = value.to_ascii_lowercase();
        let value = value.trim().trim_end_matches("!important").trim();
        match name.as_str() {
            "display" if value == "none" => return true,
            "visibility" if value == "hidden" => return true,
            "opacity" if number(value) == Some(0.0) => return true,
            "font-size" if number(value) == Some(0.0) || value == "1px" => return true,
            "height" | "max-height" if number(value) == Some(0.0) => zero_height = true,
            "overflow" if value == "hidden" => clipped = true,
            _ => {}
        }
    }
    zero_height && clipped
}

#[derive(Default)]
struct PreheaderSink {
    state: RefCell<PreheaderState>,
}

#[derive(Default)]
struct PreheaderState {
    /// Open elements, each with whether it hides its contents.
    open: Vec<(LocalName, bool)>,
    ignored: Vec<LocalName>,
    text: String,
    /// Visible text has appeared; nothing after it is a preheader.
    done: bool,
}

impl TokenSink for PreheaderSink {
    type Handle = ();

    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        let mut state = self.state.borrow_mut();
        if state.done {
            return TokenSinkResult::Continue;
        }
        match token {
            Token::TagToken(tag) => {
                let name = tag.name.as_ref();
                if tag.kind == TagKind::EndTag {
                    if let Some(index) = state.ignored.iter().rposition(|name| *name == tag.name) {
                        state.ignored.truncate(index);
                    }
                    if let Some(index) = state.open.iter().rposition(|(name, _)| *name == tag.name) {
                        state.open.truncate(index);
                    }
                } else {
                    if is_non_content(name) {
                        state.ignored.push(tag.name.clone());
                    } else if !tag.self_closing && !VOID_ELEMENTS.contains(&name) {
                        let hidden = tag.attrs.iter().any(|attribute| match attribute.name.local.as_ref() {
                            "hidden" => true,
                            "style" => style_hides(&attribute.value),
                            _ => false,
                        });
                        state.open.push((tag.name.clone(), hidden));
                    }
                    if let Some(raw) = raw_kind(name) {
                        return TokenSinkResult::RawData(raw);
                    }
                }
                // Element boundaries separate words.
                state.text.push(' ');
            }
            Token::CharacterTokens(text) if state.ignored.is_empty() => {
                if state.open.iter().any(|(_, hidden)| *hidden) {
                    state.text.push_str(&text);
                    if state.text.len() > MAX_PREHEADER_CHARS * 16 {
                        state.done = true;
                    }
                } else if text.chars().any(|c| !c.is_whitespace() && !is_invisible(c)) {
                    state.done = true;
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// The preheader: text the sender hid with inline styles ahead of any visible
/// text, written to be read in an inbox preview rather than in the message.
/// Invisible padding is dropped and whitespace collapsed; `None` when there
/// is no such text, or it has fewer than `MIN_PREHEADER_WORDS` words or more
/// than `MAX_PREHEADER_CHARS` characters.
pub fn preheader(html: &str) -> Option<String> {
    let tokenizer = Tokenizer::new(PreheaderSink::default(), Default::default());
    let input = BufferQueue::default();
    input.push_back(html.into());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let text = tokenizer.sink.state.into_inner().text;
    let collapsed = text
        .split_whitespace()
        .map(|word| word.trim_matches(is_invisible))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let words = collapsed
        .split(' ')
        .filter(|word| word.chars().filter(|c| c.is_alphabetic()).count() >= 2)
        .count();
    (words >= MIN_PREHEADER_WORDS && collapsed.chars().count() <= MAX_PREHEADER_CHARS).then_some(collapsed)
}

/// Some senders generate their text/plain alternative with an HTML-to-text
/// converter that keeps `<style>` contents, so the "plain text" opens with a
/// stylesheet. When the text begins with one or more complete CSS rules —
/// a selector list or at-rule followed by a `{ property: value; ... }` block —
/// returns what follows them; otherwise `None`. Prose never matches: a brace
/// block only counts when every entry in it is a `property: value`
/// declaration (or, for an at-rule, nested rules of that shape).
pub fn after_leading_stylesheet(text: &str) -> Option<&str> {
    let mut rest = text;
    let mut found = false;
    while let Some(end) = css_rule_end(rest.trim_start()) {
        rest = &rest.trim_start()[end..];
        found = true;
    }
    found.then(|| rest.trim_start())
}

/// Byte length of the CSS rule or at-rule at the start of `text`.
fn css_rule_end(text: &str) -> Option<usize> {
    if text.starts_with('@') {
        let stop = text.find(['{', ';', '}'])?;
        return match text.as_bytes()[stop] {
            b';' => Some(stop + 1),
            b'{' => {
                let body = &text[stop + 1..];
                if let Some(end) = declaration_block_end(body) {
                    return Some(stop + 1 + end);
                }
                let mut inner = body;
                let mut nested = false;
                while let Some(end) = css_rule_end(inner.trim_start()) {
                    inner = &inner.trim_start()[end..];
                    nested = true;
                }
                let inner = inner.trim_start();
                (nested && inner.starts_with('}')).then(|| text.len() - inner.len() + 1)
            }
            _ => None,
        };
    }
    // `<` never appears in a selector; it does in markup ahead of a <style>.
    let open = text.find(['{', '}', ';', '@', '<'])?;
    if text.as_bytes()[open] != b'{' || text[..open].trim().is_empty() || text[..open].contains("\n\n") {
        return None;
    }
    declaration_block_end(&text[open + 1..]).map(|end| open + 1 + end)
}

/// Length through the closing `}` when `body` is a declaration list.
fn declaration_block_end(body: &str) -> Option<usize> {
    let close = body.find(['}', '{'])?;
    if body.as_bytes()[close] != b'}' {
        return None;
    }
    let mut declarations = body[..close].split(';').map(str::trim).filter(|entry| !entry.is_empty()).peekable();
    declarations.peek()?;
    declarations.all(is_declaration).then_some(close + 1)
}

fn is_declaration(entry: &str) -> bool {
    let Some((name, value)) = entry.split_once(':') else {
        return false;
    };
    let name = name.trim().trim_start_matches('-');
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::{after_leading_stylesheet, image_only_words, preheader, MAX_PREHEADER_CHARS, MIN_PREHEADER_WORDS};

    #[test]
    fn finds_text_hidden_ahead_of_the_visible_message() {
        for (html, expected) in [
            // A hidden div first in the body, then a padding div.
            (
                "<html><head><title>Play</title><style>.x { display: none; }</style></head><body><div style=\"display:none;max-height:0;overflow:hidden;mso-hide:all\">Available through Thursday.</div><div style=\"display: none !important\">&zwnj;&nbsp;&zwnj;&nbsp;&#847; &shy;</div><table><tr><td>Your Play Points</td></tr></table></body></html>",
                "Available through Thursday.",
            ),
            // A zero-size span inside a layout table, padded with invisible characters.
            (
                "<table role=presentation><tr><td><img src=\"cid:logo\" alt=\"\"><span style=\"FONT-SIZE: 0PX; color:#ffffff\">Delivered 1 item: <b>Drinkware</b> &#847; &shy; &#847;</span></td></tr><tr><td>Your Orders</td></tr></table>",
                "Delivered 1 item: Drinkware",
            ),
            // Clipped to zero height, then the hidden attribute, then transparency.
            ("<div style=\"max-height: 0px; overflow: hidden\">Your weekly performance</div><p hidden>and tips</p><span style=\"opacity:0.0\">to reach more people</span><p>Hi Joel</p>", "Your weekly performance and tips to reach more people"),
        ] {
            assert_eq!(preheader(html).as_deref(), Some(expected), "{html}");
        }
    }

    #[test]
    fn finds_no_preheader_without_hidden_leading_text() {
        for html in [
            // Visible text comes first.
            "<p>View in browser</p><div style=\"display:none\">Available through Thursday.</div>",
            // Hidden only by a class rule, which inline styles cannot show.
            "<style>.pre { display: none }</style><div class=pre>Available through Thursday.</div><p>Hi</p>",
            // A hidden void element hides nothing after it.
            "<img style=\"display:none\" src=\"cid:x\">Available through Thursday.",
            // Outlook-only hiding, and a zero height that does not clip.
            "<div style=\"mso-hide:all\">Available through Thursday.</div>",
            "<div style=\"max-height:0\">Available through Thursday.</div>",
            // A hidden tracking token is not words.
            "<div style=\"display:none\">m7d\u{200D}bmt\u{FEFF}ixxl</div><p>Hi there</p>",
            // An unclosed hidden element swallowing the message.
            &format!("<span style=\"display:none\">{}", "Long story words. ".repeat(100)),
            "<p>Hello</p>",
            "",
        ] {
            assert_eq!(preheader(html), None, "{html}");
        }
    }

    #[test]
    fn image_only_words_are_alt_text_the_message_never_shows() {
        let words = |html: &str| {
            let mut words: Vec<String> = image_only_words(html).into_iter().collect();
            words.sort();
            words
        };
        // A logo in a table cell, and an icon row in a signature.
        assert_eq!(
            words("<table><tr><td><img src=\"cid:a\" alt=\"Brand logo\"></td><td>Your brand weekly digest</td></tr></table>"),
            ["logo"]
        );
        assert_eq!(
            words("<p>Ann Lee<br>Director</p><a href=\"https://example.com\"><img src=\"x.png\" title=\"Icon\" alt=\"facebook\"></a>"),
            ["facebook", "icon"]
        );
        // Alt text inside ignored blocks, and other attributes, do not count.
        assert!(words("<head><title>t</title></head><noscript><img alt=\"Tracking pixel\"></noscript><div title=\"Note\">Hi</div><img src=\"x\">").is_empty());
    }

    #[test]
    fn preheader_limits_hold_at_their_exact_values() {
        let words = |count: usize| vec!["word"; count].join(" ");
        for (count, kept) in [(MIN_PREHEADER_WORDS - 1, false), (MIN_PREHEADER_WORDS, true), (MIN_PREHEADER_WORDS + 1, true)] {
            let html = format!("<div style=\"display:none\">{} x 1</div>", words(count));
            assert_eq!(preheader(&html).is_some(), kept, "{count} words");
        }
        let sized = |chars: usize| format!("<div style=\"display:none\">abc def {}</div>", "g".repeat(chars - 8));
        for (chars, kept) in [(MAX_PREHEADER_CHARS - 1, true), (MAX_PREHEADER_CHARS, true), (MAX_PREHEADER_CHARS + 1, false)] {
            let found = preheader(&sized(chars));
            assert_eq!(found.is_some(), kept, "{chars} chars");
            if let Some(text) = found {
                assert_eq!(text.chars().count(), chars);
            }
        }
    }

    #[test]
    fn finds_message_text_after_a_leading_stylesheet() {
        for (text, rest) in [
            (".hero { margin: 0 30px !important; margin-top: 4px; } Hello", "Hello"),
            ("\n  #outlook a { padding:0; }\n  body { margin:0; }\n\nHi there", "Hi there"),
            ("@media only screen and (max-width: 481px) { .a { font-size: 11px !important; } td.b { padding: 3px; } } Order 42", "Order 42"),
            ("@font-face { font-family: X; src: local(X); } @charset \"utf-8\"; Thanks", "Thanks"),
            ("Brand96body, table tr, a,\nspan { font-family: Verdana, sans-serif; }\nReview", "Review"),
            ("a:visited { color: #000; }", ""),
        ] {
            assert_eq!(after_leading_stylesheet(text), Some(rest), "{text}");
        }
    }

    #[test]
    fn leaves_prose_and_partial_css_alone() {
        for text in [
            "Hello world",
            "Please use @media screen { p { color: red; } } in the stylesheet.",
            "Dear {first_name}, your order shipped.",
            "Template { name } is ready",
            "Agenda; see { time: 3pm } below",
            "Intro\n\n.card { color: red; }",
            ".hero { margin: 0 30px",
            "<html><head><style>p { color: red; }</style></head><body>Hi</body></html>",
            "@media screen { prose here }",
            "",
        ] {
            assert_eq!(after_leading_stylesheet(text), None, "{text}");
        }
    }
}
