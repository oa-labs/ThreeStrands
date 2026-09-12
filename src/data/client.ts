import { invoke } from "@tauri-apps/api/core";
import type {
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
  listThreads(): Promise<Thread[]>;
  getThread(id: string): Promise<ThreadDetail>;
  searchThreads(request: SearchThreadsRequest): Promise<Thread[]>;
  mutateThread(mutation: ThreadMutation): Promise<void>;
  sync(): Promise<SyncStatus>;
  syncStatus(): Promise<SyncStatus>;
  googleAuthStatus(): Promise<AuthStatus>;
  connectGoogle(): Promise<SyncStatus>;
  disconnectGoogle(): Promise<void>;
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
  listThreads: () => invoke("list_threads"),
  getThread: (id) => invoke("get_thread", { id }),
  searchThreads: (request) => invoke("search_threads", { request }),
  mutateThread: (mutation) => invoke("mutate_thread", { mutation }),
  sync: () => invoke("sync_account"),
  syncStatus: () => invoke("sync_status"),
  googleAuthStatus: () => invoke("google_auth_status"),
  connectGoogle: () => invoke("connect_google"),
  disconnectGoogle: () => invoke("disconnect_google"),
  listLabels: () => invoke("list_labels"),
  createLabel: (name) => invoke("create_label", { request: { name } }),
  updateLabel: (id, name) => invoke("update_label", { request: { id, name } }),
  deleteLabel: (id) => invoke("delete_label", { id }),
};

export const mailClient = isTauri() ? tauriClient : demoClient;
