import { afterEach, describe, expect, it, vi } from "vitest";
import { addMeetingToDailyNote, addToNote } from "./noteCapture";
import { mailClient } from "./data/client";
import { registerNoteOwner } from "./noteMutations";
import { writeNoteDraft } from "./noteDrafts";
import type { Note } from "./domain";
const note: Note = { id: "daily:2026-11-10", title: "2026-11-10", dailyDate: "2026-11-10", body: "Existing\n- [Conversation](reference)\n", createdAt: "created", updatedAt: "captured", trashed: false, trashedAt: null };
afterEach(() => { vi.restoreAllMocks(); localStorage.clear(); });
describe("Add to Note", () => {
  const request = { thread: "conversation", message: "message", quote: "a passage", dateLabel: "Oct 30", files: ["file-1"] };
  const addition = (extra: Partial<import("./noteEntities").NoteAddition> = {}): import("./noteEntities").NoteAddition => ({ note, added: true, previousBody: "Existing\n", createdNote: false, attachments: [{ id: "att", noteId: note.id, name: "a.png", mimeType: "image/png", size: 3, image: true, available: true }], ...extra });

  it("writes against the version it read after saving typing, and Undo hands back exactly what was written", async () => {
    const order: string[] = [];
    const flush = vi.fn(async () => { order.push("flush"); });
    const stop = registerNoteOwner(note.id, { flush, unsaved: () => false, refresh: vi.fn(async () => undefined) });
    vi.spyOn(mailClient, "getNote").mockImplementation(async () => { order.push("read"); return { ...note, updatedAt: "before" }; });
    const add = vi.spyOn(mailClient, "addToNote").mockImplementation(async () => { order.push("write"); return addition(); });
    const undoWrite = vi.spyOn(mailClient, "undoAddToNote").mockResolvedValue(note);
    try {
      const result = await addToNote({ choice: { kind: "note", id: note.id }, ...request });
      expect(order).toEqual(["flush", "read", "write"]);
      expect(add).toHaveBeenCalledExactlyOnceWith({ target: { kind: "note", id: note.id }, expected: "before", ...request });
      await result.undo();
      expect(undoWrite).toHaveBeenCalledExactlyOnceWith({ noteId: note.id, expected: "captured", previousBody: "Existing\n", createdNote: false, attachments: ["att"] });
    } finally { stop(); }
  });

  it("a day's note that doesn't exist yet is written against no version", async () => {
    vi.spyOn(mailClient, "getNote").mockResolvedValue(null);
    const add = vi.spyOn(mailClient, "addToNote").mockResolvedValue(addition());
    await addToNote({ choice: { kind: "note", id: "daily:2026-11-10" }, ...request });
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ expected: null }));
  });

  it("a link already in the note changes nothing and has nothing to undo", async () => {
    vi.spyOn(mailClient, "getNote").mockResolvedValue(note);
    vi.spyOn(mailClient, "addToNote").mockResolvedValue(addition({ added: false, attachments: [] }));
    const undoWrite = vi.spyOn(mailClient, "undoAddToNote");
    const changed = vi.fn();
    window.addEventListener("threestrands-notes-changed", changed);
    try {
      const result = await addToNote({ choice: { kind: "note", id: note.id }, ...request, quote: null, files: [] });
      expect(result.added).toBe(false);
      await result.undo();
      expect(undoWrite).not.toHaveBeenCalled();
      expect(changed).not.toHaveBeenCalled();
    } finally { window.removeEventListener("threestrands-notes-changed", changed); }
  });

  it("Undo refuses, and says so, when the note changed after the addition", async () => {
    vi.spyOn(mailClient, "getNote").mockResolvedValue(note);
    vi.spyOn(mailClient, "addToNote").mockResolvedValue(addition());
    vi.spyOn(mailClient, "undoAddToNote").mockRejectedValue(new Error("This note changed after it was added to."));
    await expect((await addToNote({ choice: { kind: "note", id: note.id }, ...request })).undo()).rejects.toThrow("changed after");
  });

  it("refuses to append behind a closed draft", async () => {
    writeNoteDraft(note.id, { title: note.title, body: "draft", base: null });
    const add = vi.spyOn(mailClient, "addToNote");
    await expect(addToNote({ choice: { kind: "note", id: note.id }, ...request })).rejects.toThrow("saved draft");
    expect(add).not.toHaveBeenCalled();
  });

  it("waits for the editor's save and refuses unresolved typing", async () => {
    const flush = vi.fn(async () => undefined);
    const stop = registerNoteOwner(note.id, { flush, unsaved: () => true, refresh: vi.fn(async () => undefined) });
    const add = vi.spyOn(mailClient, "addToNote");
    try {
      await expect(addToNote({ choice: { kind: "note", id: note.id }, ...request })).rejects.toThrow("unsaved changes");
      expect(flush).toHaveBeenCalledOnce();
      expect(add).not.toHaveBeenCalled();
    } finally { stop(); }
  });

  it("creates a new note natively against no version, and Undo says it was created", async () => {
    const add = vi.spyOn(mailClient, "addToNote").mockResolvedValue(addition({ createdNote: true, previousBody: "" }));
    const undoWrite = vi.spyOn(mailClient, "undoAddToNote").mockResolvedValue({ ...note, trashed: true });
    const result = await addToNote({ choice: { kind: "new", title: "Board prep" }, ...request });
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ target: { kind: "new", title: "Board prep" }, expected: null }));
    await result.undo();
    expect(undoWrite).toHaveBeenCalledWith(expect.objectContaining({ createdNote: true }));
  });

  describe("for a meeting without a note", () => {
    const entry = { event: { version: 1 as const, kind: "event" as const, id: "evt", account: "you@example.com", calendar: "work" }, time: "9:30am", attendees: [], name: "Sync", linkNote: false };
    const meetingNote: Note = { ...note, id: "meeting", title: "Sync", dailyDate: null, body: "", updatedAt: "created" };
    const day: Note = { ...note, id: "daily:2026-11-12", updatedAt: "day" };
    const meeting = { note: day, added: true, previousBody: "", meetingNote, createdMeetingNote: true };

    it("creates the note linked by the meeting, adds to it, and Undo takes back the addition, the line, and the empty note", async () => {
      const addMeeting = vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue(meeting);
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      const add = vi.spyOn(mailClient, "addToNote").mockResolvedValue(addition({ note: { ...meetingNote, body: "> q", updatedAt: "added" }, previousBody: "" }));
      vi.spyOn(mailClient, "undoAddToNote").mockResolvedValue({ ...meetingNote, updatedAt: "restored" });
      const save = vi.spyOn(mailClient, "saveDailyNote").mockResolvedValue({ saved: true, note: day });
      vi.spyOn(mailClient, "getNote").mockResolvedValue({ ...meetingNote, updatedAt: "restored" });
      const trash = vi.spyOn(mailClient, "trashNote").mockResolvedValue({ ...meetingNote, trashed: true });
      const result = await addToNote({ choice: { kind: "meeting", date: "2026-11-12", entry }, ...request });
      expect(addMeeting).toHaveBeenCalledExactlyOnceWith("2026-11-12", null, { ...entry, linkNote: true });
      expect(add).toHaveBeenCalledWith(expect.objectContaining({ target: { kind: "note", id: "meeting" }, expected: "created" }));
      await result.undo();
      expect(save).toHaveBeenCalledExactlyOnceWith("2026-11-12", "", "day");
      expect(trash).toHaveBeenCalledExactlyOnceWith("meeting");
    });

    it("takes the meeting's line back when the addition fails", async () => {
      vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue(meeting);
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      vi.spyOn(mailClient, "addToNote").mockRejectedValue(new Error("That passage is too long"));
      const save = vi.spyOn(mailClient, "saveDailyNote").mockResolvedValue({ saved: true, note: day });
      vi.spyOn(mailClient, "getNote").mockResolvedValue(meetingNote);
      vi.spyOn(mailClient, "trashNote").mockResolvedValue({ ...meetingNote, trashed: true });
      await expect(addToNote({ choice: { kind: "meeting", date: "2026-11-12", entry }, ...request })).rejects.toThrow("too long");
      expect(save).toHaveBeenCalledOnce();
    });

    it("reports both the addition failure and a rejected meeting note rollback", async () => {
      vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue(meeting);
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      vi.spyOn(mailClient, "addToNote").mockRejectedValue(new Error("That passage is too long"));
      const save = vi.spyOn(mailClient, "saveDailyNote").mockResolvedValue({ saved: false, note: day });
      const trash = vi.spyOn(mailClient, "trashNote");

      await expect(addToNote({ choice: { kind: "meeting", date: "2026-11-12", entry }, ...request })).rejects.toThrow(
        "Add to Note failed (That passage is too long). Could not fully undo the meeting note setup. That day's note changed after the meeting was added.",
      );
      expect(save).toHaveBeenCalledExactlyOnceWith("2026-11-12", "", "day");
      expect(trash).not.toHaveBeenCalled();
    });

    it("refuses when the meeting is already in its day without a note, writing nothing more", async () => {
      vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue({ ...meeting, added: false, meetingNote: null, createdMeetingNote: false });
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      const add = vi.spyOn(mailClient, "addToNote");
      await expect(addToNote({ choice: { kind: "meeting", date: "2026-11-12", entry }, ...request })).rejects.toThrow("already in its day");
      expect(add).not.toHaveBeenCalled();
    });
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

describe("opening a meeting note from the planning inbox", () => {
  const event = {
    id: "occurrence", accountId: "you@example.com", calendarId: "work", seriesId: "series", title: "Planning sync", allDay: false,
    start: new Date(2026, 10, 12, 9).toISOString(), end: new Date(2026, 10, 12, 10).toISOString(), attendees: ["you@example.com", "pat@example.org"],
  };

  it("rechecks occurrence backlinks and opens an existing dedicated note without writing", async () => {
    const { planningMeetingNote } = await import("./noteCapture");
    const backlinks = vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([
      { noteId: "daily:2026-11-12", title: "Daily", dailyDate: "2026-11-12", context: "" },
      { noteId: "meeting", title: "Planning sync", dailyDate: null, context: "" },
    ]);
    const add = vi.spyOn(mailClient, "addMeetingToDailyNote");
    const result = await planningMeetingNote(event, [event.accountId]);
    expect(result.noteId).toBe("meeting");
    expect(backlinks).toHaveBeenCalledExactlyOnceWith({ version: 1, kind: "event", id: "occurrence", account: "you@example.com", calendar: "work", series: "series" });
    expect(result.href).toContain("occurrence");
    expect(add).not.toHaveBeenCalled();
  });

  it("shares repeated activations and writes an occurrence entry on its own day with the current revision", async () => {
    const { planningMeetingNote } = await import("./noteCapture");
    vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([]);
    vi.spyOn(mailClient, "getDailyNote").mockResolvedValue({ ...note, updatedAt: "current" });
    vi.spyOn(mailClient, "resolveContactIds").mockResolvedValue({ "pat@example.org": "pat" });
    const add = vi.spyOn(mailClient, "addMeetingToDailyNote").mockResolvedValue({ note: { ...note, id: "daily:2026-11-12" }, added: true, previousBody: "Before", meetingNote: null, createdMeetingNote: false });
    const first = planningMeetingNote(event, [event.accountId]);
    expect(planningMeetingNote(event, [event.accountId])).toBe(first);
    expect((await first).noteId).toBe("daily:2026-11-12");
    expect(add).toHaveBeenCalledExactlyOnceWith("2026-11-12", "current", expect.objectContaining({
      event: expect.objectContaining({ id: "occurrence", series: "series" }), name: "Planning sync", linkNote: false,
      attendees: [{ label: "pat@example.org", reference: { version: 1, kind: "contact", id: "pat@example.org", contact: "pat" } }],
    }));
  });

  it("does not write when backlinks are unavailable and allows retrying after the failure", async () => {
    const { planningMeetingNote } = await import("./noteCapture");
    const backlinks = vi.spyOn(mailClient, "noteBacklinks").mockRejectedValueOnce(new Error("Unavailable"));
    const add = vi.spyOn(mailClient, "addMeetingToDailyNote");
    await expect(planningMeetingNote(event, [])).rejects.toThrow("Unavailable");
    expect(add).not.toHaveBeenCalled();
    backlinks.mockResolvedValue([{ noteId: "recovered", title: "Recovered", dailyDate: null, context: "" }]);
    expect((await planningMeetingNote(event, [])).noteId).toBe("recovered");
  });
});
