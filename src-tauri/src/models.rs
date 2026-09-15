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
    /// Newest *inbound* message's timestamp — unlike `last_message_at`, sending
    /// a reply doesn't bump this, so the inbox order doesn't jump on send.
    pub last_received_at: String,
    pub unread: bool,
    pub starred: bool,
    pub archived: bool,
    pub trashed: bool,
    pub labels: Vec<String>,
    pub account_id: String,
    /// Match excerpt from the FTS5 index, wrapping hits in `\u{1}`/`\u{2}`
    /// markers. Only populated by `search_threads`; `None` elsewhere.
    pub match_snippet: Option<String>,
    pub summary: Option<String>,
    pub summary_generated_at: Option<String>,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPage {
    pub threads: Vec<Thread>,
    pub has_more: bool,
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
    pub unread: bool,
    pub unsubscribe: Option<UnsubscribeInfo>,
    pub attachments: Vec<MessageAttachment>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    #[serde(default)]
    pub content_id: Option<String>,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeInfo {
    pub methods: Vec<UnsubscribeMethod>,
    pub list_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UnsubscribeMethod {
    OneClick,
    Mailto,
    Web,
}

#[derive(Debug, Clone)]
pub struct UnsubscribeTarget {
    pub request_id: String,
    pub method: UnsubscribeMethod,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeResult {
    pub method: UnsubscribeMethod,
    pub outcome: String,
    pub http_status: Option<u16>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub thread: Thread,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriageEventKind {
    Open,
    Close,
    Disposition,
    Restore,
    Response,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriageContext {
    Inbox,
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriageAction {
    Archive,
    Trash,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageEvent {
    pub thread_id: String,
    pub kind: TriageEventKind,
    pub context: TriageContext,
    #[serde(default)]
    pub action: Option<TriageAction>,
    #[serde(default)]
    pub opened: bool,
    #[serde(default)]
    pub dwell_ms: Option<i64>,
    #[serde(default)]
    pub scrolled: bool,
    #[serde(default)]
    pub batch: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageSenderStats {
    pub account_id: String,
    pub sender_email: String,
    pub sender_domain: String,
    pub exposure_count: i64,
    pub engaged_view_count: i64,
    pub disposition_count: i64,
    pub archive_count: i64,
    pub trash_count: i64,
    pub quick_disposition_count: i64,
    pub batch_disposition_count: i64,
    pub restore_count: i64,
    pub response_count: i64,
    pub quick_disposition_rate: f64,
    pub last_seen_at: String,
}

/// A past correspondent ranked for compose autocomplete. Built entirely from
/// local send/receive history (plus anything explicitly pinned) rather than
/// an imported address book, so every suggestion is someone the user has
/// actually exchanged mail with.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactSuggestion {
    pub email: String,
    pub display_name: Option<String>,
    pub sent_count: i64,
    pub received_count: i64,
    pub last_interacted_at: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryResult {
    pub summary: String,
    pub generated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchThreadsRequest {
    pub query: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub include_archived: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ThreadMutation {
    Archive {
        thread_id: String,
        value: bool,
    },
    Trash {
        thread_id: String,
        value: bool,
    },
    Spam {
        thread_id: String,
        value: bool,
    },
    Read {
        thread_id: String,
        value: bool,
    },
    Star {
        thread_id: String,
        value: bool,
    },
    Label {
        thread_id: String,
        label_id: String,
        value: bool,
    },
}

impl ThreadMutation {
    pub fn thread_id(&self) -> &str {
        match self {
            Self::Archive { thread_id, .. }
            | Self::Trash { thread_id, .. }
            | Self::Spam { thread_id, .. }
            | Self::Read { thread_id, .. }
            | Self::Star { thread_id, .. }
            | Self::Label { thread_id, .. } => thread_id,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLabelRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLabelRequest {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub configured: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub email: String,
    pub display_name: Option<String>,
    pub color: String,
    pub status: String,
    pub sort_order: i64,
    pub connected_at: String,
    pub last_synced_at: Option<String>,
}
