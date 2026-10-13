//! Inert email HTML-to-text extraction. This is not the reader's sanitizer:
//! no markup is returned, no resources are fetched, and no code is executed.
//! Tokenization handles attributes, quoted `>`, entities, comments, and raw
//! style/script text without letting non-content blocks enter inbox previews.

use html5ever::tokenizer::{
    states::RawKind, BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer,
};
use html5ever::LocalName;
use std::cell::RefCell;

#[derive(Default)]
struct TextSink {
    state: RefCell<TextState>,
}

#[derive(Default)]
struct TextState {
    text: String,
    pending_space: bool,
    ignored: Vec<LocalName>,
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
                    if matches!(
                        name,
                        "head" | "style" | "script" | "template" | "noscript" | "title"
                    ) {
                        state.ignored.push(tag.name.clone());
                    }
                    // Raw-text states recognize only the matching end tag:
                    // markup-like CSS/JS strings cannot end an ignored block.
                    let raw = match name {
                        "script" => Some(RawKind::ScriptData),
                        "style" | "noscript" | "xmp" | "iframe" | "noembed" => {
                            Some(RawKind::Rawtext)
                        }
                        "title" | "textarea" => Some(RawKind::Rcdata),
                        _ => None,
                    };
                    if let Some(raw) = raw {
                        return TokenSinkResult::RawData(raw);
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

pub(crate) fn html_to_text(html: &str) -> String {
    let tokenizer = Tokenizer::new(TextSink::default(), Default::default());
    let input = BufferQueue::default();
    input.push_back(html.into());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.state.into_inner().text.trim().to_string()
}

/// Some senders generate their text/plain alternative with an HTML-to-text
/// converter that keeps `<style>` contents, so the "plain text" opens with a
/// stylesheet. When the text begins with one or more complete CSS rules —
/// a selector list or at-rule followed by a `{ property: value; ... }` block —
/// returns what follows them; otherwise `None`. Prose never matches: a brace
/// block only counts when every entry in it is a `property: value`
/// declaration (or, for an at-rule, nested rules of that shape).
pub(crate) fn after_leading_stylesheet(text: &str) -> Option<&str> {
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
    use super::after_leading_stylesheet;

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
