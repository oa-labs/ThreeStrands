//! The pure IMAP search renderer (Phase 5 run 1).
//!
//! `docs/imap-design.md` ("Server search"). The provider-neutral
//! [`SearchQuery`](crate::search_query::SearchQuery) AST is rendered here into
//! an IMAP `UID SEARCH` plan: which mailboxes to search, the exact
//! `CHARSET UTF-8 …` criteria string to hand to
//! [`ImapSession::uid_search`](super::session::ImapSession::uid_search), and
//! the terms the server cannot express that the caller must apply to the
//! fetched results locally (today only `has:attachment`).
//!
//! This run is a PURE module plus its policy limits and tests. NOTHING calls
//! it yet — no provider wiring, no `MailSync::search`, no `server_search:
//! true`, no frontend. Later runs (local operator filtering, acquire-by-UID +
//! pinning, the IMAP `search()`, UI) consume [`render_imap`], so the surface
//! is kept small, typed and easy to review.
//!
//! ## Design decisions recorded here (also noted in `docs/imap-design.md`)
//!
//! * `to:` renders `(OR OR TO … CC … BCC …)`, matching Gmail, where `to:`
//!   includes Cc and Bcc.
//! * `before:`/`after:` use `BEFORE`/`SINCE` with FIXED ENGLISH month
//!   abbreviations (`1-Jan-2024`), never the locale, and are server-timezone,
//!   date-only comparisons (RFC 3501 §6.4.4).
//! * A term the server cannot honestly evaluate (`Raw`, invalid characters, an
//!   over-limit value, an empty query, a cross-mailbox combination) makes the
//!   whole outcome [`ImapSearchOutcome::Skipped`] — the caller keeps local
//!   results only. We NEVER send a `Raw` token as `TEXT`, and never silently
//!   drop a term.
//! * A term the server CAN prove matches nothing (an unknown label, an unknown
//!   folder, a label name that cannot be a keyword atom) makes the outcome
//!   [`ImapSearchOutcome::NoServerMatches`].
//!
//! ## Wire safety
//!
//! User text is hostile. [`quote_search_string`] is the ONE function that
//! builds every quoted value: it REJECTS (not strips) CR, LF and NUL, escapes
//! `\` and `"`, and passes non-ASCII through as UTF-8 inside the quoted string
//! (the command carries `CHARSET UTF-8`). It never emits an IMAP literal
//! (`{n}`) — a past literal bug desynchronised the connection, and quoted
//! UTF-8 is enough on Dovecot (confirmed by the gated live test). A value can
//! therefore never introduce a new search key.

use chrono::Datelike;

use crate::search_query::{render_gmail_term, DateBound, MessageState, QueryTerm, SearchQuery};

use super::plan::{LabelKind, SyncPlan};
use super::policy;
use super::settings::LabelStorage;
use super::write::validate_flag;

// This module has no non-test caller until a later Phase 5 run wires it into
// `MailSync::search`; the module tree's `#![allow(dead_code)]` already covers
// that, so no new allow is added here.

// ===========================================================================
// Public outcome types
// ===========================================================================

/// Everything the renderer needs about the account to turn a query into a
/// server-search plan: the sync plan (which resolves mailbox roles, user
/// folders and label folders to their location ids) and how this account
/// stores user labels.
pub struct ImapSearchContext<'a> {
    /// The account's sync plan, used to resolve `in:` and folder `label:`
    /// terms to concrete mailbox wire names and the default search set.
    pub plan: &'a SyncPlan,
    /// How this account stores ThreeStrands user labels, which decides how a
    /// `label:` term is honoured.
    pub label_storage: LabelStorage,
    /// The label container's wire name for label-folder mode, if configured.
    /// Only read when `label_storage == Folders`.
    pub label_container: Option<&'a str>,
    /// The hierarchy delimiter the account uses (defaults to `/` when `None`
    /// or empty), used to build a label folder's mailbox name.
    pub delimiter: Option<&'a str>,
}

/// The outcome of rendering a [`SearchQuery`] into an IMAP server search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImapSearchOutcome {
    /// Run these server searches.
    Plan(ImapSearchPlan),
    /// The server cannot answer, and we KNOW the answer is "nothing" — for
    /// example an unknown label or an unknown folder. Carries a plain-language
    /// reason the UI can word.
    NoServerMatches { reason: SearchNotice },
    /// Do NOT run a server search: the query has a term we cannot honestly
    /// evaluate server-side. Carries every reason. The caller keeps local
    /// results only.
    Skipped { reasons: Vec<SearchNotice> },
}

/// A ready-to-run server search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImapSearchPlan {
    /// Mailboxes to search, exact `LIST` wire names, in a deterministic order.
    pub mailboxes: Vec<String>,
    /// The string passed to
    /// [`ImapSession::uid_search`](super::session::ImapSession::uid_search),
    /// starting with `CHARSET UTF-8 `.
    pub criteria: String,
    /// Terms the server cannot express that the caller must apply to the
    /// fetched results locally.
    pub local_filters: Vec<LocalFilter>,
}

/// A term IMAP `SEARCH` cannot express server-side, applied to the fetched
/// results instead. Today the only one is `has:attachment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalFilter {
    /// `has:attachment` — the caller keeps only results that have an
    /// attachment (decided from the fetched structure, not the server).
    HasAttachment,
}

/// A small, typed reason the UI can word later — NOT a free-form string built
/// from user text alone. Each variant carries only the bounded data the UI
/// needs and renders a plain [`std::fmt::Display`] message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchNotice {
    /// The query had no terms the server could use (an empty query).
    EmptyQuery,
    /// A query token this AST keeps verbatim ([`QueryTerm::Raw`]) — negation,
    /// `OR`/`AND`, parentheses, an unknown operator — that must never be sent
    /// as `TEXT`. Carries the token.
    UnsupportedSyntax { token: String },
    /// A term value carried CR, LF or NUL. Carries the field name it came from.
    InvalidCharacters { field: SearchField },
    /// A term value was over a policy limit. Carries which limit.
    OverLimit { limit: OverLimit },
    /// Two terms named DIFFERENT mailboxes, which cannot be one server search.
    CannotCombineMailboxes { first: String, second: String },
    /// A `label:` term under an account with no label storage.
    NoLabelStorage,
    /// A `label:` name that cannot be a valid IMAP keyword atom, so no message
    /// can carry it. Carries the name.
    LabelNotAKeyword { name: String },
    /// A `label:` folder name that resolves to no mailbox in the plan.
    UnknownLabelFolder { name: String },
    /// An `in:` location that matches no mailbox in the plan.
    UnknownMailbox { name: String },
}

/// Which policy limit an [`SearchNotice::OverLimit`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverLimit {
    /// Over [`policy::MAX_SEARCH_TERMS`].
    Terms,
    /// Over [`policy::MAX_SEARCH_TERM_BYTES`] (one user value).
    TermBytes,
    /// Over [`policy::MAX_SEARCH_QUERY_BYTES`] (the whole rendered criteria).
    QueryBytes,
    /// Over [`policy::MAX_SEARCH_TARGET_MAILBOXES`].
    TargetMailboxes,
}

/// Which user-supplied field a value came from, for an
/// [`SearchNotice::InvalidCharacters`] message. Not the whole [`QueryTerm`] —
/// just enough for the UI to say where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchField {
    FreeText,
    From,
    To,
    Subject,
    Label,
    In,
}

impl std::fmt::Display for SearchField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            SearchField::FreeText => "search text",
            SearchField::From => "from:",
            SearchField::To => "to:",
            SearchField::Subject => "subject:",
            SearchField::Label => "label:",
            SearchField::In => "in:",
        };
        f.write_str(name)
    }
}

impl std::fmt::Display for OverLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            OverLimit::Terms => "too many search terms",
            OverLimit::TermBytes => "a search term is too long",
            OverLimit::QueryBytes => "the search query is too long",
            OverLimit::TargetMailboxes => "the search would touch too many mailboxes",
        };
        f.write_str(text)
    }
}

impl std::fmt::Display for SearchNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchNotice::EmptyQuery => {
                f.write_str("the search is empty, so nothing was searched on the server")
            }
            SearchNotice::UnsupportedSyntax { token } => write!(
                f,
                "the term {token:?} can't be searched on the server, so only locally-synced \
                 results are shown"
            ),
            SearchNotice::InvalidCharacters { field } => write!(
                f,
                "the {field} value contains characters that can't be searched on the server"
            ),
            SearchNotice::OverLimit { limit } => write!(
                f,
                "{limit}, so it was not run on the server (locally-synced results only)"
            ),
            SearchNotice::CannotCombineMailboxes { first, second } => write!(
                f,
                "this search names two different mailboxes ({first:?} and {second:?}), which the \
                 server can't search together"
            ),
            SearchNotice::NoLabelStorage => {
                f.write_str("this account has no label storage, so the label can't be searched")
            }
            SearchNotice::LabelNotAKeyword { name } => write!(
                f,
                "no message can carry the label {name:?} (it is not a valid server keyword)"
            ),
            SearchNotice::UnknownLabelFolder { name } => {
                write!(f, "there is no label folder named {name:?}")
            }
            SearchNotice::UnknownMailbox { name } => {
                write!(f, "there is no mailbox named {name:?}")
            }
        }
    }
}

// ===========================================================================
// The renderer
// ===========================================================================

/// Turn a parsed [`SearchQuery`] into an [`ImapSearchOutcome`].
///
/// The match on [`QueryTerm`] has NO `_` arm, so adding a new AST term fails to
/// compile here until this renderer decides what to do with it.
pub fn render_imap(query: &SearchQuery, ctx: &ImapSearchContext<'_>) -> ImapSearchOutcome {
    // An empty query is never `ALL`: refuse it.
    if query.terms.is_empty() {
        return ImapSearchOutcome::Skipped {
            reasons: vec![SearchNotice::EmptyQuery],
        };
    }

    // Count the user-supplied terms against the term cap first, each
    // alternative of an `OR` included.
    let term_count = query.terms.iter().map(|term| term.leaves().len()).sum();
    if policy::check_search_terms(term_count).is_err() {
        return ImapSearchOutcome::Skipped {
            reasons: vec![SearchNotice::OverLimit {
                limit: OverLimit::Terms,
            }],
        };
    }

    let mut builder = PlanBuilder::new();

    for term in &query.terms {
        match term {
            QueryTerm::FreeText { text, .. } => {
                builder.push_quoted("TEXT", text, SearchField::FreeText);
            }
            QueryTerm::From(value) => {
                builder.push_quoted("FROM", value, SearchField::From);
            }
            QueryTerm::To(value) => {
                // Gmail's `to:` includes Cc and Bcc, so match all three.
                match quote_or_note(value, SearchField::To) {
                    Ok(quoted) => builder.push_criteria(format!(
                        "(OR OR TO {quoted} CC {quoted} BCC {quoted})"
                    )),
                    Err(notice) => builder.skip(notice),
                }
            }
            QueryTerm::Subject(value) => {
                builder.push_quoted("SUBJECT", value, SearchField::Subject);
            }
            QueryTerm::Date { bound, date } => {
                let key = match bound {
                    DateBound::After => "SINCE",
                    DateBound::Before => "BEFORE",
                };
                builder.push_criteria(format!("{key} {}", format_imap_date(date)));
            }
            QueryTerm::State(MessageState::Unread) => builder.push_criteria("UNSEEN".to_string()),
            QueryTerm::State(MessageState::Starred) => builder.push_criteria("FLAGGED".to_string()),
            QueryTerm::Label(name) => builder.push_label(name, ctx),
            QueryTerm::In(location) => builder.push_in(location, ctx),
            QueryTerm::HasAttachment => builder.local_filters.push(LocalFilter::HasAttachment),
            QueryTerm::Not(_) | QueryTerm::AnyOf(_) => {
                let criterion = match term {
                    QueryTerm::Not(inner) => plain_criterion(inner, ctx).map(|criterion| format!("NOT {criterion}")),
                    QueryTerm::AnyOf(alternatives) => any_of_criterion(alternatives, ctx),
                    _ => unreachable!("matched Not or AnyOf"),
                };
                match criterion {
                    Ok(criterion) => builder.push_criteria(criterion),
                    // Name the whole term the user wrote (`-in:trash`), not
                    // the part inside it that the server can't express.
                    Err(SearchNotice::UnsupportedSyntax { .. }) => builder.skip(SearchNotice::UnsupportedSyntax {
                        token: render_gmail_term(term),
                    }),
                    Err(notice) => builder.skip(notice),
                }
            }
            QueryTerm::Raw(token) => builder.skip(SearchNotice::UnsupportedSyntax {
                token: token.clone(),
            }),
        }
    }

    builder.finish(ctx)
}

/// One parenthesized `SEARCH` key for a term inside `NOT` or `OR`. Only terms
/// that are a plain criterion qualify: a mailbox restriction (`in:`, a label
/// stored as a folder) or a local post-filter (`has:attachment`) cannot be
/// negated or offered as an alternative server-side, nor can a label that no
/// message can carry, so those make the whole search local-only rather than
/// quietly change its meaning.
fn plain_criterion(term: &QueryTerm, ctx: &ImapSearchContext<'_>) -> Result<String, SearchNotice> {
    let unsupported = || SearchNotice::UnsupportedSyntax {
        token: render_gmail_term(term),
    };
    let criterion = match term {
        QueryTerm::FreeText { text, .. } => format!("TEXT {}", quote_or_note(text, SearchField::FreeText)?),
        QueryTerm::From(value) => format!("FROM {}", quote_or_note(value, SearchField::From)?),
        QueryTerm::To(value) => {
            let quoted = quote_or_note(value, SearchField::To)?;
            format!("OR OR TO {quoted} CC {quoted} BCC {quoted}")
        }
        QueryTerm::Subject(value) => format!("SUBJECT {}", quote_or_note(value, SearchField::Subject)?),
        QueryTerm::Date { bound, date } => {
            let key = match bound {
                DateBound::After => "SINCE",
                DateBound::Before => "BEFORE",
            };
            format!("{key} {}", format_imap_date(date))
        }
        QueryTerm::State(MessageState::Unread) => "UNSEEN".to_string(),
        QueryTerm::State(MessageState::Starred) => "FLAGGED".to_string(),
        QueryTerm::Label(name) => {
            check_value(name, SearchField::Label)?;
            match (ctx.label_storage, validate_flag(name)) {
                (LabelStorage::Keywords, Ok(atom)) if !atom.starts_with('\\') => format!("KEYWORD {atom}"),
                _ => return Err(unsupported()),
            }
        }
        QueryTerm::Not(inner) => format!("NOT {}", plain_criterion(inner, ctx)?),
        QueryTerm::In(_) | QueryTerm::HasAttachment | QueryTerm::AnyOf(_) | QueryTerm::Raw(_) => {
            return Err(unsupported());
        }
    };
    Ok(format!("({criterion})"))
}

/// `a OR b OR c` as IMAP's binary `OR`: `OR (a) (OR (b) (c))`.
fn any_of_criterion(alternatives: &[QueryTerm], ctx: &ImapSearchContext<'_>) -> Result<String, SearchNotice> {
    let mut criteria = alternatives
        .iter()
        .map(|alternative| plain_criterion(alternative, ctx))
        .collect::<Result<Vec<_>, _>>()?;
    // Each criterion is already one parenthesized key; only a nested `OR`
    // needs wrapping to stay one key.
    let mut combined = criteria.pop().ok_or(SearchNotice::EmptyQuery)?;
    while let Some(previous) = criteria.pop() {
        let right = if combined.starts_with("OR ") { format!("({combined})") } else { combined };
        combined = format!("OR {previous} {right}");
    }
    Ok(combined)
}

// ===========================================================================
// Plan building
// ===========================================================================

/// Accumulates the criteria terms, mailbox restriction, local filters and any
/// skip/no-match reasons as the renderer walks the AST. `finish` resolves the
/// accumulated state into the final [`ImapSearchOutcome`].
struct PlanBuilder {
    /// Server-side criteria fragments, in AST order (an implicit AND).
    criteria: Vec<String>,
    /// Terms the caller must apply locally.
    local_filters: Vec<LocalFilter>,
    /// A mailbox restriction from an `in:` or folder `label:` term, with the
    /// human token that asked for it (for a "cannot combine" message). `None`
    /// means "no restriction yet" (the default set is used).
    restriction: Option<MailboxRestriction>,
    /// Reasons that force a `Skipped` outcome (seen `Raw`, invalid chars, a
    /// cross-mailbox combination, an over-limit value).
    skip_reasons: Vec<SearchNotice>,
    /// A reason the server can prove the result is empty (unknown label /
    /// folder / mailbox, a non-keyword label). The FIRST one wins the outcome
    /// when nothing forced a skip.
    no_match: Option<SearchNotice>,
}

/// A resolved mailbox restriction: the exact wire names to search, plus the
/// human token that requested them so a conflicting second restriction can be
/// named.
struct MailboxRestriction {
    mailboxes: Vec<String>,
    requested_by: String,
}

impl PlanBuilder {
    fn new() -> Self {
        Self {
            criteria: Vec::new(),
            local_filters: Vec::new(),
            restriction: None,
            skip_reasons: Vec::new(),
            no_match: None,
        }
    }

    /// Push a `KEY "value"` criterion, or record the reason the value could
    /// not be quoted (invalid characters / over-limit).
    fn push_quoted(&mut self, key: &str, value: &str, field: SearchField) {
        match quote_or_note(value, field) {
            Ok(quoted) => self.push_criteria(format!("{key} {quoted}")),
            Err(notice) => self.skip(notice),
        }
    }

    fn push_criteria(&mut self, fragment: String) {
        self.criteria.push(fragment);
    }

    fn skip(&mut self, notice: SearchNotice) {
        self.skip_reasons.push(notice);
    }

    /// Record a "the answer is nothing" reason, keeping the first seen.
    fn no_match(&mut self, notice: SearchNotice) {
        if self.no_match.is_none() {
            self.no_match = Some(notice);
        }
    }

    /// Apply a mailbox restriction, detecting a conflict with an earlier one
    /// that named a DIFFERENT mailbox set (which cannot be one server search).
    fn restrict(&mut self, mailboxes: Vec<String>, requested_by: String) {
        match &self.restriction {
            Some(existing) if existing.mailboxes != mailboxes => {
                self.skip(SearchNotice::CannotCombineMailboxes {
                    first: existing.requested_by.clone(),
                    second: requested_by,
                });
            }
            Some(_) => {} // Same restriction twice: harmless.
            None => {
                self.restriction = Some(MailboxRestriction {
                    mailboxes,
                    requested_by,
                });
            }
        }
    }

    /// Handle a `label:` term, which depends on the account's label storage.
    fn push_label(&mut self, name: &str, ctx: &ImapSearchContext<'_>) {
        // Reject control characters / over-length first, whatever the mode.
        if let Err(notice) = check_value(name, SearchField::Label) {
            self.skip(notice);
            return;
        }
        match ctx.label_storage {
            LabelStorage::Keywords => {
                // A keyword label is searched with `KEYWORD <atom>`, using the
                // SAME name->keyword validation the write path uses
                // (`write::validate_flag`): a user label is stored as a bare
                // atom keyword, so a name that is not a valid atom can never
                // have been stored, and no message can carry it.
                match validate_flag(name) {
                    Ok(atom) if !atom.starts_with('\\') => {
                        self.push_criteria(format!("KEYWORD {atom}"));
                    }
                    // A valid system flag spelling or an invalid atom is not a
                    // storable user keyword — no message carries it.
                    _ => self.no_match(SearchNotice::LabelNotAKeyword {
                        name: name.to_string(),
                    }),
                }
            }
            LabelStorage::Folders => {
                // The label is a folder: restrict the search to that folder's
                // mailbox and DROP the term from the criteria. An unknown label
                // folder means nothing matches.
                match self.label_folder_mailbox(name, ctx) {
                    Some(mailbox) => self.restrict(vec![mailbox], format!("label:{name}")),
                    None => self.no_match(SearchNotice::UnknownLabelFolder {
                        name: name.to_string(),
                    }),
                }
            }
            LabelStorage::None => self.no_match(SearchNotice::NoLabelStorage),
        }
    }

    /// Resolve a label-folder name to its mailbox wire name, confirming the
    /// plan actually knows that `lf:` label (so an unknown label is caught).
    fn label_folder_mailbox(&self, name: &str, ctx: &ImapSearchContext<'_>) -> Option<String> {
        let container = ctx.label_container?;
        let delimiter = hierarchy_delimiter(ctx.delimiter);
        let mailbox = format!(
            "{container}{delimiter}{}",
            super::write::encode_modified_utf7(name)
        );
        // The plan must carry this mailbox as a label folder; otherwise the
        // label does not exist for this account.
        let entry = ctx.plan.entry_for(&mailbox)?;
        (entry.label_kind == LabelKind::LabelFolder).then_some(mailbox)
    }

    /// Handle an `in:` term: restrict the search to the named location's
    /// mailbox(es). Supports the system locations plus a case-insensitive user
    /// folder match. An unknown location means nothing matches.
    fn push_in(&mut self, location: &str, ctx: &ImapSearchContext<'_>) {
        if let Err(notice) = check_value(location, SearchField::In) {
            self.skip(notice);
            return;
        }
        match resolve_location(location, ctx.plan) {
            Some(mailboxes) => self.restrict(mailboxes, format!("in:{location}")),
            None => self.no_match(SearchNotice::UnknownMailbox {
                name: location.to_string(),
            }),
        }
    }

    /// Resolve the accumulated state into the final outcome.
    fn finish(mut self, ctx: &ImapSearchContext<'_>) -> ImapSearchOutcome {
        // A term we cannot honestly evaluate forces Skipped regardless of
        // anything else — the caller keeps local results only.
        if !self.skip_reasons.is_empty() {
            return ImapSearchOutcome::Skipped {
                reasons: self.skip_reasons,
            };
        }
        // A term the server can prove matches nothing wins next.
        if let Some(reason) = self.no_match.take() {
            return ImapSearchOutcome::NoServerMatches { reason };
        }

        // Determine the mailbox set: an explicit restriction, else the default
        // (INBOX, Archive, Sent, present-in-plan only).
        let mailboxes = match self.restriction.take() {
            Some(restriction) => restriction.mailboxes,
            None => default_search_mailboxes(ctx.plan),
        };
        if policy::check_search_target_mailboxes(mailboxes.len()).is_err() {
            return ImapSearchOutcome::Skipped {
                reasons: vec![SearchNotice::OverLimit {
                    limit: OverLimit::TargetMailboxes,
                }],
            };
        }
        // A query made only of a mailbox restriction and/or a local filter has
        // no server criteria. We never send `ALL`; such a query matches
        // everything in the mailbox set, so fall back to the server-cheap
        // `UNDELETED` only if there is at least one real criterion. With no
        // criteria at all the server search is meaningless — skip it.
        if self.criteria.is_empty() {
            return ImapSearchOutcome::Skipped {
                reasons: vec![SearchNotice::EmptyQuery],
            };
        }

        let criteria = format!("CHARSET UTF-8 {}", self.criteria.join(" "));
        // Guard the whole rendered criteria length (the raw query bound).
        if policy::check_search_query_bytes(criteria.len()).is_err() {
            return ImapSearchOutcome::Skipped {
                reasons: vec![SearchNotice::OverLimit {
                    limit: OverLimit::QueryBytes,
                }],
            };
        }

        ImapSearchOutcome::Plan(ImapSearchPlan {
            mailboxes,
            criteria,
            local_filters: self.local_filters,
        })
    }
}

// ===========================================================================
// Mailbox resolution
// ===========================================================================

/// The default mailbox set when no `in:`/folder `label:` term restricts it:
/// INBOX, the Archive-role mailbox and the Sent-role mailbox, each only when
/// present in the plan, in that deterministic order. Trash and Junk are never
/// searched unless asked for explicitly.
fn default_search_mailboxes(plan: &SyncPlan) -> Vec<String> {
    use super::mailboxes::MailboxRole;
    let mut out = Vec::new();
    if let Some(inbox) = plan.entries.iter().find(|e| e.is_inbox) {
        out.push(inbox.mailbox.clone());
    }
    for role in [MailboxRole::Archive, MailboxRole::Sent] {
        if let Some(entry) = plan.entries.iter().find(|e| e.role == Some(role)) {
            out.push(entry.mailbox.clone());
        }
    }
    out
}

/// Resolve an `in:` location token to the mailbox wire name(s) to search:
/// `inbox`, `sent`, `trash`, `spam`/`junk`, `archive`, or a user folder matched
/// case-insensitively against the plan's mailbox names. Returns `None` for an
/// unknown location.
fn resolve_location(location: &str, plan: &SyncPlan) -> Option<Vec<String>> {
    use super::mailboxes::MailboxRole;
    let lower = location.to_ascii_lowercase();
    let role = match lower.as_str() {
        "inbox" => {
            return plan
                .entries
                .iter()
                .find(|e| e.is_inbox)
                .map(|e| vec![e.mailbox.clone()]);
        }
        "sent" => Some(MailboxRole::Sent),
        "trash" => Some(MailboxRole::Trash),
        "spam" | "junk" => Some(MailboxRole::Junk),
        "archive" => Some(MailboxRole::Archive),
        _ => None,
    };
    if let Some(role) = role {
        return plan
            .entries
            .iter()
            .find(|e| e.role == Some(role))
            .map(|e| vec![e.mailbox.clone()]);
    }
    // A user folder: match the token case-insensitively against a synced
    // mailbox's wire name. Only a selectable, synced mailbox is a valid target.
    plan.entries
        .iter()
        .find(|e| {
            e.synced_now
                && !e.is_inbox
                && super::write::decode_modified_utf7(&e.mailbox)
                    .unwrap_or_else(|_| e.mailbox.clone())
                    .eq_ignore_ascii_case(location)
        })
        .map(|e| vec![e.mailbox.clone()])
}

// ===========================================================================
// Wire safety + value checks
// ===========================================================================

/// Check a user value for the control characters that would break out of a
/// quoted string, and for the per-term byte limit. Returns the reason to skip
/// the whole server search when it fails. This is the gate every user value
/// passes before it is quoted.
fn check_value(value: &str, field: SearchField) -> Result<(), SearchNotice> {
    if value.contains(['\r', '\n', '\0']) {
        return Err(SearchNotice::InvalidCharacters { field });
    }
    if policy::check_search_term_bytes(value.len()).is_err() {
        return Err(SearchNotice::OverLimit {
            limit: OverLimit::TermBytes,
        });
    }
    Ok(())
}

/// Check then quote a user value, mapping a failure to its [`SearchNotice`].
fn quote_or_note(value: &str, field: SearchField) -> Result<String, SearchNotice> {
    check_value(value, field)?;
    Ok(quote_search_string(value))
}

/// Quote a value for an IMAP `SEARCH` string: wrap in `"` with `\` and `"`
/// escaped. The caller MUST have rejected CR/LF/NUL first (via [`check_value`])
/// — this function assumes those are absent. Non-ASCII bytes pass through as
/// UTF-8 (the command carries `CHARSET UTF-8`); NO literal (`{n}`) is ever
/// emitted.
fn quote_search_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Format a date for an IMAP `SINCE`/`BEFORE` key: `d-Mon-yyyy` with the day
/// UNPADDED and a FIXED ENGLISH month abbreviation (never the locale), e.g.
/// `1-Jan-2024`. IMAP date comparisons are server-timezone, date-only
/// (RFC 3501 §6.4.4).
fn format_imap_date(date: &chrono::NaiveDate) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // `month()` is 1..=12, so the index is month-1.
    let month = MONTHS[(date.month() - 1) as usize];
    format!("{}-{month}-{:04}", date.day(), date.year())
}

/// The hierarchy delimiter, defaulting to `/` when `None` or empty — the same
/// rule the plan and provider use.
fn hierarchy_delimiter(delimiter: Option<&str>) -> &str {
    delimiter.filter(|d| !d.is_empty()).unwrap_or("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::plan::{build_plan, CatalogMailbox};
    use crate::provider::imap::settings::{ImapAccountSettings, SecurityMode};
    use std::collections::BTreeMap;

    // --- fixtures ---------------------------------------------------------

    fn settings(label_storage: LabelStorage, container: Option<&str>) -> ImapAccountSettings {
        ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1143,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@proton.me".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_security: SecurityMode::StartTls,
            smtp_username: "me@proton.me".into(),
            mailbox_overrides: BTreeMap::new(),
            archive_mailbox: None,
            label_storage,
            label_container: container.map(str::to_string),
            identities: vec![],
            pinned_fingerprints: BTreeMap::new(),
            server_saves_sent: false,
        }
    }

    fn mb(name: &str, special_use: Option<&str>) -> CatalogMailbox {
        CatalogMailbox {
            name: name.into(),
            special_use: special_use.map(str::to_string),
        }
    }

    /// A standard plan: INBOX, Sent, Archive, Trash, Junk, a user folder, and
    /// (in folder-label mode) a label folder under `Labels`.
    fn standard_plan(config: &ImapAccountSettings) -> SyncPlan {
        let catalog = vec![
            mb("INBOX", None),
            mb("Sent", Some("\\Sent")),
            mb("Archive", Some("\\Archive")),
            mb("Trash", Some("\\Trash")),
            mb("Spam", Some("\\Junk")),
            mb("Clients/Acme", None),  // a user folder
            mb("Labels/Work", None),   // a label folder (folder mode)
        ];
        build_plan(&catalog, Some("/"), config)
    }

    fn ctx<'a>(plan: &'a SyncPlan, config: &'a ImapAccountSettings) -> ImapSearchContext<'a> {
        ImapSearchContext {
            plan,
            label_storage: config.label_storage,
            label_container: config.label_container.as_deref(),
            delimiter: Some("/"),
        }
    }

    #[test]
    fn human_folder_names_resolve_to_the_original_wire_mailboxes() {
        let config = settings(LabelStorage::Folders, Some("Labels"));
        for display in ["R&D", "Décisions", "日本語"] {
            let wire = super::super::write::encode_modified_utf7(display);
            let label_mailbox = format!("Labels/{wire}");
            let plan = build_plan(
                &[mb(&wire, None), mb(&label_mailbox, None)],
                Some("/"),
                &config,
            );
            for (query, expected) in [
                (format!("in:{display} from:alice"), wire.clone()),
                (format!("label:{display} from:alice"), label_mailbox.clone()),
            ] {
                let outcome = render_imap(&SearchQuery::parse(&query), &ctx(&plan, &config));
                assert_eq!(plan_of(&outcome).mailboxes, vec![expected], "{query}");
                assert_eq!(criteria_of(&outcome), "CHARSET UTF-8 FROM \"alice\"");
            }
        }
    }

    fn render(input: &str, label_storage: LabelStorage) -> ImapSearchOutcome {
        let config = settings(label_storage, Some("Labels"));
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        render_imap(&SearchQuery::parse(input), &context)
    }

    /// The criteria of a `Plan` outcome, or panic.
    fn criteria_of(outcome: &ImapSearchOutcome) -> &str {
        match outcome {
            ImapSearchOutcome::Plan(plan) => &plan.criteria,
            other => panic!("expected a Plan, got {other:?}"),
        }
    }

    fn plan_of(outcome: &ImapSearchOutcome) -> ImapSearchPlan {
        match outcome {
            ImapSearchOutcome::Plan(plan) => plan.clone(),
            other => panic!("expected a Plan, got {other:?}"),
        }
    }

    // --- per-term rendering ----------------------------------------------

    #[test]
    fn free_text_renders_text_quoted() {
        assert_eq!(criteria_of(&render("hello", LabelStorage::Keywords)), "CHARSET UTF-8 TEXT \"hello\"");
        // A quoted phrase renders the same TEXT key (quoting is a parse detail).
        assert_eq!(
            criteria_of(&render("\"quarterly report\"", LabelStorage::Keywords)),
            "CHARSET UTF-8 TEXT \"quarterly report\""
        );
    }

    #[test]
    fn from_subject_and_to_render_their_keys() {
        assert_eq!(
            criteria_of(&render("from:alice@example.com", LabelStorage::Keywords)),
            "CHARSET UTF-8 FROM \"alice@example.com\""
        );
        assert_eq!(
            criteria_of(&render("subject:update", LabelStorage::Keywords)),
            "CHARSET UTF-8 SUBJECT \"update\""
        );
        // to: includes Cc and Bcc, matching Gmail.
        assert_eq!(
            criteria_of(&render("to:bob", LabelStorage::Keywords)),
            "CHARSET UTF-8 (OR OR TO \"bob\" CC \"bob\" BCC \"bob\")"
        );
    }

    #[test]
    fn date_terms_use_fixed_english_months_unpadded_day() {
        assert_eq!(
            criteria_of(&render("after:2024-01-01", LabelStorage::Keywords)),
            "CHARSET UTF-8 SINCE 1-Jan-2024"
        );
        assert_eq!(
            criteria_of(&render("before:2023/12/31", LabelStorage::Keywords)),
            "CHARSET UTF-8 BEFORE 31-Dec-2023"
        );
        // A leap day.
        assert_eq!(
            criteria_of(&render("after:2024-02-29", LabelStorage::Keywords)),
            "CHARSET UTF-8 SINCE 29-Feb-2024"
        );
    }

    #[test]
    fn all_twelve_months_render_in_english_independent_of_locale() {
        let expected = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        for (index, name) in expected.iter().enumerate() {
            let month = index + 1;
            let input = format!("after:2024-{month:02}-15");
            assert_eq!(
                criteria_of(&render(&input, LabelStorage::Keywords)),
                format!("CHARSET UTF-8 SINCE 15-{name}-2024"),
                "month {month}"
            );
        }
    }

    #[test]
    fn state_terms_render_unseen_and_flagged() {
        assert_eq!(criteria_of(&render("is:unread", LabelStorage::Keywords)), "CHARSET UTF-8 UNSEEN");
        assert_eq!(criteria_of(&render("is:starred", LabelStorage::Keywords)), "CHARSET UTF-8 FLAGGED");
    }

    #[test]
    fn has_attachment_is_a_local_filter_but_needs_a_server_criterion() {
        // has:attachment alone has no server criterion -> Skipped (empty query
        // on the server), never ALL.
        assert!(matches!(
            render("has:attachment", LabelStorage::Keywords),
            ImapSearchOutcome::Skipped { .. }
        ));
        // Combined with a real term, it is carried as a local filter.
        let outcome = render("from:alice has:attachment", LabelStorage::Keywords);
        let plan = plan_of(&outcome);
        assert_eq!(plan.criteria, "CHARSET UTF-8 FROM \"alice\"");
        assert_eq!(plan.local_filters, vec![LocalFilter::HasAttachment]);
    }

    #[test]
    fn raw_terms_are_skipped_never_sent_as_text() {
        // An unknown operator and mixed grouping are Raw; a negated or OR'd
        // location is modelled but has no server criterion. None is ever sent
        // as TEXT.
        for input in ["-in:trash", "in:inbox OR in:sent", "(a b OR c)", "category:promotions", "{from:a has:attachment}"] {
            match render(input, LabelStorage::Keywords) {
                ImapSearchOutcome::Skipped { reasons } => {
                    assert!(
                        reasons.iter().any(|r| matches!(r, SearchNotice::UnsupportedSyntax { .. })),
                        "{input}: {reasons:?}"
                    );
                }
                other => panic!("{input}: expected Skipped, got {other:?}"),
            }
        }
    }

    #[test]
    fn negation_renders_not() {
        assert_eq!(
            criteria_of(&render("-from:alice roadmap", LabelStorage::Keywords)),
            "CHARSET UTF-8 NOT (FROM \"alice\") TEXT \"roadmap\""
        );
        assert_eq!(
            criteria_of(&render("-label:work", LabelStorage::Keywords)),
            "CHARSET UTF-8 NOT (KEYWORD work)"
        );
    }

    #[test]
    fn alternatives_render_nested_binary_or() {
        assert_eq!(
            criteria_of(&render("from:a OR from:b", LabelStorage::Keywords)),
            "CHARSET UTF-8 OR (FROM \"a\") (FROM \"b\")"
        );
        assert_eq!(
            criteria_of(&render("{from:a from:b -is:unread}", LabelStorage::Keywords)),
            "CHARSET UTF-8 OR (FROM \"a\") (OR (FROM \"b\") (NOT (UNSEEN)))"
        );
        // `to:` keeps covering Cc and Bcc inside an alternative.
        assert_eq!(
            criteria_of(&render("to:a OR is:starred", LabelStorage::Keywords)),
            "CHARSET UTF-8 OR (OR OR TO \"a\" CC \"a\" BCC \"a\") (FLAGGED)"
        );
    }

    #[test]
    fn a_skip_names_the_whole_negated_term() {
        match render("in:sent -in:trash", LabelStorage::Keywords) {
            ImapSearchOutcome::Skipped { reasons } => assert_eq!(
                reasons,
                vec![SearchNotice::UnsupportedSyntax { token: "-in:trash".into() }]
            ),
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[test]
    fn alternatives_count_toward_the_term_cap() {
        let at_cap = (0..policy::MAX_SEARCH_TERMS).map(|i| format!("from:a{i}")).collect::<Vec<_>>().join(" OR ");
        assert!(matches!(render(&at_cap, LabelStorage::Keywords), ImapSearchOutcome::Plan(_)));
        let over = format!("{at_cap} OR from:extra");
        assert_eq!(
            render(&over, LabelStorage::Keywords),
            ImapSearchOutcome::Skipped { reasons: vec![SearchNotice::OverLimit { limit: OverLimit::Terms }] }
        );
    }

    // --- labels -----------------------------------------------------------

    #[test]
    fn a_keyword_label_renders_keyword_atom() {
        assert_eq!(
            criteria_of(&render("label:work", LabelStorage::Keywords)),
            "CHARSET UTF-8 KEYWORD work"
        );
        assert_eq!(
            criteria_of(&render("label:$Important", LabelStorage::Keywords)),
            "CHARSET UTF-8 KEYWORD $Important"
        );
    }

    #[test]
    fn a_keyword_label_that_is_not_an_atom_is_no_server_matches() {
        // A label with a space cannot be a bare keyword atom, so no message
        // carries it.
        let config = settings(LabelStorage::Keywords, None);
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        // `label:"two words"` parses to Label("two words").
        let outcome = render_imap(&SearchQuery::parse("label:\"two words\""), &context);
        match outcome {
            ImapSearchOutcome::NoServerMatches {
                reason: SearchNotice::LabelNotAKeyword { name },
            } => assert_eq!(name, "two words"),
            other => panic!("expected NoServerMatches, got {other:?}"),
        }
    }

    #[test]
    fn a_folder_label_restricts_the_mailbox_and_drops_the_criterion() {
        // `label:Work` under folder mode -> search only Labels/Work, no KEYWORD.
        let config = settings(LabelStorage::Folders, Some("Labels"));
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        // Combine with a real criterion so the plan is non-empty.
        let outcome = render_imap(&SearchQuery::parse("label:Work from:alice"), &context);
        let plan = plan_of(&outcome);
        assert_eq!(plan.mailboxes, vec!["Labels/Work"]);
        assert_eq!(plan.criteria, "CHARSET UTF-8 FROM \"alice\"", "the label term is dropped");
    }

    #[test]
    fn an_unknown_folder_label_is_no_server_matches() {
        let config = settings(LabelStorage::Folders, Some("Labels"));
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        let outcome = render_imap(&SearchQuery::parse("label:Nope"), &context);
        assert!(matches!(
            outcome,
            ImapSearchOutcome::NoServerMatches {
                reason: SearchNotice::UnknownLabelFolder { .. }
            }
        ));
    }

    #[test]
    fn a_label_under_no_storage_is_no_server_matches() {
        let outcome = render("label:work", LabelStorage::None);
        assert!(matches!(
            outcome,
            ImapSearchOutcome::NoServerMatches {
                reason: SearchNotice::NoLabelStorage
            }
        ));
    }

    // --- in: / mailbox selection -----------------------------------------

    #[test]
    fn default_mailboxes_are_inbox_archive_sent_in_order() {
        let plan = plan_of(&render("from:alice", LabelStorage::Keywords));
        assert_eq!(plan.mailboxes, vec!["INBOX", "Archive", "Sent"]);
    }

    #[test]
    fn in_terms_resolve_system_locations_and_user_folders() {
        for (input, expected) in [
            ("in:inbox from:a", "INBOX"),
            ("in:sent from:a", "Sent"),
            ("in:archive from:a", "Archive"),
            ("in:trash from:a", "Trash"),
            ("in:spam from:a", "Spam"),
            ("in:junk from:a", "Spam"),
            ("in:Clients/Acme from:a", "Clients/Acme"),
        ] {
            let plan = plan_of(&render(input, LabelStorage::Keywords));
            assert_eq!(plan.mailboxes, vec![expected.to_string()], "{input}");
        }
    }

    #[test]
    fn in_match_is_case_insensitive_for_user_folders() {
        let plan = plan_of(&render("in:clients/acme from:a", LabelStorage::Keywords));
        assert_eq!(plan.mailboxes, vec!["Clients/Acme"]);
    }

    #[test]
    fn an_unknown_in_location_is_no_server_matches() {
        let outcome = render("in:nowhere from:a", LabelStorage::Keywords);
        assert!(matches!(
            outcome,
            ImapSearchOutcome::NoServerMatches {
                reason: SearchNotice::UnknownMailbox { .. }
            }
        ));
    }

    #[test]
    fn two_different_mailboxes_cannot_be_combined() {
        // in:inbox plus a label folder name different mailboxes -> Skipped.
        let config = settings(LabelStorage::Folders, Some("Labels"));
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        let outcome = render_imap(&SearchQuery::parse("in:inbox label:Work from:a"), &context);
        match outcome {
            ImapSearchOutcome::Skipped { reasons } => assert!(
                reasons
                    .iter()
                    .any(|r| matches!(r, SearchNotice::CannotCombineMailboxes { .. })),
                "{reasons:?}"
            ),
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[test]
    fn the_same_mailbox_named_twice_is_not_a_conflict() {
        // in:inbox twice is harmless.
        let plan = plan_of(&render("in:inbox in:inbox from:a", LabelStorage::Keywords));
        assert_eq!(plan.mailboxes, vec!["INBOX"]);
    }

    #[test]
    fn a_mailbox_absent_from_the_plan_is_not_in_the_default_set() {
        // A plan with only INBOX: Archive/Sent are not searched.
        let config = settings(LabelStorage::Keywords, None);
        let plan = build_plan(&[mb("INBOX", None)], Some("/"), &config);
        let context = ctx(&plan, &config);
        let outcome = render_imap(&SearchQuery::parse("from:a"), &context);
        assert_eq!(plan_of(&outcome).mailboxes, vec!["INBOX"]);
    }

    // --- empty query ------------------------------------------------------

    #[test]
    fn an_empty_query_is_skipped_never_all() {
        let config = settings(LabelStorage::Keywords, None);
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        let outcome = render_imap(&SearchQuery::default(), &context);
        assert!(matches!(
            outcome,
            ImapSearchOutcome::Skipped { reasons } if reasons == vec![SearchNotice::EmptyQuery]
        ));
    }

    // --- combinations + determinism --------------------------------------

    #[test]
    fn a_combination_renders_each_term_in_order() {
        let outcome = render(
            "from:alice subject:\"q3 report\" is:unread after:2024-01-01",
            LabelStorage::Keywords,
        );
        assert_eq!(
            criteria_of(&outcome),
            "CHARSET UTF-8 FROM \"alice\" SUBJECT \"q3 report\" UNSEEN SINCE 1-Jan-2024"
        );
    }

    #[test]
    fn the_same_query_and_plan_render_byte_identically() {
        let input = "from:alice subject:hi is:starred in:inbox has:attachment";
        let first = render(input, LabelStorage::Keywords);
        let second = render(input, LabelStorage::Keywords);
        assert_eq!(first, second);
        let plan = plan_of(&first);
        assert_eq!(plan.mailboxes, vec!["INBOX"]);
        assert_eq!(plan.local_filters, vec![LocalFilter::HasAttachment]);
    }

    // --- wire safety (hostile inputs) ------------------------------------

    /// Tokenise a rendered criteria string into balanced quoted strings and
    /// bare keys, so a test can assert no user value ever introduces a new key
    /// and no CR/LF/NUL ever appears. Returns `(keys, quoted_values)`.
    fn tokenise(criteria: &str) -> (Vec<String>, Vec<String>) {
        assert!(
            !criteria.contains(['\r', '\n', '\u{0}']),
            "criteria must never contain CR/LF/NUL: {criteria:?}"
        );
        let mut keys = Vec::new();
        let mut quoted = Vec::new();
        let mut chars = criteria.chars().peekable();
        let mut current = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    // A quoted string: consume to the matching unescaped quote.
                    if !current.trim().is_empty() {
                        keys.extend(current.split_whitespace().map(str::to_string));
                    }
                    current.clear();
                    let mut value = String::new();
                    while let Some(c) = chars.next() {
                        if c == '\\' {
                            // Escaped char: keep the next literally.
                            if let Some(next) = chars.next() {
                                value.push(next);
                            }
                        } else if c == '"' {
                            break;
                        } else {
                            value.push(c);
                        }
                    }
                    quoted.push(value);
                }
                _ => current.push(c),
            }
        }
        if !current.trim().is_empty() {
            keys.extend(current.split_whitespace().map(str::to_string));
        }
        (keys, quoted)
    }

    #[test]
    fn hostile_free_text_is_contained_in_one_balanced_quoted_string() {
        // Each hostile value must appear ONLY as one balanced quoted string,
        // never introducing a new key.
        for hostile in [
            "a\"b",         // embedded quote
            "a\\b",         // backslash
            "a)b(c",        // parens
            "a{5}b",        // literal-looking
            "a}b",
            "a*b%c",        // wildcards
            "  leading",    // leading spaces
            "trailing  ",
            "ALL",          // a value that looks like a search key
            "OR",
            "UNSEEN",
            "a\"unclosed",  // an unclosed quote in the value
        ] {
            // Use a from: term so the key is known (FROM).
            let input = format!("from:{hostile}");
            // The parser may split on spaces; use a quoted phrase to keep it one
            // term where spaces matter.
            let input = if hostile.contains(' ') {
                format!("from:\"{}\"", hostile.replace('"', ""))
            } else {
                input
            };
            let outcome = render(&input, LabelStorage::Keywords);
            // Some of these (none here) could be Skipped; all these are valid
            // values, so expect a Plan.
            let plan = plan_of(&outcome);
            let (keys, quoted) = tokenise(&plan.criteria);
            // The only keys allowed are the CHARSET prefix and FROM — a value
            // never becomes a key.
            for key in &keys {
                assert!(
                    matches!(key.as_str(), "CHARSET" | "UTF-8" | "FROM"),
                    "value {hostile:?} introduced a stray key {key:?} in {:?}",
                    plan.criteria
                );
            }
            assert_eq!(quoted.len(), 1, "exactly one quoted value for {hostile:?}");
        }
    }

    #[test]
    fn cr_lf_and_nul_in_a_value_skip_the_server_search() {
        for bad in ["a\rb", "a\nb", "a\u{0}b"] {
            // Build the AST directly so the control char survives (the parser
            // tokenises on whitespace but CR/LF are whitespace too; construct
            // the term explicitly).
            let query = SearchQuery {
                terms: vec![QueryTerm::From(bad.to_string())],
            };
            let config = settings(LabelStorage::Keywords, None);
            let plan = standard_plan(&config);
            let context = ctx(&plan, &config);
            match render_imap(&query, &context) {
                ImapSearchOutcome::Skipped { reasons } => assert!(
                    reasons
                        .iter()
                        .any(|r| matches!(r, SearchNotice::InvalidCharacters { .. })),
                    "{bad:?}: {reasons:?}"
                ),
                other => panic!("{bad:?}: expected Skipped, got {other:?}"),
            }
        }
    }

    #[test]
    fn non_ascii_passes_through_as_utf8_in_the_quoted_string() {
        for value in ["Éléments", "日本語", "café"] {
            let outcome = render(&format!("subject:{value}"), LabelStorage::Keywords);
            assert_eq!(
                criteria_of(&outcome),
                format!("CHARSET UTF-8 SUBJECT \"{value}\""),
                "non-ASCII must survive verbatim as UTF-8"
            );
        }
    }

    #[test]
    fn a_keyword_folder_or_in_name_that_looks_like_a_search_key_cannot_inject() {
        // A folder named literally "UNSEEN" is matched by name and quoted into
        // a mailbox restriction (never into the criteria), so it cannot inject.
        // ("UNSEEN" is not a mailbox-role name, so it stays a synced user
        // folder; "ALL" would resolve to the \All aggregate role instead.)
        let config = settings(LabelStorage::Keywords, None);
        let plan = build_plan(&[mb("INBOX", None), mb("UNSEEN", None)], Some("/"), &config);
        let context = ctx(&plan, &config);
        let outcome = render_imap(&SearchQuery::parse("in:UNSEEN from:a"), &context);
        let rendered = plan_of(&outcome);
        assert_eq!(rendered.mailboxes, vec!["UNSEEN"]);
        assert_eq!(rendered.criteria, "CHARSET UTF-8 FROM \"a\"");
    }

    #[test]
    fn a_very_long_term_is_skipped_over_the_limit() {
        let long = "x".repeat(policy::MAX_SEARCH_TERM_BYTES + 1);
        let query = SearchQuery {
            terms: vec![QueryTerm::From(long)],
        };
        let config = settings(LabelStorage::Keywords, None);
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        match render_imap(&query, &context) {
            ImapSearchOutcome::Skipped { reasons } => assert!(reasons.iter().any(|r| matches!(
                r,
                SearchNotice::OverLimit {
                    limit: OverLimit::TermBytes
                }
            ))),
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[test]
    fn too_many_terms_are_skipped() {
        let terms: Vec<QueryTerm> = (0..=policy::MAX_SEARCH_TERMS)
            .map(|i| QueryTerm::From(format!("a{i}")))
            .collect();
        let query = SearchQuery { terms };
        let config = settings(LabelStorage::Keywords, None);
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        match render_imap(&query, &context) {
            ImapSearchOutcome::Skipped { reasons } => assert_eq!(
                reasons,
                vec![SearchNotice::OverLimit {
                    limit: OverLimit::Terms
                }]
            ),
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    // --- notices render plain language -----------------------------------

    #[test]
    fn notices_display_as_plain_language() {
        let samples = [
            SearchNotice::EmptyQuery,
            SearchNotice::UnsupportedSyntax { token: "-in:trash".into() },
            SearchNotice::InvalidCharacters { field: SearchField::From },
            SearchNotice::OverLimit { limit: OverLimit::TermBytes },
            SearchNotice::CannotCombineMailboxes { first: "in:inbox".into(), second: "label:Work".into() },
            SearchNotice::NoLabelStorage,
            SearchNotice::LabelNotAKeyword { name: "two words".into() },
            SearchNotice::UnknownLabelFolder { name: "Nope".into() },
            SearchNotice::UnknownMailbox { name: "nowhere".into() },
        ];
        for notice in samples {
            assert!(!notice.to_string().is_empty());
        }
    }

    /// The parser can never put a `"` inside a term, but the renderer must not
    /// rely on that: an AST built by any other caller still has to stay inside
    /// one balanced quoted string.
    #[test]
    fn ast_values_with_quotes_and_backslashes_stay_in_one_quoted_string() {
        for (value, expected) in [
            ("a\"b", "\"a\\\"b\""),
            ("a\\b", "\"a\\\\b\""),
            ("ends with backslash\\", "\"ends with backslash\\\\\""),
            ("\\\"", "\"\\\\\\\"\""),
            ("\"; UNSEEN \"", "\"\\\"; UNSEEN \\\"\""),
            ("x\") OR ALL (\"", "\"x\\\") OR ALL (\\\"\""),
        ] {
            for term in [
                QueryTerm::From(value.to_string()),
                QueryTerm::Subject(value.to_string()),
                QueryTerm::FreeText {
                    text: value.to_string(),
                    quoted: true,
                },
            ] {
                let config = settings(LabelStorage::Keywords, None);
                let plan = standard_plan(&config);
                let context = ctx(&plan, &config);
                let outcome = render_imap(&SearchQuery { terms: vec![term] }, &context);
                let rendered = plan_of(&outcome);
                assert!(
                    rendered.criteria.ends_with(expected),
                    "{value:?} rendered as {:?}",
                    rendered.criteria
                );
                let (keys, quoted) = tokenise(&rendered.criteria);
                assert_eq!(quoted, vec![value.to_string()], "{value:?}");
                for key in &keys {
                    assert!(
                        matches!(key.as_str(), "CHARSET" | "UTF-8" | "FROM" | "SUBJECT" | "TEXT"),
                        "{value:?} introduced stray key {key:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn to_term_repeats_one_quoted_value_three_times_and_nothing_else() {
        let term = QueryTerm::To("a\"b\\".to_string());
        let config = settings(LabelStorage::Keywords, None);
        let plan = standard_plan(&config);
        let context = ctx(&plan, &config);
        let outcome = render_imap(&SearchQuery { terms: vec![term] }, &context);
        assert_eq!(
            criteria_of(&outcome),
            "CHARSET UTF-8 (OR OR TO \"a\\\"b\\\\\" CC \"a\\\"b\\\\\" BCC \"a\\\"b\\\\\")"
        );
    }
}

// ===========================================================================
// Gated live test (Dovecot container)
// ===========================================================================
//
// Compiles ALWAYS; runs only when THREESTRANDS_IMAP_IT=1 AND the Dovecot
// container's fingerprint is in DOVECOT_TEST_FP, exactly like the live tests in
// `provider/tests/live.rs` / `fetch.rs`. It creates a SCRATCH mailbox it also deletes
// (the harness's seed messages are never touched), APPENDs a handful of known
// messages, then renders each case through `render_imap` and runs the REAL
// criteria with `ImapSession::uid_search`, asserting the exact UID set. This
// worker's sandbox has NO docker, so this did NOT run here; it is written to
// compile and skip cleanly, and the reviewer runs it against the container.
#[cfg(test)]
mod live_search {
    use super::*;
    use crate::provider::imap::connection::{
        ConnectionConfig, ConnectionRole, ImapConnectionManager, TlsMode,
    };
    use crate::provider::imap::plan::{build_plan, CatalogMailbox};
    use crate::provider::imap::session::ImapSession;
    use crate::provider::imap::settings::{ImapAccountSettings, SecurityMode};
    use crate::provider::imap::tls::parse_sha256_fingerprint;
    use std::collections::BTreeMap;

    const HOST: &str = "127.0.0.1";
    const PORT: u16 = 11143;
    const USER: &str = "test@threestrands.test";
    const PASSWORD: &str = "testpassword";
    // A scratch mailbox name no seed uses; created and deleted by this test.
    const SCRATCH: &str = "P5R1-Search-Scratch";

    fn gated() -> Option<ConnectionConfig> {
        if std::env::var("THREESTRANDS_IMAP_IT").ok().as_deref() != Some("1") {
            eprintln!("skipping live_search: THREESTRANDS_IMAP_IT != 1 (no Dovecot container)");
            return None;
        }
        let fp = parse_sha256_fingerprint(&std::env::var("DOVECOT_TEST_FP").ok()?)
            .expect("DOVECOT_TEST_FP must be 64 hex digits");
        Some(ConnectionConfig {
            host: HOST.into(),
            port: PORT,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(fp),
        })
    }

    fn live_settings() -> ImapAccountSettings {
        ImapAccountSettings {
            imap_host: HOST.into(),
            imap_port: PORT,
            imap_security: SecurityMode::StartTls,
            imap_username: USER.into(),
            smtp_host: HOST.into(),
            smtp_port: 1025,
            smtp_security: SecurityMode::StartTls,
            smtp_username: USER.into(),
            mailbox_overrides: BTreeMap::new(),
            archive_mailbox: None,
            label_storage: LabelStorage::Keywords,
            label_container: None,
            identities: vec![],
            pinned_fingerprints: BTreeMap::new(),
            server_saves_sent: false,
        }
    }

    /// A plan whose only searchable mailbox is the scratch mailbox, so the
    /// renderer targets it directly via `in:`.
    fn scratch_plan(config: &ImapAccountSettings) -> SyncPlan {
        build_plan(
            &[
                CatalogMailbox { name: "INBOX".into(), special_use: None },
                CatalogMailbox { name: SCRATCH.into(), special_use: None },
            ],
            Some("/"),
            config,
        )
    }

    /// `extra` is zero or more complete `Name: value\r\n` header lines (used to
    /// give a message a real `Cc:` or `Bcc:` header).
    fn message(
        message_id: &str,
        from: &str,
        to: &str,
        extra: &str,
        subject: &str,
        date: &str,
        body: &str,
    ) -> Vec<u8> {
        format!(
            "Message-ID: {message_id}\r\nFrom: {from}\r\nTo: {to}\r\n{extra}Subject: {subject}\r\n\
             Date: {date}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn live_rendered_criteria_match_the_expected_uid_sets() {
        let Some(config) = gated() else { return };
        let (mut session, _lease) = ImapConnectionManager::new(config)
            .connect_leased(ConnectionRole::Command, USER, PASSWORD)
            .await
            .expect("live connect");

        // Fresh scratch mailbox (delete a stale one first, tolerated).
        {
            let write = session.as_write_session().expect("write session");
            let arg = super::super::write::build_mailbox_arg(SCRATCH).unwrap();
            let _ = write.delete_mailbox_arg(&arg).await;
        }
        session.create_mailbox(SCRATCH).await.expect("create scratch");

        // Seed a handful of known messages. UID order follows APPEND order
        // (UIDs assigned sequentially from UIDNEXT), so we record them in order
        // and map rendered cases to the expected positions.
        let seeds: &[(&str, &str, &str, &str, &str, &str, &str)] = &[
            // (message-id, from, to, extra headers, subject, date, body)
            ("<p5r1-a@x>", "alice@example.com", "bob@example.com", "", "ASCII hello", "01 Jan 2024 10:00:00 +0000", "plain ascii body"),
            ("<p5r1-b@x>", "éve@example.com", "team-b@example.com", "Cc: carol-cc@example.com\r\n", "Éléments", "15 Feb 2024 10:00:00 +0000", "contient des Éléments"),
            ("<p5r1-c@x>", "someone@example.com", "team-c@example.com", "Bcc: dave-bcc@example.com\r\n", "日本語 word", "20 Mar 2024 10:00:00 +0000", "CJK body"),
            ("<p5r1-d@x>", "quote@example.com", "team@example.com", "", "has a \" and \\ in it", "10 Apr 2024 10:00:00 +0000", "quote and backslash"),
        ];
        {
            let write = session.as_write_session().expect("write session");
            for (mid, from, to, extra, subject, date, body) in seeds {
                let raw = message(mid, from, to, extra, subject, date, body);
                // Every seed is appended \Seen (the primitive needs a flag
                // list); the per-case flags are set afterwards with UID STORE,
                // the same path the app uses for read/star.
                let flags: Vec<String> = vec!["\\Seen".into()];
                write.append(SCRATCH, &flags, &raw).await.expect("append seed");
            }
        }

        // EXAMINE the scratch mailbox and collect its UIDs in order so we can
        // map message positions to UIDs.
        session.examine(SCRATCH).await.expect("examine scratch");
        let mut all_uids = session.uid_search("ALL").await.expect("search ALL");
        all_uids.sort_unstable();
        assert_eq!(all_uids.len(), seeds.len(), "all seeds present");
        // Position i (APPEND order) maps to the i-th smallest UID.
        let uid_of = |i: usize| all_uids[i];
        {
            use super::super::write::{build_uid_set, StoreOp};
            // STORE needs a read-write SELECT, not the read-only EXAMINE above.
            session.select(SCRATCH).await.expect("select scratch");
            let write = session.as_write_session().expect("write session");
            // a and d are unread (the \Seen flag is removed). \Flagged cannot be
            // set on this container: its global ACL grants `lrstiepxka`, which
            // has no `w` (write-flags) right, so `+FLAGS (\Flagged)` is silently
            // ignored. Do not "fix" that here: the same ACL is what withholds
            // keyword creation to mirror Proton Bridge.
            let unread = build_uid_set(&[uid_of(0), uid_of(3)]).unwrap();
            write
                .uid_store(&unread, StoreOp::Remove, &["\\Seen".into()])
                .await
                .expect("unread a and d");
        }
        session.examine(SCRATCH).await.expect("re-examine after flags");

        let config = live_settings();
        let plan = scratch_plan(&config);
        let context = ImapSearchContext {
            plan: &plan,
            label_storage: config.label_storage,
            label_container: None,
            delimiter: Some("/"),
        };

        // Run a rendered query restricted to the scratch mailbox via in:, and
        // return the sorted UID result. The `in:` term names SCRATCH by its
        // exact wire name (case-insensitive match in the plan).
        async fn run_query(
            session: &mut dyn ImapSession,
            context: &ImapSearchContext<'_>,
            query: &SearchQuery,
        ) -> Vec<u32> {
            let outcome = render_imap(query, context);
            let plan = match outcome {
                ImapSearchOutcome::Plan(plan) => plan,
                other => panic!("{query:?}: expected a Plan, got {other:?}"),
            };
            assert_eq!(plan.mailboxes, vec![SCRATCH.to_string()], "{query:?}");
            session.examine(&plan.mailboxes[0]).await.expect("examine");
            let mut uids = session.uid_search(&plan.criteria).await.expect("uid_search");
            uids.sort_unstable();
            uids
        }

        async fn run(
            session: &mut dyn ImapSession,
            context: &ImapSearchContext<'_>,
            query: &str,
        ) -> Vec<u32> {
            let scoped = format!("in:{SCRATCH} {query}");
            run_query(session, context, &SearchQuery::parse(&scoped)).await
        }

        // ASCII text in the body/subject of message a.
        assert_eq!(run(&mut session, &context, "ascii").await, vec![uid_of(0)]);
        // Non-ASCII text (accented + CJK) — matches b and c respectively.
        assert_eq!(run(&mut session, &context, "Éléments").await, vec![uid_of(1)]);
        assert_eq!(run(&mut session, &context, "日本語").await, vec![uid_of(2)]);
        // A value containing a quote and a backslash (message d). The parser
        // can never produce a `"` inside a term (a quote toggles phrase mode),
        // so build the AST directly: this exercises the renderer's escaping
        // against the real server, which is the point of the case.
        let escaped = SearchQuery {
            terms: vec![
                QueryTerm::In(SCRATCH.to_string()),
                QueryTerm::Subject("has a \" and \\ in it".to_string()),
            ],
        };
        assert_eq!(
            run_query(&mut session, &context, &escaped).await,
            vec![uid_of(3)]
        );
        // A value that ENDS in a backslash must not swallow its closing quote.
        let trailing = SearchQuery {
            terms: vec![
                QueryTerm::In(SCRATCH.to_string()),
                QueryTerm::Subject("in it\\".to_string()),
            ],
        };
        assert_eq!(
            run_query(&mut session, &context, &trailing).await,
            Vec::<u32>::new(),
            "a trailing backslash must stay inside its quoted string and match nothing here"
        );
        // FROM.
        assert_eq!(run(&mut session, &context, "from:alice@example.com").await, vec![uid_of(0)]);
        // The TO/CC/BCC OR: message a is a To:, b has a real Cc:, c a real Bcc:.
        assert_eq!(run(&mut session, &context, "to:bob@example.com").await, vec![uid_of(0)]);
        assert_eq!(run(&mut session, &context, "to:carol-cc@example.com").await, vec![uid_of(1)]);
        assert_eq!(run(&mut session, &context, "to:dave-bcc@example.com").await, vec![uid_of(2)]);
        // IMAP address matching is a SUBSTRING match, unlike Gmail's: a shorter
        // address matches every address that contains it. Pin that behaviour so
        // a later run does not assume otherwise.
        assert_eq!(run(&mut session, &context, "to:ve-bcc@example.com").await, vec![uid_of(2)]);
        // SINCE/BEFORE compare the server's INTERNALDATE (arrival time), NOT the
        // `Date:` header: APPEND without a date stamps "now", so the seeds'
        // header dates (Jan-Apr 2024) play no part. Assert against days
        // relative to today (two days either side, so a timezone edge at
        // midnight cannot flip the answer), and pin that SINCE/BEFORE take the
        // rendered `d-Mon-yyyy` form.
        let today = chrono::Utc::now().date_naive();
        let fmt = |days: i64| (today + chrono::Duration::days(days)).format("%Y-%m-%d").to_string();
        let everything = vec![uid_of(0), uid_of(1), uid_of(2), uid_of(3)];
        assert_eq!(run(&mut session, &context, &format!("after:{}", fmt(-2))).await, everything);
        assert_eq!(run(&mut session, &context, &format!("before:{}", fmt(-2))).await, Vec::<u32>::new());
        assert_eq!(run(&mut session, &context, &format!("after:{}", fmt(2))).await, Vec::<u32>::new());
        assert_eq!(run(&mut session, &context, &format!("before:{}", fmt(2))).await, everything);
        // FLAGGED: nothing in this container can be starred (see the ACL note
        // above), so a positive hit is not testable here. The case proves the
        // rendered `FLAGGED` is valid syntax the server accepts: an empty set,
        // not BAD.
        assert_eq!(
            run(&mut session, &context, "is:starred").await,
            Vec::<u32>::new()
        );
        // UNSEEN: a and d had \Seen removed.
        assert_eq!(
            run(&mut session, &context, "is:unread").await,
            vec![uid_of(0), uid_of(3)]
        );

        // KEYWORD: this container withholds keyword creation (it mirrors Proton
        // Bridge, whose PERMANENTFLAGS omits `\*`), so no keyword can be stored
        // and a positive hit is not testable here. The case still proves the
        // rendered `KEYWORD <atom>` is valid syntax the server accepts: it must
        // answer with an empty set, not BAD.
        assert_eq!(
            run(&mut session, &context, "label:p5label").await,
            Vec::<u32>::new()
        );

        // Clean up: delete the scratch mailbox (seed mailboxes untouched).
        {
            let write = session.as_write_session().expect("write session");
            let arg = super::super::write::build_mailbox_arg(SCRATCH).unwrap();
            let _ = write.delete_mailbox_arg(&arg).await;
        }
        let _ = session.logout().await;
    }
}
