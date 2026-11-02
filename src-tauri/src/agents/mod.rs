//! Durable, device-local delegated work. Phase 1 runs only a labeled fixture
//! workflow; no model, network, credentials, or personal-task mutations.
use serde::{Deserialize, Serialize};

pub(crate) mod policy;
mod runner;
pub(crate) use runner::Runner;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateAgentTask {
    pub account_id: String,
    pub objective: String,
    pub source_thread_id: Option<String>,
    pub linked_task_id: Option<String>,
    pub workflow: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTask {
    pub id: String,
    pub account_id: String,
    pub objective: String,
    pub source_thread_id: Option<String>,
    pub linked_task_id: Option<String>,
    pub workflow: String,
    pub revision: i64,
    pub created_at: i64,
    pub archived: bool,
    pub unread: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    Queued,
    Running,
    WaitingForInput,
    Paused,
    Completed,
    Failed,
    Cancelled,
}
impl RunState {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingForInput => "waiting_for_input",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRun {
    pub id: String,
    pub task_id: String,
    pub state: RunState,
    pub reason: Option<String>,
    pub generation: i64,
    pub checkpoint: i64,
    pub owner: Option<String>,
    pub lease_until: Option<i64>,
    pub started_at: Option<i64>,
    pub not_before: i64,
    pub dispatches: i64,
    pub attempts: i64,
    pub active_seconds: i64,
    pub question_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTurn {
    pub id: String,
    pub role: String,
    pub text: String,
    pub question_id: Option<String>,
    pub answer_to: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentArtifact {
    pub id: String,
    pub version: i64,
    pub kind: String,
    pub body: String,
    pub not_researched: bool,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDetail {
    pub task: AgentTask,
    pub run: AgentRun,
    pub turns: Vec<AgentTurn>,
    pub artifacts: Vec<AgentArtifact>,
}

#[derive(Clone, Debug)]
pub(crate) struct Lease {
    pub run: AgentRun,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepResult {
    Prepared,
    EvidenceCollected,
    Publish,
    TransientFailure,
}

/// Deterministic fixture. The question is asked before an independent evidence
/// step; publication waits for its answer. Native persistence enforces order.
pub(crate) fn fixture_step(checkpoint: i64) -> StepResult {
    match checkpoint {
        0 => StepResult::Prepared,
        1 => StepResult::EvidenceCollected,
        _ => StepResult::Publish,
    }
}
