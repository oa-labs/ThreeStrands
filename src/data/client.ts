import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  AuthStatus,
  CalendarPreview,
  ContactSuggestion,
  Label,
  MailboxUnreadCounts,
  ReplyAssistContext,
  ReplyAssistResult,
  SearchThreadsRequest,
  SplitInbox,
  SplitInboxMatchKind,
  SummaryResult,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadPage,
  ThreadMutation,
  TriageEvent,
  TriageSenderStats,
  UnreadCounts,
  UnsubscribeResult,
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
  replyAssistContext(draftId: string): Promise<ReplyAssistContext>;
  generateReply(
    context: ReplyAssistContext,
    instruction: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
  ): Promise<ReplyAssistResult>;
  searchThreads(request: SearchThreadsRequest, accountId?: string): Promise<Thread[]>;
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
  /** Lists labels for the primary account, or for the specified account when provided. */
  listLabels(accountId?: string): Promise<Label[]>;
  createLabel(name: string): Promise<Label>;
  updateLabel(id: string, name: string): Promise<Label>;
  deleteLabel(id: string): Promise<void>;
  listSplitInboxes(): Promise<SplitInbox[]>;
  createSplitInbox(name: string, matchKind: SplitInboxMatchKind, matchValue: string): Promise<SplitInbox>;
  updateSplitInbox(id: string, name: string): Promise<SplitInbox>;
  deleteSplitInbox(id: string): Promise<void>;
  reorderSplitInboxes(ids: string[]): Promise<void>;
  listSplitInboxPage(splitInboxId: string, accountId: string | undefined, offset: number, limit: number): Promise<ThreadPage>;
}

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

const tauriClient: MailClient = {
  ...nativeCorrespondence,
  listThreads: (accountId) => invoke("list_threads", { accountId }),
  listAllMail: (accountId) => invoke("list_all_mail", { accountId }),
  listTrash: (accountId) => invoke("list_trash", { accountId }),
  listThreadsPage: (accountId, offset, limit) => invoke("list_threads_page", { accountId, offset, limit }),
  listAllMailPage: (accountId, offset, limit) => invoke("list_all_mail_page", { accountId, offset, limit }),
  listTrashPage: (accountId, offset, limit) => invoke("list_trash_page", { accountId, offset, limit }),
  listUnreadCounts: () => invoke("list_unread_counts"),
  mailboxUnreadCounts: (accountId) => invoke("mailbox_unread_counts", { accountId }),
  getThread: (id) => invoke("get_thread", { id }),
  openAttachment: (messageId, attachmentId) => invoke("open_attachment", { messageId, attachmentId }),
  saveAttachment: (messageId, attachmentId) => invoke("save_attachment", { messageId, attachmentId }),
  fetchRemoteImage: (url) => invoke("fetch_remote_image", { url }),
  fetchAttachmentImage: (messageId, attachmentId) => invoke("fetch_attachment_image", { messageId, attachmentId }),
  previewCalendarAttachment: (messageId, attachmentId) => invoke("preview_calendar_attachment", { messageId, attachmentId }),
  summarizeThread: (threadId, provider, model, endpoint) =>
    invoke("ai_summarize_thread", { threadId, provider, model, endpoint }),
  replyAssistContext: (draftId) => invoke("ai_reply_assist_context", { draftId }),
  generateReply: (context, instruction, provider, model, endpoint) =>
    invoke("ai_generate_reply", { context, instruction, provider, model, endpoint }),
  searchThreads: (request, accountId) => invoke("search_threads", { request, accountId }),
  mutateThread: (mutation) => invoke("mutate_thread", { mutation }),
  mutateThreads: (mutations) => invoke("mutate_threads", { mutations }),
  recordTriageEvent: (event) => invoke("record_triage_event", { event }),
  listTriageSenderStats: (accountId, limit) => invoke("list_triage_sender_stats", { accountId, limit }),
  listContactSuggestions: (accountId, query, limit) => invoke("list_contact_suggestions", { accountId, query, limit }),
  pinContact: (accountId, email, displayName) => invoke("pin_contact", { accountId, email, displayName }),
  unpinContact: (accountId, email) => invoke("unpin_contact", { accountId, email }),
  unsubscribe: (messageId) => invoke("unsubscribe", { messageId }),
  sync: () => invoke("sync_account"),
  flushPending: () => invoke("flush_pending_mutations"),
  syncStatus: () => invoke("sync_status"),
  googleAuthStatus: () => invoke("google_auth_status"),
  connectGoogle: () => invoke("connect_google"),
  disconnectGoogle: () => invoke("disconnect_google"),
  listAccounts: () => invoke("list_accounts"),
  addAccount: () => invoke("add_account"),
  removeAccount: (email) => invoke("remove_account", { email }),
  reconnectAccount: (email) => invoke("reconnect_account", { email }),
  setAccountDisplayName: (email, displayName) => invoke("set_account_display_name", { email, displayName }),
  setAccountColor: (email, color) => invoke("set_account_color", { email, color }),
  reorderAccounts: (emails) => invoke("reorder_accounts", { emails }),
  listLabels: (accountId) => invoke("list_labels", { accountId }),
  createLabel: (name) => invoke("create_label", { request: { name } }),
  updateLabel: (id, name) => invoke("update_label", { request: { id, name } }),
  deleteLabel: (id) => invoke("delete_label", { id }),
  listSplitInboxes: () => invoke("list_split_inboxes"),
  createSplitInbox: (name, matchKind, matchValue) =>
    invoke("create_split_inbox", { request: { name, matchKind, matchValue } }),
  updateSplitInbox: (id, name) => invoke("update_split_inbox", { request: { id, name } }),
  deleteSplitInbox: (id) => invoke("delete_split_inbox", { id }),
  reorderSplitInboxes: (ids) => invoke("reorder_split_inboxes", { ids }),
  listSplitInboxPage: (splitInboxId, accountId, offset, limit) =>
    invoke("list_split_inbox_page", { splitInboxId, accountId, offset, limit }),
};

export const mailClient = isTauri() ? tauriClient : demoClient;
