//! Local custom vocabulary extraction from Sent email only.
//!
//! Scans the user's own sent messages in SQLite to surface recurring domain
//! terms, acronyms, mixed-case identifiers, and proper nouns that appear across
//! multiple distinct threads. Suggestions are returned for explicit user review
//! and are never added or uploaded automatically.

use super::policy;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VocabularySuggestion {
    pub term: String,
    pub thread_count: usize,
}

const COMMON_UPPERCASE: &[&str] = &[
    "RE", "FW", "FWD", "FYI", "ASAP", "TBD", "TBA", "ETA", "AM", "PM", "US", "UK", "EU", "OK",
    "CC", "BCC", "PS", "PPS", "ID", "VS", "OR", "AND", "THE", "FOR", "ALL", "MON", "TUE", "WED",
    "THU", "FRI", "SAT", "SUN", "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP",
    "OCT", "NOV", "DEC", "UTC", "GMT", "EST", "EDT", "PST", "PDT", "CST", "CDT", "MST", "MDT",
    "HTTP", "HTTPS", "HTML", "JSON", "UTF", "PDF", "CSV", "DOC", "DOCX",
];

const COMMON_STOP_WORDS: &[&str] = &[
    "about",
    "above",
    "across",
    "after",
    "again",
    "against",
    "all",
    "almost",
    "alone",
    "along",
    "already",
    "also",
    "although",
    "always",
    "among",
    "and",
    "another",
    "any",
    "anyone",
    "anything",
    "anyway",
    "april",
    "are",
    "around",
    "ask",
    "asked",
    "asking",
    "august",
    "back",
    "based",
    "because",
    "been",
    "before",
    "being",
    "below",
    "best",
    "better",
    "between",
    "both",
    "bring",
    "but",
    "call",
    "called",
    "calling",
    "calls",
    "can",
    "cant",
    "case",
    "change",
    "check",
    "checking",
    "clear",
    "close",
    "come",
    "coming",
    "confirm",
    "confirmed",
    "contact",
    "could",
    "course",
    "create",
    "date",
    "day",
    "days",
    "dear",
    "december",
    "details",
    "did",
    "does",
    "doing",
    "done",
    "dont",
    "down",
    "draft",
    "during",
    "each",
    "early",
    "email",
    "emails",
    "end",
    "enough",
    "even",
    "every",
    "everyone",
    "everything",
    "example",
    "expect",
    "far",
    "february",
    "feel",
    "few",
    "file",
    "files",
    "final",
    "find",
    "first",
    "follow",
    "following",
    "for",
    "forward",
    "found",
    "friday",
    "from",
    "full",
    "further",
    "get",
    "getting",
    "give",
    "given",
    "giving",
    "glad",
    "going",
    "good",
    "got",
    "great",
    "group",
    "had",
    "happy",
    "has",
    "have",
    "having",
    "hear",
    "hello",
    "help",
    "here",
    "hey",
    "hi",
    "high",
    "home",
    "hope",
    "hopefully",
    "hour",
    "hours",
    "how",
    "however",
    "idea",
    "if",
    "important",
    "include",
    "including",
    "info",
    "information",
    "instead",
    "into",
    "invite",
    "is",
    "issue",
    "issues",
    "item",
    "items",
    "january",
    "july",
    "june",
    "just",
    "keep",
    "keeping",
    "kind",
    "know",
    "known",
    "last",
    "late",
    "later",
    "latest",
    "least",
    "leave",
    "less",
    "let",
    "lets",
    "like",
    "likely",
    "line",
    "link",
    "list",
    "little",
    "long",
    "longer",
    "look",
    "looking",
    "looks",
    "made",
    "mail",
    "make",
    "making",
    "many",
    "march",
    "may",
    "maybe",
    "mean",
    "meeting",
    "meetings",
    "message",
    "might",
    "minute",
    "minutes",
    "monday",
    "month",
    "months",
    "more",
    "morning",
    "most",
    "much",
    "must",
    "name",
    "near",
    "need",
    "needed",
    "needs",
    "never",
    "new",
    "next",
    "nice",
    "night",
    "note",
    "notes",
    "nothing",
    "notice",
    "november",
    "now",
    "number",
    "october",
    "off",
    "office",
    "often",
    "okay",
    "once",
    "one",
    "only",
    "open",
    "option",
    "order",
    "other",
    "others",
    "our",
    "out",
    "over",
    "own",
    "part",
    "past",
    "people",
    "per",
    "perhaps",
    "person",
    "phone",
    "place",
    "plan",
    "planned",
    "planning",
    "plans",
    "please",
    "point",
    "points",
    "possible",
    "possibly",
    "present",
    "pretty",
    "probably",
    "problem",
    "project",
    "put",
    "question",
    "questions",
    "quick",
    "quickly",
    "quite",
    "rather",
    "reach",
    "read",
    "ready",
    "real",
    "really",
    "reason",
    "receive",
    "received",
    "recent",
    "recently",
    "regarding",
    "regards",
    "related",
    "remain",
    "remember",
    "reply",
    "report",
    "request",
    "review",
    "right",
    "room",
    "said",
    "same",
    "saturday",
    "save",
    "say",
    "saying",
    "says",
    "schedule",
    "scheduled",
    "second",
    "see",
    "seeing",
    "seem",
    "seems",
    "seen",
    "send",
    "sending",
    "sent",
    "september",
    "set",
    "setting",
    "several",
    "share",
    "shared",
    "she",
    "should",
    "show",
    "since",
    "small",
    "some",
    "someone",
    "something",
    "sometimes",
    "soon",
    "sorry",
    "sort",
    "sound",
    "sounds",
    "start",
    "started",
    "starting",
    "state",
    "status",
    "still",
    "stop",
    "subject",
    "such",
    "sunday",
    "sure",
    "sync",
    "take",
    "taken",
    "taking",
    "talk",
    "talking",
    "team",
    "tell",
    "terms",
    "test",
    "than",
    "thank",
    "thanks",
    "that",
    "thats",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "thing",
    "things",
    "think",
    "thinking",
    "this",
    "those",
    "though",
    "thought",
    "three",
    "through",
    "thursday",
    "time",
    "times",
    "today",
    "together",
    "tomorrow",
    "tonight",
    "too",
    "took",
    "top",
    "topic",
    "total",
    "track",
    "try",
    "trying",
    "tuesday",
    "two",
    "under",
    "understand",
    "unless",
    "until",
    "update",
    "updated",
    "updates",
    "upon",
    "use",
    "used",
    "useful",
    "using",
    "usually",
    "very",
    "via",
    "wait",
    "waiting",
    "want",
    "wanted",
    "wants",
    "was",
    "way",
    "ways",
    "wednesday",
    "week",
    "weeks",
    "well",
    "went",
    "were",
    "what",
    "whatever",
    "when",
    "where",
    "whether",
    "which",
    "while",
    "who",
    "whole",
    "whom",
    "whose",
    "why",
    "will",
    "with",
    "within",
    "without",
    "work",
    "working",
    "works",
    "would",
    "write",
    "writing",
    "written",
    "wrote",
    "yeah",
    "year",
    "years",
    "yes",
    "yesterday",
    "yet",
    "you",
    "your",
    "yours",
    "zero",
];

fn strip_quoted_and_signatures(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "--" || trimmed.starts_with("-- ") {
            break;
        }
        if trimmed.eq_ignore_ascii_case("-----Original Message-----")
            || (trimmed.starts_with("On ") && trimmed.ends_with("wrote:"))
        {
            break;
        }
        if trimmed.starts_with('>') {
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    out
}

fn token_rejected(raw: &str) -> bool {
    if raw.len() > policy::SENT_VOCAB_MAX_TOKEN_CHARS {
        return true;
    }
    let lower = raw.to_ascii_lowercase();
    if raw.contains('@')
        || lower.contains("://")
        || lower.starts_with("www.")
        || raw.chars().any(|c| c.is_ascii_digit())
    {
        return true;
    }
    // Reject long unbroken lowercase or mixed alphanumeric strings that resemble hashes/tokens.
    raw.len() >= 24 && !raw.contains(['-', '.'])
}

fn clean_token(raw: &str) -> &str {
    raw.trim_matches(|c: char| !c.is_alphabetic() && c != '.' && c != '-')
        .trim_end_matches('.')
        .trim_end_matches("'s")
        .trim_end_matches('\u{2019}')
}

fn is_all_caps_acronym(term: &str) -> bool {
    (2..=8).contains(&term.len())
        && term.chars().all(|c| c.is_ascii_uppercase())
        && !COMMON_UPPERCASE.contains(&term)
}

fn is_mixed_case_identifier(term: &str) -> bool {
    let mut chars = term.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_alphabetic() {
        return false;
    }
    let has_lower = term.chars().any(|c| c.is_ascii_lowercase());
    let has_inner_upper = term.chars().skip(1).any(|c| c.is_ascii_uppercase());
    has_lower
        && has_inner_upper
        && term
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '.')
}

fn is_compound_technical(term: &str) -> bool {
    (term.contains('.') || term.contains('-'))
        && term.len() >= 4
        && !term.starts_with('.')
        && !term.starts_with('-')
        && term
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '.' || c == '-')
        && term
            .split(['.', '-'])
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphabetic()))
}

fn is_capitalized_proper_noun(term: &str) -> bool {
    let mut chars = term.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_uppercase()
        && term.len() >= 3
        && chars.all(|c| c.is_ascii_lowercase() || c == '-' || c == '\'')
        && !COMMON_STOP_WORDS.contains(&term.to_ascii_lowercase().as_str())
}

fn is_stop_word(term: &str) -> bool {
    let lower = term.to_ascii_lowercase();
    COMMON_STOP_WORDS.contains(&lower.as_str())
        || COMMON_UPPERCASE
            .iter()
            .any(|u| u.eq_ignore_ascii_case(&lower))
}

#[derive(Default)]
struct CandidateStats {
    threads: HashSet<String>,
    casings: HashMap<String, usize>,
}

fn record_candidate(map: &mut HashMap<String, CandidateStats>, term: &str, thread_id: &str) {
    let trimmed = term.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > policy::CUSTOM_VOCAB_MAX_TERM_CHARS
        || is_stop_word(trimmed)
    {
        return;
    }
    let key = trimmed.to_ascii_lowercase();
    let stats = map.entry(key).or_default();
    stats.threads.insert(thread_id.to_string());
    *stats.casings.entry(trimmed.to_string()).or_insert(0) += 1;
}

/// Extracts recurring distinctive vocabulary candidates from `(thread_id, text)`
/// pairs belonging to **Sent** mail only. Requires appearance across at least
/// [`policy::SENT_VOCAB_MIN_THREADS`] distinct threads and excludes terms already
/// saved in `existing`.
pub fn extract_vocabulary_suggestions(
    messages: &[(String, String)],
    existing: &[String],
) -> Vec<VocabularySuggestion> {
    let existing_keys: HashSet<String> = existing
        .iter()
        .map(|term| term.trim().to_ascii_lowercase())
        .collect();
    let mut stats: HashMap<String, CandidateStats> = HashMap::new();

    for (thread_id, raw_text) in messages {
        let cleaned = strip_quoted_and_signatures(raw_text);
        let mut prev_cap: Option<(&str, bool)> = None;
        for (line_index, line) in cleaned.lines().enumerate() {
            let mut at_sentence_start = line_index == 0 || line.trim().is_empty();
            for raw in line.split_whitespace() {
                let ends_sentence = raw.ends_with(['.', '!', '?', ':']);
                if token_rejected(raw) {
                    prev_cap = None;
                    at_sentence_start = ends_sentence;
                    continue;
                }
                let term = clean_token(raw);
                if term.len() < 2 {
                    prev_cap = None;
                    at_sentence_start = ends_sentence;
                    continue;
                }

                if is_all_caps_acronym(term)
                    || is_mixed_case_identifier(term)
                    || is_compound_technical(term)
                {
                    record_candidate(&mut stats, term, thread_id);
                    prev_cap = None;
                } else if is_capitalized_proper_noun(term) {
                    if !at_sentence_start {
                        record_candidate(&mut stats, term, thread_id);
                    }
                    if let Some((prev_term, prev_allowed)) = prev_cap {
                        if !at_sentence_start || prev_allowed {
                            let phrase = format!("{prev_term} {term}");
                            record_candidate(&mut stats, &phrase, thread_id);
                        }
                    }
                    prev_cap = Some((term, !at_sentence_start));
                } else {
                    prev_cap = None;
                }

                at_sentence_start = ends_sentence;
            }
            prev_cap = None;
        }
    }

    let mut suggestions: Vec<VocabularySuggestion> = stats
        .into_iter()
        .filter(|(key, entry)| {
            !existing_keys.contains(key) && entry.threads.len() >= policy::SENT_VOCAB_MIN_THREADS
        })
        .filter_map(|(_, entry)| {
            let best_casing = entry
                .casings
                .into_iter()
                .max_by(|(a_term, a_count), (b_term, b_count)| {
                    a_count.cmp(b_count).then_with(|| b_term.cmp(a_term))
                })?
                .0;
            Some(VocabularySuggestion {
                term: best_casing,
                thread_count: entry.threads.len(),
            })
        })
        .collect();

    suggestions.sort_by(|a, b| {
        b.thread_count
            .cmp(&a.thread_count)
            .then_with(|| {
                a.term
                    .to_ascii_lowercase()
                    .cmp(&b.term.to_ascii_lowercase())
            })
            .then_with(|| a.term.cmp(&b.term))
    });
    suggestions.truncate(policy::SENT_VOCAB_MAX_SUGGESTIONS);
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn sent_pair(thread: &str, text: &str) -> (String, String) {
        (thread.to_string(), text.to_string())
    }

    #[test]
    fn requires_minimum_distinct_threads_and_ranks_higher_frequency_first() {
        let messages = vec![
            sent_pair(
                "t1",
                "Rolling out ThreeStrands with CoreAudio and QBR review.",
            ),
            sent_pair(
                "t2",
                "Checking ThreeStrands and CoreAudio again before QBR.",
            ),
            sent_pair(
                "t3",
                "Finalizing ThreeStrands and CoreAudio before our QBR with WASAPI notes.",
            ),
            sent_pair("t4", "Shared another ThreeStrands milestone."),
            // Two threads only — must be filtered out below threshold 3.
            sent_pair("t5", "Mentioned EphemeralProj once."),
            sent_pair("t6", "Mentioned EphemeralProj twice."),
        ];
        let got = extract_vocabulary_suggestions(&messages, &[]);
        let terms: Vec<&str> = got.iter().map(|s| s.term.as_str()).collect();
        assert_eq!(terms, vec!["ThreeStrands", "CoreAudio", "QBR"]);
        assert_eq!(got[0].thread_count, 4);
        assert_eq!(got[1].thread_count, 3);
    }

    #[test]
    fn filters_stopwords_emails_urls_digits_secrets_quotes_and_existing_terms() {
        let messages = vec![
            sent_pair(
                "t1",
                "We will discuss WhisperKit, whisper.cpp, user@example.com, https://example.com/secret, API2026, tok_dummy_abcdefghijklmnopqrstuvwxyz, thanks.\n> QuotedSecretTerm\n-- \nSignatureBrand\nOn Mon wrote:\nEarlierQuotedAcronym",
            ),
            sent_pair(
                "t2",
                "Please test WhisperKit and whisper.cpp with user@example.com https://example.com/secret API2026 tok_dummy_abcdefghijklmnopqrstuvwxyz thanks.\n> QuotedSecretTerm",
            ),
            sent_pair(
                "t3",
                "Confirming WhisperKit and whisper.cpp plus user@example.com https://example.com/secret API2026 tok_dummy_abcdefghijklmnopqrstuvwxyz.",
            ),
        ];
        let got = extract_vocabulary_suggestions(&messages, &["whisperkit".into()]);
        let terms: Vec<&str> = got.iter().map(|s| s.term.as_str()).collect();
        assert_eq!(terms, vec!["whisper.cpp"]);
    }

    #[test]
    fn caps_suggestions_at_policy_limit() {
        let mut messages = Vec::new();
        for t in 1..=3 {
            let mut body = String::from("Sending terms: ");
            for i in 0..(policy::SENT_VOCAB_MAX_SUGGESTIONS + 15) {
                body.push_str(&format!("TermCamel{} ", (b'A' + (i % 26) as u8) as char));
                // Construct unique CamelCase alphabetical identifiers with no digits.
                let a = (b'A' + (i / 26) as u8) as char;
                let b = (b'a' + (i % 26) as u8) as char;
                body.push_str(&format!("Proj{a}{b}End "));
            }
            messages.push(sent_pair(&format!("t{t}"), &body));
        }
        let got = extract_vocabulary_suggestions(&messages, &[]);
        assert_eq!(got.len(), policy::SENT_VOCAB_MAX_SUGGESTIONS);
    }

    #[test]
    fn db_query_reads_sent_messages_only_and_ignores_received_mail() {
        let db = Database::open_memory();
        db.test_with_connection(|c| {
            for (tid, mid, sender, body, direction) in [
                ("th1", "m1", "me@example.com", "Deploying ThreeStrands soon.", "sent"),
                ("th2", "m2", "me@example.com", "Testing ThreeStrands today.", "sent"),
                ("th3", "m3", "Me <me@example.com>", "Shipping ThreeStrands tomorrow.", "sent"),
                ("th4", "m4", "newsletter@vendor.com", "Buy NewsletterSpamWidget now!", "received"),
                ("th5", "m5", "promo@vendor.com", "Buy NewsletterSpamWidget today!", "received"),
                ("th6", "m6", "deals@vendor.com", "Buy NewsletterSpamWidget forever!", "received"),
            ] {
                c.execute(
                    "INSERT INTO threads(id,provider_thread_id,subject,snippet,participants_json,last_message_at,labels_json,account_id)
                     VALUES(?1,?1,'Sync','','[]','2026-11-27T10:00:00Z','[]','me@example.com')",
                    [tid],
                )?;
                c.execute(
                    "INSERT INTO messages(id,thread_id,sender,recipients_json,sent_at,body_html,body_text)
                     VALUES(?1,?2,?3,'[]','2026-11-27T10:00:00Z','',?4)",
                    [mid, tid, sender, body],
                )?;
                c.execute(
                    "INSERT INTO contact_interactions(message_id,thread_id,account_id,email,display_name,direction,sent_at)
                     VALUES(?1,?2,'me@example.com',?3,'',?4,'2026-11-27T10:00:00Z')",
                    [mid, tid, sender, direction],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let sent = db
            .sent_message_texts_for_vocabulary(policy::SENT_VOCAB_SCAN_LIMIT)
            .unwrap();
        assert_eq!(sent.len(), 3);
        let suggestions = extract_vocabulary_suggestions(&sent, &[]);
        assert_eq!(
            suggestions,
            vec![VocabularySuggestion {
                term: "ThreeStrands".into(),
                thread_count: 3
            }]
        );
    }
}
