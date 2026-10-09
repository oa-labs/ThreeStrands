//! A provider-neutral search query.
//!
//! Today a user's search string is handed to Gmail exactly as typed. The
//! IMAP/SMTP design (`docs/imap-design.md`, "Server search") calls for the
//! string to be parsed **once** into a neutral [`SearchQuery`] AST, which each
//! provider then renders into its own wire syntax: Gmail back into Gmail
//! query syntax (behaviour unchanged), and IMAP into
//! `UID SEARCH CHARSET UTF-8 …`.
//!
//! This module is phase 1, slice 1 of that work: the AST, a parser for it,
//! and the Gmail renderer, with round-trip tests. It is a self-contained seam
//! — the live Gmail search path is **not** re-wired through it yet. See
//! [`SearchQuery::render_gmail`] for why.
//!
//! The IMAP renderer and `has:attachment` local filtering are deliberately
//! left out of this slice (`// IMAP:` seams below).

// Seam module: the AST, parser and Gmail renderer are exercised by this
// module's own tests and will be consumed by the live search path and the
// IMAP renderer in later phases (see `docs/imap-design.md`, "Server search",
// phases 1, 5). Until then the public items have no non-test caller, so a
// normal build would flag them as dead code. The allow is scoped to this
// module and removed once a caller is wired in.
#![allow(dead_code)]

use chrono::NaiveDate;

/// The field a `before:`/`after:` date term constrains.
///
/// Both map to a calendar date; the provider decides the comparison. Gmail
/// uses `before:`/`after:`; IMAP will use `BEFORE`/`SINCE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateBound {
    /// Mail strictly before the date (Gmail `before:`, IMAP `BEFORE`).
    Before,
    /// Mail on or after the date (Gmail `after:`, IMAP `SINCE`).
    After,
}

/// A boolean state a message can be in, expressed as `is:` in the query
/// language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageState {
    /// `is:unread` — Gmail `is:unread`, IMAP `UNSEEN`.
    Unread,
    /// `is:starred` — Gmail `is:starred`, IMAP `FLAGGED`.
    Starred,
}

/// One term of a parsed search query.
///
/// The AST stays flat: the surface language is a conjunction of terms
/// (space-separated, implicitly ANDed), matching what both Gmail and IMAP
/// `SEARCH` accept without parentheses. Matches should list every variant
/// rather than use a `_` arm, so a new term fails to compile until each
/// renderer decides what to do with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryTerm {
    /// Literal text the user is looking for: a bare word, or a double-quoted
    /// phrase when `quoted` is true. Quoting is kept because Gmail treats a
    /// quoted word as an exact match. The text never contains `"`.
    FreeText { text: String, quoted: bool },
    /// `from:<value>`.
    From(String),
    /// `to:<value>`.
    To(String),
    /// `subject:<value>`.
    Subject(String),
    /// `before:<date>` / `after:<date>`, accepted as `YYYY/MM/DD` or
    /// `YYYY-MM-DD`. Any other date text is kept as [`QueryTerm::Raw`].
    Date { bound: DateBound, date: NaiveDate },
    /// `is:unread` / `is:starred`.
    State(MessageState),
    /// `label:<value>` — a user label.
    Label(String),
    /// `in:<value>` — a location (folder/mailbox or system location).
    In(String),
    /// `has:attachment`. IMAP cannot express this server-side, so it becomes a
    /// local post-filter there (left unimplemented in this slice).
    HasAttachment,
    /// Query syntax this AST doesn't model, kept verbatim: negation
    /// (`-in:trash`), `OR`/`AND`, parentheses and braces, unknown operators
    /// (`category:promotions`), unknown `is:`/`has:` values, empty operator
    /// values and unparseable dates. Gmail receives it unchanged. It is never
    /// literal text, so the IMAP renderer must not send it as a `TEXT` search.
    Raw(String),
}

/// A parsed, provider-neutral search query: a conjunction of [`QueryTerm`]s in
/// the order they were written.
///
/// Order is preserved so the Gmail renderer can reproduce the original
/// intent, which is what keeps the round trip stable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub terms: Vec<QueryTerm>,
}

impl SearchQuery {
    /// Parse a raw search string into the neutral AST.
    ///
    /// Grammar: whitespace-separated tokens, each either `field:value` for a
    /// recognised field, query syntax the AST doesn't model
    /// ([`QueryTerm::Raw`]), or free text. Double quotes group words into one
    /// token, and they can open at the start of a token or right after a
    /// `field:` prefix. A quote that is never closed runs to the end of the
    /// input and is treated as closed there. Field names and `is:`/`has:`
    /// values are case-insensitive. Nothing a user types is silently dropped.
    pub fn parse(input: &str) -> Self {
        SearchQuery {
            terms: tokenize(input)
                .iter()
                .map(|token| parse_token(token))
                .collect(),
        }
    }

    /// Render the query back into Gmail query syntax.
    ///
    /// Every term renders to a token that parses back to the same term, so
    /// `parse(render(q)) == q` for any parsed query. The rendered string is
    /// Gmail-equivalent to the input but not always byte-identical: dates are
    /// normalised to `YYYY/MM/DD`, `is:`/`has:` values are lowercased, and
    /// quotes are added wherever a value would otherwise re-parse differently.
    ///
    /// It is intentionally **not** wired into the live search path in this
    /// slice. Gmail stays byte-for-byte unchanged until the IMAP renderer
    /// lands and the switch can be tested end to end.
    pub fn render_gmail(&self) -> String {
        self.terms
            .iter()
            .map(|term| match term {
                QueryTerm::FreeText { text, quoted } => {
                    if *quoted || !renders_as(text, term) {
                        format!("\"{text}\"")
                    } else {
                        text.clone()
                    }
                }
                QueryTerm::From(value) => render_field("from", value, term),
                QueryTerm::To(value) => render_field("to", value, term),
                QueryTerm::Subject(value) => render_field("subject", value, term),
                QueryTerm::Date { bound, date } => {
                    let key = match bound {
                        DateBound::Before => "before",
                        DateBound::After => "after",
                    };
                    format!("{key}:{}", date.format("%Y/%m/%d"))
                }
                QueryTerm::State(MessageState::Unread) => "is:unread".to_string(),
                QueryTerm::State(MessageState::Starred) => "is:starred".to_string(),
                QueryTerm::Label(value) => render_field("label", value, term),
                QueryTerm::In(value) => render_field("in", value, term),
                QueryTerm::HasAttachment => "has:attachment".to_string(),
                QueryTerm::Raw(token) => token.clone(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    // IMAP: `render_imap(&self) -> ImapSearchKey` lands in phase 5. It turns
    // these terms into `UID SEARCH CHARSET UTF-8 …`
    // (TEXT/FROM/TO/SUBJECT/SINCE/BEFORE/UNSEEN/FLAGGED/KEYWORD) and reports
    // `HasAttachment` and `Raw` as terms it cannot express server-side, so the
    // caller post-filters or reports them.
}

/// Whether `candidate`, parsed on its own, yields exactly `term`.
fn renders_as(candidate: &str, term: &QueryTerm) -> bool {
    SearchQuery::parse(candidate).terms == [term.clone()]
}

/// Render `key:value`, quoting the value only when the bare form would parse
/// back to something else (whitespace, a leading parenthesis, and so on).
fn render_field(key: &str, value: &str, term: &QueryTerm) -> String {
    let bare = format!("{key}:{value}");
    if renders_as(&bare, term) {
        bare
    } else {
        format!("{key}:\"{value}\"")
    }
}

/// Split an input string into tokens, honouring double-quoted phrases.
///
/// Whitespace outside quotes separates tokens; runs of whitespace collapse.
/// Quotes stay in the token text. A quote left open at the end of the input
/// is closed, so every token has balanced quotes and renders back unchanged.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if in_quotes {
        current.push('"');
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Turn one token into a [`QueryTerm`].
fn parse_token(token: &str) -> QueryTerm {
    if is_unmodelled_syntax(token) {
        return QueryTerm::Raw(token.to_string());
    }
    let Some((field, raw_value)) = split_field(token) else {
        let (text, quoted) = strip_quotes(token);
        if text.is_empty() {
            return QueryTerm::Raw(token.to_string());
        }
        return QueryTerm::FreeText { text, quoted };
    };

    let (value, _) = strip_quotes(raw_value);
    // An operator with nothing after it, or with a grouped value such as
    // `from:(a b)`, isn't something this AST models.
    if value.is_empty() || raw_value.starts_with(['(', '{']) {
        return QueryTerm::Raw(token.to_string());
    }
    let raw = || QueryTerm::Raw(token.to_string());
    match field.as_str() {
        "from" => QueryTerm::From(value),
        "to" => QueryTerm::To(value),
        "subject" => QueryTerm::Subject(value),
        "label" => QueryTerm::Label(value),
        "in" => QueryTerm::In(value),
        "before" => parse_date(&value).map_or_else(raw, |date| QueryTerm::Date {
            bound: DateBound::Before,
            date,
        }),
        "after" => parse_date(&value).map_or_else(raw, |date| QueryTerm::Date {
            bound: DateBound::After,
            date,
        }),
        "is" => match value.to_ascii_lowercase().as_str() {
            "unread" => QueryTerm::State(MessageState::Unread),
            "starred" => QueryTerm::State(MessageState::Starred),
            _ => raw(),
        },
        "has" if value.eq_ignore_ascii_case("attachment") => QueryTerm::HasAttachment,
        _ => raw(),
    }
}

/// Query syntax that is never literal text: negation, boolean operators, and
/// grouping. Quoted phrases start with `"`, so they never match here.
fn is_unmodelled_syntax(token: &str) -> bool {
    (token.len() > 1 && token.starts_with('-'))
        || token == "OR"
        || token == "AND"
        || token.starts_with(['(', '{'])
        || token.ends_with([')', '}'])
}

/// Split `field:value` at the first colon, returning
/// `(lowercased field, raw value)`. Returns `None` when there is no colon, the
/// field is not a run of ASCII letters (so times like `12:30` stay free text),
/// a quote comes before the colon (so `"a:b"` is free text), or the value
/// starts with `//` (so URLs like `https://…` stay free text).
fn split_field(token: &str) -> Option<(String, &str)> {
    let colon = token.find(':')?;
    let field = &token[..colon];
    let value = &token[colon + 1..];
    if field.is_empty() || !field.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if value.starts_with("//") {
        return None;
    }
    Some((field.to_ascii_lowercase(), value))
}

/// Remove every double quote, returning the text and whether any was present.
/// Quotes only group words; Gmail can't search for a literal `"`.
fn strip_quotes(value: &str) -> (String, bool) {
    let quoted = value.contains('"');
    (value.replace('"', ""), quoted)
}

/// Parse a `before:`/`after:` date as `YYYY/MM/DD` or `YYYY-MM-DD`.
fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y/%m/%d")
        .or_else(|_| NaiveDate::parse_from_str(value, "%Y-%m-%d"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<QueryTerm> {
        SearchQuery::parse(input).terms
    }

    fn word(text: &str) -> QueryTerm {
        QueryTerm::FreeText {
            text: text.into(),
            quoted: false,
        }
    }

    fn phrase(text: &str) -> QueryTerm {
        QueryTerm::FreeText {
            text: text.into(),
            quoted: true,
        }
    }

    fn raw(token: &str) -> QueryTerm {
        QueryTerm::Raw(token.into())
    }

    fn date(bound: DateBound, y: i32, m: u32, d: u32) -> QueryTerm {
        QueryTerm::Date {
            bound,
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
        }
    }

    #[test]
    fn parses_free_text() {
        assert_eq!(parse("hello"), vec![word("hello")]);
    }

    #[test]
    fn parses_multiple_free_text_words_as_separate_terms() {
        assert_eq!(
            parse("quarterly report"),
            vec![word("quarterly"), word("report")]
        );
    }

    #[test]
    fn parses_quoted_free_text_as_one_term() {
        assert_eq!(
            parse("\"quarterly report\""),
            vec![phrase("quarterly report")]
        );
    }

    #[test]
    fn parses_from() {
        assert_eq!(
            parse("from:alice@example.com"),
            vec![QueryTerm::From("alice@example.com".into())]
        );
    }

    #[test]
    fn parses_to() {
        assert_eq!(parse("to:bob"), vec![QueryTerm::To("bob".into())]);
    }

    #[test]
    fn parses_subject_with_quoted_phrase() {
        assert_eq!(
            parse("subject:\"status update\""),
            vec![QueryTerm::Subject("status update".into())]
        );
    }

    #[test]
    fn parses_before_and_after() {
        assert_eq!(
            parse("before:2026-01-01"),
            vec![date(DateBound::Before, 2026, 1, 1)]
        );
        assert_eq!(
            parse("after:2026/01/01"),
            vec![date(DateBound::After, 2026, 1, 1)]
        );
    }

    #[test]
    fn parses_unpadded_dates() {
        assert_eq!(
            parse("before:2026/1/5"),
            vec![date(DateBound::Before, 2026, 1, 5)]
        );
    }

    #[test]
    fn unparseable_dates_are_raw() {
        assert_eq!(parse("before:yesterday"), vec![raw("before:yesterday")]);
        assert_eq!(parse("after:2026-13-01"), vec![raw("after:2026-13-01")]);
        assert_eq!(parse("after:2026-01-01x"), vec![raw("after:2026-01-01x")]);
    }

    #[test]
    fn parses_is_states() {
        assert_eq!(
            parse("is:unread"),
            vec![QueryTerm::State(MessageState::Unread)]
        );
        assert_eq!(
            parse("is:starred"),
            vec![QueryTerm::State(MessageState::Starred)]
        );
    }

    #[test]
    fn parses_label_and_in() {
        assert_eq!(parse("label:work"), vec![QueryTerm::Label("work".into())]);
        assert_eq!(parse("in:sent"), vec![QueryTerm::In("sent".into())]);
    }

    #[test]
    fn parses_has_attachment() {
        assert_eq!(parse("has:attachment"), vec![QueryTerm::HasAttachment]);
    }

    #[test]
    fn is_with_unknown_value_is_raw() {
        assert_eq!(parse("is:muted"), vec![raw("is:muted")]);
    }

    #[test]
    fn has_with_unknown_value_is_raw() {
        assert_eq!(parse("has:nothing"), vec![raw("has:nothing")]);
    }

    #[test]
    fn field_matching_is_case_insensitive() {
        assert_eq!(parse("From:alice"), vec![QueryTerm::From("alice".into())]);
        assert_eq!(parse("SUBJECT:hi"), vec![QueryTerm::Subject("hi".into())]);
    }

    #[test]
    fn is_and_has_values_are_case_insensitive() {
        assert_eq!(
            parse("is:UNREAD"),
            vec![QueryTerm::State(MessageState::Unread)]
        );
        assert_eq!(
            parse("IS:Starred"),
            vec![QueryTerm::State(MessageState::Starred)]
        );
        assert_eq!(parse("has:Attachment"), vec![QueryTerm::HasAttachment]);
    }

    #[test]
    fn unknown_field_prefix_is_raw() {
        assert_eq!(
            parse("category:promotions"),
            vec![raw("category:promotions")]
        );
    }

    #[test]
    fn empty_operator_values_are_raw() {
        assert_eq!(parse("from:"), vec![raw("from:")]);
        assert_eq!(parse("subject:\"\""), vec![raw("subject:\"\"")]);
    }

    #[test]
    fn negation_boolean_operators_and_grouping_are_raw() {
        assert_eq!(parse("-in:trash"), vec![raw("-in:trash")]);
        assert_eq!(parse("-meeting"), vec![raw("-meeting")]);
        assert_eq!(
            parse("from:a OR from:b"),
            vec![
                QueryTerm::From("a".into()),
                raw("OR"),
                QueryTerm::From("b".into())
            ]
        );
        assert_eq!(parse("from:(a b)"), vec![raw("from:(a"), raw("b)")]);
        assert_eq!(parse("{x y}"), vec![raw("{x"), raw("y}")]);
    }

    #[test]
    fn lowercase_or_and_a_lone_dash_are_free_text() {
        assert_eq!(
            parse("this or that"),
            vec![word("this"), word("or"), word("that")]
        );
        assert_eq!(parse("-"), vec![word("-")]);
    }

    #[test]
    fn url_is_not_treated_as_a_field() {
        assert_eq!(
            parse("https://example.com"),
            vec![word("https://example.com")]
        );
    }

    #[test]
    fn time_in_free_text_is_not_a_field() {
        // The field name must be all-letters, so `12:30` stays free text.
        assert_eq!(parse("12:30"), vec![word("12:30")]);
    }

    #[test]
    fn colon_inside_quotes_is_not_a_field() {
        assert_eq!(parse("\"a:b\""), vec![phrase("a:b")]);
        assert_eq!(parse("\"from:alice\""), vec![phrase("from:alice")]);
    }

    #[test]
    fn unclosed_quote_runs_to_the_end_and_is_closed() {
        assert_eq!(
            parse("subject:\"a b"),
            vec![QueryTerm::Subject("a b".into())]
        );
        assert_eq!(parse("x \"a b"), vec![word("x"), phrase("a b")]);
    }

    #[test]
    fn parses_combination_in_order() {
        assert_eq!(
            parse("from:alice subject:\"q3 report\" is:unread later"),
            vec![
                QueryTerm::From("alice".into()),
                QueryTerm::Subject("q3 report".into()),
                QueryTerm::State(MessageState::Unread),
                word("later"),
            ]
        );
    }

    #[test]
    fn empty_and_whitespace_input_parse_to_no_terms() {
        assert!(parse("").is_empty());
        assert!(parse("   \t  ").is_empty());
    }

    #[test]
    fn collapses_runs_of_whitespace() {
        assert_eq!(parse("a    b"), vec![word("a"), word("b")]);
    }

    // --- Gmail renderer round-trip ---------------------------------------

    /// parse → render → parse yields the same AST, and a second render is
    /// byte-stable.
    fn assert_round_trip(input: &str) {
        let first = SearchQuery::parse(input);
        let rendered = first.render_gmail();
        let second = SearchQuery::parse(&rendered);
        assert_eq!(
            first, second,
            "AST changed across render for input {input:?}: rendered {rendered:?}"
        );
        assert_eq!(rendered, second.render_gmail());
    }

    #[test]
    fn round_trips_each_term_kind() {
        for input in [
            "hello",
            "from:alice@example.com",
            "to:bob@example.com",
            "subject:update",
            "before:2026-01-01",
            "after:2025/12/31",
            "is:unread",
            "is:starred",
            "label:work",
            "in:sent",
            "has:attachment",
            "category:promotions",
        ] {
            assert_round_trip(input);
        }
    }

    #[test]
    fn round_trips_quoted_values() {
        assert_round_trip("subject:\"quarterly report\"");
        assert_round_trip("\"two words\"");
        assert_round_trip("from:\"Alice Smith\"");
    }

    #[test]
    fn round_trips_combinations() {
        assert_round_trip("from:alice subject:\"q3 report\" is:unread label:work later");
        assert_round_trip("in:sent before:2026-01-01 has:attachment");
    }

    #[test]
    fn round_trips_inputs_that_used_to_change_meaning() {
        for input in [
            // Quoted phrases that look like operators stay literal text.
            "\"from:alice\"",
            "\"is:unread\"",
            "\"-in:trash\"",
            "\"OR\"",
            "\"(draft)\"",
            // Unclosed quotes.
            "subject:\"a b",
            "\"a b",
            "-in:\"a b",
            // Values that need quoting to survive.
            "subject:\"(draft)\"",
            "label:\"-x\"",
            // Unmodelled syntax.
            "from:a OR from:b",
            "from:(a b)",
            "in:sent -in:trash -in:spam",
            "from: x",
            "https://example.com 12:30",
        ] {
            assert_round_trip(input);
        }
    }

    #[test]
    fn quoted_phrase_that_looks_like_an_operator_renders_quoted() {
        let query = SearchQuery::parse("\"from:alice\"");
        assert_eq!(query.render_gmail(), "\"from:alice\"");
    }

    #[test]
    fn quoted_single_word_keeps_its_quotes() {
        assert_eq!(SearchQuery::parse("\"hello\"").render_gmail(), "\"hello\"");
        assert_eq!(SearchQuery::parse("hello").render_gmail(), "hello");
    }

    #[test]
    fn unclosed_quote_renders_closed() {
        assert_eq!(
            SearchQuery::parse("subject:\"a b").render_gmail(),
            "subject:\"a b\""
        );
    }

    #[test]
    fn renders_recognised_terms_to_gmail_syntax() {
        let query = SearchQuery::parse("from:alice is:unread subject:\"big news\"");
        assert_eq!(
            query.render_gmail(),
            "from:alice is:unread subject:\"big news\""
        );
    }

    #[test]
    fn renders_normalised_dates_and_states() {
        let query = SearchQuery::parse("before:2026-1-5 IS:UNREAD has:ATTACHMENT");
        assert_eq!(
            query.render_gmail(),
            "before:2026/01/05 is:unread has:attachment"
        );
    }

    #[test]
    fn renders_phrase_value_quoted() {
        let query = SearchQuery::parse("subject:\"status update\"");
        assert_eq!(query.render_gmail(), "subject:\"status update\"");
    }

    #[test]
    fn empty_query_renders_empty_string() {
        assert_eq!(SearchQuery::default().render_gmail(), "");
    }

    /// The hard-coded sent-backfill query uses negation, which this AST keeps
    /// as raw syntax rather than modelling. It still renders back verbatim, so
    /// Gmail would receive the same string.
    #[test]
    fn sent_backfill_query_keeps_negation_as_raw_and_renders_verbatim() {
        let input = "in:sent -in:trash -in:spam";
        let parsed = SearchQuery::parse(input);
        assert_eq!(
            parsed.terms,
            vec![
                QueryTerm::In("sent".into()),
                raw("-in:trash"),
                raw("-in:spam"),
            ]
        );
        assert_eq!(parsed.render_gmail(), input);
    }
}
