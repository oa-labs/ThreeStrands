//! Validates suggested meetings and tasks against the messages they cite.

use serde_json::json;

use crate::models::{ActionAnalysis, ActionProposal, ProposalEvidence};

use super::context::ActionMessageInput;

pub(super) const MAX_ACTION_PROPOSALS: usize = 10;

const MAX_EVIDENCE_CHARS: usize = 1_000;

/// Mirrors `MeetingProposal` and `TaskProposal`. Strict structured output
/// requires every property to be listed as required, so optional values are
/// nullable instead of omittable; serde's `deny_unknown_fields` still rejects
/// anything outside these shapes.
pub(super) fn proposal_array_schema() -> serde_json::Value {
    let nullable_string = json!({"type": ["string", "null"]});
    let nullable_integer = json!({"type": ["integer", "null"], "minimum": 0});
    let evidence = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["sourceMessageId", "excerpt"],
        "properties": {
            "sourceMessageId": {"type": "string"},
            "excerpt": {"type": "string"},
        },
    });
    let meeting = json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "type", "intent", "title", "participants", "location", "rawTimeLanguage",
            "normalizedStart", "normalizedEnd", "eventDate", "searchRangeStart", "searchRangeEnd",
            "durationMinutes", "timeZone", "confidence", "evidence",
        ],
        "properties": {
            "type": {"type": "string", "enum": ["meeting"]},
            "intent": {"type": "string", "enum": ["attend", "schedule"]},
            "title": {"type": "string"},
            "participants": {"type": "array", "items": {"type": "string"}},
            "location": nullable_string,
            "rawTimeLanguage": {"type": "string"},
            "normalizedStart": nullable_string,
            "normalizedEnd": nullable_string,
            "eventDate": nullable_string,
            "searchRangeStart": nullable_string,
            "searchRangeEnd": nullable_string,
            "durationMinutes": nullable_integer,
            "timeZone": nullable_string,
            "confidence": {"type": "number"},
            "evidence": evidence,
        },
    });
    let task = json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "type", "kind", "title", "notes", "dueKind", "dueValue", "timeZone",
            "repeatIntervalDays", "goalId", "confidence", "evidence",
        ],
        "properties": {
            "type": {"type": "string", "enum": ["task"]},
            "kind": {"type": "string", "enum": ["action", "follow_up", "waiting_for"]},
            "title": {"type": "string"},
            "notes": nullable_string,
            "dueKind": {"type": "string", "enum": ["none", "date", "datetime"]},
            "dueValue": nullable_string,
            "timeZone": nullable_string,
            "repeatIntervalDays": nullable_integer,
            "goalId": nullable_string,
            "confidence": {"type": "number"},
            "evidence": evidence,
        },
    });
    json!({"type": "array", "items": {"anyOf": [meeting, task]}})
}

pub(super) fn proposal_evidence(proposal: &ActionProposal) -> &ProposalEvidence {
    match proposal {
        ActionProposal::Meeting(meeting) => &meeting.evidence,
        ActionProposal::Task(task) => &task.evidence,
    }
}

pub(super) fn validate_proposal_items(
    items: Vec<serde_json::Value>,
    messages: &[ActionMessageInput],
) -> Result<ActionAnalysis, String> {
    // Each proposal stands or falls on its own: one malformed or unverifiable
    // item is withheld and counted rather than discarding the whole set.
    // Valid proposals past the limit are counted, not shown, so an email
    // listing many events still yields its first ones.
    let mut analysis = ActionAnalysis::default();
    for item in items {
        if analysis.proposals.len() == MAX_ACTION_PROPOSALS {
            analysis.extra_count += 1;
            continue;
        }
        let mut proposal: ActionProposal = match serde_json::from_value(item) {
            Ok(proposal) => proposal,
            Err(error) => {
                log::warn!(target: "ai_analyze_thread", "withheld action proposal with an invalid schema: {error}");
                analysis.hidden_count += 1;
                continue;
            }
        };
        match validate_action_proposal(&mut proposal, messages) {
            Ok(()) => analysis.proposals.push(proposal),
            Err(error) => {
                log::warn!(target: "ai_analyze_thread", "withheld action proposal: {error}");
                analysis.hidden_count += 1;
            }
        }
    }
    Ok(analysis)
}

/// Validates one proposal and resolves its evidence to the exact source text
/// it quotes, so downstream review and stored tasks never carry model-altered
/// excerpts.
fn validate_action_proposal(
    proposal: &mut ActionProposal,
    messages: &[ActionMessageInput],
) -> Result<(), String> {
    let evidence = match proposal {
        ActionProposal::Meeting(value) => {
            // A start without an end takes its length from durationMinutes;
            // an end without a start is meaningless.
            if !matches!(value.intent.as_str(), "attend" | "schedule")
                || value.title.trim().is_empty()
                || value.raw_time_language.trim().is_empty()
                || value.participants.len() > 100
                || !value.confidence.is_finite()
                || !(0.0..=1.0).contains(&value.confidence)
                || (value.normalized_end.is_some() && value.normalized_start.is_none())
                || (value.search_range_start.is_some() != value.search_range_end.is_some())
            {
                return Err("The AI provider returned an invalid meeting proposal".to_string());
            }
            if let Some(date) = &value.event_date {
                if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
                    return Err("The AI provider returned an invalid meeting date".to_string());
                }
            }
            // An exact start is more specific than a bare date.
            if value.normalized_start.is_some() {
                value.event_date = None;
            }
            if let Some(duration) = value.duration_minutes {
                if !(5..=720).contains(&duration) {
                    return Err("The AI provider returned an invalid meeting duration".to_string());
                }
            }
            if let Some(time_zone) = &value.time_zone {
                if time_zone.parse::<chrono_tz::Tz>().is_err() {
                    return Err("The AI provider returned an invalid meeting timezone".to_string());
                }
            }
            for timestamp in [
                value.normalized_start.as_deref(),
                value.normalized_end.as_deref(),
                value.search_range_start.as_deref(),
                value.search_range_end.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if chrono::DateTime::parse_from_rfc3339(timestamp).is_err() {
                    return Err("The AI provider returned an invalid meeting timestamp".to_string());
                }
            }
            &mut value.evidence
        }
        ActionProposal::Task(value) => {
            if !matches!(value.kind.as_str(), "action" | "follow_up" | "waiting_for")
                || value.title.trim().is_empty()
                || !matches!(value.due_kind.as_str(), "none" | "date" | "datetime")
                || (value.due_kind == "none" && value.due_value.is_some())
                || (value.due_kind != "none" && value.due_value.is_none())
                || !value.confidence.is_finite()
                || !(0.0..=1.0).contains(&value.confidence)
            {
                return Err("The AI provider returned an invalid task proposal".to_string());
            }
            if let Some(interval) = value.repeat_interval_days {
                if !(1..=3650).contains(&interval) {
                    return Err("The AI provider returned an invalid repeat interval".to_string());
                }
            }
            if let Some(time_zone) = &value.time_zone {
                if time_zone.parse::<chrono_tz::Tz>().is_err() {
                    return Err("The AI provider returned an invalid task timezone".to_string());
                }
            }
            if let Some(due_value) = &value.due_value {
                let valid = match value.due_kind.as_str() {
                    "date" => chrono::NaiveDate::parse_from_str(due_value, "%Y-%m-%d").is_ok(),
                    "datetime" => chrono::DateTime::parse_from_rfc3339(due_value).is_ok(),
                    _ => false,
                };
                if !valid {
                    return Err("The AI provider returned an invalid task due value".to_string());
                }
            }
            &mut value.evidence
        }
    };
    if evidence.source_message_id.trim().is_empty()
        || evidence.excerpt.trim().is_empty()
        || evidence.excerpt.chars().count() > MAX_EVIDENCE_CHARS
    {
        return Err("The AI provider returned invalid proposal evidence".to_string());
    }
    let (message_id, excerpt) = resolve_evidence(evidence, messages)?;
    if excerpt.chars().count() > MAX_EVIDENCE_CHARS {
        return Err("The AI provider returned invalid proposal evidence".to_string());
    }
    evidence.source_message_id = message_id;
    evidence.excerpt = excerpt;
    Ok(())
}

/// Finds the analyzed message text an evidence excerpt quotes. Models often
/// alter whitespace, typographic quotes, dashes, or reply `>` markers when
/// quoting, and sometimes cite the wrong message ID, so matching compares
/// normalized text: the cited message first, then the rest of the thread in
/// order. The result is always a verbatim slice of an analyzed message body;
/// an excerpt that does not appear anywhere in the thread is rejected.
pub(super) fn resolve_evidence(
    evidence: &ProposalEvidence,
    messages: &[ActionMessageInput],
) -> Result<(String, String), String> {
    let (needle, _) = normalize_evidence_text(&evidence.excerpt);
    if needle.is_empty() {
        return Err("The AI provider returned invalid proposal evidence".to_string());
    }
    let cited = messages
        .iter()
        .filter(|message| message.id == evidence.source_message_id);
    let others = messages
        .iter()
        .filter(|message| message.id != evidence.source_message_id);
    for message in cited.chain(others) {
        if let Some(excerpt) = find_normalized(&message.body_text, &needle) {
            return Ok((message.id.clone(), excerpt.to_string()));
        }
    }
    if messages
        .iter()
        .all(|message| message.id != evidence.source_message_id)
    {
        return Err("The AI provider cited a message outside the analyzed thread".to_string());
    }
    Err("The AI provider returned unverifiable proposal evidence".to_string())
}

/// Returns the verbatim slice of `body` whose normalized form is `needle`.
fn find_normalized<'a>(body: &'a str, needle: &str) -> Option<&'a str> {
    let (haystack, spans) = normalize_evidence_text(body);
    let byte_start = haystack.find(needle)?;
    let start = haystack[..byte_start].chars().count();
    let end = start + needle.chars().count() - 1;
    Some(&body[spans[start].0..spans[end].1])
}

/// Normalizes text for evidence comparison: whitespace runs and line-leading
/// `>` quote markers collapse to one space, typographic quotes and dashes map
/// to ASCII, and invisible formatting characters are dropped. Returns the
/// normalized text and, per normalized char, the byte span it came from.
fn normalize_evidence_text(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut normalized = String::with_capacity(text.len());
    let mut spans = Vec::with_capacity(text.len());
    let mut pending_space: Option<(usize, usize)> = None;
    let mut at_line_start = true;
    for (start, character) in text.char_indices() {
        let span = (start, start + character.len_utf8());
        if matches!(
            character,
            '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
        ) {
            continue;
        }
        if character == '\n' || character == '\r' {
            at_line_start = true;
            pending_space.get_or_insert(span);
            continue;
        }
        if character.is_whitespace() || (at_line_start && character == '>') {
            pending_space.get_or_insert(span);
            continue;
        }
        at_line_start = false;
        if let Some(space) = pending_space.take() {
            if !normalized.is_empty() {
                normalized.push(' ');
                spans.push(space);
            }
        }
        normalized.push(match character {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            other => other,
        });
        spans.push(span);
    }
    (normalized, spans)
}
