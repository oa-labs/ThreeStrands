import { noteItemRanges, extractNoteEntityLinks, MAX_NOTE_TASKS, noteReferenceKey, noteReferenceSeriesKey, contactNoteReferences, markdownEntityLink, type NoteDocument } from "../noteEntities";
import type { ContactProfile, Thread, ThreadTask, ScheduleEvent, Note, NoteLinks, NoteSave, NoteSearchHit, NoteSummary, NoteTitle } from "../domain";
import { collapseWhitespace, extractWikiLinks, noteTitleKey, rewriteWikiLinks } from "../noteWikiLinks";
import { notePlainText, notePreview, noteInlineText } from "../notePreview";
import { insertMeetingLine, meetingLine, meetingNoteTitle } from "../noteMeetings";
import type { MailClient } from "./client";
import type { DemoNoteSeed } from "./demoDataset";

type DemoNotes = Pick<MailClient, "noteTaskWarnings" | "noteLinkChoices" | "resolveNoteEvent" | "noteDocument" | "listNoteTasks" | "patchNoteItem" | "noteBacklinks" | "resolveNoteConversation" | "resolveNoteContact" | "captureNoteConversation" | "addMeetingToDailyNote" | "listRecentNotes" | "getNote" | "getDailyNote" | "createNote" | "updateNote" | "saveDailyNote" | "recordNoteVisit" | "searchNotes" | "listTrashedNotes" | "listDailyNoteDates" | "trashNote" | "restoreNote" | "deleteNote" | "suggestNoteTitles" | "findNoteByTitle" | "noteLinks" | "notesStorage" | "chooseNotesExportFolder" | "exportNotes" | "chooseNotesImportFolder" | "importNotes" | "importNoteFile" | "addNoteAttachment" | "noteAttachment" | "noteAttachmentImage" | "openNoteAttachment" | "saveNoteAttachment" | "noteAttachmentPreview" | "setNoteAttachmentStar">;

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
/** Most characters of a backlink's line, as natively (MAX_LINK_CONTEXT_CHARS). */
const LINK_CONTEXT_CHARS = 160;
/** A backlink's line as text, cut with an ellipsis, as natively. */
const linkContext = (line: string) => {
  const text = [...notePlainText(line)];
  return text.length <= LINK_CONTEXT_CHARS ? text.join("") : `${text.slice(0, LINK_CONTEXT_CHARS - 1).join("")}…`;
};

/** In-memory notes for the browser demo and tests, following the native store's rules. */
export function createDemoNotes(sources: { threads(): Thread[]; tasks(): ThreadTask[]; events(): ScheduleEvent[]; contacts?(): ContactProfile[] | Promise<ContactProfile[]> } = { threads: () => [], tasks: () => [], events: () => [] }, seeds: readonly DemoNoteSeed[] = []): DemoNotes {
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
  // Seeds load in order, so the last one is the most recently changed note.
  for (const [index, seed] of seeds.entries()) {
    const time = now();
    const daily = "dailyDate" in seed;
    if (daily && !DATE.test(seed.dailyDate)) throw new Error("That isn't a valid date");
    const title = daily ? seed.dailyDate : noteTitle(seed.title);
    if (!daily && titleTaken(title, "")) throw new Error(`A note named “${title}” already exists`);
    const id = daily ? `daily:${seed.dailyDate}` : `demo-note-seed-${index}`;
    if (notes.has(id)) throw new Error(`${title} is seeded twice`);
    notes.set(id, { id, title, body: normalize(id, seed.body), dailyDate: daily ? seed.dailyDate : null, createdAt: time, updatedAt: time, trashed: false, trashedAt: null });
  }
  return {
    async noteLinkChoices(kind, query) {
      const conversation = sources.threads().slice().sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt)).map((thread) => ({ label: thread.subject || "(No subject)", detail: `${thread.participants.join(", ")} · ${thread.accountId}`, sentAt: thread.lastMessageAt, reference: { version: 1 as const, kind: "conversation" as const, id: thread.providerThreadId, account: thread.accountId } }));
      if (kind === "contact") return ((await sources.contacts?.()) ?? []).flatMap((contact) => {
        const reference = contactNoteReferences(contact.addresses)[0];
        return reference ? [{ label: contact.displayName?.trim() || reference.id, detail: contact.addresses.join(" · "), reference: { ...reference, ...(!contact.id.startsWith("derived:") ? { contact: contact.id } : {}) } }] : [];
      }).filter((choice) => `${choice.label} ${choice.detail}`.toLowerCase().includes(query.trim().toLowerCase())).slice(0, 50);
      const ordinaryTasks = sources.tasks().map((task) => ({ label: task.title, detail: task.accountId, reference: { version: 1 as const, kind: "task" as const, id: task.id } }));
      const noteTasks = [...notes.values()].filter((note) => !note.trashed).flatMap((note) => document(note).items.filter((item) => item.task).map((item) => ({ label: item.title, detail: `Note · ${note.title}`, reference: { version: 1 as const, kind: "task" as const, id: item.id, note: note.id } })));
      const events = sources.events().flatMap((event) => {
        const reference = { version: 1 as const, kind: "event" as const, id: event.id, account: event.accountId, calendar: event.calendarId };
        return [{ label: event.title, detail: `${event.accountId} · ${event.calendarId} · ${event.start}${event.seriesId ? " · This occurrence" : ""}`, reference }, ...(event.seriesId ? [{ label: event.title, detail: `${event.accountId} · Entire series`, reference: { ...reference, id: event.seriesId } }] : [])];
      });
      return (kind === "conversation" ? conversation : kind === "task" ? [...ordinaryTasks, ...noteTasks] : events).filter((choice) => `${choice.label}${kind === "conversation" ? ` ${choice.detail}` : ""}`.toLowerCase().includes(query.toLowerCase())).slice(0, 50);
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
    async noteBacklinks(reference) {
      const key = noteReferenceKey(reference);
      return [...notes.values()].filter((note) => !note.trashed).flatMap((note) => { const link = document(note).links.find((link) => noteReferenceKey(link.reference) === key || noteReferenceSeriesKey(link.reference) === key); return link ? [{ noteId: note.id, title: note.title, dailyDate: note.dailyDate, context: link.context, reference: link.reference }] : []; });
    },
    async resolveNoteContact(reference) {
      const contacts = await sources.contacts?.() ?? [];
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
    async addMeetingToDailyNote(date, expected, entry) {
      const id = `daily:${date}`, note = notes.get(id);
      if ((note?.updatedAt ?? null) !== expected || note?.trashed) throw new Error("That day's note changed. Open it before adding this meeting.");
      const title = entry.linkNote ? meetingNoteTitle(entry.name) : null;
      const line = meetingLine(entry, title), previousBody = note?.body ?? "";
      if (document(note ?? { body: "" } as Note).links.some((link) => noteReferenceKey(link.reference) === noteReferenceKey(entry.event))) return { note: structuredClone(note!), added: false, previousBody, meetingNote: null, createdMeetingNote: false };
      const body = insertMeetingLine(previousBody, line);
      if (body === null) throw new Error("That day's note has a code block that never closes where the meeting would go. Open it before adding this meeting.");
      let meetingNote: Note | null = null, createdMeetingNote = false;
      if (title !== null) {
        meetingNote = [...notes.values()].find((other) => !other.trashed && !other.dailyDate && key(other.title) === key(title)) ?? null;
        if (!meetingNote) {
          const time = now();
          meetingNote = { id: `demo-note-${crypto.randomUUID()}`, title: noteTitle(title), body: "", dailyDate: null, createdAt: time, updatedAt: time, trashed: false, trashedAt: null };
          notes.set(meetingNote.id, meetingNote); createdMeetingNote = true;
        }
      }
      const next = note ? edit(note, note.title, body) : { id, title: date, body: normalize(id, body), dailyDate: date, createdAt: now(), updatedAt: now(), trashed: false, trashedAt: null };
      notes.set(id, next);
      return { note: structuredClone(next), added: true, previousBody, meetingNote: meetingNote && structuredClone(meetingNote), createdMeetingNote };
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
    async listDailyNoteDates(from, to) {
      if (!DATE.test(from) || !DATE.test(to)) throw new Error("That isn't a valid date");
      return [...notes.values()]
        .filter((note) => note.dailyDate && note.dailyDate >= from && note.dailyDate <= to && !note.trashed && note.body.trim())
        .map((note) => note.dailyDate!)
        .sort();
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
    async importNoteFile() {
      throw new Error("Importing a Markdown file needs the ThreeStrands app");
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
          .map((link) => ({ noteId: other.id, title: other.title, dailyDate: other.dailyDate, context: linkContext(link.context) })));
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
    async searchNotes(query, limit, scope = "active") {
      const words = collapseWhitespace(query.toLowerCase()).split(" ").filter(Boolean);
      if (!words.length) return [];
      return [...notes.values()]
        .filter((note) => (scope === "all" || note.trashed === (scope === "trash")) && words.every((word) => `${note.title} ${note.body}`.toLowerCase().includes(word)))
        .slice(0, limit)
        .map((note): NoteSearchHit => ({ id: note.id, title: note.title, dailyDate: note.dailyDate, matchSnippet: preview(note.body), trashed: note.trashed }));
    },
  };
}
