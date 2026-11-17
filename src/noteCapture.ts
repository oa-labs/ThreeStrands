import { mailClient } from "./data/client";
import { notesChanged, prepareNoteMutation } from "./noteMutations";
import { noteTargetKey, todayTarget } from "./noteDrafts";
import type { MeetingEntry } from "./noteMeetings";

/** Local date is captured once, before asynchronous work. */
export async function captureConversation(threadId: string, now = new Date()) {
  const target = todayTarget(now), id = noteTargetKey(target);
  await prepareNoteMutation(id);
  const current = await mailClient.getDailyNote(target.date);
  const captured = await mailClient.captureNoteConversation(target.date, threadId, current?.updatedAt ?? null);
  await notesChanged(id);
  return { ...captured, async undo() {
    await prepareNoteMutation(id);
    const result = await mailClient.saveDailyNote(target.date, captured.previousBody, captured.note.updatedAt);
    if (!result.saved) throw new Error("Today's note changed after capture. Open Note to remove this reference.");
    await notesChanged(id);
  } };
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
  return { ...captured, async undo() {
    if (!captured.added) return;
    await prepareNoteMutation(id);
    const result = await mailClient.saveDailyNote(date, captured.previousBody, captured.note.updatedAt);
    if (!result.saved) throw new Error("That day's note changed after the meeting was added. Open Note to remove it.");
    await notesChanged(id);
    const created = captured.createdMeetingNote ? captured.meetingNote : null;
    if (!created) return;
    const now = await mailClient.getNote(created.id);
    if (now && !now.trashed && now.body === "" && now.updatedAt === created.updatedAt) {
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
