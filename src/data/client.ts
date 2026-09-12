import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  AuthStatus,
  Label,
  SearchThreadsRequest,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadMutation,
} from "../domain";
import { nativeCorrespondence, type CorrespondenceClient } from "../correspondence";
import { demoClient } from "./demoClient";

export interface MailClient extends CorrespondenceClient {
  /** Omitted or `"all"` merges every connected account; a specific email scopes to just it. */
  listThreads(accountId?: string): Promise<Thread[]>;
  getThread(id: string): Promise<ThreadDetail>;
  searchThreads(request: SearchThreadsRequest, accountId?: string): Promise<Thread[]>;
  mutateThread(mutation: ThreadMutation): Promise<void>;
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
  getThread: (id) => invoke("get_thread", { id }),
  searchThreads: (request, accountId) => invoke("search_threads", { request, accountId }),
  mutateThread: (mutation) => invoke("mutate_thread", { mutation }),
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
