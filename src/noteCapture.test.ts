import { afterEach, describe, expect, it, vi } from "vitest";
import { addMeetingToDailyNote, captureConversation } from "./noteCapture";
import { mailClient } from "./data/client";
import { registerNoteOwner } from "./noteMutations";
import { writeNoteDraft } from "./noteDrafts";
import type { Note } from "./domain";
const note: Note = { id: "daily:2026-11-10", title: "2026-11-10", dailyDate: "2026-11-10", body: "Existing\n- [Conversation](reference)\n", createdAt: "created", updatedAt: "captured", trashed: false, trashedAt: null };
afterEach(() => { vi.restoreAllMocks(); localStorage.clear(); });
describe("conversation capture", () => {
  it("fixes the local date before async work and only undoes the captured version", async () => {
    const get = vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
    const capture = vi.spyOn(mailClient, "captureNoteConversation").mockResolvedValue({ note, added: true, previousBody: "Existing\n" });
    const save = vi.spyOn(mailClient, "saveDailyNote").mockResolvedValue({ saved: false, note: { ...note, updatedAt: "newer" } });
    const result = await captureConversation("conversation", new Date(2026, 10, 10, 23, 59, 59));
    expect(get).toHaveBeenCalledExactlyOnceWith("2026-11-10");
    expect(capture).toHaveBeenCalledExactlyOnceWith("2026-11-10", "conversation", null);
    await expect(result.undo()).rejects.toThrow("changed after capture");
    expect(save).toHaveBeenCalledExactlyOnceWith("2026-11-10", "Existing\n", "captured");
  });
  it("refuses to append behind a closed draft", async () => {
    writeNoteDraft(note.id, { title: note.title, body: "draft", base: null });
    const capture = vi.spyOn(mailClient, "captureNoteConversation");
    await expect(captureConversation("conversation", new Date(2026, 10, 10))).rejects.toThrow("saved draft");
    expect(capture).not.toHaveBeenCalled();
  });
  it("waits for the editor's save and refuses unresolved typing", async () => {
    const flush = vi.fn(async () => undefined);
    const stop = registerNoteOwner(note.id, { flush, unsaved: () => true, refresh: vi.fn(async () => undefined) });
    try { await expect(captureConversation("conversation", new Date(2026, 10, 10))).rejects.toThrow("unsaved changes"); expect(flush).toHaveBeenCalledOnce(); }
    finally { stop(); }
  });
});

describe("adding a meeting to its day's note", () => {
  const entry = { event: { version: 1 as const, kind: "event" as const, id: "evt", account: "you@example.com", calendar: "work" }, time: "9:30am", attendees: [], name: "Sync", linkNote: true };
  const day: Note = { ...note, id: "daily:2026-11-12", title: "2026-11-12", dailyDate: "2026-11-12", updatedAt: "added" };
  const meetingNote: Note = { ...note, id: "meeting", title: "Sync", dailyDate: null, body: "", updatedAt: "created" };
  const added = { note: day, added: true, previousBody: "Before\n", meetingNote, createdMeetingNote: true };

  it("writes to the event's own day and Undo removes the line and the untouched note it created", async () => {
    vi.spyOn(mailClient, "getDailyNote").mockResolvedValue({ ...day, updatedAt: "before" });
    const add = vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue(added);
    const save = vi.spyOn(mailClient, "saveDailyNote").mockResolvedValue({ saved: true, note: day });
    vi.spyOn(mailClient, "getNote").mockResolvedValue(meetingNote);
    const trash = vi.spyOn(mailClient, "trashNote").mockResolvedValue({ ...meetingNote, trashed: true });
    const result = await addMeetingToDailyNote("2026-11-12", entry);
    expect(add).toHaveBeenCalledExactlyOnceWith("2026-11-12", "before", entry);
    await result.undo();
    expect(save).toHaveBeenCalledExactlyOnceWith("2026-11-12", "Before\n", "added");
    expect(trash).toHaveBeenCalledExactlyOnceWith("meeting");
  });

  it("Undo keeps a meeting note someone wrote in, and keeps everything when the day changed", async () => {
    vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
    vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue(added);
    const save = vi.spyOn(mailClient, "saveDailyNote").mockResolvedValueOnce({ saved: true, note: day }).mockResolvedValueOnce({ saved: false, note: { ...day, updatedAt: "newer" } });
    vi.spyOn(mailClient, "getNote").mockResolvedValue({ ...meetingNote, body: "Agenda", updatedAt: "edited" });
    const trash = vi.spyOn(mailClient, "trashNote");
    await (await addMeetingToDailyNote("2026-11-12", entry)).undo();
    expect(trash).not.toHaveBeenCalled();
    await expect((await addMeetingToDailyNote("2026-11-12", entry)).undo()).rejects.toThrow("changed after the meeting was added");
    expect(save).toHaveBeenCalledTimes(2);
    expect(trash).not.toHaveBeenCalled();
  });

  it("does nothing to undo when the occurrence was already in the note", async () => {
    vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(day);
    vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue({ ...added, added: false, meetingNote: null, createdMeetingNote: false });
    const save = vi.spyOn(mailClient, "saveDailyNote");
    await (await addMeetingToDailyNote("2026-11-12", entry)).undo();
    expect(save).not.toHaveBeenCalled();
  });

  it("refuses to add behind a closed draft of that day", async () => {
    writeNoteDraft("daily:2026-11-12", { title: "2026-11-12", body: "draft", base: null });
    const add = vi.spyOn(mailClient, "addMeetingToDailyNote");
    await expect(addMeetingToDailyNote("2026-11-12", entry)).rejects.toThrow("saved draft");
    expect(add).not.toHaveBeenCalled();
  });
});

it("a guarded task mutation advances through its own editor flush, but not a stale card", async () => {
  const { changeNoteItem } = await import("./noteMutations");
  let version = "v1";
  const stop = registerNoteOwner(note.id, { version: () => version, flush: async () => { version = "v2"; }, unsaved: () => false, refresh: vi.fn(async () => undefined) });
  const patch = vi.spyOn(mailClient, "patchNoteItem").mockResolvedValue(note);
  try {
    await changeNoteItem(note.id, "marker", "v1", { checked: true });
    expect(patch).toHaveBeenLastCalledWith(note.id, "marker", "v2", true, undefined);
    await changeNoteItem(note.id, "marker", "stale", { task: false });
    expect(patch).toHaveBeenLastCalledWith(note.id, "marker", "stale", undefined, false);
  } finally { stop(); }
});
