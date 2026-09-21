import {
  BOUNDED_LOCAL_READ,
  invokeWithPolicy,
  WAIT_FOR_NATIVE_COMPLETION,
} from "../invoke";
import type {
  Account,
  ActionProposal,
  AuthStatus,
  AvailabilityPreferences,
  AvailabilityResult,
  CalendarAccount,
  CalendarOption,
  CalendarPreview,
  ContactSuggestion,
  CreateTaskRequest,
  Label,
  MailboxUnreadCounts,
  RecoveryStatus,
  ReplyAssistContext,
  ReplyAssistResult,
  SearchThreadsRequest,
  ScheduleResult,
  Snippet,
  SplitInbox,
  SplitInboxMatchKind,
  SummaryResult,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadPage,
  ThreadTask,
  ThreadMutation,
  TriageEvent,
  TriageSenderStats,
  UnreadCounts,
  UnsubscribeResult,
  UpdateTaskRequest,
  ProposedTimeCheck,
} from "../domain";
import type { AiProvider } from "../aiSettings";
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
  /** Parses an iCalendar attachment natively and returns display-safe event metadata. */
  previewCalendarAttachment(messageId: string, attachmentId: string): Promise<CalendarPreview>;
  summarizeThread(
    threadId: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
  ): Promise<SummaryResult>;
  analyzeThread(
    threadId: string,
    userTimeZone: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
  ): Promise<ActionProposal[]>;
  replyAssistContext(draftId: string): Promise<ReplyAssistContext>;
  generateReply(
    context: ReplyAssistContext,
    instruction: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
  ): Promise<ReplyAssistResult>;
  searchThreads(request: SearchThreadsRequest, accountId?: string): Promise<Thread[]>;
  /** Fetches Gmail search hits missing from the local cache so a subsequent local search can include historical archived mail. */
  backfillSearchThreads(query: string, accountId?: string): Promise<void>;
  mutateThread(mutation: ThreadMutation): Promise<void>;
  mutateThreads(mutations: ThreadMutation[]): Promise<void>;
  recordTriageEvent(event: TriageEvent): Promise<void>;
  listTriageSenderStats(accountId: string, limit?: number): Promise<TriageSenderStats[]>;
  /** Ranked past correspondents for compose autocomplete, built from local mail history rather than an imported address book. */
  listContactSuggestions(accountId: string, query: string, limit?: number): Promise<ContactSuggestion[]>;
  pinContact(accountId: string, email: string, displayName: string | null): Promise<void>;
  unpinContact(accountId: string, email: string): Promise<void>;
  unsubscribe(messageId: string): Promise<UnsubscribeResult>;
  sync(): Promise<SyncStatus>;
  flushPending(): Promise<SyncStatus>;
  syncStatus(): Promise<SyncStatus>;
  /** `null` unless this launch had to recover the local database cache. */
  recoveryStatus(): Promise<RecoveryStatus | null>;
  googleAuthStatus(): Promise<AuthStatus>;
  connectGoogle(): Promise<SyncStatus>;
  disconnectGoogle(): Promise<void>;
  listAccounts(): Promise<Account[]>;
  addAccount(): Promise<Account>;
  removeAccount(email: string): Promise<void>;
  reconnectAccount(email: string): Promise<Account>;
  setAccountDisplayName(email: string, displayName: string | null): Promise<void>;
  setAccountColor(email: string, color: string): Promise<void>;
  reorderAccounts(emails: string[]): Promise<void>;
  listCalendarAccounts(): Promise<CalendarAccount[]>;
  addCalendarAccount(): Promise<CalendarAccount>;
  reconnectCalendarAccount(email: string): Promise<CalendarAccount>;
  removeCalendarAccount(email: string): Promise<void>;
  listCalendarOptions(): Promise<CalendarOption[]>;
  setCalendarSelection(accountId: string, calendarIds: string[]): Promise<CalendarOption[]>;
  listScheduleEvents(timeMin: string, timeMax: string, timeZone: string): Promise<ScheduleResult>;
  findAvailability(request: { rangeStart: string; rangeEnd: string; preferences: AvailabilityPreferences }): Promise<AvailabilityResult>;
  checkProposedTime(request: { start: string; end: string; timeZone: string }): Promise<ProposedTimeCheck>;
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
  listUnreadCounts: () => read("list_unread_counts"),
  mailboxUnreadCounts: (accountId) => read("mailbox_unread_counts", { accountId }),
  getThread: (id) => read("get_thread", { id }),
  openAttachment: (messageId, attachmentId) => complete("open_attachment", { messageId, attachmentId }),
  saveAttachment: (messageId, attachmentId) => complete("save_attachment", { messageId, attachmentId }),
  fetchRemoteImage: (url) => complete("fetch_remote_image", { url }),
  fetchAttachmentImage: (messageId, attachmentId) => complete("fetch_attachment_image", { messageId, attachmentId }),
  previewCalendarAttachment: (messageId, attachmentId) => complete("preview_calendar_attachment", { messageId, attachmentId }),
  summarizeThread: (threadId, provider, model, endpoint) =>
    complete("ai_summarize_thread", { threadId, provider, model, endpoint }),
  analyzeThread: (threadId, userTimeZone, provider, model, endpoint) =>
    complete("ai_analyze_thread", { threadId, userTimeZone, provider, model, endpoint }),
  replyAssistContext: (draftId) => read("ai_reply_assist_context", { draftId }),
  generateReply: (context, instruction, provider, model, endpoint) =>
    complete("ai_generate_reply", { context, instruction, provider, model, endpoint }),
  searchThreads: (request, accountId) => read("search_threads", { request, accountId }),
  backfillSearchThreads: (query, accountId) => complete("backfill_search_threads", { query, accountId }),
  mutateThread: (mutation) => complete("mutate_thread", { mutation }),
  mutateThreads: (mutations) => complete("mutate_threads", { mutations }),
  recordTriageEvent: (event) => complete("record_triage_event", { event }),
  listTriageSenderStats: (accountId, limit) => read("list_triage_sender_stats", { accountId, limit }),
  listContactSuggestions: (accountId, query, limit) => read("list_contact_suggestions", { accountId, query, limit }),
  pinContact: (accountId, email, displayName) => complete("pin_contact", { accountId, email, displayName }),
  unpinContact: (accountId, email) => complete("unpin_contact", { accountId, email }),
  unsubscribe: (messageId) => complete("unsubscribe", { messageId }),
  sync: () => complete("sync_account"),
  flushPending: () => complete("flush_pending_mutations"),
  syncStatus: () => read("sync_status"),
  recoveryStatus: () => read("recovery_status"),
  googleAuthStatus: () => read("google_auth_status"),
  connectGoogle: () => complete("connect_google"),
  disconnectGoogle: () => complete("disconnect_google"),
  listAccounts: () => read("list_accounts"),
  addAccount: () => complete("add_account"),
  removeAccount: (email) => complete("remove_account", { email }),
  reconnectAccount: (email) => complete("reconnect_account", { email }),
  setAccountDisplayName: (email, displayName) => complete("set_account_display_name", { email, displayName }),
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
};

export const mailClient = isTauri() ? tauriClient : demoClient;
