import { afterEach, describe, expect, it, vi } from "vitest";
import { captureConversation } from "./noteCapture";
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
