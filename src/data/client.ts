import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  AuthStatus,
  Label,
  SearchThreadsRequest,
  SummaryResult,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadPage,
  ThreadMutation,
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
  getThread(id: string): Promise<ThreadDetail>;
  summarizeThread(
    threadId: string,
    provider: AiProvider,
    model: string,
    endpoint: string | null,
  ): Promise<SummaryResult>;
  searchThreads(request: SearchThreadsRequest, accountId?: string): Promise<Thread[]>;
  mutateThread(mutation: ThreadMutation): Promise<void>;
  mutateThreads(mutations: ThreadMutation[]): Promise<void>;
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
  setAccountColor(email: string, color: string): Promise<void>;
  reorderAccounts(emails: string[]): Promise<void>;
  listLabels(): Promise<Label[]>;
  createLabel(name: string): Promise<Label>;
  updateLabel(id: string, name: string): Promise<Label>;
  deleteLabel(id: string): Promise<void>;
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
  getThread: (id) => invoke("get_thread", { id }),
  summarizeThread: (threadId, provider, model, endpoint) =>
    invoke("ai_summarize_thread", { threadId, provider, model, endpoint }),
  searchThreads: (request, accountId) => invoke("search_threads", { request, accountId }),
  mutateThread: (mutation) => invoke("mutate_thread", { mutation }),
  mutateThreads: (mutations) => invoke("mutate_threads", { mutations }),
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
  setAccountColor: (email, color) => invoke("set_account_color", { email, color }),
  reorderAccounts: (emails) => invoke("reorder_accounts", { emails }),
  listLabels: () => invoke("list_labels"),
  createLabel: (name) => invoke("create_label", { request: { name } }),
  updateLabel: (id, name) => invoke("update_label", { request: { id, name } }),
  deleteLabel: (id) => invoke("delete_label", { id }),
};

export const mailClient = isTauri() ? tauriClient : demoClient;
