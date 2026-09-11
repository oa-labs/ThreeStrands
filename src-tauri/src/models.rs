use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub provider_thread_id: String,
    pub subject: String,
    pub snippet: String,
    pub participants: Vec<String>,
    pub last_message_at: String,
    pub unread: bool,
    pub starred: bool,
    pub archived: bool,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub thread_id: String,
    pub sender: String,
    pub recipients: Vec<String>,
    pub sent_at: String,
    pub body_html: String,
    pub body_text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub thread: Thread,
    pub messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchThreadsRequest {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThreadMutation {
    Archive { thread_id: String, value: bool },
    Read { thread_id: String, value: bool },
    Star { thread_id: String, value: bool },
}

impl ThreadMutation {
    pub fn thread_id(&self) -> &str {
        match self {
            Self::Archive { thread_id, .. }
            | Self::Read { thread_id, .. }
            | Self::Star { thread_id, .. } => thread_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: &'static str,
    pub last_successful_sync: Option<String>,
    pub cursor: Option<String>,
    pub pending_mutations: i64,
    pub error: Option<String>,
}
