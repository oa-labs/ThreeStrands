use serde::{Deserialize, Deserializer, Serialize};

fn deserialize_optional_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    pub writable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCalendarEventRequest {
    pub account_id: String,
    pub calendar_id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub description: String,
    pub attendees: Vec<String>,
}

/// Replaces an owned event's editable fields. All-day events carry `YYYY-MM-DD`
/// dates with an exclusive end, matching Google; timed events carry RFC 3339.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCalendarEventRequest {
    pub account_id: String,
    pub calendar_id: String,
    pub event_id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub location: String,
    pub description: String,
    pub attendees: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEvent {
    pub id: String,
    pub account_id: String,
    pub calendar_id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub location: Option<String>,
    pub description: Option<String>,
    pub conference_url: Option<String>,
    /// Lowercased addresses of the other people on the event.
    pub attendees: Vec<String>,
    pub response_status: Option<String>,
    pub can_respond: bool,
    /// The connected calendar organizes this event and can write to it, so it
    /// may be edited or deleted from the app.
    pub can_edit: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleResult {
    pub events: Vec<ScheduleEvent>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityCandidate {
    pub start: String,
    pub end: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityResult {
    pub candidates: Vec<AvailabilityCandidate>,
    pub checked_calendar_count: usize,
    pub total_calendar_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusyInterval {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedTimeCheck {
    pub status: String,
    pub conflicts: Vec<BusyInterval>,
    pub checked_calendar_count: usize,
    pub total_calendar_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposalEvidence {
    pub source_message_id: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeetingProposal {
    pub intent: String,
    pub title: String,
    pub participants: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    pub raw_time_language: String,
    #[serde(default)]
    pub normalized_start: Option<String>,
    #[serde(default)]
    pub normalized_end: Option<String>,
    #[serde(default)]
    pub search_range_start: Option<String>,
    #[serde(default)]
    pub search_range_end: Option<String>,
    #[serde(default)]
    pub duration_minutes: Option<u32>,
    #[serde(default)]
    pub time_zone: Option<String>,
    pub confidence: f32,
    pub evidence: ProposalEvidence,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskProposal {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default = "default_proposal_due_kind")]
    pub due_kind: String,
    #[serde(default)]
    pub due_value: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub repeat_interval_days: Option<u32>,
    /// A goal the task would support; absent from suggestions saved before goals.
    #[serde(default)]
    pub goal_id: Option<String>,
    pub confidence: f32,
    pub evidence: ProposalEvidence,
}

fn default_proposal_due_kind() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ActionProposal {
    Meeting(MeetingProposal),
    Task(TaskProposal),
}

/// The verified proposals from one thread analysis. `hidden_count` counts
/// provider proposals that were withheld because they failed schema or
/// evidence validation; they are never shown to the user.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionAnalysis {
    pub proposals: Vec<ActionProposal>,
    pub hidden_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityWindow {
    pub weekday: u8,
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityPreferences {
    pub time_zone: String,
    pub working_windows: Vec<AvailabilityWindow>,
    pub default_duration_minutes: u32,
    pub slot_increment_minutes: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindAvailabilityRequest {
    pub range_start: String,
    pub range_end: String,
    pub preferences: AvailabilityPreferences,
    /// Spreads candidates across days; `None` keeps the earliest slots.
    #[serde(default)]
    pub max_per_day: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckProposedTimeRequest {
    pub start: String,
    pub end: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTask {
    pub id: String,
    pub account_id: String,
    pub thread_id: Option<String>,
    pub source_message_id: Option<String>,
    pub subject_snapshot: Option<String>,
    pub title: String,
    pub notes: Option<String>,
    pub kind: String,
    pub due_kind: String,
    pub due_value: Option<String>,
    pub time_zone: Option<String>,
    pub repeat_interval_days: Option<i64>,
    pub status: String,
    pub completion_source: Option<String>,
    pub evidence_text: Option<String>,
    pub wait_after: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    /// Absent from tasks written before goals existed.
    #[serde(default)]
    pub goal_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub account_id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub source_message_id: Option<String>,
    #[serde(default)]
    pub subject_snapshot: Option<String>,
    pub title: String,
    #[serde(default)]
    pub notes: Option<String>,
    pub kind: String,
    #[serde(default = "default_task_due_kind")]
    pub due_kind: String,
    #[serde(default)]
    pub due_value: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub repeat_interval_days: Option<i64>,
    #[serde(default)]
    pub evidence_text: Option<String>,
    #[serde(default)]
    pub goal_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskRequest {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub notes: Option<Option<String>>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub due_kind: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub due_value: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub time_zone: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub repeat_interval_days: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub goal_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub id: String,
    pub account_id: String,
    pub title: String,
    pub notes: Option<String>,
    /// `year`, `half`, or `quarter`.
    pub horizon: String,
    /// One period of the horizon: `2026`, `2026-H2`, or `2026-Q4`.
    pub period: String,
    /// `active`, `achieved`, or `dropped`.
    pub status: String,
    /// A goal of a longer horizon, in an enclosing period, that this one supports.
    pub parent_goal_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGoalRequest {
    pub account_id: String,
    pub title: String,
    #[serde(default)]
    pub notes: Option<String>,
    pub horizon: String,
    pub period: String,
    #[serde(default)]
    pub parent_goal_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGoalRequest {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub notes: Option<Option<String>>,
    #[serde(default)]
    pub horizon: Option<String>,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub parent_goal_id: Option<Option<String>>,
}

fn default_task_due_kind() -> String {
    "none".to_string()
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
    /// The `last_message_at` the summary was written from. A summary is
    /// stale once the thread has a newer message than this.
    pub summary_revision: Option<String>,
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

/// A named set of saved contacts, shared by every account. `member_ids`
/// lists only members whose contact is present on this device.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContactGroup {
    pub id: String,
    pub name: String,
    pub member_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactProfile {
    pub id: String,
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub bio: Option<String>,
    pub notes: Option<String>,
    pub links: Vec<String>,
    pub photo_data: Option<String>,
    pub favorite: bool,
    pub addresses: Vec<String>,
    pub sent_count: i64,
    pub received_count: i64,
    pub last_interacted_at: Option<String>,
    /// `MM-DD`, or `YYYY-MM-DD` when the year is known.
    #[serde(default)]
    pub birthday: Option<String>,
    #[serde(default)]
    pub keep_in_touch: KeepInTouch,
    /// When the next keep-in-touch reminder falls due. Derived from
    /// `keep_in_touch` and mail history on every read; never stored.
    #[serde(default)]
    pub keep_in_touch_due_at: Option<String>,
}

/// Keep-in-touch reminder settings stored on a saved contact. Every field is
/// optional so profiles, sync records, and exports from older builds read as
/// "reminders off".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KeepInTouch {
    /// Days between touches; `None` turns reminders off.
    pub interval_days: Option<i64>,
    /// When reminders were turned on; the due date counts from here until
    /// there is any interaction.
    pub started_at: Option<String>,
    /// The next reminder is pushed to this instant until a newer touch.
    pub snoozed_until: Option<String>,
    pub snoozed_at: Option<String>,
    /// Latest touch logged by hand (a call, a coffee). It also outlives
    /// mail retention pruning.
    pub last_touch_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactRecord {
    pub id: String,
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub bio: Option<String>,
    pub notes: Option<String>,
    pub links: Vec<String>,
    pub photo_data: Option<String>,
    pub favorite: bool,
    pub addresses: Vec<String>,
    #[serde(default)]
    pub birthday: Option<String>,
    #[serde(default)]
    pub keep_in_touch: KeepInTouch,
}
impl From<&ContactProfile> for ContactRecord {
    fn from(c: &ContactProfile) -> Self {
        Self {
            birthday: c.birthday.clone(),
            keep_in_touch: c.keep_in_touch.clone(),
            id: c.id.clone(),
            display_name: c.display_name.clone(),
            role: c.role.clone(),
            company: c.company.clone(),
            location: c.location.clone(),
            bio: c.bio.clone(),
            notes: c.notes.clone(),
            links: c.links.clone(),
            photo_data: c.photo_data.clone(),
            favorite: c.favorite,
            addresses: c.addresses.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactTimelineItem {
    pub thread_id: String,
    pub account_id: String,
    pub contact_email: String,
    pub subject: String,
    pub snippet: String,
    pub sent_at: String,
    pub labels: Vec<String>,
}

/// Local correspondence history with one person, across every address they
/// use. Automated mail is excluded, matching the rest of the contact index.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactActivity {
    pub sent_count: i64,
    pub received_count: i64,
    pub thread_count: i64,
    pub first_at: Option<String>,
    pub last_sent_at: Option<String>,
    /// Newest first, one entry per received message.
    pub recent_received_at: Vec<String>,
}

/// One attachment a person sent, with the message it arrived on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactFile {
    pub message_id: String,
    pub thread_id: String,
    pub subject: String,
    pub sent_at: String,
    pub attachment: MessageAttachment,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactFiles {
    /// Newest first.
    pub files: Vec<ContactFile>,
    pub total: i64,
}

/// Someone else at an email domain, from local correspondence history.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainPerson {
    pub email: String,
    pub display_name: Option<String>,
    pub last_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainContext {
    /// Most recently active first.
    pub people: Vec<DomainPerson>,
    pub threads: Vec<ContactTimelineItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveContactRequest {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub bio: Option<String>,
    pub notes: Option<String>,
    pub links: Vec<String>,
    pub photo_data: Option<String>,
    pub favorite: bool,
    pub addresses: Vec<String>,
    #[serde(default)]
    pub birthday: Option<String>,
    /// `None` keeps the stored settings. The webview changes reminders only
    /// through the dedicated keep-in-touch commands, so a profile form
    /// holding an older copy can never overwrite a newer snooze or touch.
    #[serde(skip)]
    pub keep_in_touch: Option<KeepInTouch>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactFieldSuggestion {
    pub field: String,
    pub value: String,
    pub source_message_id: String,
    pub source_thread_id: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryResult {
    pub summary: String,
    pub generated_at: String,
    /// The thread's `last_message_at` the summary was written from.
    pub revision: String,
}

/// One day's AI provider usage for a provider and model. Cost is known only
/// for requests whose provider reported it (`reported_cost_requests`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageDay {
    pub day: String,
    pub provider: String,
    pub model: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reported_cost_requests: i64,
    pub reported_cost_usd: f64,
}

/// One earlier exchange in a thread chat, as the client kept it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

/// A question about the open conversation. `search_mailbox` applies to this
/// question only; `include_proposals` reflects the Suggestions feature.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadChatRequest {
    pub thread_id: String,
    pub question: String,
    #[serde(default)]
    pub history: Vec<ChatTurn>,
    pub search_mailbox: bool,
    pub include_proposals: bool,
    pub contact_id: Option<String>,
    pub user_time_zone: String,
    /// Attachments in this conversation the user chose to share with the
    /// provider for this question.
    #[serde(default)]
    pub attachments: Vec<ChatAttachmentRef>,
}

/// An attachment in the open conversation, by message and attachment id.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatAttachmentRef {
    pub message_id: String,
    pub attachment_id: String,
}

/// An attachment whose text was shared with the provider while answering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAttachmentSource {
    pub message_id: String,
    pub attachment_id: String,
    pub filename: String,
    /// Only the start of the file was shared.
    pub truncated: bool,
}

/// A range the chat asked the app to search for open times; the times shown
/// always come from the user's calendar.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAvailability {
    pub range_start: String,
    pub range_end: String,
    pub duration_minutes: Option<u32>,
}

/// A conversation shared with the provider while answering.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSource {
    pub thread_id: String,
    pub account_id: String,
    pub subject: String,
    pub last_message_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadChatReply {
    pub answer: String,
    pub analysis: ActionAnalysis,
    pub reply_draft: Option<String>,
    /// Other conversations the answer says it relied on.
    pub sources: Vec<ChatSource>,
    /// Every other conversation shared because the question searched all mail.
    pub searched: Vec<ChatSource>,
    /// Every attachment whose text was shared for this question.
    pub attachments: Vec<ChatAttachmentSource>,
    pub availability: Option<ChatAvailability>,
}

/// The combined result of one brief request: the persisted summary and the
/// verified proposals.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadBriefResult {
    pub summary: SummaryResult,
    pub analysis: ActionAnalysis,
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

/// A user-authored canned-text template inserted into a compose body via the
/// snippet picker. Purely local, and global across accounts — unlike
/// `SplitInbox`, there's no per-account ownership.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSnippetRequest {
    pub name: String,
    pub body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSnippetRequest {
    pub id: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub configured: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// The mail providers this build can connect, i.e. the `accounts.provider`
/// values it understands. [`Account::provider`] stays a plain string at the
/// storage and transfer boundary; code that has to act on the provider —
/// choosing a credential flow or a sync backend — parses it into this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailProviderKind {
    Gmail,
}

impl MailProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "gmail" => Some(Self::Gmail),
            _ => None,
        }
    }
}

/// The `accounts.provider` values this build understands. Checked wherever a
/// provider string reaches storage from outside the process — today that is
/// only the settings-transfer import path, since every other writer sets it
/// from a [`MailProviderKind`] itself.
pub fn is_known_account_provider(value: &str) -> bool {
    MailProviderKind::parse(value).is_some()
}

#[cfg(test)]
mod mail_provider_kind_tests {
    use super::{is_known_account_provider, MailProviderKind};

    #[test]
    // One entry per provider; the list grows as providers are added.
    #[allow(clippy::single_element_loop)]
    fn stored_provider_strings_round_trip() {
        for kind in [MailProviderKind::Gmail] {
            assert_eq!(MailProviderKind::parse(kind.as_str()), Some(kind));
            assert!(is_known_account_provider(kind.as_str()));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str()),
                "the IPC spelling must match the stored one"
            );
        }
        assert_eq!(MailProviderKind::parse("Gmail"), None);
        assert_eq!(MailProviderKind::parse("imap"), None);
    }
}
