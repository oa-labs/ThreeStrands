import { readLocalStorage } from "./useLocalStorageState";
import { dateInputValue } from "./keepInTouch";

/** What the note editor has open: a note, or a day whose note may not exist yet. */
export type DailyTarget = { kind: "daily"; date: string };
export type NoteTarget = { kind: "note"; id: string } | DailyTarget;

/** One stable key per target; a day's key is its daily note's id. */
export function noteTargetKey(target: NoteTarget): string {
  return target.kind === "note" ? target.id : `daily:${target.date}`;
}

/** Today's daily note, by the device's local calendar. */
export function todayTarget(now = new Date()): DailyTarget {
  return { kind: "daily", date: dateInputValue(now) };
}

/** The target a stored key names. */
export function parseNoteTargetKey(key: string): NoteTarget {
  const date = key.startsWith("daily:") ? key.slice("daily:".length) : null;
  return date && /^\d{4}-\d{2}-\d{2}$/.test(date) ? { kind: "daily", date } : { kind: "note", id: key };
}

/**
 * Text typed into a note but not yet saved, kept on this device so a crash
 * or a closed window loses nothing. `base` is the `updatedAt` of the version
 * the typing started from (null for a day without a note), so reopening can
 * tell whether the note changed meanwhile.
 */
export type NoteDraft = { title: string; body: string; base: string | null };

const DRAFT_PREFIX = "threestrands.notes.draft.";
const LAST_TARGET_KEY = "threestrands.notes.last";

export function readNoteDraft(key: string): NoteDraft | null {
  return readLocalStorage(DRAFT_PREFIX + key, (raw) => {
    if (!raw) return null;
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object") return null;
    const { title, body, base } = value as Record<string, unknown>;
    return typeof title === "string" && typeof body === "string" && (base === null || typeof base === "string") ? { title, body, base } : null;
  }, null);
}

/** Returns whether the draft was kept; a full or blocked storage keeps only the editor's copy. */
export function writeNoteDraft(key: string, draft: NoteDraft): boolean {
  try {
    localStorage.setItem(DRAFT_PREFIX + key, JSON.stringify(draft));
    return true;
  } catch {
    return false;
  }
}

export function clearNoteDraft(key: string): void {
  try { localStorage.removeItem(DRAFT_PREFIX + key); } catch { /* Nothing was kept. */ }
}

/** The note this device had open last, so Notes reopens it. */
export function readLastNoteTarget(): NoteTarget | null {
  return readLocalStorage(LAST_TARGET_KEY, (raw) => raw ? parseNoteTargetKey(raw) : null, null);
}

export function writeLastNoteTarget(target: NoteTarget): void {
  try { localStorage.setItem(LAST_TARGET_KEY, noteTargetKey(target)); } catch { /* Notes opens Today next time. */ }
}
