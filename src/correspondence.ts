import { invoke } from "@tauri-apps/api/core";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward";
export type Attachment = { id: string; name: string; size: number; mime: string; ready: boolean; messageId: string | null; providerId: string | null };
export type Draft = {
  id: string; revision: number; account: string; mode: ComposeMode;
  sourceId: string | null; threadId: string | null; replyId: string | null; references: string[];
  to: string; cc: string; bcc: string; subject: string; body: string; bodyHtml?: string;
  attachments: Attachment[]; updatedAt: number;
};
export type OutboxItem = { id: string; draft: Draft; state: "undo_pending" | "ready" | "sending" | "sent" | "failed" | "uncertain" | "canceled"; deadline: number; error: string | null };
export interface CorrespondenceClient {
  senderIdentity(): Promise<string>;
  /** `account` is required for reply/replyAll/forward (the source thread's owning account) and optional for "new" (defaults to the most-recently-used account). */
  createDraft(mode: ComposeMode, sourceId?: string, account?: string): Promise<Draft>;
  /** Changes a "new" message's sending account; reply/replyAll/forward stay locked to their source thread's account. */
  setDraftAccount(id: string, account: string): Promise<Draft>;
  saveDraft(draft: Draft): Promise<Draft>;
  listDrafts(): Promise<Draft[]>;
  discardDraft(id: string): Promise<void>;
  queueDraft(id: string, revision: number): Promise<OutboxItem>;
  listOutbox(): Promise<OutboxItem[]>;
  cancelSend(id: string): Promise<Draft>;
  recoverSend(id: string): Promise<Draft>;
  reconcileSend(id: string): Promise<void>;
  attachFiles(id: string): Promise<Draft>;
  removeAttachment(id: string, attachmentId: string): Promise<Draft>;
  fetchAttachment(id: string, attachmentId: string): Promise<Draft>;
}
const request = <T>(op: string, args: object = {}) => invoke<T>("correspondence_request", { request: { op, ...args } });
export const nativeCorrespondence: CorrespondenceClient = {
  senderIdentity: () => request("identity"),
  createDraft: (mode, sourceId, account) => request("create", { mode, sourceId, account }),
  setDraftAccount: (id, account) => request("setAccount", { id, account }),
  saveDraft: (draft) => request("save", { draft }),
  listDrafts: () => request("listDrafts"),
  discardDraft: (id) => request("discard", { id }),
  queueDraft: (id, revision) => request("queue", { id, revision }),
  listOutbox: () => request("listOutbox"),
  cancelSend: (id) => request("cancel", { id }),
  recoverSend: (id) => request("recover", { id }),
  reconcileSend: (id) => request("reconcile", { id }),
  attachFiles: (id) => request("attach", { id }),
  removeAttachment: (id, attachmentId) => request("removeAttachment", { id, attachmentId }),
  fetchAttachment: (id, attachmentId) => request("fetchAttachment", { id, attachmentId }),
};
