import type { MeetingCommitment, MeetingContext, MeetingConversation, MeetingLimits, MeetingNoteSource, MeetingOccurrence, MeetingReadiness, MeetingSection, PrepReservation, ScheduleEvent, Thread, ThreadTask, Note } from "../domain";
import { boundedText, insertPrepTask, meetingBlocks, occurrenceBlocks, PREP_TASK_PREFIX, type MeetingBlock } from "../meetingExcerpt";
import { insertMeetingLine, meetingLine } from "../noteMeetings";
import { MAX_READINESS_EVENTS } from "../meetingPrep";
import { extractNoteEntityLinks, normalizeContactAddress, noteReferenceKey, type NoteReference } from "../noteEntities";
import { extractWikiLinks } from "../noteWikiLinks";
import type { MailClient } from "./client";

/** The native limits (`src-tauri/src/meeting_policy.rs`), so the demo bounds results the same way. */
export const DEMO_MEETING_LIMITS: MeetingLimits = { noteSources: 20, priorOccurrences: 8, excerptBytes: 4096, linkedNotePreviewBytes: 1024, attendees: 50, conversations: 10, mailLookbackDays: 90, commitments: 25 };

type Sources = {
  events(): ScheduleEvent[];
  threads(): Thread[];
  tasks(): ThreadTask[];
  mailAccounts(): string[];
  ownAddresses(): string[];
  threadForMessage(id: string): string | null;
  now?(): Date;
  notes: Pick<MailClient, "noteBacklinks" | "getNote" | "noteDocument" | "findNoteByTitle" | "getDailyNote" | "saveDailyNote">;
};

function bounded<T>(items: T[], limit: number): MeetingSection<T> {
  const omitted = Math.max(0, items.length - limit);
  return { status: omitted ? "limited" : "ready", items: items.slice(0, limit), omitted, message: null };
}
const unavailable = <T>(message: string): MeetingSection<T> => ({ status: "unavailable", items: [], omitted: 0, message });

/** As natively (`GENERIC_TITLE_WORDS`): words too common to say two titles share a topic. */
const GENERIC_TITLE_WORDS = new Set(["about", "accepted", "call", "catch", "chat", "check", "daily", "declined", "from", "invitation", "meeting", "meetings", "monthly", "review", "standup", "sync", "team", "tentative", "update", "updated", "updates", "weekly", "with"]);
const titleWords = (text: string) => new Set(text.split(/[^\p{L}\p{N}]+/u).map((word) => word.replace(/[A-Z]/g, (ch) => ch.toLowerCase())).filter((word) => [...word].length >= 4 && !GENERIC_TITLE_WORDS.has(word)));

const instant = (start: string) => /^\d{4}-\d{2}-\d{2}$/.test(start) ? new Date(`${start}T00:00:00`).getTime() : Date.parse(start);
const localDate = (time: number) => { const d = new Date(time); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; };

/** Meeting preparation for the browser demo: the native rules over in-memory data. */
export function createDemoMeetings(sources: Sources): Pick<MailClient, "meetingContext" | "meetingReadiness" | "reserveMeetingPrep"> {
  const cached = (reference: NoteReference) => sources.events().find((event) => event.accountId === reference.account && event.calendarId === reference.calendar && event.id === reference.id) ?? null;
  const notesLinking = async (reference: NoteReference) => {
    const notes: Note[] = [];
    for (const backlink of await sources.notes.noteBacklinks(reference)) {
      const note = await sources.notes.getNote(backlink.noteId);
      if (note && !note.trashed) notes.push(note);
    }
    return notes;
  };
  const toSources = (note: Note, blocks: MeetingBlock[]): MeetingNoteSource[] => blocks.map((block) => {
    const { text, truncated } = boundedText(note.body.slice(block.start, block.end), DEMO_MEETING_LIMITS.excerptBytes);
    return { noteId: note.id, title: note.title, dailyDate: note.dailyDate, revision: note.updatedAt, kind: block.kind, text, truncated, start: block.start, end: block.end };
  });

  async function context(reference: NoteReference, mailScope: string[] | null = null): Promise<MeetingContext> {
    if (reference.kind !== "event" || !reference.account || !reference.calendar) throw new Error("Invalid event reference");
    const now = sources.now?.() ?? new Date();
    const event = cached(reference);
    const key = noteReferenceKey(reference);

    const excerpts: { note: Note; block: MeetingBlock }[] = [];
    const thisSources: MeetingNoteSource[] = [];
    const seen = new Set<string>();
    for (const note of await notesLinking(reference)) {
      const blocks = occurrenceBlocks(note.body, reference);
      if (!blocks.length) continue;
      seen.add(note.id);
      thisSources.push(...toSources(note, blocks));
      for (const block of blocks) if (block.kind === "excerpt") excerpts.push({ note, block });
    }
    for (const { note, block } of excerpts) for (const link of extractWikiLinks(note.body.slice(block.start, block.end))) {
      const linked = await sources.notes.findNoteByTitle(link.title);
      const full = linked && !linked.dailyDate ? await sources.notes.getNote(linked.id) : null;
      if (!full || full.trashed || seen.has(full.id)) continue;
      seen.add(full.id);
      const { text, truncated } = boundedText(full.body, DEMO_MEETING_LIMITS.linkedNotePreviewBytes);
      thisSources.push({ noteId: full.id, title: full.title, dailyDate: null, revision: full.updatedAt, kind: "linkedNote", text, truncated, start: 0, end: text.length });
    }

    const seriesId = (event?.seriesId ?? reference.series ?? null);
    const series = seriesId && seriesId !== reference.id ? { ...reference, id: seriesId, series: undefined, contact: undefined } : null;
    const seriesKey = series ? noteReferenceKey(series) : null;
    const seriesSources: MeetingNoteSource[] = [];
    const occurrences = new Map<string, { reference: NoteReference; date: string | null }>();
    if (series) for (const note of await notesLinking(series)) {
      seriesSources.push(...toSources(note, meetingBlocks(note.body, (link) => link.kind === "event" && noteReferenceKey(link) === seriesKey)));
      for (const link of extractNoteEntityLinks(note.body).links) {
        const other = link.reference;
        if (other.kind !== "event" || other.series !== seriesId || noteReferenceKey(other) === key || other.account !== reference.account || other.calendar !== reference.calendar) continue;
        const known = occurrences.get(noteReferenceKey(other));
        const date = [known?.date, note.dailyDate].filter((d): d is string => Boolean(d)).sort()[0] ?? null;
        occurrences.set(noteReferenceKey(other), { reference: other, date });
      }
    }
    let previousMeetings: MeetingSection<MeetingOccurrence>;
    if (!series) previousMeetings = bounded([], 0);
    else if (!event) previousMeetings = unavailable("Earlier meetings can't be ordered until this meeting's time is on this device.");
    else {
      const thisStart = instant(event.start), thisDate = localDate(thisStart);
      const prior: { order: number; key: string; reference: NoteReference; start: string | null; date: string | null }[] = [];
      for (const [occurrenceKey, { reference: other, date }] of occurrences) {
        const start = cached(other)?.start ?? null;
        const order = start ? instant(start) : date && date < thisDate ? new Date(`${date}T00:00:00`).getTime() : NaN;
        if (!Number.isFinite(order) || order >= thisStart) continue;
        prior.push({ order, key: occurrenceKey, reference: other, start, date });
      }
      prior.sort((a, b) => b.order - a.order || a.key.localeCompare(b.key));
      const items: MeetingOccurrence[] = [];
      for (const item of prior.slice(0, DEMO_MEETING_LIMITS.priorOccurrences)) {
        const occurrenceSources: MeetingNoteSource[] = [];
        for (const note of await notesLinking(item.reference)) occurrenceSources.push(...toSources(note, occurrenceBlocks(note.body, item.reference)));
        items.push({ reference: item.reference, start: item.start, date: item.date, sources: occurrenceSources.slice(0, DEMO_MEETING_LIMITS.noteSources) });
      }
      const omitted = Math.max(0, prior.length - DEMO_MEETING_LIMITS.priorOccurrences);
      previousMeetings = { status: omitted ? "limited" : "ready", items, omitted, message: null };
    }

    const known = new Set(sources.mailAccounts());
    if (mailScope?.some((account) => !known.has(account))) throw new Error("That mail account isn't on this device");
    const scope = mailScope === null ? (known.has(reference.account) ? [reference.account] : []) : mailScope;
    const own = new Set(sources.ownAddresses().map(normalizeContactAddress));
    const attendees = [...new Set((event?.attendees ?? []).map(normalizeContactAddress))].filter((address) => address.includes("@") && !own.has(address));
    const lookup = attendees.slice(0, DEMO_MEETING_LIMITS.attendees);

    let conversations: MeetingSection<MeetingConversation>;
    if (!event) conversations = unavailable("Related mail needs this meeting's attendees, which aren't on this device.");
    else {
      const items: MeetingConversation[] = [];
      const originThread = event.sourceMessageId ? sources.threadForMessage(event.sourceMessageId) : null;
      const origin = originThread ? sources.threads().find((thread) => thread.id === originThread && known.has(thread.accountId) && !thread.trashed) : undefined;
      if (origin) items.push({ threadId: origin.id, accountId: origin.accountId, subject: origin.subject, lastMessageAt: origin.lastMessageAt, reason: "origin", attendees: [], related: true });
      if (!scope.length) conversations = { ...bounded(items, DEMO_MEETING_LIMITS.conversations), status: "scopeRequired", message: "Choose which mail accounts to look in for related mail." };
      else {
        const since = now.getTime() - DEMO_MEETING_LIMITS.mailLookbackDays * 86_400_000;
        const recent = sources.threads()
          .filter((thread) => scope.includes(thread.accountId) && !thread.trashed && Date.parse(thread.lastMessageAt) >= since)
          .sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt) || a.id.localeCompare(b.id));
        // As natively: conversations naming the meeting first, then related
        // attendee mail, then the rest, newest first within each.
        const title = event.title.split(/\s+/).filter(Boolean).join(" ").toLowerCase();
        if ([...title].length >= 4) for (const thread of recent) {
          if (thread.subject.toLowerCase().includes(title) && !items.some((item) => item.threadId === thread.id)) items.push({ threadId: thread.id, accountId: thread.accountId, subject: thread.subject, lastMessageAt: thread.lastMessageAt, reason: "title", attendees: [], related: true });
        }
        const topic = titleWords(event.title);
        const matches = recent
          .filter((thread) => !items.some((item) => item.threadId === thread.id))
          .map((thread) => {
            const matched = lookup.filter((address) => thread.participants.some((participant) => normalizeContactAddress(participant).includes(address))).sort();
            return { thread, matched, related: matched.length === lookup.length || [...titleWords(thread.subject)].some((word) => topic.has(word)) };
          })
          .filter(({ matched }) => matched.length)
          .sort((a, b) => Number(!a.related) - Number(!b.related));
        for (const { thread, matched, related } of matches) items.push({ threadId: thread.id, accountId: thread.accountId, subject: thread.subject, lastMessageAt: thread.lastMessageAt, reason: "attendees", attendees: matched, related });
        conversations = bounded(items, DEMO_MEETING_LIMITS.conversations);
      }
    }

    const commitments: MeetingCommitment[] = [];
    const active = (task: ThreadTask) => task.status === "open" || task.status === "in_progress";
    for (const { note, block } of excerpts) {
      const inside = (at: number) => block.start <= at && at < block.end;
      const document = await sources.notes.noteDocument(note.id);
      for (const item of document.items) if (item.task && !item.checked && inside(item.start)) commitments.push({ id: `note:${note.id}:${item.id}`, title: item.title, status: "open", noteId: note.id, threadId: null, accountId: null, origin: "meetingNote", candidate: false });
      for (const link of document.links) if (link.reference.kind === "task" && inside(link.start) && !link.reference.note) {
        const task = sources.tasks().find((candidate) => candidate.id === link.reference.id && active(candidate));
        if (task) commitments.push({ id: task.id, title: task.title, status: task.status, noteId: null, threadId: task.threadId, accountId: task.accountId, origin: "meetingNote", candidate: false });
      }
    }
    for (const conversation of conversations.items) for (const task of sources.tasks().filter((task) => task.threadId === conversation.threadId && active(task))) {
      const candidate = conversation.reason === "attendees";
      commitments.push({ id: task.id, title: task.title, status: task.status, noteId: null, threadId: task.threadId, accountId: task.accountId, origin: candidate ? "attendeeConversation" : conversation.reason === "title" ? "titleConversation" : "originConversation", candidate });
    }
    const unique = commitments.filter((item, index) => commitments.findIndex((other) => other.id === item.id) === index);
    unique.sort((a, b) => Number(a.candidate) - Number(b.candidate));

    return {
      reference,
      event: event ? { title: event.title, start: event.start, end: event.end, allDay: event.allDay, seriesId: series ? seriesId : null, attendees, attendeesOmitted: Math.max(0, attendees.length - DEMO_MEETING_LIMITS.attendees), sourceMessageId: event.sourceMessageId ?? null, responseStatus: event.responseStatus ?? null } : null,
      mailScope: scope,
      thisMeeting: bounded(thisSources, DEMO_MEETING_LIMITS.noteSources),
      previousMeetings,
      seriesNotes: bounded(seriesSources, DEMO_MEETING_LIMITS.noteSources),
      conversations,
      commitments: { ...bounded(unique, DEMO_MEETING_LIMITS.commitments), message: event ? null : "Only commitments in this meeting's notes are shown." },
      limits: DEMO_MEETING_LIMITS,
    };
  }

  const prepTaskId = async (note: Note, occurrence: NoteReference) => {
    const block = occurrenceBlocks(note.body, occurrence).find((candidate) => candidate.kind === "excerpt");
    if (!block) return null;
    const prefix = PREP_TASK_PREFIX.trimEnd().toLowerCase();
    const item = (await sources.notes.noteDocument(note.id)).items.find((item) => item.task && block.start < item.start && item.start < block.end && item.title.toLowerCase().startsWith(prefix));
    return item ? `note:${note.id}:${item.id}` : null;
  };

  return {
    async reserveMeetingPrep(date, expected, entry): Promise<PrepReservation> {
      const note = await sources.notes.getDailyNote(date);
      if ((note?.updatedAt ?? null) !== expected || note?.trashed) throw new Error("That day's note changed. Open it before reserving prep time.");
      const previousBody = note?.body ?? "";
      let body = previousBody, addedMeetingLine = false;
      let result = insertPrepTask(body, entry.event, entry.name);
      if (result === "noMeetingLine") {
        if (extractNoteEntityLinks(body).links.some((link) => noteReferenceKey(link.reference) === noteReferenceKey(entry.event))) throw new Error("This meeting is mentioned in that day's note, but not as a meeting line. Open the note to add prep time.");
        const withLine = insertMeetingLine(body, meetingLine({ ...entry, linkNote: false }, null));
        if (withLine === null) throw new Error("That day's note has a code block that never closes where the meeting would go. Open it before reserving prep time.");
        body = withLine;
        addedMeetingLine = true;
        result = insertPrepTask(body, entry.event, entry.name);
      }
      if (result === "existing") return { note: note!, added: false, addedMeetingLine: false, previousBody, taskId: await prepTaskId(note!, entry.event) };
      if (result === "noMeetingLine") throw new Error("The meeting's line couldn't be found after adding it");
      const saved = await sources.notes.saveDailyNote(date, result, expected);
      if (!saved.saved || !saved.note) throw new Error("That day's note changed. Open it before reserving prep time.");
      return { note: saved.note, added: true, addedMeetingLine, previousBody, taskId: await prepTaskId(saved.note, entry.event) };
    },
    meetingContext: (reference, mailScope = null) => context(reference, mailScope),
    async meetingReadiness(references, mailScope = null) {
      if (references.length > MAX_READINESS_EVENTS) throw new Error(`At most ${MAX_READINESS_EVENTS} meetings can be checked at once`);
      return Promise.all(references.map(async (reference): Promise<MeetingReadiness> => {
        const full = await context(reference, mailScope);
        const sections = { thisMeeting: full.thisMeeting, previousMeetings: full.previousMeetings, seriesNotes: full.seriesNotes, conversations: full.conversations, commitments: full.commitments };
        return {
          reference,
          eventAvailable: full.event !== null,
          hasOccurrenceNote: full.thisMeeting.items.some((source) => source.kind !== "reference"),
          priorOccurrencesWithNotes: full.previousMeetings.items.filter((item) => item.sources.length).length + full.previousMeetings.omitted,
          seriesNotes: full.seriesNotes.items.length + full.seriesNotes.omitted,
          openCommitments: full.commitments.items.filter((item) => !item.candidate).length,
          candidateCommitments: full.commitments.items.filter((item) => item.candidate).length,
          latestCorrespondenceAt: full.conversations.items.map((item) => item.lastMessageAt).sort().at(-1) ?? null,
          scopeRequired: full.conversations.status === "scopeRequired",
          unavailableSections: Object.entries(sections).filter(([, section]) => section.status === "unavailable").map(([name]) => name),
          error: null,
        };
      }));
    },
  };
}
