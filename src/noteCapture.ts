import { mailClient } from "./data/client";
import { notesChanged, prepareNoteMutation } from "./noteMutations";
import { noteTargetKey, todayTarget } from "./noteDrafts";

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
