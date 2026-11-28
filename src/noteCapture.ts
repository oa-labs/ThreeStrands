import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { notesChanged, prepareNoteMutation } from "./noteMutations";
import { noteTargetKey } from "./noteDrafts";
import type { AddToNoteTarget } from "./noteEntities";
import type { ScheduleEvent } from "./domain";
import { meetingDate, meetingEventReference, primaryMeetingNote } from "./meetingNoteEntry";
import { noteReferenceHref } from "./noteEntities";
import type { MeetingEntry } from "./noteMeetings";
import { defaultMeetingEntry } from "./meetingPrep";

/** Where the user chose to add: a note (a daily note is `daily:<date>` and may not exist yet), a new note named by the user, or a note to create for a meeting. */
export type AddToNoteChoice =
  | { kind: "note"; id: string }
  | { kind: "new"; title: string }
  | { kind: "meeting"; date: string; entry: MeetingEntry };

/**
 * Add to Note: one guarded native write of a passage (or a link to the
 * conversation) and copies of the chosen files, after saving any open typing
 * in the note. A meeting without a note first gets one, as Add Note's
 * "Create backlinked note" does; if the addition fails, try taking back
 * the meeting note setup. Undo restores the note while nothing changed it
 * since, deletes the files this action stored, and moves a note it created to Trash.
 */
export async function addToNote(input: { choice: AddToNoteChoice; thread: string; message: string; quote: string | null; dateLabel: string; files: string[] }) {
  const { choice } = input;
  let meeting: Awaited<ReturnType<typeof addMeetingToDailyNote>> | null = null;
  try {
    let target: AddToNoteTarget, expected: string | null = null;
    if (choice.kind === "meeting") {
      meeting = await addMeetingToDailyNote(choice.date, { ...choice.entry, linkNote: true });
      if (!meeting.meetingNote) throw new Error("That meeting is already in its day's note. Open the day's note to give it a note of its own.");
      target = { kind: "note", id: meeting.meetingNote.id };
      expected = meeting.meetingNote.updatedAt;
    } else target = choice.kind === "new" ? { kind: "new", title: choice.title } : { kind: "note", id: choice.id };
    if (target.kind === "note") {
      await prepareNoteMutation(target.id);
      if (choice.kind === "note") expected = (await mailClient.getNote(target.id))?.updatedAt ?? null;
    }
    const added = await mailClient.addToNote({ target, expected, thread: input.thread, message: input.message, quote: input.quote, dateLabel: input.dateLabel, files: input.files });
    if (added.added) await notesChanged(added.note.id);
    const parent = meeting;
    return { ...added, async undo() {
      if (added.added) {
        await prepareNoteMutation(added.note.id);
        const restored = await mailClient.undoAddToNote({ noteId: added.note.id, expected: added.note.updatedAt, previousBody: added.previousBody, createdNote: added.createdNote, attachments: added.attachments.map((file) => file.id) });
        await notesChanged(added.note.id);
        await parent?.undo({ meetingNoteVersion: restored.updatedAt });
      }
    } };
  } catch (reason) {
    if (meeting?.added) {
      try {
        await meeting.undo();
      } catch (rollbackFailure) {
        throw new AggregateError([reason, rollbackFailure],
          `Add to Note failed (${errorMessage(reason)}). Could not fully undo the meeting note setup. ${errorMessage(rollbackFailure)}`,
          { cause: rollbackFailure });
      }
    }
    throw reason;
  }
}

/** Writes a meeting's line into `date`'s daily note (the event's own day,
 * fixed by the caller), after saving any open typing there. Undo restores
 * that note if nothing changed it since, and moves a meeting note this
 * action created to Trash if it is still empty and unchanged. */
export async function addMeetingToDailyNote(date: string, entry: MeetingEntry) {
  const id = noteTargetKey({ kind: "daily", date });
  await prepareNoteMutation(id);
  const current = await mailClient.getDailyNote(date);
  const captured = await mailClient.addMeetingToDailyNote(date, current?.updatedAt ?? null, entry);
  if (captured.added) {
    await notesChanged(id);
    if (captured.createdMeetingNote && captured.meetingNote) await notesChanged(captured.meetingNote.id);
  }
  return { ...captured, async undo(options?: { meetingNoteVersion?: string }) {
    if (!captured.added) return;
    await prepareNoteMutation(id);
    const result = await mailClient.saveDailyNote(date, captured.previousBody, captured.note.updatedAt);
    if (!result.saved) throw new Error("That day's note changed after the meeting was added. Open Note to remove it.");
    await notesChanged(id);
    const created = captured.createdMeetingNote ? captured.meetingNote : null;
    if (!created) return;
    const now = await mailClient.getNote(created.id);
    // `meetingNoteVersion` is the version of the note after something this action wrote there was taken back.
    if (now && !now.trashed && now.body === "" && now.updatedAt === (options?.meetingNoteVersion ?? created.updatedAt)) {
      await mailClient.trashNote(created.id);
      await notesChanged(created.id);
    }
  } };
}

/** Reserve Prep Time: a prep task under the meeting's line in `date`'s daily
 * note, adding the line when the note has none, after saving any open typing
 * there. Undo restores that note if nothing changed it since. */
export async function reserveMeetingPrep(date: string, entry: MeetingEntry) {
  const id = noteTargetKey({ kind: "daily", date });
  await prepareNoteMutation(id);
  const current = await mailClient.getDailyNote(date);
  const reserved = await mailClient.reserveMeetingPrep(date, current?.updatedAt ?? null, entry);
  if (reserved.added) await notesChanged(id);
  return { ...reserved, async undo() {
    if (!reserved.added) return;
    await prepareNoteMutation(id);
    const result = await mailClient.saveDailyNote(date, reserved.previousBody, reserved.note.updatedAt);
    if (!result.saved) throw new Error("That day's note changed after prep time was reserved. Open Note to remove it.");
    await notesChanged(id);
  } };
}

/** Adds items to `date`'s plan after saving any open typing in that day's
 * note. Undo restores the note if nothing changed it since. */
export async function addToDailyPlan(date: string, additions: import("./dailyPlan").PlanAddition[]) {
  const id = noteTargetKey({ kind: "daily", date });
  await prepareNoteMutation(id);
  const current = await mailClient.getDailyNote(date);
  const saved = await mailClient.addToDailyPlan(date, current?.updatedAt ?? null, additions);
  if (saved.changed.length) await notesChanged(id);
  return { ...saved, async undo() {
    if (!saved.changed.length) return;
    await prepareNoteMutation(id);
    const result = await mailClient.saveDailyNote(date, saved.previousBody, saved.note.updatedAt);
    if (!result.saved) throw new Error("That day's note changed after the plan was updated. Open Note to change it.");
    await notesChanged(id);
  } };
}

/** Takes one item off `date`'s plan, after saving any open typing there. */
export async function removeFromDailyPlan(date: string, key: string) {
  const id = noteTargetKey({ kind: "daily", date });
  await prepareNoteMutation(id);
  const current = await mailClient.getDailyNote(date);
  const saved = await mailClient.removeFromDailyPlan(date, current?.updatedAt ?? null, key);
  await notesChanged(id);
  return { ...saved, async undo() {
    await prepareNoteMutation(id);
    const result = await mailClient.saveDailyNote(date, saved.previousBody, saved.note.updatedAt);
    if (!result.saved) throw new Error("That day's note changed after the plan was updated. Open Note to change it.");
    await notesChanged(id);
  } };
}

/**
 * Plan Today's save: every change in one guarded write to `date`'s note,
 * after saving any open typing there. The routine read the plan at `base`;
 * when that day's note has moved on only in ways the changes don't depend on
 * (every item they name is still in the plan, none of it free text), the save
 * goes ahead on the current version, so writes the routine made itself, such
 * as Reserve Prep Time, don't refuse it. Otherwise it refuses as stale. Undo
 * restores the body the save replaced while the note is as it left it.
 */
export async function saveDailyPlan(date: string, base: string | null, operationId: string, changes: import("./domain").PlanChanges) {
  const id = noteTargetKey({ kind: "daily", date });
  await prepareNoteMutation(id);
  let expected = base;
  const current = await mailClient.dailyPlan(date);
  if (current.revision !== base) {
    const keys = new Set(current.items.map((item) => item.key));
    const named = [...changes.items.flatMap((entry) => entry.kind === "existing" ? [entry.key] : []), ...changes.remove];
    if (named.every((key) => keys.has(key) && !key.startsWith("line:"))) expected = current.revision;
  }
  const saved = await mailClient.saveDailyPlan(date, expected, operationId, changes);
  if (saved.changed) await notesChanged(id);
  return { ...saved, async undo() {
    if (!saved.changed) return;
    await prepareNoteMutation(id);
    await mailClient.undoDailyPlanSave(date, operationId);
    await notesChanged(id);
  } };
}

/** Review Follow-through: appends the accepted items to `destination` after
 * saving any open typing there. Only a version advance caused by that
 * editor's own flush is taken as current; anything else is checked natively
 * against the version the review started from. Undo removes exactly that
 * section while the note is unchanged since. */
export async function saveFollowThrough(
  reference: import("./noteEntities").NoteReference,
  destination: { noteId: string; dailyDate: string | null; revision: string },
  operationId: string,
  accepted: import("./domain").AcceptedFollowThrough,
) {
  const key = destination.dailyDate ? noteTargetKey({ kind: "daily", date: destination.dailyDate }) : destination.noteId;
  const prepared = await prepareNoteMutation(key);
  const expected = prepared.before === destination.revision ? prepared.after ?? destination.revision : destination.revision;
  const saved = await mailClient.saveMeetingFollowThrough(reference, { noteId: destination.noteId }, expected, operationId, accepted);
  await notesChanged(key);
  return { ...saved, async undo() {
    await prepareNoteMutation(key);
    await mailClient.undoMeetingFollowThrough(operationId);
    await notesChanged(key);
  } };
}

/** Saves an ad-hoc recording's follow-through section under `## Meetings` in `date`'s daily note (or in a dedicated backlinked meeting note). */
export async function saveAdhocFollowThrough(
  date: string,
  timeLabel: string,
  expectedRevision: string | null,
  operationId: string,
  accepted: import("./domain").AcceptedFollowThrough,
  linkNote = false,
) {
  const dailyKey = noteTargetKey({ kind: "daily", date });
  const prepared = await prepareNoteMutation(dailyKey);
  const expected = prepared.before === expectedRevision ? prepared.after ?? expectedRevision : expectedRevision;
  const saved = await mailClient.saveAdhocFollowThrough(date, timeLabel, expected, operationId, accepted, linkNote);
  await notesChanged(dailyKey);
  if (saved.note.id !== dailyKey) await notesChanged(saved.note.id);
  return { ...saved, async undo() {
    await prepareNoteMutation(dailyKey);
    if (saved.note.id !== dailyKey) await prepareNoteMutation(saved.note.id);
    await mailClient.undoMeetingFollowThrough(operationId);
    await notesChanged(dailyKey);
    if (saved.note.id !== dailyKey) await notesChanged(saved.note.id);
  } };
}

/** Adds accepted agenda bullets to an existing meeting note or the meeting's
 * daily note after flushing any open typing there. Undo removes exactly the
 * inserted text while the note is unchanged since. */
export async function saveMeetingAgenda(
  event: ScheduleEvent,
  ownAddresses: readonly string[],
  destination: { noteId: string; dailyDate: string | null; revision: string } | null,
  operationId: string,
  items: readonly string[],
) {
  const reference = meetingEventReference(event);
  if (!reference) throw new Error("This meeting can't be added to a note.");
  const date = meetingDate(event);
  const key = destination
    ? (destination.dailyDate ? noteTargetKey({ kind: "daily", date: destination.dailyDate }) : destination.noteId)
    : noteTargetKey({ kind: "daily", date });
  const prepared = await prepareNoteMutation(key);
  let expected: string | null;
  if (destination) {
    expected = prepared.before === destination.revision ? prepared.after ?? destination.revision : destination.revision;
  } else {
    expected = (await mailClient.getDailyNote(date))?.updatedAt ?? null;
  }
  const entry = await defaultMeetingEntry(event, ownAddresses);
  const saved = await mailClient.saveMeetingAgenda(reference, destination?.noteId ?? null, date, expected, operationId, entry, [...items]);
  await notesChanged(key);
  return { ...saved, async undo() {
    await prepareNoteMutation(key);
    await mailClient.undoMeetingAgenda(operationId);
    await notesChanged(key);
  } };
}

/** Open an occurrence's existing note, or add its entry to its day's note.
 * Concurrent activations share the guarded write; a failed lookup never creates a duplicate. */
const openingMeetings = new Map<string, Promise<{ noteId: string; href: string; undo?: () => Promise<void> }>>();
export function planningMeetingNote(event: ScheduleEvent, ownAddresses: readonly string[]) {
  const reference = meetingEventReference(event);
  if (!reference) return Promise.reject(new Error("This event can't be added to a note"));
  const href = noteReferenceHref(reference);
  const pending = openingMeetings.get(href);
  if (pending) return pending;
  const opening = (async () => {
    const existing = primaryMeetingNote(await mailClient.noteBacklinks(reference));
    if (existing) return { noteId: existing.noteId, href };
    const captured = await addMeetingToDailyNote(meetingDate(event), await defaultMeetingEntry(event, ownAddresses));
    return { noteId: captured.note.id, href, ...(captured.added ? { undo: captured.undo } : {}) };
  })();
  openingMeetings.set(href, opening);
  void opening.finally(() => { openingMeetings.delete(href); }).catch(() => {});
  return opening;
}
