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
