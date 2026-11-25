//! Pure candidate structures passed between `db/agents` and `agents/analyzer`.

use super::{ProposalEvidence, ProposalKind, ProposalPayload};

#[derive(Clone, Debug)]
pub(crate) struct CandidateMessage {
    pub id: String,
    pub sender: String,
    pub recipients: Vec<String>,
    pub sent_at: String,
    pub body_text: String,
    pub is_own: bool,
    pub has_unsubscribe: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RelatedThreadContext {
    pub thread_id: String,
    pub subject: String,
    pub last_message_at: String,
    pub snippet: String,
    pub user_replied: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RelatedEventContext {
    pub event_id: String,
    pub title: String,
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateThread {
    pub thread_id: String,
    pub account_id: String,
    pub subject: String,
    pub last_message_at: String,
    pub age_days: i64,
    pub has_unsubscribe: bool,
    pub messages: Vec<CandidateMessage>,
    pub related_threads: Vec<RelatedThreadContext>,
    pub related_events: Vec<RelatedEventContext>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeterministicProposal {
    pub account_id: String,
    pub thread_id: String,
    pub thread_revision: String,
    pub kind: ProposalKind,
    pub headline: String,
    pub reason: String,
    pub payload: ProposalPayload,
    pub evidence: Vec<ProposalEvidence>,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateBatch {
    pub deterministic: Vec<DeterministicProposal>,
    pub ai_candidates: Vec<CandidateThread>,
}
