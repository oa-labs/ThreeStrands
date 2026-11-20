import { invoke } from "@tauri-apps/api/core";
import type { Note, NoteLinks, NoteSave, NoteSearchHit, NoteSummary, NoteTitle, ThreadTask } from "../../src/domain";
import type { NoteDraft, NoteTarget } from "../../src/noteDrafts";
import type { NoteDocument, NoteTaskRecord } from "../../src/noteEntities";

export type NoteView = "recent" | "starred" | "trash";
export type NoteAction = "star" | "unstar" | "trash" | "restore";
export type Conflict = {
  field: string;
  candidates: Array<{ operationId: string; deviceId: string; value: unknown }>;
};
export type OpenNote = {
  note: Note | null;
  document: NoteDocument | null;
  links: NoteLinks | null;
  conflicts: Conflict[];
};
export type SyncStatus = {
  enrolled: boolean;
  revoked: boolean;
  awaitingAdmissionFrom: string | null;
  lastSuccessAt: string | null;
  lastError: string | null;
};
export type JoinPreview = {
  inviterName: string;
  expiresAt: string;
  connectors: Array<{ location: string; label?: string; needsCredentials: boolean }>;
};
export type S3Credentials = { accessKeyId: string; secretAccessKey: string; sessionToken?: string };
export type TaskEdit = { id: string; title?: string; notes?: string | null; dueKind?: "none" | "date"; dueValue?: string | null; timeZone?: null };

export const mobile = {
  draft: (key: string) => invoke<NoteDraft | null>("mobile_draft", { key }),
  writeDraft: (key: string, draft: NoteDraft | null, expected: NoteDraft | null) =>
    invoke<void>("mobile_write_draft", { key, draft, expected }),
  note: (target: NoteTarget) => invoke<OpenNote>("mobile_note", { target }),
  notes: (view: NoteView, query: string) => invoke<Array<NoteSummary | NoteSearchHit>>("mobile_notes", { view, query }),
  titles: (query: string) => invoke<NoteTitle[]>("mobile_note_titles", { query }),
  findNote: (title: string) => invoke<NoteTitle | null>("mobile_find_note", { title }),
  saveNote: (target: NoteTarget, title: string, body: string, expected: string | null) =>
    invoke<NoteSave>("mobile_save_note", { target, title, body, expected }),
  createNote: (title: string, body: string) => invoke<Note>("mobile_create_note", { title, body }),
  noteAction: (id: string, action: NoteAction) => invoke<Note>("mobile_note_action", { id, action }),
  resolveNote: (id: string, field: string, operation: string) => invoke<void>("mobile_resolve_note", { id, field, operation }),
  checkNoteItem: (id: string, marker: string, expected: string, checked: boolean) =>
    invoke<Note>("mobile_check_note_item", { id, marker, expected, checked }),
  tasks: () => invoke<{ standalone: ThreadTask[]; fromNotes: NoteTaskRecord[] }>("mobile_tasks"),
  createTask: (title: string, notes: string | null, dueDate: string | null) =>
    invoke<ThreadTask>("mobile_create_task", { title, notes, dueDate }),
  updateTask: (request: TaskEdit) => invoke<ThreadTask>("mobile_update_task", { request }),
  checkTask: (id: string, checked: boolean, expected: string, restoreStatus?: "in_progress") =>
    invoke<ThreadTask>("mobile_check_task", { id, checked, expected, restoreStatus }),
  syncStatus: () => invoke<SyncStatus>("mobile_sync_status"),
  syncNow: () => invoke<SyncStatus>("mobile_sync_now"),
  previewJoin: (code: string) => invoke<JoinPreview>("mobile_preview_join", { code }),
  join: (code: string, credentials: S3Credentials | null) => invoke<SyncStatus>("mobile_join", { code, credentials }),
  disconnect: () => invoke<SyncStatus>("mobile_disconnect"),
  export: () => invoke<boolean>("mobile_export"),
  openUrl: (url: string) => invoke<void>("mobile_open_url", { url }),
};

export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "message" in error && typeof error.message === "string") return error.message;
  return "ThreeStrands could not finish that operation.";
}
