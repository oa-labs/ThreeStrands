import { invoke } from "@tauri-apps/api/core";
import type {
  SearchThreadsRequest,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadMutation,
} from "../domain";
import { demoClient } from "./demoClient";

export interface MailClient {
  listThreads(): Promise<Thread[]>;
  getThread(id: string): Promise<ThreadDetail>;
  searchThreads(request: SearchThreadsRequest): Promise<Thread[]>;
  mutateThread(mutation: ThreadMutation): Promise<void>;
  sync(): Promise<SyncStatus>;
  syncStatus(): Promise<SyncStatus>;
}

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

const tauriClient: MailClient = {
  listThreads: () => invoke("list_threads"),
  getThread: (id) => invoke("get_thread", { id }),
  searchThreads: (request) => invoke("search_threads", { request }),
  mutateThread: (mutation) => invoke("mutate_thread", { mutation }),
  sync: () => invoke("sync_account"),
  syncStatus: () => invoke("sync_status"),
};

export const mailClient = isTauri() ? tauriClient : demoClient;
