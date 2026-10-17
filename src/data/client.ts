import type { GmailSignature, Signature, SignatureDefaults, SignatureLibrary } from "../signatures";
import type { FileQuery, FilePage, ContextQuery, WorkspaceConversation } from "../domain";
import {
  BOUNDED_LOCAL_READ,
  invokeWithPolicy,
  WAIT_FOR_NATIVE_COMPLETION,
} from "../invoke";
import type {
  Account,
  ActionAnalysis,
  ActionProposal,
  AiUsageDay,
  AuthStatus,
  AvailabilityPreferences,
  AvailabilityResult,
  CalendarAccount,
  CalendarOption,
  CalendarPreview,
  CreateCalendarEventRequest,
  DefaultAppRole,
  DefaultAppStatus,
  UpdateCalendarEventRequest,
  ContactSuggestion,
  ContactFormat,
  ContactImportPreview,
  ContactImportResult,
  ContactActivity,
  ContactFiles,
  ContactGroup,
  ContactGroupRecipients,
  ContactProfile,
  ContactTimelineItem,
  SaveContactRequest,
  ContactEnrichmentResult,
  ContactFieldSuggestion,
  CreateTaskRequest,
  CreateGoalRequest,
  Goal,
  Label,
  MailboxUnreadCounts,
  RecoveryStatus,
  ReplyAssistContext,
  ReplyAssistResult,
  DraftReviewRequest,
  FindTimesRequest,
  TimeSearch,
  DraftReviewResult,
  SearchThreadsRequest,
  SearchReport,
  ScheduleResult,
  ScheduleEvent,
  Snippet,
  SplitInbox,
  SplitInboxMatchKind,
  SummaryResult,
  ThreadBriefResult,
  ThreadChatReply,
  ThreadChatRequest,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadPage,
  ThreadTask,
  ThreadMutation,
  TriageEvent,
  TriageSenderStats,
  SenderHistory,
  MessageTrust,
  SenderFamiliarity,
  SenderRule,
  MuteResult,
  UnreadCounts,
  UnsubscribeResult,
  UpdateTaskRequest,
  UpdateGoalRequest,
  ProposedTimeCheck,
  MailProvider,
} from "../domain";
import type {
  ImapDiscoveryResult,
  ImapCertificateProbe,
  ImapSecurityMode,
  ImapSetupRequest,
  ImapMailboxMapping,
  ImapMailboxMappingCommit,
  ImapSendingOptions,
  ImapConnectionSettings,
  ImapConnectionUpdate,
  SendingIdentity,
  SendingIdentityInput,
} from "../domain";
import type { AiProvider, AiReasoning } from "../aiSettings";
import { nativeCorrespondence, type CorrespondenceClient } from "../correspondence";
import { demoClient } from "./demoClient";

export interface MailClient extends CorrespondenceClient {
  /** Omitted or `"all"` merges every connected account; a specific email scopes to just it. */
  listThreads(accountId?: string): Promise<Thread[]>;
  /** Everything except Trash — archived and inbox threads both included. */
  listAllMail(accountId?: string): Promise<Thread[]>;
  /** Only trashed threads. */
  listTrash(accountId?: string): Promise<Thread[]>;
  listThreadsPage(accountId: string | undefined, offset: number, limit: number): Promise<ThreadPage>;
  listAllMailPage(accountId: string | undefined, offset: number, limit: number): Promise<ThreadPage>;
  listTrashPage(accountId: string | undefined, offset: number, limit: number): Promise<ThreadPage>;
  /** Conversations holding a message one of the account's addresses sent, newest activity first. */
  listSentPage(accountId: string | undefined, offset: number, limit: number): Promise<ThreadPage>;
  listUnreadCounts(): Promise<UnreadCounts>;
  /** Unread totals for the Inbox and each split inbox tab, scoped to `accountId` (merged across all when omitted). */
  mailboxUnreadCounts(accountId?: string): Promise<MailboxUnreadCounts>;
  getThread(id: string): Promise<ThreadDetail>;
  openAttachment(messageId: string, attachmentId: string): Promise<void>;
  saveAttachment(messageId: string, attachmentId: string): Promise<void>;
  /** Fetches a remote image on the reader's behalf and resolves to a `data:` URI; see SafeMessage's `resolveImage` prop. */
  fetchRemoteImage(url: string): Promise<string>;
  /** Resolves an embedded MIME image to a `data:` URI without contacting a remote sender host. */
  fetchAttachmentImage(messageId: string, attachmentId: string): Promise<string>;
  /** A validated PNG `data:` URI picturing the attachment, or null when it has no preview (risky, too large, or not macOS). */
  filePreview(messageId: string, attachmentId: string): Promise<string | null>;
  /** Parses an iCalendar attachment natively and returns display-safe event metadata. */
  previewCalendarAttachment(messageId: string, attachmentId: string): Promise<CalendarPreview>;
  summarizeThread(
    threadId: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
    reasoning?: AiReasoning,
  ): Promise<SummaryResult>;
  analyzeThread(
    threadId: string,
    userTimeZone: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
  ): Promise<ActionAnalysis>;
  threadChat(request: ThreadChatRequest, provider: AiProvider, model: string, endpoint: string | null): Promise<ThreadChatReply>;
  /**
   * Drops a handled suggestion from the thread's saved suggestions for
   * `revision` (its newest message time), so it isn't offered again. Resolves
   * false when nothing saved matched, as for a suggestion from chat.
   */
  removeThreadSuggestion(threadId: string, revision: string, proposal: ActionProposal): Promise<boolean>;
  /** AI provider usage for the last `days` local days, including today. */
  aiUsageSummary(days: number): Promise<AiUsageDay[]>;
  briefThread(
    threadId: string,
    userTimeZone: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
  ): Promise<ThreadBriefResult>;
  replyAssistContext(draftId: string): Promise<ReplyAssistContext>;
  reviewDraft(request: DraftReviewRequest, provider: AiProvider, model: string, endpoint: string | null): Promise<DraftReviewResult>;
  /** Reads when to meet from a draft's conversation; the calendar is searched separately. */
  findTimes(request: FindTimesRequest, provider: AiProvider, model: string, endpoint: string | null): Promise<TimeSearch>;
  generateReply(
    context: ReplyAssistContext,
    instruction: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
    reasoning?: AiReasoning,
  ): Promise<ReplyAssistResult>;
  searchThreads(request: SearchThreadsRequest, accountId?: string): Promise<Thread[]>;
  /** Fetches Gmail search hits missing from the local cache so a subsequent local search can include historical archived mail. Returns a report of what the server search could do, for the UI to word. */
  backfillSearchThreads(query: string, accountId?: string): Promise<SearchReport>;
  mutateThread(mutation: ThreadMutation): Promise<void>;
  mutateThreads(mutations: ThreadMutation[]): Promise<void>;
  recordTriageEvent(event: TriageEvent): Promise<void>;
  listTriageSenderStats(accountId: string, limit?: number): Promise<TriageSenderStats[]>;
  /** Ranked past correspondents for compose autocomplete, built from local mail history rather than an imported address book. */
  listContactSuggestions(accountId: string, query: string, limit?: number): Promise<ContactSuggestion[]>;
  listContactProfiles(query?: string, limit?: number, accountId?: string): Promise<ContactProfile[]>;
  /** Maps each address owned by a saved contact to that contact id; unowned addresses are omitted. */
  resolveContactIds(emails: string[]): Promise<Record<string, string>>;
  getContactProfile(id: string): Promise<ContactProfile | null>;
  saveContactProfile(request: SaveContactRequest): Promise<ContactProfile>;
  deleteContactProfile(id: string): Promise<void>;
  previewContactImport(): Promise<ContactImportPreview | null>;
  importContacts(contacts: SaveContactRequest[]): Promise<ContactImportResult>;
  exportContacts(format: ContactFormat): Promise<boolean>;
  mergeContacts(targetId: string, sourceIds: string[]): Promise<ContactProfile>;
  listContactSuppressions(): Promise<string[]>;
  setContactSuppressed(email: string, suppressed: boolean): Promise<void>;

  /** Every contact group, by name. */
  listContactGroups(): Promise<ContactGroup[]>;
  /**
   * Creates a group with optional first members. `contactIds` may name
   * `derived:<email>` contacts, which are saved first; each typed address in
   * `emails` joins as its saved contact, or as a new one.
   */
  createContactGroup(name: string, contactIds?: string[], emails?: string[]): Promise<ContactGroup>;
  /** Every group with its present members and each member's primary address, for compose. */
  listContactGroupRecipients(): Promise<ContactGroupRecipients[]>;
  renameContactGroup(id: string, name: string): Promise<ContactGroup>;
  /** Adds members the same way `createContactGroup` resolves them. */
  addContactGroupMembers(id: string, contactIds: string[], emails?: string[]): Promise<ContactGroup>;
  removeContactGroupMembers(id: string, contactIds: string[]): Promise<ContactGroup>;
  /** Deletes the group; its contacts stay. */
  deleteContactGroup(id: string): Promise<void>;
  /** Saved contacts with keep-in-touch reminders or a birthday, soonest reminder first. */
  listKeepInTouch(): Promise<ContactProfile[]>;
  /** Sets the interval for each contact (saving `derived:<email>` contacts first), or turns reminders off with null. */
  setKeepInTouch(ids: string[], intervalDays: number | null): Promise<ContactProfile[]>;
  /** Pushes the next reminder to `until` (an ISO instant), or clears the snooze with null. */
  snoozeKeepInTouch(id: string, until: string | null): Promise<ContactProfile>;
  /** Logs a touch outside email at the current time, ending any snooze. */
  markContacted(id: string): Promise<ContactProfile>;
  contactTimeline(id: string, offset?: number, limit?: number, accountId?: string): Promise<ContactTimelineItem[]>;
  /** Counts and timing of local correspondence with a saved id or `derived:<email>`. */
  contactActivity(id: string): Promise<ContactActivity>;
  /** Stored mail from one address in one account, including automated mail. */
  senderHistory(accountId: string, email: string, limit?: number): Promise<SenderHistory>;
  /** Trust signals from a message's stored headers; null when they are not cached. */
  messageTrust(messageId: string): Promise<MessageTrust | null>;
  /** How well the user knows `email`, the sender of `messageId`, who goes by `name`. */
  senderFamiliarity(messageId: string, email: string, name: string): Promise<SenderFamiliarity>;
  listSenderRules(): Promise<SenderRule[]>;
  /** Mutes a sender in one account, marking read and archiving the listed Inbox conversations from them. */
  muteSender(accountId: string, matchKind: SenderRule["matchKind"], matchValue: string, archiveThreadIds: string[]): Promise<MuteResult>;
  deleteSenderRule(id: string): Promise<void>;
  /** Non-inline attachments the person sent, newest first. */
  contextCandidates(query: ContextQuery): Promise<WorkspaceConversation[]>;
  listFiles(query: FileQuery): Promise<FilePage>;
  fileStars(messageIds: string[]): Promise<[string, string][]>;
  /** Without an account, the message's owning account is used. */
  setFileStar(accountId: string | undefined, messageId: string, attachmentId: string, starred: boolean): Promise<void>;
  contactFiles(id: string, limit: number): Promise<ContactFiles>;
  /** Other correspondents at `domain` and their conversations, leaving out `exclude`. */
  /** Open tasks from any conversation with the contact (a saved id or `derived:<email>`). */
  listContactTasks(id: string): Promise<ThreadTask[]>;
  /** Enhances a contact from local email history. `emptyFields` lists the fields holding nothing; only those are ever suggested for, so filled fields are neither revisited nor overwritten. */
  enrichContact(id: string, provider: AiProvider, model: string, endpoint: string | null, emptyFields: ContactFieldSuggestion["field"][], searchMore?: boolean, accountId?: string, reasoning?: AiReasoning): Promise<ContactEnrichmentResult>;
  pinContact(accountId: string, email: string, displayName: string | null): Promise<void>;
  unpinContact(accountId: string, email: string): Promise<void>;
  unsubscribe(messageId: string): Promise<UnsubscribeResult>;
  sync(): Promise<SyncStatus>;
  flushPending(): Promise<SyncStatus>;
  syncStatus(): Promise<SyncStatus>;
  /** Accounts checking their provider for mail right now; `mail-sync-activity` events report changes. */
  mailSyncActivity(): Promise<string[]>;
  /** Requeues every permanently failed mailbox operation for another attempt. */
  retryFailedMutations(): Promise<SyncStatus>;
  /** Clears failed-operation and quarantined-message reports once reviewed. */
  dismissSyncProblems(): Promise<SyncStatus>;
  /** `null` unless this launch had to recover the local database cache. */
  recoveryStatus(): Promise<RecoveryStatus | null>;
  googleAuthStatus(): Promise<AuthStatus>;
  connectGoogle(): Promise<SyncStatus>;
  disconnectGoogle(): Promise<void>;
  listAccounts(): Promise<Account[]>;
  /** Signs in a new account through `provider`'s OAuth flow. */
  addAccount(provider: MailProvider): Promise<Account>;
  /** Autodiscover IMAP/SMTP settings for an email; null = use manual setup. */
  discoverImapSettings(email: string): Promise<ImapDiscoveryResult | null>;
  /** Probe a server's TLS certificate for the cert-trust step (no login). */
  probeImapCertificate(
    host: string,
    port: number,
    security: ImapSecurityMode,
  ): Promise<ImapCertificateProbe>;
  /** Probe SMTP independently; it may use a different certificate from IMAP. */
  probeSmtpCertificate(host: string, port: number, security: ImapSecurityMode): Promise<ImapCertificateProbe>;
  /** Test the entered IMAP + SMTP settings and, on success, save the account. */
  testAndSaveImapAccount(request: ImapSetupRequest): Promise<Account>;
  /** Discover a set-up IMAP account's mailboxes and propose a special-use mapping (persists the catalog). */
  discoverImapMailboxes(email: string): Promise<ImapMailboxMapping>;
  /** Persist the user's confirmed mailbox mapping (Archive choice, per-role overrides, label container). */
  commitImapMailboxMapping(commit: ImapMailboxMappingCommit): Promise<void>;
  /** Start syncing a saved IMAP account after confirming or skipping mapping review. */
  finishImapAccountSetup(email: string): Promise<void>;
  /** An IMAP account's sending options for the Settings panel (identities, sent-copy override + detection). Refuses a non-IMAP account. */
  getImapSendingOptions(account: string): Promise<ImapSendingOptions>;
  /** Replace an IMAP account's sending identities (strict validation; login stays first). Returns the saved list. */
  setImapIdentities(account: string, identities: SendingIdentityInput[]): Promise<SendingIdentity[]>;
  /** Set the "my server saves sent mail itself" override (true = don't save a copy). */
  setImapServerSavesSent(account: string, enabled: boolean): Promise<void>;
  /** Clear this device's Sent-copy detection so the next send re-detects. */
  resetImapSentCopyDetection(account: string): Promise<void>;
  /** An IMAP account's non-secret server settings. Refuses a non-IMAP account. */
  getImapConnectionSettings(account: string): Promise<ImapConnectionSettings>;
  /** Test and save edited server settings or a new password; marks the account connected and restarts sync. */
  updateImapConnection(update: ImapConnectionUpdate): Promise<Account>;
  removeAccount(email: string): Promise<void>;
  reconnectAccount(email: string): Promise<Account>;
  setAccountDisplayName(email: string, displayName: string | null): Promise<void>;
  setAccountNickname(email: string, nickname: string | null): Promise<void>;
  setAccountColor(email: string, color: string): Promise<void>;
  reorderAccounts(emails: string[]): Promise<void>;
  listCalendarAccounts(): Promise<CalendarAccount[]>;
  addCalendarAccount(): Promise<CalendarAccount>;
  reconnectCalendarAccount(email: string): Promise<CalendarAccount>;
  removeCalendarAccount(email: string): Promise<void>;
  listCalendarOptions(): Promise<CalendarOption[]>;
  setCalendarSelection(accountId: string, calendarIds: string[]): Promise<CalendarOption[]>;
  listScheduleEvents(timeMin: string, timeMax: string, timeZone: string): Promise<ScheduleResult>;
  updateCalendarResponse(event: ScheduleEvent, responseStatus: "accepted" | "declined" | "tentative"): Promise<ScheduleEvent>;
  /** Finds an invitation on the connected calendars by its iCalendar UID; null when none has it. */
  findCalendarInvitation(uid: string): Promise<ScheduleEvent | null>;
  defaultAppStatus(): Promise<DefaultAppStatus>;
  /** Asks macOS (which confirms with the user) to make ThreeStrands the default for `role`. */
  makeDefaultApp(role: DefaultAppRole): Promise<DefaultAppStatus>;
  createCalendarEvent(request: CreateCalendarEventRequest): Promise<ScheduleEvent>;
  /** Changes an event the user organizes and notifies its guests. */
  updateCalendarEvent(request: UpdateCalendarEventRequest): Promise<ScheduleEvent>;
  /** Deletes an event the user organizes and notifies its guests. */
  deleteCalendarEvent(event: ScheduleEvent): Promise<void>;
  /** `maxPerDay` spreads the candidates across days instead of the earliest slots of one day. */
  findAvailability(request: { rangeStart: string; rangeEnd: string; preferences: AvailabilityPreferences; maxPerDay?: number }): Promise<AvailabilityResult>;
  checkProposedTime(request: { start: string; end: string; timeZone: string; driveTimeBufferMinutes?: number }): Promise<ProposedTimeCheck>;
  /** Lists labels for the primary account, or for the specified account when provided. */
  listLabels(accountId?: string): Promise<Label[]>;
  /** Mutates the primary account's labels, or the specified account's when provided. */
  createLabel(name: string, accountId?: string): Promise<Label>;
  updateLabel(id: string, name: string, accountId?: string): Promise<Label>;
  deleteLabel(id: string, accountId?: string): Promise<void>;
  listSplitInboxes(): Promise<SplitInbox[]>;
  createSplitInbox(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<SplitInbox>;
  updateSplitInbox(id: string, name: string): Promise<SplitInbox>;
  deleteSplitInbox(id: string): Promise<void>;
  reorderSplitInboxes(ids: string[]): Promise<void>;
  /** Always scoped to the split's own account — see `SplitInbox.accountId`. */
  listSplitInboxPage(splitInboxId: string, offset: number, limit: number): Promise<ThreadPage>;
  listGmailSignatures(account: string): Promise<GmailSignature[]>;
  importGmailSignature(account: string, signature: Signature, useAsDefault: boolean): Promise<void>;
  signatureLibrary(): Promise<SignatureLibrary>;
  saveSignature(signature: Signature): Promise<void>;
  deleteSignature(id: string): Promise<void>;
  saveSignatureDefaults(defaults: SignatureDefaults): Promise<void>;
  deleteSignatureDefaults(account: string, address: string): Promise<void>;
  listSnippets(): Promise<Snippet[]>;
  createSnippet(name: string, body: string): Promise<Snippet>;
  updateSnippet(id: string, name: string, body: string): Promise<Snippet>;
  deleteSnippet(id: string): Promise<void>;
  listTasks(accountId?: string, status?: ThreadTask["status"]): Promise<ThreadTask[]>;
  createTask(request: CreateTaskRequest): Promise<ThreadTask>;
  updateTask(request: UpdateTaskRequest): Promise<ThreadTask>;
  setTaskStatus(id: string, status: ThreadTask["status"], source?: "user" | "reply" | "external"): Promise<ThreadTask>;
  recordFollowUp(id: string): Promise<ThreadTask>;
  reconcileTasks(): Promise<number>;
  listGoals(accountId?: string): Promise<Goal[]>;
  createGoal(request: CreateGoalRequest): Promise<Goal>;
  updateGoal(request: UpdateGoalRequest): Promise<Goal>;
  /** Deletes the goal and unlinks the tasks and goals that supported it. */
  deleteGoal(id: string): Promise<void>;
}

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

const read = <T>(command: string, args?: Record<string, unknown>) =>
  invokeWithPolicy<T>(command, args, BOUNDED_LOCAL_READ);
const complete = <T>(command: string, args?: Record<string, unknown>) =>
  invokeWithPolicy<T>(command, args, WAIT_FOR_NATIVE_COMPLETION);

const tauriClient: MailClient = {
  ...nativeCorrespondence,
  listThreads: (accountId) => read("list_threads", { accountId }),
  listAllMail: (accountId) => read("list_all_mail", { accountId }),
  listTrash: (accountId) => read("list_trash", { accountId }),
  listThreadsPage: (accountId, offset, limit) => read("list_threads_page", { accountId, offset, limit }),
  listAllMailPage: (accountId, offset, limit) => read("list_all_mail_page", { accountId, offset, limit }),
  listTrashPage: (accountId, offset, limit) => read("list_trash_page", { accountId, offset, limit }),
  listSentPage: (accountId, offset, limit) => read("list_sent_page", { accountId, offset, limit }),
  listUnreadCounts: () => read("list_unread_counts"),
  mailboxUnreadCounts: (accountId) => read("mailbox_unread_counts", { accountId }),
  getThread: (id) => read("get_thread", { id }),
  openAttachment: (messageId, attachmentId) => complete("open_attachment", { messageId, attachmentId }),
  saveAttachment: (messageId, attachmentId) => complete("save_attachment", { messageId, attachmentId }),
  fetchRemoteImage: (url) => complete("fetch_remote_image", { url }),
  fetchAttachmentImage: (messageId, attachmentId) => complete("fetch_attachment_image", { messageId, attachmentId }),
  filePreview: (messageId, attachmentId) => complete("file_preview", { messageId, attachmentId }),
  previewCalendarAttachment: (messageId, attachmentId) => complete("preview_calendar_attachment", { messageId, attachmentId }),
  summarizeThread: (threadId, provider, model, endpoint, reasoning = "default") =>
    complete("ai_summarize_thread", { threadId, provider, model, endpoint, reasoning }),
  analyzeThread: (threadId, userTimeZone, provider, model, endpoint) =>
    complete("ai_analyze_thread", { threadId, userTimeZone, provider, model, endpoint }),
  aiUsageSummary: (days) => read("ai_usage_summary", { days }),
  threadChat: (request, provider, model, endpoint) => complete("ai_thread_chat", { request, provider, model, endpoint }),
  removeThreadSuggestion: (threadId, revision, proposal) => complete("remove_thread_suggestion", { threadId, revision, proposal }),
  briefThread: (threadId, userTimeZone, provider, model, endpoint) =>
    complete("ai_brief_thread", { threadId, userTimeZone, provider, model, endpoint }),
  replyAssistContext: (draftId) => read("ai_reply_assist_context", { draftId }),
  reviewDraft: (request, provider, model, endpoint) => complete("ai_review_draft", { request, provider, model, endpoint }),
  findTimes: (request, provider, model, endpoint) => complete("ai_find_times", { request, provider, model, endpoint }),
  generateReply: (context, instruction, provider, model, endpoint, reasoning = "default") =>
    complete("ai_generate_reply", { context, instruction, provider, model, endpoint, reasoning }),
  searchThreads: (request, accountId) => read("search_threads", { request, accountId }),
  backfillSearchThreads: (query, accountId) => complete("backfill_search_threads", { query, accountId }),
  mutateThread: (mutation) => complete("mutate_thread", { mutation }),
  mutateThreads: (mutations) => complete("mutate_threads", { mutations }),
  recordTriageEvent: (event) => complete("record_triage_event", { event }),
  listTriageSenderStats: (accountId, limit) => read("list_triage_sender_stats", { accountId, limit }),
  listContactSuggestions: (accountId, query, limit) => read("list_contact_suggestions", { accountId, query, limit }),
  listContactProfiles: (query = "", limit = 500, accountId) => read("list_contact_profiles", { query, limit, accountId }),
  resolveContactIds: (emails) => read("resolve_contact_ids", { emails }),
  getContactProfile: (id) => read("get_contact_profile", { id }),
  saveContactProfile: (request) => complete("save_contact_profile", { request }),
  deleteContactProfile: (id) => complete("delete_contact_profile", { id }),
  previewContactImport: () => complete("preview_contact_import"),
  importContacts: (contacts) => complete("import_contacts", { contacts }),
  exportContacts: (format) => complete("export_contacts", { format }),
  mergeContacts: (targetId, sourceIds) => complete("merge_contacts", { targetId, sourceIds }),
  listContactSuppressions: () => read("list_contact_suppressions"),
  setContactSuppressed: (email, suppressed) => complete("set_contact_suppressed", { email, suppressed }),

  listContactGroups: () => read("list_contact_groups"),
  createContactGroup: (name, contactIds = [], emails = []) => complete("create_contact_group", { name, contactIds, emails }),
  listContactGroupRecipients: () => read("list_contact_group_recipients"),
  renameContactGroup: (id, name) => complete("rename_contact_group", { id, name }),
  addContactGroupMembers: (id, contactIds, emails = []) => complete("add_contact_group_members", { id, contactIds, emails }),
  removeContactGroupMembers: (id, contactIds) => complete("remove_contact_group_members", { id, contactIds }),
  deleteContactGroup: (id) => complete("delete_contact_group", { id }),
  listKeepInTouch: () => read("list_keep_in_touch"),
  setKeepInTouch: (ids, intervalDays) => complete("set_keep_in_touch", { ids, intervalDays }),
  snoozeKeepInTouch: (id, until) => complete("snooze_keep_in_touch", { id, until }),
  markContacted: (id) => complete("mark_contacted", { id }),
  contactTimeline: (id, offset = 0, limit = 30, accountId) => read("contact_timeline", { id, offset, limit, accountId }),
  contactActivity: (id) => read("contact_activity", { id }),
  senderHistory: (accountId, email, limit) => read("sender_history", { accountId, email, limit }),
  messageTrust: (messageId) => read("message_trust", { messageId }),
  senderFamiliarity: (messageId, email, name) => read("sender_familiarity", { messageId, email, name }),
  listSenderRules: () => read("list_sender_rules"),
  muteSender: (accountId, matchKind, matchValue, archiveThreadIds) => complete("mute_sender", { accountId, matchKind, matchValue, archiveThreadIds }),
  deleteSenderRule: (id) => complete("delete_sender_rule", { id }),
  contextCandidates: (query) => read("context_candidates", { query }),
  listFiles: (query) => read("list_files", { query }),
  fileStars: (messageIds) => read("file_stars", { messageIds }),
  setFileStar: (accountId, messageId, attachmentId, starred) => complete("set_file_star", { accountId, messageId, attachmentId, starred }),
  contactFiles: (id, limit) => read("contact_files", { id, limit }),
  listContactTasks: (id) => read("list_contact_tasks", { id }),
  enrichContact: (id, provider, model, endpoint, emptyFields, searchMore = false, accountId, reasoning = "default") => complete("ai_enrich_contact", { id, provider, model, endpoint, emptyFields, searchMore, accountId, reasoning }),
  pinContact: (accountId, email, displayName) => complete("pin_contact", { accountId, email, displayName }),
  unpinContact: (accountId, email) => complete("unpin_contact", { accountId, email }),
  unsubscribe: (messageId) => complete("unsubscribe", { messageId }),
  sync: () => complete("sync_account"),
  flushPending: () => complete("flush_pending_mutations"),
  syncStatus: () => read("sync_status"),
  mailSyncActivity: () => read("mail_sync_activity"),
  retryFailedMutations: () => complete("retry_failed_mutations"),
  dismissSyncProblems: () => complete("dismiss_sync_problems"),
  recoveryStatus: () => read("recovery_status"),
  googleAuthStatus: () => read("google_auth_status"),
  connectGoogle: () => complete("connect_google"),
  disconnectGoogle: () => complete("disconnect_google"),
  listAccounts: () => read("list_accounts"),
  addAccount: (provider) => complete("add_account", { provider }),
  discoverImapSettings: (email) => complete("discover_imap_settings", { email }),
  probeImapCertificate: (host, port, security) =>
    complete("probe_imap_certificate", { host, port, security }),
  probeSmtpCertificate: (host, port, security) =>
    complete("probe_smtp_certificate", { host, port, security }),
  testAndSaveImapAccount: (request) =>
    complete("test_and_save_imap_account", { request }),
  discoverImapMailboxes: (email) => complete("discover_imap_mailboxes", { email }),
  commitImapMailboxMapping: (commit) => complete("commit_imap_mailbox_mapping", { commit }),
  finishImapAccountSetup: (email) => complete("finish_imap_account_setup", { email }),
  getImapSendingOptions: (account) => complete("get_imap_sending_options", { account }),
  setImapIdentities: (account, identities) => complete("set_imap_identities", { account, identities }),
  setImapServerSavesSent: (account, enabled) => complete("set_imap_server_saves_sent", { account, enabled }),
  resetImapSentCopyDetection: (account) => complete("reset_imap_sent_copy_detection", { account }),
  getImapConnectionSettings: (account) => complete("get_imap_connection_settings", { account }),
  updateImapConnection: (update) => complete("update_imap_connection", { update }),
  removeAccount: (email) => complete("remove_account", { email }),
  reconnectAccount: (email) => complete("reconnect_account", { email }),
  setAccountDisplayName: (email, displayName) => complete("set_account_display_name", { email, displayName }),
  setAccountNickname: (email, nickname) => complete("set_account_nickname", { email, nickname }),
  setAccountColor: (email, color) => complete("set_account_color", { email, color }),
  reorderAccounts: (emails) => complete("reorder_accounts", { emails }),
  listCalendarAccounts: () => read("list_calendar_accounts"),
  addCalendarAccount: () => complete("add_calendar_account"),
  reconnectCalendarAccount: (email) => complete("reconnect_calendar_account", { email }),
  removeCalendarAccount: (email) => complete("remove_calendar_account", { email }),
  listCalendarOptions: () => complete("list_calendar_options"),
  setCalendarSelection: (accountId, calendarIds) =>
    complete("set_calendar_selection", { accountId, calendarIds }),
  listScheduleEvents: (timeMin, timeMax, timeZone) =>
    complete("list_schedule_events", { timeMin, timeMax, timeZone }),
  updateCalendarResponse: (event, responseStatus) =>
    complete("update_calendar_response", { accountId: event.accountId, calendarId: event.calendarId, eventId: event.id, responseStatus }),
  findCalendarInvitation: (uid) => complete("find_calendar_invitation", { uid }),
  defaultAppStatus: () => read("default_app_status"),
  makeDefaultApp: (role) => complete("make_default_app", { role }),
  createCalendarEvent: (request) => complete("create_calendar_event", { request }),
  updateCalendarEvent: (request) => complete("update_calendar_event", { request }),
  deleteCalendarEvent: (event) =>
    complete("delete_calendar_event", { accountId: event.accountId, calendarId: event.calendarId, eventId: event.id }),
  findAvailability: (request) => complete("find_availability", { request }),
  checkProposedTime: (request) => complete("check_proposed_time", { request }),
  listLabels: (accountId) => read("list_labels", { accountId }),
  createLabel: (name, accountId) => complete("create_label", { request: { name, accountId } }),
  updateLabel: (id, name, accountId) => complete("update_label", { request: { id, name, accountId } }),
  deleteLabel: (id, accountId) => complete("delete_label", { id, accountId }),
  listSplitInboxes: () => read("list_split_inboxes"),
  createSplitInbox: (name, matchKind, matchValue, accountId) =>
    complete("create_split_inbox", { request: { name, matchKind, matchValue, accountId } }),
  updateSplitInbox: (id, name) => complete("update_split_inbox", { request: { id, name } }),
  deleteSplitInbox: (id) => complete("delete_split_inbox", { id }),
  reorderSplitInboxes: (ids) => complete("reorder_split_inboxes", { ids }),
  listSplitInboxPage: (splitInboxId, offset, limit) =>
    read("list_split_inbox_page", { splitInboxId, offset, limit }),
  listGmailSignatures: (account) => complete("list_gmail_signatures", { account }),
  importGmailSignature: (account, signature, useAsDefault) => complete("import_gmail_signature", { account, signature, useAsDefault }),
  signatureLibrary: () => read("signature_library"),
  saveSignature: (signature) => complete("save_signature", { signature }),
  deleteSignature: (id) => complete("delete_signature", { id }),
  saveSignatureDefaults: (defaults) => complete("save_signature_defaults", { defaults }),
  deleteSignatureDefaults: (account, address) => complete("delete_signature_defaults", { account, address }),
  listSnippets: () => read("list_snippets"),
  createSnippet: (name, body) => complete("create_snippet", { request: { name, body } }),
  updateSnippet: (id, name, body) => complete("update_snippet", { request: { id, name, body } }),
  deleteSnippet: (id) => complete("delete_snippet", { id }),
  listTasks: (accountId, status) => read("list_tasks", { accountId, status }),
  createTask: (request) => complete("create_task", { request }),
  updateTask: (request) => complete("update_task", { request }),
  setTaskStatus: (id, status, source = "user") => complete("set_task_status", { id, status, source }),
  recordFollowUp: (id) => complete("record_follow_up", { id }),
  reconcileTasks: () => complete("reconcile_tasks"),
  listGoals: (accountId) => read("list_goals", { accountId }),
  createGoal: (request) => complete("create_goal", { request }),
  updateGoal: (request) => complete("update_goal", { request }),
  deleteGoal: (id) => complete("delete_goal", { id }),
};

// Release builds compile `__DEMO_CLIENT__` to false, which drops the demo client.
export const mailClient = __DEMO_CLIENT__ && !isTauri() ? demoClient : tauriClient;
