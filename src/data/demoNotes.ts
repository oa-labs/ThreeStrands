import { noteItemRanges, extractNoteEntityLinks, MAX_NOTE_TASKS, noteReferenceKey, markdownEntityLink, type NoteDocument } from "../noteEntities";
import type { ContactProfile, Thread, ThreadTask, ScheduleEvent, Note, NoteLinks, NoteSave, NoteSearchHit, NoteSummary, NoteTitle } from "../domain";
import { collapseWhitespace, extractWikiLinks, noteTitleKey, rewriteWikiLinks } from "../noteWikiLinks";
import { notePreview, noteInlineText } from "../notePreview";
import type { MailClient } from "./client";

type DemoNotes = Pick<MailClient, "noteTaskWarnings" | "noteLinkChoices" | "resolveNoteEvent" | "noteDocument" | "listNoteTasks" | "patchNoteItem" | "noteBacklinks" | "resolveNoteConversation" | "resolveNoteContact" | "captureNoteConversation" | "listRecentNotes" | "getNote" | "getDailyNote" | "createNote" | "updateNote" | "saveDailyNote" | "recordNoteVisit" | "searchNotes" | "listTrashedNotes" | "trashNote" | "restoreNote" | "deleteNote" | "suggestNoteTitles" | "findNoteByTitle" | "noteLinks" | "notesStorage" | "chooseNotesExportFolder" | "exportNotes" | "chooseNotesImportFolder" | "importNotes" | "addNoteAttachment" | "noteAttachment" | "noteAttachmentImage" | "openNoteAttachment" | "saveNoteAttachment" | "noteAttachmentPreview" | "setNoteAttachmentStar">;

const DATE = /^\d{4}-\d{2}-\d{2}$/;
const clean = collapseWhitespace;
const key = noteTitleKey;

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
export function createDemoNotes(sources: { threads(): Thread[]; tasks(): ThreadTask[]; events(): ScheduleEvent[]; contacts?(): ContactProfile[] } = { threads: () => [], tasks: () => [], events: () => [] }): DemoNotes {
  const notes = new Map<string, Note>();
  const provisional = new Map<string, string>();
  const itemId = (noteId: string, body: string, item: ReturnType<typeof noteItemRanges>[number], occurrence: number) => {
    if (item.id) return item.id;
    const key = JSON.stringify([noteId, body.slice(item.textStart, item.end), occurrence]);
    if (!provisional.has(key)) provisional.set(key, crypto.randomUUID());
    return provisional.get(key)!;
  };
  const normalize = (id: string, body: string) => {
    const items = noteItemRanges(body);
    if (items.filter((item) => item.task).length > MAX_NOTE_TASKS) throw new Error("This note has too many tasks (maximum 500)");
    extractNoteEntityLinks(body);
    const seen = new Set<string>();
    const markers = items.map((item, i) => { const marker = itemId(id, body, item, i); const duplicate = seen.has(marker); seen.add(marker); return { item, marker: duplicate ? crypto.randomUUID() : marker, duplicate }; });
    for (const { item, marker, duplicate } of markers.reverse()) if (item.task && (!item.id || duplicate)) {
      const before = body.slice(0, item.end).replace(duplicate ? /[ \t]*<!-- threestrands-task:[^\n]* -->$/ : /$^/, "");
      body = before + ` <!-- threestrands-task:${marker} -->` + body.slice(item.end);
    }
    return body;
  };
  const visits = new Map<string, string>();
  // Each change gets a distinct, increasing time, as the native store's versions do.
  let clock = Date.now();
  const now = () => new Date(++clock).toISOString();
  const titleTaken = (title: string, id: string) => [...notes.values()].some((note) => note.id !== id && !note.trashed && key(note.title) === key(title));
  const edit = (note: Note, title: string, body: string): Note => {
    if (note.trashed) throw new Error("Restore this note to edit it");
    if (note.title === title && note.body === body) return note;
    if (note.title !== title && titleTaken(title, note.id)) throw new Error(`A note named “${title}” already exists`);
    const next = { ...note, title, body: normalize(note.id, body), updatedAt: now() };
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
  const document = (note: Note): NoteDocument => {
    const items = noteItemRanges(note.body).map((item, index) => ({ ...item, id: itemId(note.id, note.body, item, index), persisted: Boolean(item.id), title: noteInlineText(note.body.slice(item.textStart, item.end).replace(/<!--.*?-->/g, "").trim()) }));
    const { links } = extractNoteEntityLinks(note.body);
    return { items, links, warning: null };
  };
  return {
    async noteLinkChoices(kind, query) {
      const conversation = sources.threads().map((thread) => ({ label: thread.subject || "(No subject)", detail: thread.accountId, reference: { version: 1 as const, kind: "conversation" as const, id: thread.providerThreadId, account: thread.accountId } }));
      const ordinaryTasks = sources.tasks().map((task) => ({ label: task.title, detail: task.accountId, reference: { version: 1 as const, kind: "task" as const, id: task.id } }));
      const noteTasks = [...notes.values()].filter((note) => !note.trashed).flatMap((note) => document(note).items.filter((item) => item.task).map((item) => ({ label: item.title, detail: `Note · ${note.title}`, reference: { version: 1 as const, kind: "task" as const, id: item.id, note: note.id } })));
      const events = sources.events().flatMap((event) => {
        const reference = { version: 1 as const, kind: "event" as const, id: event.id, account: event.accountId, calendar: event.calendarId };
        return [{ label: event.title, detail: `${event.accountId} · ${event.calendarId} · ${event.start}${event.seriesId ? " · This occurrence" : ""}`, reference }, ...(event.seriesId ? [{ label: event.title, detail: `${event.accountId} · Entire series`, reference: { ...reference, id: event.seriesId } }] : [])];
      });
      return (kind === "conversation" ? conversation : kind === "task" ? [...ordinaryTasks, ...noteTasks] : events).filter((choice) => choice.label.toLowerCase().includes(query.toLowerCase())).slice(0, 50);
    },
    async resolveNoteEvent(reference) { return structuredClone(sources.events().find((event) => event.accountId === reference.account && event.calendarId === reference.calendar && (event.id === reference.id || event.seriesId === reference.id)) ?? null); },
    async noteDocument(id) { return document(existing(id)); },
    async noteTaskWarnings() { return []; },
    async listNoteTasks() { return [...notes.values()].filter((note) => !note.trashed).flatMap((note) => document(note).items.filter((item) => item.task).map((item) => ({ id: `note:${note.id}:${item.id}`, markerId: item.id, noteId: note.id, noteTitle: note.title, title: item.title, checked: item.checked, updatedAt: note.updatedAt, completedAt: item.checked ? note.updatedAt : null, conflicted: false }))); },
    async patchNoteItem(id, marker, expected, checked, task) {
      const note = existing(id);
      if (note.updatedAt !== expected || note.trashed) throw new Error("This note changed. Open it to resolve your changes.");
      const item = document(note).items.find((item) => item.id === marker);
      if (!item) throw new Error("This item changed. Open its note to try again.");
      const round = task ?? item.task, state = (checked ?? item.checked) ? "x" : " ";
      let body = note.body;
      if (!item.persisted && (item.task || round)) body = body.slice(0, item.end) + ` <!-- threestrands-task:${item.id} -->` + body.slice(item.end);
      body = body.slice(0, item.markerStart) + (round ? `(${state})` : `[${state}]`) + body.slice(item.markerEnd);
      return structuredClone(edit(note, note.title, body));
    },
    async noteBacklinks(reference) { return [...notes.values()].filter((note) => !note.trashed).flatMap((note) => { const link = document(note).links.find((link) => noteReferenceKey(link.reference) === noteReferenceKey(reference)); return link ? [{ noteId: note.id, title: note.title, dailyDate: note.dailyDate, context: link.context, reference: link.reference }] : []; }); },
    async resolveNoteContact(reference) {
      const contacts = sources.contacts?.() ?? [];
      return (contacts.find((contact) => contact.id === reference.contact) ?? contacts.find((contact) => contact.addresses.includes(reference.id)))?.id ?? null;
    },
    async resolveNoteConversation(reference) { return sources.threads().find((thread) => thread.accountId === reference.account && thread.providerThreadId === reference.id)?.id ?? null; },
    async captureNoteConversation(date, thread, expected) {
      const id = `daily:${date}`, note = notes.get(id);
      if ((note?.updatedAt ?? null) !== expected || note?.trashed) throw new Error("Today's note changed. Open it before adding this conversation.");
      const source = sources.threads().find((item) => item.id === thread);
      if (!source) throw new Error("Conversation unavailable on this device");
      const previousBody = note?.body ?? "", reference = { version: 1 as const, kind: "conversation" as const, id: source.providerThreadId, account: source.accountId };
      if (extractNoteEntityLinks(previousBody).unclosedFence) throw new Error("Today's note ends inside a code block. Open it before adding this conversation.");
      const added = !document(note ?? { body: "" } as Note).links.some((link) => noteReferenceKey(link.reference) === noteReferenceKey(reference));
      const body = added ? `${previousBody}${previousBody && !previousBody.endsWith("\n") ? "\n" : ""}- ${markdownEntityLink(source.subject || "(No subject)", reference)}\n` : previousBody;
      const next = note ? edit(note, note.title, body) : { id, title: date, body: normalize(id, body), dailyDate: date, createdAt: now(), updatedAt: now(), trashed: false, trashedAt: null };
      notes.set(id, next); return { note: structuredClone(next), added, previousBody };
    },
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
      note.body = normalize(note.id, note.body);
      notes.set(note.id, note);
      return structuredClone(note);
    },
    async updateNote(id, title, body, expectedUpdatedAt): Promise<NoteSave> {
      const current = notes.get(id) ?? null;
      // A note in Trash is never current, as natively.
      if (current?.updatedAt !== expectedUpdatedAt || current.trashed) return { saved: false, note: structuredClone(current) };
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
    async chooseNotesImportFolder() {
      return null;
    },
    async importNotes() {
      throw new Error("Importing notes needs the ThreeStrands app");
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
      if ((current?.updatedAt ?? null) !== expectedUpdatedAt || current?.trashed) return { saved: false, note: structuredClone(current) };
      if (current) return { saved: true, note: structuredClone(edit(current, current.title, body)) };
      if (!body) return { saved: true, note: null };
      const time = now();
      const note: Note = { id: `daily:${date}`, title: date, body, dailyDate: date, createdAt: time, updatedAt: time, trashed: false, trashedAt: null };
      note.body = normalize(note.id, note.body);
      notes.set(note.id, note);
      return { saved: true, note: structuredClone(note) };
    },
    async recordNoteVisit(id) {
      if (notes.has(id)) visits.set(id, now());
    },
    async searchNotes(query, limit) {
      const words = collapseWhitespace(query.toLowerCase()).split(" ").filter(Boolean);
      if (!words.length) return [];
      return [...notes.values()]
        .filter((note) => !note.trashed && words.every((word) => `${note.title} ${note.body}`.toLowerCase().includes(word)))
        .slice(0, limit)
        .map((note): NoteSearchHit => ({ id: note.id, title: note.title, dailyDate: note.dailyDate, matchSnippet: preview(note.body) }));
    },
  };
}
