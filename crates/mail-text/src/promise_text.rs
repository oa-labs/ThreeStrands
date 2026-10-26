//! First-person promises in the user's own sent mail, such as "I'll send the
//! report over tomorrow", for Today's Promises section.
//!
//! Detection is deliberately narrow: a sentence must open a promise ("I'll",
//! "I will", "I'm going to", "let me", and the "we" forms) and name a
//! deliverable verb within a few words. Questions are skipped. Anything
//! uncertain stays out, since a promise listed in error is noise the user has
//! to dismiss.

/// Most characters of the sentence Today quotes back.
pub const MAX_PROMISE_EXCERPT_CHARS: usize = 160;

/// Words allowed between the opener and the verb, as in "I'll also try to send".
pub const MAX_WORDS_BEFORE_VERB: usize = 3;

const OPENERS: &[&[&str]] = &[
    &["i'll"],
    &["i", "will"],
    &["i'm", "going", "to"],
    &["i", "am", "going", "to"],
    &["let", "me"],
    &["we'll"],
    &["we", "will"],
    &["we're", "going", "to"],
];

/// Verbs that name something the writer will hand over or do for the reader.
const VERBS: &[&str] = &[
    "send", "share", "forward", "follow", "circle", "look", "check", "run", "write", "draft",
    "prepare", "put", "reconnect", "review", "confirm", "update", "book", "schedule", "set",
    "dig", "call", "reach", "email", "finish", "fix", "loop", "pull", "compile", "submit",
    "reply", "respond", "ping", "introduce", "find", "research", "sign",
];

/// What may follow "get" for it to promise something: "get back to you",
/// "get you the numbers", "get it over".
const GET_OBJECTS: &[&str] = &["back", "you", "it", "that", "this", "those", "these", "the", "them"];

/// The first sentence in `text` that makes a promise, trimmed and capped at
/// `MAX_PROMISE_EXCERPT_CHARS`, or None.
pub fn promise_sentence(text: &str) -> Option<String> {
    sentences(text)
        .find(|sentence| !sentence.ends_with('?') && makes_promise(sentence))
        .map(excerpt)
}

fn sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive(['.', '!', '?', '\n'])
        .map(|sentence| sentence.trim().trim_end_matches(['.', '!', '\n']).trim())
        .filter(|sentence| !sentence.is_empty())
}

fn makes_promise(sentence: &str) -> bool {
    let words: Vec<String> = sentence
        .split_whitespace()
        .map(|word| {
            word.replace('\u{2019}', "'")
                .trim_matches(|character: char| !character.is_alphanumeric() && character != '\'')
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();
    (0..words.len()).any(|start| {
        OPENERS.iter().any(|opener| {
            let end = start + opener.len();
            end <= words.len()
                && words[start..end].iter().zip(opener.iter()).all(|(word, expected)| word == expected)
                && names_deliverable(&words[end..])
        })
    })
}

fn names_deliverable(after: &[String]) -> bool {
    after.iter().take(MAX_WORDS_BEFORE_VERB + 1).enumerate().any(|(index, word)| {
        VERBS.contains(&word.as_str())
            || (word == "get" && after.get(index + 1).is_some_and(|next| GET_OBJECTS.contains(&next.as_str())))
    })
}

fn excerpt(sentence: &str) -> String {
    if sentence.chars().count() <= MAX_PROMISE_EXCERPT_CHARS {
        return sentence.to_string();
    }
    let cut: String = sentence.chars().take(MAX_PROMISE_EXCERPT_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_promise_and_quotes_its_sentence() {
        let text = "Hi Jill,\n\nThanks for the note. I'll reconnect your server to the AI stream, run the report and send it over.\n\nJoel";
        assert_eq!(
            promise_sentence(text).as_deref(),
            Some("I'll reconnect your server to the AI stream, run the report and send it over")
        );
    }

    #[test]
    fn accepts_each_opener_and_a_few_words_before_the_verb() {
        for text in [
            "I will send the deck Friday.",
            "I\u{2019}m going to also share the notes.",
            "Let me check with Pete.",
            "We'll get back to you next week.",
            "I'll try to quickly put together a one-pager.",
            "I'll get you the numbers.",
        ] {
            assert!(promise_sentence(text).is_some(), "{text}");
        }
    }

    #[test]
    fn skips_questions_plans_and_requests() {
        for text in [
            "Should I send it now?",
            "I'll send it to whom?",
            "I'll be there at 3.",
            "Let me know what you think.",
            "I'll get there early.",
            "Will you send the deck?",
            "I sent the report yesterday.",
            // The verb is too far from the opener to be what was promised.
            "I'll be out on Monday and Tuesday, then send.",
        ] {
            assert_eq!(promise_sentence(text), None, "{text}");
        }
    }

    #[test]
    fn caps_a_long_sentence_at_the_excerpt_limit() {
        // "I'll send " is ten characters.
        let below = format!("I'll send {}", "a".repeat(MAX_PROMISE_EXCERPT_CHARS - 11));
        assert_eq!(promise_sentence(&below), Some(below.clone()));
        let exact = format!("I'll send {}", "a".repeat(MAX_PROMISE_EXCERPT_CHARS - 10));
        assert_eq!(promise_sentence(&exact), Some(exact.clone()));
        let above = format!("I'll send {}", "a".repeat(MAX_PROMISE_EXCERPT_CHARS));
        let quoted = promise_sentence(&above).unwrap();
        assert_eq!(quoted.chars().count(), MAX_PROMISE_EXCERPT_CHARS);
        assert!(quoted.ends_with('…'));
    }
}
