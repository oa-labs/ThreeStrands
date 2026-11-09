import { afterEach, describe, expect, it, vi } from "vitest";
import { clearNoteDraft, noteTargetKey, parseNoteTargetKey, readLastNoteTarget, readNoteDraft, todayTarget, writeLastNoteTarget, writeNoteDraft } from "./noteDrafts";

describe("note targets and drafts", () => {
  afterEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("keys a day by its daily note id and reads keys back", () => {
    const today = todayTarget(new Date(2026, 10, 9, 23, 30));
    expect(today).toEqual({ kind: "daily", date: "2026-11-09" });
    expect(noteTargetKey(today)).toBe("daily:2026-11-09");
    expect(parseNoteTargetKey("daily:2026-11-09")).toEqual(today);
    expect(parseNoteTargetKey("note-1")).toEqual({ kind: "note", id: "note-1" });
    expect(parseNoteTargetKey("daily:soon")).toEqual({ kind: "note", id: "daily:soon" });
  });

  it("keeps an unsaved draft with the version it started from, and ignores a malformed one", () => {
    expect(writeNoteDraft("n1", { title: "Plan", body: "typed", base: "v1" })).toBe(true);
    expect(readNoteDraft("n1")).toEqual({ title: "Plan", body: "typed", base: "v1" });
    clearNoteDraft("n1");
    expect(readNoteDraft("n1")).toBeNull();
    localStorage.setItem("threestrands.notes.draft.n2", "{\"title\":1}");
    expect(readNoteDraft("n2")).toBeNull();
    localStorage.setItem("threestrands.notes.draft.n3", "not json");
    expect(readNoteDraft("n3")).toBeNull();
  });

  it("reports when storage refuses a draft", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("full"); });
    expect(writeNoteDraft("n1", { title: "", body: "x", base: null })).toBe(false);
  });

  it("remembers the last open note on this device", () => {
    expect(readLastNoteTarget()).toBeNull();
    writeLastNoteTarget({ kind: "note", id: "n1" });
    expect(readLastNoteTarget()).toEqual({ kind: "note", id: "n1" });
  });
});
