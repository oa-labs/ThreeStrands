import { describe, expect, it } from "vitest";
import { createDemoNotes } from "./demoNotes";

describe("demo notes", () => {
  it("follows the native title rules", async () => {
    const notes = createDemoNotes();
    expect((await notes.createNote("  Project   plan ", "")).title).toBe("Project plan");
    await expect(notes.createNote("project plan", "")).rejects.toThrow("already exists");
    await expect(notes.createNote("2026-11-09", "")).rejects.toThrow("daily note");
    await expect(notes.createNote("a|b", "")).rejects.toThrow("can't contain");
  });

  it("saves only from the current version and never creates an empty day", async () => {
    const notes = createDemoNotes();
    const note = await notes.createNote("Plan", "v1");
    const saved = await notes.updateNote(note.id, "Plan", "v2", note.updatedAt);
    expect(saved.saved).toBe(true);
    const stale = await notes.updateNote(note.id, "Plan", "v3", note.updatedAt);
    expect(stale).toEqual({ saved: false, note: saved.note });

    expect(await notes.saveDailyNote("2026-11-09", "", null)).toEqual({ saved: true, note: null });
    expect(await notes.getDailyNote("2026-11-09")).toBeNull();
    const day = await notes.saveDailyNote("2026-11-09", "Standup", null);
    expect(day.note).toMatchObject({ id: "daily:2026-11-09", title: "2026-11-09", dailyDate: "2026-11-09" });
    expect((await notes.saveDailyNote("2026-11-09", "Again", null)).saved).toBe(false);
  });

  it("lists recently opened notes first and searches titles and text", async () => {
    const notes = createDemoNotes();
    const first = await notes.createNote("First", "alpha");
    await notes.createNote("Second", "alpha beta");
    await notes.recordNoteVisit(first.id);
    expect((await notes.listRecentNotes(10)).map((note) => note.title)).toEqual(["First", "Second"]);
    expect((await notes.searchNotes("beta", 10)).map((hit) => hit.title)).toEqual(["Second"]);
    expect(await notes.searchNotes("  ", 10)).toEqual([]);
  });
  it("moves notes through Trash by the native rules", async () => {
    const notes = createDemoNotes();
    const plan = await notes.createNote("Plan", "old");
    await expect(notes.deleteNote(plan.id)).rejects.toThrow("Move this note to Trash");
    await notes.trashNote(plan.id);
    expect((await notes.listRecentNotes(10)).map((note) => note.id)).not.toContain(plan.id);
    expect((await notes.listTrashedNotes(10)).map((note) => note.id)).toEqual([plan.id]);
    // A note in Trash is never current: the save comes back with it, for the editor to offer a restore.
    const refused = await notes.updateNote(plan.id, "Plan", "edit", (await notes.getNote(plan.id))!.updatedAt);
    expect(refused).toMatchObject({ saved: false, note: { id: plan.id, trashed: true, body: "old" } });

    const replacement = await notes.createNote("plan", "new");
    await expect(notes.restoreNote(plan.id)).rejects.toThrow("Rename that note");
    await notes.updateNote(replacement.id, "New plan", "new", replacement.updatedAt);
    expect((await notes.restoreNote(plan.id)).trashed).toBe(false);

    await notes.trashNote(plan.id);
    await notes.deleteNote(plan.id);
    expect(await notes.getNote(plan.id)).toBeNull();
    expect(await notes.listTrashedNotes(10)).toEqual([]);
  });
});
