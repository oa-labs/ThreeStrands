//! Standing, configurable Inbox Assistants backed by local SQLite state and
//! the user's configured AI provider. Proposals always require user review.

use serde::{Deserialize, Serialize};

use crate::ai::{AiProvider, Reasoning};

pub(crate) mod analyzer;
pub(crate) mod candidates;
pub(crate) mod policy;
pub(crate) mod runner;
pub(crate) use runner::Runner;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentId {
    StaleReads,
    ResolvedAndExpired,
    AgingReplies,
}

impl AgentId {
    pub(crate) const ALL: [Self; 3] = [
        Self::StaleReads,
        Self::ResolvedAndExpired,
        Self::AgingReplies,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StaleReads => "stale_reads",
            Self::ResolvedAndExpired => "resolved_and_expired",
            Self::AgingReplies => "aging_replies",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "stale_reads" => Some(Self::StaleReads),
            "resolved_and_expired" => Some(Self::ResolvedAndExpired),
            "aging_replies" => Some(Self::AgingReplies),
            _ => None,
        }
    }

    pub(crate) fn catalog(self) -> AgentCatalogEntry {
        match self {
            Self::StaleReads => AgentCatalogEntry {
                name: "Stale Reads Summarizer",
                description: "Summarizes aging newsletters and long unread emails so you can read the gist, save to Notes, and archive.",
                default_age_days: 7,
                default_max_threads: 15,
                default_include_starred: false,
            },
            Self::ResolvedAndExpired => AgentCatalogEntry {
                name: "Expired & Already-Handled Sweeper",
                description: "Spots aging emails already handled in another thread, completed task, or past meeting, or past an expired date.",
                default_age_days: 7,
                default_max_threads: 15,
                default_include_starred: false,
            },
            Self::AgingReplies => AgentCatalogEntry {
                name: "Aging Reply Drafter",
                description: "Detects unreplied emails from people and stages a draft reply (plus a polite check-in for older threads) for your review.",
                default_age_days: 3,
                default_max_threads: 10,
                default_include_starred: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AgentCatalogEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub default_age_days: i64,
    pub default_max_threads: i64,
    pub default_include_starred: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InboxAgentConfig {
    pub agent_id: AgentId,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub age_days: i64,
    pub max_threads_per_run: i64,
    pub include_starred: bool,
    pub account_id: Option<String>,
    pub schedule_hours: Option<i64>,
    pub last_run_at: Option<i64>,
    pub last_run_status: Option<String>,
    pub last_run_error: Option<String>,
    pub last_run_scanned: i64,
    pub last_run_proposed: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateInboxAgentConfig {
    pub agent_id: AgentId,
    pub enabled: bool,
    pub age_days: i64,
    pub max_threads_per_run: i64,
    pub include_starred: bool,
    pub account_id: Option<String>,
    pub schedule_hours: Option<i64>,
}

/// Non-secret AI provider settings supplied by the UI for manual and scheduled runs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiRunConfig {
    pub provider: AiProvider,
    pub model: String,
    pub fast_model: Option<String>,
    pub endpoint: Option<String>,
    #[serde(default)]
    pub reasoning: Reasoning,
}

impl AiRunConfig {
    pub(crate) fn fast_or_main_model(&self) -> (&str, Reasoning) {
        if let Some(fast) = self
            .fast_model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            (fast, self.reasoning)
        } else {
            (self.model.trim(), Reasoning::Default)
        }
    }

    pub(crate) fn main_model(&self) -> (&str, Reasoning) {
        (self.model.trim(), Reasoning::Default)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InboxAgentRun {
    pub id: String,
    pub agent_id: AgentId,
    pub account_id: Option<String>,
    pub state: RunState,
    pub reason: Option<String>,
    pub scanned_count: i64,
    pub proposed_count: i64,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProposalKind {
    ArchiveWithSummary,
    ArchiveAsResolved,
    ArchiveAsExpired,
    DraftReply,
}

impl ProposalKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ArchiveWithSummary => "archive_with_summary",
            Self::ArchiveAsResolved => "archive_as_resolved",
            Self::ArchiveAsExpired => "archive_as_expired",
            Self::DraftReply => "draft_reply",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "archive_with_summary" => Some(Self::ArchiveWithSummary),
            "archive_as_resolved" => Some(Self::ArchiveAsResolved),
            "archive_as_expired" => Some(Self::ArchiveAsExpired),
            "draft_reply" => Some(Self::DraftReply),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProposalState {
    Pending,
    Accepted,
    Dismissed,
    Superseded,
}

impl ProposalState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
            Self::Superseded => "superseded",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "dismissed" => Some(Self::Dismissed),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProposalEvidence {
    pub kind: String,
    pub id: String,
    pub label: String,
    pub excerpt: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProposalPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggest_save_to_note: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expired_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_draft: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gentle_out_draft: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InboxAgentProposal {
    pub id: String,
    pub agent_id: AgentId,
    pub account_id: String,
    pub thread_id: String,
    pub thread_revision: String,
    pub thread_subject: String,
    pub thread_sender: String,
    pub thread_last_received_at: String,
    pub thread_snippet: String,
    pub thread_unread: bool,
    pub thread_starred: bool,
    pub kind: ProposalKind,
    pub state: ProposalState,
    pub headline: String,
    pub reason: String,
    pub payload: ProposalPayload,
    pub evidence: Vec<ProposalEvidence>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InboxAgentsSnapshot {
    pub runner_enabled: bool,
    pub agents: Vec<InboxAgentConfig>,
    pub active_runs: Vec<InboxAgentRun>,
    pub proposals: Vec<InboxAgentProposal>,
}
