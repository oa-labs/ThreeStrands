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
