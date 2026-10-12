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

    // IMAP: the IMAP renderer lives in `provider::imap::search`
    // (`render_imap`), landed in phase 5. It turns these terms into
    // `UID SEARCH CHARSET UTF-8 …`
    // (TEXT/FROM/TO/SUBJECT/SINCE/BEFORE/UNSEEN/FLAGGED/KEYWORD) and reports
    // `HasAttachment` as a local post-filter and `Raw` as a term it cannot
    // express server-side, so the caller post-filters or keeps local results.
}

// ---------------------------------------------------------------------------
// Phase 5 run 2a: operator-aware LOCAL search.
//
// The result list the user sees is a re-query of the LOCAL index
// (`Database::search_threads`). Historically that function handed the raw
// string to `fts_query`, so operators (`from:`, `is:unread`, dates, …) never
// worked locally. This section is the NEUTRAL, provider-independent decision
// of whether a parsed query engages the structured local path or the unchanged
// legacy FTS path, plus the limits that path enforces and the day-comparison
// it uses for dates. The SQL that executes a `StructuredLocalSearch` lives in
// `db::search` (it needs the schema); everything here is pure and directly
// tested. The parser and `render_gmail` above are deliberately NOT touched.
// ---------------------------------------------------------------------------

/// Most operator terms a structured local query may carry.
///
/// A real operator query is a handful of terms; dozens is a mistake or abuse.
/// 16 is generous while bounding the SQL a structured query builds. This is
/// the LOCAL analogue of `provider::imap::policy::MAX_SEARCH_TERMS` and is
/// kept here, in the neutral query module, so the local path depends on no
/// provider code. Over the limit is a typed rejection, never a silent clamp or
/// a fall back to the legacy path. Counts only the terms the structured path
/// acts on (operators + free text), i.e. every parsed term, since a structured
/// query by construction has no `Raw`/`Label`/`In` term.
pub const MAX_LOCAL_SEARCH_TERMS: usize = 16;

/// Longest one user operator VALUE may be, in UTF-8 bytes (a `from:`, `to:`,
/// `subject:` or free-text value). 256 bytes is far longer than any real
/// search value while bounding one `LIKE`/`MATCH` parameter. Over the limit is
/// a typed rejection, never truncated. (Date and state terms carry no
/// user-controlled free value — a date is already parsed to a `NaiveDate` and
/// a state is an enum — so only textual values are measured.)
pub const MAX_LOCAL_SEARCH_TERM_BYTES: usize = 256;

/// Why a structured local query was refused outright (NOT routed to legacy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalSearchLimit {
    /// Over [`MAX_LOCAL_SEARCH_TERMS`].
    TooManyTerms,
    /// Some operator/free-text value is over [`MAX_LOCAL_SEARCH_TERM_BYTES`].
    TermTooLong,
}

impl LocalSearchLimit {
    /// A plain, user-facing message. The caller surfaces this through the
    /// `DbResult`/`DatabaseError` path, so the string is what the user reads.
    pub fn message(self) -> &'static str {
        match self {
            // Deliberately the same plain sentence for both: the brief asks for
            // "That search is too long." and the user does not need to know
            // which internal bound they crossed.
            LocalSearchLimit::TooManyTerms | LocalSearchLimit::TermTooLong => {
                "That search is too long."
            }
        }
    }
}

/// Reject a structured local query that is over a limit. Returns `Ok(())` when
/// every bound holds (including exactly at it); the first bound crossed wins.
/// This is checked only for a query already decided to be `Structured` — a
/// legacy query is never length-limited here, since it takes the unchanged
/// path with its own existing behaviour.
pub fn check_local_search_limits(plan: &StructuredLocalSearch) -> Result<(), LocalSearchLimit> {
    if plan.term_count() > MAX_LOCAL_SEARCH_TERMS {
        return Err(LocalSearchLimit::TooManyTerms);
    }
    for value in plan.text_values() {
        if value.len() > MAX_LOCAL_SEARCH_TERM_BYTES {
            return Err(LocalSearchLimit::TermTooLong);
        }
    }
    Ok(())
}

/// The decision of how a parsed query runs against the LOCAL index.
///
/// This is the core routing invariant of run 2a, as a pure function so it is
/// table-testable in isolation. The structured path engages iff (1) at least
/// one term is a recognised structured operator — `From`, `To`, `Subject`,
/// `Date`, `State` or `HasAttachment` — AND (2) NO term is `Raw`, `Label` or
/// `In`. Everything else — free text only, any `Raw` term (negation, `OR`,
/// parentheses, unknown operators, unparseable colons), and any `label:`/`in:`
/// query — is [`LocalSearchPlan::Legacy`] and takes the unchanged
/// `fts_query(raw)` path with byte-identical results to before this run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalSearchPlan {
    /// Run the pre-run-2a path unchanged on the raw query string.
    Legacy,
    /// Run the operator-aware structured path.
    Structured(StructuredLocalSearch),
}

impl LocalSearchPlan {
    /// Decide the plan from an already-parsed query.
    pub fn from_query(query: &SearchQuery) -> Self {
        let has_blocker = query.terms.iter().any(|term| {
            matches!(
                term,
                QueryTerm::Raw(_) | QueryTerm::Label(_) | QueryTerm::In(_)
            )
        });
        if has_blocker {
            return LocalSearchPlan::Legacy;
        }
        let has_operator = query.terms.iter().any(|term| {
            matches!(
                term,
                QueryTerm::From(_)
                    | QueryTerm::To(_)
                    | QueryTerm::Subject(_)
                    | QueryTerm::Date { .. }
                    | QueryTerm::State(_)
                    | QueryTerm::HasAttachment
            )
        });
        if !has_operator {
            return LocalSearchPlan::Legacy;
        }
        LocalSearchPlan::Structured(StructuredLocalSearch {
            terms: query.terms.clone(),
        })
    }

    /// Parse `raw` and decide the plan in one step.
    pub fn parse(raw: &str) -> Self {
        Self::from_query(&SearchQuery::parse(raw))
    }
}

/// A query routed to the structured local path. Holds the parsed terms (every
/// one is `From`/`To`/`Subject`/`Date`/`State`/`HasAttachment`/`FreeText` by
/// construction — never `Raw`/`Label`/`In`, which would have routed to
/// legacy). The SQL builder in `db::search` consumes this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredLocalSearch {
    pub terms: Vec<QueryTerm>,
}

impl StructuredLocalSearch {
    /// How many terms the structured path acts on (all of them).
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// The free user text, rebuilt exactly as the legacy FTS path would see
    /// it: the `FreeText` terms only, in order, with quoted terms re-quoted.
    /// `None` when the query has no free text (so the SQL omits `MATCH`
    /// entirely). This reuses the SAME quoting the raw path uses so free-text
    /// matching is unchanged.
    pub fn free_text(&self) -> Option<String> {
        let parts: Vec<String> = self
            .terms
            .iter()
            .filter_map(|term| match term {
                QueryTerm::FreeText { text, quoted: true } => Some(format!("\"{text}\"")),
                QueryTerm::FreeText { text, quoted: false } => Some(text.clone()),
                _ => None,
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }

    /// Every user-controlled TEXT value in the query (operator values and
    /// free-text words), for the per-value byte-length limit. Dates and states
    /// carry no free text and are excluded.
    pub fn text_values(&self) -> Vec<&str> {
        self.terms
            .iter()
            .filter_map(|term| match term {
                QueryTerm::From(v)
                | QueryTerm::To(v)
                | QueryTerm::Subject(v)
                | QueryTerm::FreeText { text: v, .. } => Some(v.as_str()),
                QueryTerm::Date { .. } | QueryTerm::State(_) | QueryTerm::HasAttachment => None,
                // Unreachable for a structured plan, but handled so a new
                // variant forces a compile error rather than a silent skip.
                QueryTerm::Label(v) | QueryTerm::In(v) | QueryTerm::Raw(v) => Some(v.as_str()),
            })
            .collect()
    }
}

/// Escape a user substring for a SQL `LIKE ? ESCAPE '\'` pattern: the three
/// LIKE metacharacters `%`, `_` and the escape char `\` are each prefixed with
/// `\` so they match LITERALLY, and the result is wrapped in `%…%` for a
/// substring match. The value goes in as a bound PARAMETER; this only shapes
/// the pattern, never interpolates the value into SQL.
///
/// NOTE: SQLite's `LIKE` is ASCII case-insensitive ONLY (`a`==`A`, but `é` and
/// `É` are distinct). That is accepted for this run: a non-ASCII value matches
/// its own exact case. `thread_search` (FTS5) handles diacritic-folded free
/// text; operator substring matches do not fold case outside ASCII.
pub fn like_contains_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped.push('%');
    escaped
}

/// The inclusive-lower / exclusive-upper RFC 3339 instant bounds for comparing
/// `messages.sent_at` against a calendar day in UTC.
///
/// `messages.sent_at` is an RFC 3339 string (the message's arrival time; for
/// Gmail its `internalDate`). `after:d` means "on or at/after the START of day
/// d (UTC)" and `before:d` means "strictly before the START of day d (UTC)",
/// which is a DATE-ONLY comparison on UTC days. Returned as the start-of-day
/// instant `d` formatted `YYYY-MM-DDT00:00:00+00:00`: because RFC 3339 UTC
/// timestamps sort lexicographically in time order, a plain string `>=` / `<`
/// against this bound is the correct day comparison for any UTC-offset
/// timestamp SQLite normalises the same way. (The IMAP server search instead
/// compares its own `INTERNALDATE` in SERVER time, so edge-of-day results can
/// differ between the local list and a server search — an accepted, documented
/// difference.)
///
/// Kept pure and tested so the comparison is defined in exactly one place.
pub fn utc_day_start_rfc3339(date: NaiveDate) -> String {
    // Midnight UTC at the start of `date`. `and_hms_opt(0,0,0)` is always Some.
    let start = date.and_hms_opt(0, 0, 0).expect("00:00:00 is a valid time");
    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(start, chrono::Utc).to_rfc3339()
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

    // --- Phase 5 run 2a: local-search routing, limits, date & escaping ----

    fn plan(input: &str) -> LocalSearchPlan {
        LocalSearchPlan::parse(input)
    }

    fn structured_terms(input: &str) -> Vec<QueryTerm> {
        match plan(input) {
            LocalSearchPlan::Structured(s) => s.terms,
            LocalSearchPlan::Legacy => panic!("{input:?} expected Structured, got Legacy"),
        }
    }

    #[test]
    fn routing_structured_queries_engage_the_operator_path() {
        // At least one recognised operator and no Raw/Label/In => Structured.
        for input in [
            "from:alice",
            "From:Alice",
            "is:unread budget",
            "before:2024/01/01",
            "has:attachment",
            "to:bob",
            "subject:report",
            "after:2024-01-01",
            "is:starred",
            "from:alice is:unread budget", // combination with free text
        ] {
            assert!(
                matches!(plan(input), LocalSearchPlan::Structured(_)),
                "{input:?} should be Structured"
            );
        }
    }

    #[test]
    fn routing_legacy_queries_take_the_unchanged_fts_path() {
        // Free text only, any Raw term, and any label:/in: query are legacy.
        for input in [
            "budget",                 // free text only
            "\"exact phrase\"",       // quoted free text only
            "foo:bar",                // unknown operator => Raw
            "10:30",                  // not an operator (field must be letters)
            "Re: meeting",            // "Re:" empty value => Raw, "meeting" free text
            "-in:trash",              // negation => Raw
            "from:alice OR from:bob", // contains Raw ("OR")
            "category:promotions",    // unknown operator => Raw
            "label:work",             // Label => legacy (2b)
            "in:inbox",               // In => legacy (2b)
            "from:alice label:work",  // operator + Label => legacy
            "from:alice -in:trash",   // operator + Raw => legacy
            "from:(a b)",             // grouped => two Raw terms
            "from:",                  // empty value => Raw
            "",                       // nothing => legacy
        ] {
            assert_eq!(
                plan(input),
                LocalSearchPlan::Legacy,
                "{input:?} should be Legacy"
            );
        }
    }

    #[test]
    fn routing_re_meeting_is_legacy_and_keeps_both_terms() {
        // "Re:" has an empty value so it is Raw; "meeting" is free text. A Raw
        // term forces legacy regardless of the free text beside it.
        assert_eq!(
            SearchQuery::parse("Re: meeting").terms,
            vec![raw("Re:"), word("meeting")]
        );
        assert_eq!(plan("Re: meeting"), LocalSearchPlan::Legacy);
    }

    #[test]
    fn structured_free_text_is_rebuilt_from_free_terms_only() {
        // Operators drop out; quoted free text is re-quoted; order preserved.
        let s = match plan("from:alice \"q3 report\" is:unread later") {
            LocalSearchPlan::Structured(s) => s,
            LocalSearchPlan::Legacy => unreachable!(),
        };
        assert_eq!(s.free_text().as_deref(), Some("\"q3 report\" later"));
        // No free text at all => None (so the SQL omits MATCH).
        let s2 = match plan("from:alice is:unread") {
            LocalSearchPlan::Structured(s) => s,
            LocalSearchPlan::Legacy => unreachable!(),
        };
        assert_eq!(s2.free_text(), None);
    }

    #[test]
    fn structured_term_count_and_text_values() {
        let s = match plan("from:alice subject:report before:2024-01-01 is:unread budget") {
            LocalSearchPlan::Structured(s) => s,
            LocalSearchPlan::Legacy => unreachable!(),
        };
        // 5 terms total (from, subject, date, state, free text).
        assert_eq!(s.term_count(), 5);
        // Only the textual values are measured for the per-value byte limit:
        // the date and the state carry no user free text.
        assert_eq!(s.text_values(), vec!["alice", "report", "budget"]);
        let _ = structured_terms; // keep the helper referenced
    }

    #[test]
    fn local_search_limits_are_the_documented_bounds() {
        assert_eq!(MAX_LOCAL_SEARCH_TERMS, 16);
        assert_eq!(MAX_LOCAL_SEARCH_TERM_BYTES, 256);
        assert!(MAX_LOCAL_SEARCH_TERMS > 0);
        assert!(MAX_LOCAL_SEARCH_TERM_BYTES > 0);
    }

    /// A structured plan of `n` `from:` operator terms.
    fn from_terms(n: usize) -> StructuredLocalSearch {
        StructuredLocalSearch {
            terms: (0..n).map(|i| QueryTerm::From(format!("a{i}"))).collect(),
        }
    }

    #[test]
    fn term_count_limit_below_at_and_above_the_bound() {
        // Below: ok.
        assert!(check_local_search_limits(&from_terms(MAX_LOCAL_SEARCH_TERMS - 1)).is_ok());
        // Exactly at the bound: ok (accepted).
        assert!(check_local_search_limits(&from_terms(MAX_LOCAL_SEARCH_TERMS)).is_ok());
        // One over: typed rejection, never a clamp or a legacy fallback.
        assert_eq!(
            check_local_search_limits(&from_terms(MAX_LOCAL_SEARCH_TERMS + 1)),
            Err(LocalSearchLimit::TooManyTerms)
        );
    }

    #[test]
    fn term_byte_limit_below_at_and_above_the_bound() {
        let at = StructuredLocalSearch {
            terms: vec![QueryTerm::From("a".repeat(MAX_LOCAL_SEARCH_TERM_BYTES))],
        };
        let below = StructuredLocalSearch {
            terms: vec![QueryTerm::From("a".repeat(MAX_LOCAL_SEARCH_TERM_BYTES - 1))],
        };
        let above = StructuredLocalSearch {
            terms: vec![QueryTerm::From("a".repeat(MAX_LOCAL_SEARCH_TERM_BYTES + 1))],
        };
        assert!(check_local_search_limits(&below).is_ok());
        assert!(check_local_search_limits(&at).is_ok());
        assert_eq!(
            check_local_search_limits(&above),
            Err(LocalSearchLimit::TermTooLong)
        );
        // A byte limit, not a char limit: a multi-byte value over 256 BYTES
        // (but far fewer chars) is still rejected.
        let multibyte = StructuredLocalSearch {
            terms: vec![QueryTerm::Subject("é".repeat(MAX_LOCAL_SEARCH_TERM_BYTES))],
        };
        assert_eq!(
            check_local_search_limits(&multibyte),
            Err(LocalSearchLimit::TermTooLong)
        );
    }

    #[test]
    fn limit_message_is_the_plain_user_sentence() {
        assert_eq!(LocalSearchLimit::TooManyTerms.message(), "That search is too long.");
        assert_eq!(LocalSearchLimit::TermTooLong.message(), "That search is too long.");
    }

    #[test]
    fn like_pattern_escapes_metacharacters_and_wraps_in_percent() {
        assert_eq!(like_contains_pattern("alice"), "%alice%");
        // The three LIKE metacharacters are escaped so they match literally.
        assert_eq!(like_contains_pattern("50%_off"), "%50\\%\\_off%");
        assert_eq!(like_contains_pattern("a\\b"), "%a\\\\b%");
        // Quotes and other characters pass through untouched (they are not LIKE
        // metacharacters; SQL injection is handled by binding, not escaping).
        assert_eq!(like_contains_pattern("o'brien \"x\""), "%o'brien \"x\"%");
        assert_eq!(like_contains_pattern(""), "%%");
    }

    #[test]
    fn utc_day_start_is_midnight_utc_and_orders_lexicographically() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        assert_eq!(utc_day_start_rfc3339(d), "2024-01-01T00:00:00+00:00");
        // Lexicographic ordering of RFC 3339 UTC strings is time ordering, so
        // a message just before the boundary sorts below it and one at/after
        // sorts at-or-above it — the property the SQL `<` / `>=` relies on.
        let bound = utc_day_start_rfc3339(d);
        assert!("2023-12-31T23:59:59+00:00".to_string() < bound);
        assert!("2024-01-01T00:00:00+00:00".to_string() >= bound);
        assert!("2024-01-01T00:00:01+00:00".to_string() >= bound);
    }
}
