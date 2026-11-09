import type { Note, NoteLinks, NoteSave, NoteSearchHit, NoteSummary, NoteTitle } from "../domain";
import { extractWikiLinks, noteTitleKey, rewriteWikiLinks } from "../noteWikiLinks";
import { notePreview } from "../notePreview";
import type { MailClient } from "./client";

type DemoNotes = Pick<MailClient, "listRecentNotes" | "getNote" | "getDailyNote" | "createNote" | "updateNote" | "saveDailyNote" | "recordNoteVisit" | "searchNotes" | "listTrashedNotes" | "trashNote" | "restoreNote" | "deleteNote" | "suggestNoteTitles" | "findNoteByTitle" | "noteLinks" | "notesStorage" | "chooseNotesExportFolder" | "exportNotes" | "addNoteAttachment" | "noteAttachment" | "noteAttachmentImage" | "openNoteAttachment" | "saveNoteAttachment" | "noteAttachmentPreview" | "setNoteAttachmentStar">;

const DATE = /^\d{4}-\d{2}-\d{2}$/;
const clean = (title: string) => title.split(/\s+/).filter(Boolean).join(" ");
const key = (title: string) => clean(title).toLocaleLowerCase();

/** A note's title, or why it can't be used, by the native rules (db/notes.rs). */
function noteTitle(title: string): string {
  const cleaned = clean(title);
  if (!cleaned) throw new Error("A note needs a title");
  if ([...cleaned].length > 200) throw new Error("A note title can be at most 200 characters");
  if (/[[\]|]/.test(cleaned)) throw new Error("A note title can't contain [, ], or |");
  if (DATE.test(cleaned)) throw new Error("A date names that day's daily note; choose another title");
  return cleaned;
}

const preview = notePreview;

/** In-memory notes for the browser demo and tests, following the native store's rules. */
export function createDemoNotes(): DemoNotes {
  const notes = new Map<string, Note>();
  const visits = new Map<string, string>();
  // Each change gets a distinct, increasing time, as the native store's versions do.
  let clock = Date.now();
  const now = () => new Date(++clock).toISOString();
  const titleTaken = (title: string, id: string) => [...notes.values()].some((note) => note.id !== id && !note.trashed && key(note.title) === key(title));
  const edit = (note: Note, title: string, body: string): Note => {
    if (note.trashed) throw new Error("Restore this note to edit it");
    if (note.title === title && note.body === body) return note;
    if (note.title !== title && titleTaken(title, note.id)) throw new Error(`A note named “${title}” already exists`);
    const next = { ...note, title, body, updatedAt: now() };
    notes.set(next.id, next);
    return next;
  };
  const summary = (note: Note): NoteSummary => ({ id: note.id, title: note.title, dailyDate: note.dailyDate, preview: preview(note.body), updatedAt: note.updatedAt, visitedAt: visits.get(note.id) ?? null, trashedAt: note.trashedAt });
  const existing = (id: string) => {
    const note = notes.get(id);
    if (!note) throw new Error("Note not found");
    return note;
  };
  const setTrashed = (id: string, trashed: boolean): Note => {
    const note = existing(id);
    if (note.trashed === trashed) return structuredClone(note);
    if (!trashed && !note.dailyDate && titleTaken(note.title, id)) throw new Error(`A note named “${note.title}” already exists. Rename that note, then restore this one.`);
    const next = { ...note, trashed, trashedAt: trashed ? now() : null };
    notes.set(id, next);
    return structuredClone(next);
  };
  return {
    async listRecentNotes(limit) {
      const recent = (note: Note) => [visits.get(note.id) ?? "", note.updatedAt].sort().at(-1)!;
      return [...notes.values()].filter((note) => !note.trashed)
        .sort((a, b) => recent(b).localeCompare(recent(a)) || a.id.localeCompare(b.id))
        .slice(0, limit)
        .map(summary);
    },
    async listTrashedNotes(limit) {
      return [...notes.values()].filter((note) => note.trashed)
        .sort((a, b) => (b.trashedAt ?? "").localeCompare(a.trashedAt ?? "") || a.id.localeCompare(b.id))
        .slice(0, limit)
        .map(summary);
    },
    async trashNote(id) {
      return setTrashed(id, true);
    },
    async restoreNote(id) {
      return setTrashed(id, false);
    },
    async deleteNote(id) {
      if (!existing(id).trashed) throw new Error("Move this note to Trash before deleting it");
      notes.delete(id);
      visits.delete(id);
    },
    async getNote(id) {
      return structuredClone(notes.get(id) ?? null);
    },
    async getDailyNote(date) {
      if (!DATE.test(date)) throw new Error("That isn't a valid date");
      return structuredClone(notes.get(`daily:${date}`) ?? null);
    },
    async createNote(title, body) {
      const cleaned = noteTitle(title);
      if (titleTaken(cleaned, "")) throw new Error(`A note named “${cleaned}” already exists`);
      const time = now();
      const note: Note = { id: `demo-note-${crypto.randomUUID()}`, title: cleaned, body, dailyDate: null, createdAt: time, updatedAt: time, trashed: false, trashedAt: null };
      notes.set(note.id, note);
      return structuredClone(note);
    },
    async updateNote(id, title, body, expectedUpdatedAt): Promise<NoteSave> {
      const current = notes.get(id) ?? null;
      if (current?.updatedAt !== expectedUpdatedAt) return { saved: false, note: structuredClone(current) };
      const oldKey = key(current.title);
      const next = edit(current, current.dailyDate ? current.title : noteTitle(title), body);
      if (key(next.title) === oldKey) return { saved: true, note: structuredClone(next) };
      // Links in other notes follow a rename, as natively.
      let updated = 0;
      for (const other of [...notes.values()]) {
        if (other.id === id || other.trashed) continue;
        const rewritten = rewriteWikiLinks(other.body, oldKey, next.title);
        if (rewritten === null) continue;
        notes.set(other.id, { ...other, body: rewritten, updatedAt: now() });
        updated++;
      }
      return { saved: true, note: structuredClone(next), relink: { updated, keptOldLink: [] } };
    },
    async notesStorage() {
      const usedBytes = [...notes.values()].reduce((total, note) => total + new TextEncoder().encode(JSON.stringify(note)).length, 0);
      return { usedBytes, limitBytes: 3 * 1024 * 1024, attachmentCount: 0, attachmentBytes: 0 };
    },
    // The browser demo keeps no files.
    async addNoteAttachment() {
      throw new Error("Adding files to notes needs the ThreeStrands app");
    },
    async noteAttachment() {
      return null;
    },
    async noteAttachmentImage() {
      return null;
    },
    async openNoteAttachment() {
      throw new Error("Opening files needs the ThreeStrands app");
    },
    async saveNoteAttachment() {
      throw new Error("Saving files needs the ThreeStrands app");
    },
    async noteAttachmentPreview() {
      return null;
    },
    async setNoteAttachmentStar() {
      throw new Error("The browser demo keeps no files in notes");
    },
    // The browser demo has no folders to write to.
    async chooseNotesExportFolder() {
      return null;
    },
    async exportNotes() {
      throw new Error("Exporting notes needs the ThreeStrands app");
    },
    async suggestNoteTitles(query, limit) {
      const wanted = noteTitleKey(query);
      const recent = (note: Note) => [visits.get(note.id) ?? "", note.updatedAt].sort().at(-1)!;
      return [...notes.values()]
        .filter((note) => !note.trashed && !note.dailyDate && key(note.title).includes(wanted))
        .sort((a, b) => Number(!key(a.title).startsWith(wanted)) - Number(!key(b.title).startsWith(wanted)) || recent(b).localeCompare(recent(a)))
        .slice(0, limit)
        .map((note): NoteTitle => ({ id: note.id, title: note.title, dailyDate: note.dailyDate }));
    },
    async findNoteByTitle(title) {
      const note = [...notes.values()].find((item) => !item.trashed && key(item.title) === noteTitleKey(title));
      return note ? { id: note.id, title: note.title, dailyDate: note.dailyDate } : null;
    },
    async noteLinks(id): Promise<NoteLinks> {
      const note = notes.get(id);
      if (!note) throw new Error("Note not found");
      const backlinks = [...notes.values()]
        .filter((other) => other.id !== id && !other.trashed)
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
        .flatMap((other) => extractWikiLinks(other.body)
          .filter((link) => noteTitleKey(link.title) === key(note.title))
          .map((link) => ({ noteId: other.id, title: other.title, dailyDate: other.dailyDate, context: link.context })));
      const links: string[] = [];
      for (const link of extractWikiLinks(note.body)) {
        if (!links.some((title) => noteTitleKey(title) === noteTitleKey(link.title))) links.push(link.title);
      }
      const exists = (title: string) => [...notes.values()].some((other) => !other.trashed && key(other.title) === noteTitleKey(title));
      return { backlinks, unlinked: links.filter((title) => noteTitleKey(title) !== key(note.title) && !exists(title)), links };
    },
    async saveDailyNote(date, body, expectedUpdatedAt): Promise<NoteSave> {
      if (!DATE.test(date)) throw new Error("That isn't a valid date");
      const current = notes.get(`daily:${date}`) ?? null;
      if ((current?.updatedAt ?? null) !== expectedUpdatedAt) return { saved: false, note: structuredClone(current) };
      if (current) return { saved: true, note: structuredClone(edit(current, current.title, body)) };
      if (!body) return { saved: true, note: null };
      const time = now();
      const note: Note = { id: `daily:${date}`, title: date, body, dailyDate: date, createdAt: time, updatedAt: time, trashed: false, trashedAt: null };
      notes.set(note.id, note);
      return { saved: true, note: structuredClone(note) };
    },
    async recordNoteVisit(id) {
      if (notes.has(id)) visits.set(id, now());
    },
    async searchNotes(query, limit) {
      const words = query.toLocaleLowerCase().split(/\s+/).filter(Boolean);
      if (!words.length) return [];
      return [...notes.values()]
        .filter((note) => !note.trashed && words.every((word) => `${note.title} ${note.body}`.toLocaleLowerCase().includes(word)))
        .slice(0, limit)
        .map((note): NoteSearchHit => ({ id: note.id, title: note.title, dailyDate: note.dailyDate, matchSnippet: preview(note.body) }));
    },
  };
}
