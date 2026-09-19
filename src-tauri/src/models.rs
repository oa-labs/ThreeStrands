use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAccount {
    pub email: String,
    pub connected_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarOption {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub primary: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEvent {
    pub id: String,
    pub account_id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub location: Option<String>,
    pub description: Option<String>,
    pub conference_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleResult {
    pub events: Vec<ScheduleEvent>,
    pub errors: Vec<String>,
}

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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyAssistMessage {
    pub sender: String,
    pub sent_at: String,
    pub body_text: String,
}

/// The exact bounded mailbox content displayed for review before it is sent
/// to the user's selected AI provider.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyAssistContext {
    pub subject: String,
    pub messages: Vec<ReplyAssistMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyAssistResult {
    pub body: String,
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
pub struct FailedMutation {
    pub id: String,
    pub kind: String,
    pub thread_id: String,
    pub attempts: i64,
    pub error: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantinedMessage {
    pub message_id: String,
    pub thread_id: String,
    pub error: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: &'static str,
    pub last_successful_sync: Option<String>,
    pub cursor: Option<String>,
    pub pending_mutations: i64,
    pub failed_mutations: Vec<FailedMutation>,
    pub quarantined_messages: Vec<QuarantinedMessage>,
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
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLabelRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub account_id: Option<String>,
}

/// A user-defined, persistent inbox view that narrows one account's inbox to
/// threads matching one rule (sending domain, Gmail label, or a substring
/// pattern against the sender address). Purely local — unlike `Label`,
/// there's no provider-side equivalent to sync against. Scoped to a single
/// account: it never applies to another account's mail, even when viewing
/// every account merged together.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitInbox {
    pub id: String,
    pub name: String,
    pub match_kind: String,
    pub match_value: String,
    pub sort_order: i64,
    pub created_at: String,
    pub account_id: String,
}

/// Unread totals for the tab bar: `inbox` counts unarchived/untrashed
/// threads that don't match any split inbox rule, and `splits` counts
/// unread threads matching each split inbox's rule, keyed by split id.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxUnreadCounts {
    pub inbox: i64,
    pub splits: std::collections::HashMap<String, i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSplitInboxRequest {
    pub name: String,
    pub match_kind: String,
    pub match_value: String,
    pub account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSplitInboxRequest {
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
    /// Which backend this account authenticates and syncs through, e.g.
    /// `"gmail"`. A plain string, like `status`, rather than a Rust enum: the
    /// set of valid values is enforced in `db::accounts` and mirrored by the
    /// frontend's `MailProvider` type, and a string round-trips through the
    /// database and the settings-transfer format without a mapping layer.
    pub provider: String,
    pub sort_order: i64,
    pub connected_at: String,
    pub last_synced_at: Option<String>,
}

/// The `accounts.provider` values this build understands. Checked wherever a
/// provider string reaches storage from outside the process — today that is
/// only the settings-transfer import path, since every other writer sets it
/// to a literal known value itself.
pub fn is_known_account_provider(value: &str) -> bool {
    matches!(value, "gmail")
}
