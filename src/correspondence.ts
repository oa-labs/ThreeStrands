import {
  BOUNDED_LOCAL_READ,
  invokeWithPolicy,
  type InvokePolicy,
  WAIT_FOR_NATIVE_COMPLETION,
} from "./invoke";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward";
export type Attachment = { id: string; name: string; size: number; mime: string; ready: boolean; messageId: string | null; providerId: string | null; inline?: boolean; contentId?: string | null };
export type Draft = {
  id: string; revision: number; account: string; mode: ComposeMode;
  sourceId: string | null; threadId: string | null; replyId: string | null; references: string[];
  to: string; cc: string; bcc: string; subject: string; body: string; bodyHtml?: string;
  /** Preserved sender content, rendered only through SafeMessage and appended by native MIME assembly. */
  forwardedContent?: { html: string; text: string } | null;
  followUpTaskId?: string | null;
  attachments: Attachment[]; updatedAt: number;
};
export type OutboxItem = { id: string; draft: Draft; state: "scheduled" | "overdue" | "undo_pending" | "ready" | "sending" | "sent" | "failed" | "uncertain" | "unverifiable" | "canceled"; deadline: number; error: string | null; providerId?: string | null; schedule?: ScheduledSend | null };
export type ScheduleSelection = { localTime: string; timeZone: string; offsetSeconds?: number | null };
export type ScheduleTimeChoice = { scheduledAt: number; offsetSeconds: number };
export type ScheduledSendReport = {
  operationId: string; ownerInstallationId: string; ownerSyncDeviceId: string | null;
  ownerNameAtCreation: string; account: string; subject: string; scheduledAt: number; timeZone: string;
  state: OutboxItem["state"]; blockedReason: string | null; reportRevision: number; statusChangedAt: number;
  ownerName?: string | null; lastContactAt?: number | null;
};
export type ScheduledSend = { report: ScheduledSendReport; canManage: boolean; visibility: string; visibilityGroupId?: string | null };
export interface CorrespondenceClient {
  senderIdentity(): Promise<string>;
  schedulingInfo(): Promise<{ deviceName: string; sharing: boolean }>;
  scheduleChoices(localTime: string, timeZone: string): Promise<ScheduleTimeChoice[]>;
  scheduleDraft(id: string, revision: number, selection: ScheduleSelection): Promise<OutboxItem>;
  rescheduleSend(id: string, revision: number, selection: ScheduleSelection): Promise<void>;
  sendScheduledNow(id: string, revision: number): Promise<void>;
  listScheduledSummaries(): Promise<ScheduledSendReport[]>;
  /** `account` is required for reply/replyAll/forward (the source thread's owning account) and optional for "new" (defaults to the most-recently-used account). */
  createDraft(mode: ComposeMode, sourceId?: string, account?: string): Promise<Draft>;
  /** Changes a "new" message's sending account; reply/replyAll/forward stay locked to their source thread's account. */
  setDraftAccount(id: string, account: string): Promise<Draft>;
  saveDraft(draft: Draft): Promise<Draft>;
  listDrafts(): Promise<Draft[]>;
  discardDraft(id: string): Promise<void>;
  queueDraft(id: string, revision: number, archiveOnSend?: boolean): Promise<OutboxItem>;
  listOutbox(): Promise<OutboxItem[]>;
  cancelSend(id: string): Promise<Draft>;
  recoverSend(id: string): Promise<Draft>;
  reconcileSend(id: string): Promise<void>;
  attachFiles(id: string): Promise<Draft>;
  attachInlineImage(id: string, name: string, mime: string, data: string): Promise<Draft>;
  readInlineImage(id: string, attachmentId: string): Promise<string>;
  removeAttachment(id: string, attachmentId: string): Promise<Draft>;
  fetchAttachment(id: string, attachmentId: string): Promise<Draft>;
}

type CorrespondenceOperation =
  | "schedulingInfo" | "scheduleChoices" | "schedule" | "reschedule" | "sendScheduledNow" | "listScheduledSummaries"
  | "identity" | "create" | "setAccount" | "save" | "listDrafts"
  | "discard" | "queue" | "listOutbox" | "cancel" | "recover"
  | "reconcile" | "attach" | "attachInline" | "readInline"
  | "removeAttachment" | "fetchAttachment";

const OPERATION_POLICY: Record<CorrespondenceOperation, InvokePolicy> = {
  schedulingInfo: BOUNDED_LOCAL_READ,
  scheduleChoices: BOUNDED_LOCAL_READ,
  schedule: WAIT_FOR_NATIVE_COMPLETION,
  reschedule: WAIT_FOR_NATIVE_COMPLETION,
  sendScheduledNow: WAIT_FOR_NATIVE_COMPLETION,
  listScheduledSummaries: BOUNDED_LOCAL_READ,
  identity: BOUNDED_LOCAL_READ,
  create: WAIT_FOR_NATIVE_COMPLETION,
  setAccount: WAIT_FOR_NATIVE_COMPLETION,
  save: WAIT_FOR_NATIVE_COMPLETION,
  listDrafts: BOUNDED_LOCAL_READ,
  discard: WAIT_FOR_NATIVE_COMPLETION,
  queue: WAIT_FOR_NATIVE_COMPLETION,
  listOutbox: BOUNDED_LOCAL_READ,
  cancel: WAIT_FOR_NATIVE_COMPLETION,
  recover: WAIT_FOR_NATIVE_COMPLETION,
  reconcile: WAIT_FOR_NATIVE_COMPLETION,
  attach: WAIT_FOR_NATIVE_COMPLETION,
  attachInline: WAIT_FOR_NATIVE_COMPLETION,
  readInline: BOUNDED_LOCAL_READ,
  removeAttachment: WAIT_FOR_NATIVE_COMPLETION,
  fetchAttachment: WAIT_FOR_NATIVE_COMPLETION,
};

const request = <T>(op: CorrespondenceOperation, args: object = {}) =>
  invokeWithPolicy<T>(
    "correspondence_request",
    { request: { op, ...args } },
    OPERATION_POLICY[op],
  );
export const nativeCorrespondence: CorrespondenceClient = {
  senderIdentity: () => request("identity"),
  schedulingInfo: () => request("schedulingInfo"),
  scheduleChoices: (localTime,timeZone) => request("scheduleChoices",{localTime,timeZone}),
  scheduleDraft: (id,revision,selection) => request("schedule",{id,revision,selection}),
  rescheduleSend: (id,revision,selection) => request("reschedule",{id,revision,selection}),
  sendScheduledNow: (id,revision) => request("sendScheduledNow",{id,revision}),
  listScheduledSummaries: () => request("listScheduledSummaries"),
  createDraft: (mode, sourceId, account) => request("create", { mode, sourceId, account }),
  setDraftAccount: (id, account) => request("setAccount", { id, account }),
  saveDraft: (draft) => request("save", { draft }),
  listDrafts: () => request("listDrafts"),
  discardDraft: (id) => request("discard", { id }),
  queueDraft: (id, revision, archiveOnSend) => request("queue", { id, revision, archiveOnSend: archiveOnSend ?? false }),
  listOutbox: () => request("listOutbox"),
  cancelSend: (id) => request("cancel", { id }),
  recoverSend: (id) => request("recover", { id }),
  reconcileSend: (id) => request("reconcile", { id }),
  attachFiles: (id) => request("attach", { id }),
  attachInlineImage: (id, name, mime, data) => request("attachInline", { id, name, mime, data }),
  readInlineImage: (id, attachmentId) => request("readInline", { id, attachmentId }),
  removeAttachment: (id, attachmentId) => request("removeAttachment", { id, attachmentId }),
  fetchAttachment: (id, attachmentId) => request("fetchAttachment", { id, attachmentId }),
};
