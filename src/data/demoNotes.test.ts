import { describe, expect, it } from "vitest";
import { createDemoNotes } from "./demoNotes";
import { noteReferenceHref } from "../noteEntities";
import { primaryMeetingNote } from "../meetingNoteEntry";
import type { ContactProfile, Thread } from "../domain";

describe("demo notes", () => {
  it("offers contacts by name or address and conversations by participant with stable references", async () => {
    const contacts = [
      { id: "c-ada", displayName: "Ada", addresses: ["ada@example.com", "ada@work.example"] },
      { id: "c-other", displayName: "Ada", addresses: ["other@example.com"] },
    ] as ContactProfile[];
    const threads = [
      { id: "local-personal", providerThreadId: "same-provider", accountId: "personal@example.com", subject: "Review", participants: ["Ada <ada@example.com>"], lastMessageAt: "2026-11-10T15:00:00Z" },
      { id: "local-work", providerThreadId: "same-provider", accountId: "work@example.com", subject: "Review", participants: ["Ada <ada@example.com>"], lastMessageAt: "2026-11-09T15:00:00Z" },
    ] as Thread[];
    const notes = createDemoNotes({ contacts: () => contacts, threads: () => threads, tasks: () => [], events: () => [] });
    expect(await notes.noteLinkChoices("contact", "Ada")).toHaveLength(2);
    const found = await notes.noteLinkChoices("contact", "ada@work.example");
    expect(found).toHaveLength(1);
    expect(found[0].reference).toEqual({ version: 1, kind: "contact", id: "ada@example.com", contact: "c-ada" });
    expect(await notes.resolveNoteContact(found[0].reference)).toBe("c-ada");
    const mail = await notes.noteLinkChoices("conversation", "ada@example.com");
    expect(mail.map((choice) => choice.sentAt)).toEqual(threads.map((thread) => thread.lastMessageAt));
    expect(await notes.resolveNoteConversation(mail[0].reference)).toBe("local-personal");
    expect(await notes.resolveNoteConversation(mail[1].reference)).toBe("local-work");
  });
  it("follows the native title rules", async () => {
    const notes = createDemoNotes();
    expect((await notes.createNote("  Project   plan ", "")).title).toBe("Project plan");
    await expect(notes.createNote("project plan", "")).rejects.toThrow("already exists");
    await expect(notes.createNote("2026-11-09", "")).rejects.toThrow("daily note");
    await expect(notes.createNote("a|b", "")).rejects.toThrow("can't contain");
  });

  it("lists notes created in a range, leaving out daily notes and notes in Trash", async () => {
    const notes = createDemoNotes();
    const kept = await notes.createNote("Kept", "About it");
    const trashed = await notes.createNote("Trashed", "");
    await notes.trashNote(trashed.id);
    await notes.saveDailyNote("2026-11-09", "A daily note", null);
    const at = Date.parse(kept.createdAt);
    const range = (start: number, end: number) => notes.listNotesCreatedBetween(new Date(at + start).toISOString(), new Date(at + end).toISOString(), 10);
    expect((await range(-1000, 1000)).map((note) => note.title)).toEqual(["Kept"]);
    expect(await range(-1000, 1000)).toEqual([{ id: kept.id, title: "Kept", preview: "About it", createdAt: kept.createdAt }]);
    expect(await range(0, 1000)).toHaveLength(1);
    expect(await range(-1000, 0)).toEqual([]);
    expect(await range(1, 1000)).toEqual([]);
  });

  it("loads seeded notes by the native rules, the last seed most recently changed", async () => {
    const notes = createDemoNotes(undefined, [
      { title: "Project plan", body: "- ( ) Ship it\n" },
      { dailyDate: "2026-11-09", body: "Linked to [[Project plan]]\n" },
    ]);
    expect((await notes.listRecentNotes(10)).map((note) => note.title)).toEqual(["2026-11-09", "Project plan"]);
    expect((await notes.getDailyNote("2026-11-09"))?.body).toContain("[[Project plan]]");
    expect(await notes.listNoteTasks()).toEqual([expect.objectContaining({ title: "Ship it", checked: false })]);
    expect(() => createDemoNotes(undefined, [{ title: "a|b", body: "" }])).toThrow("can't contain");
    expect(() => createDemoNotes(undefined, [{ title: "Plan", body: "" }, { title: "plan", body: "" }])).toThrow("already exists");
    expect(() => createDemoNotes(undefined, [{ dailyDate: "Nov 9", body: "" }])).toThrow("valid date");
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
    await notes.trashNote(first.id);
    expect((await notes.searchNotes("alpha", 10)).map((hit) => hit.title)).toEqual(["Second"]);
    expect(await notes.searchNotes("alpha", 10, "trash")).toEqual([expect.objectContaining({ id: first.id, trashed: true })]);
    expect(await notes.searchNotes("alpha", 10, "all")).toHaveLength(2);
    expect(await notes.searchNotes("alpha", 1, "all")).toHaveLength(1);
    expect(await notes.searchNotes("  ", 10, "trash")).toEqual([]);
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
  it("adds a meeting once per occurrence and links one shared meeting note, as native does", async () => {
    const notes = createDemoNotes();
    const event = (id: string) => ({ version: 1 as const, kind: "event" as const, id, account: "you@example.com", calendar: "work" });
    const entry = (id: string) => ({ event: event(id), time: "9:30am", attendees: [], name: "Weekly [sync]", linkNote: true });
    const first = await notes.addMeetingToDailyNote("2026-11-12", null, entry("a"));
    expect(first.note.body).toMatch(/^## Meetings\n\n- \[9:30am\]\(threestrands:event\/.+\) for \[\[Weekly -sync-\]\]\n$/);
    expect(first.createdMeetingNote).toBe(true);
    expect(primaryMeetingNote(await notes.noteBacklinks(event("a")))?.noteId).toBe(first.meetingNote?.id);
    expect((await notes.addMeetingToDailyNote("2026-11-12", first.note.updatedAt, entry("a"))).added).toBe(false);
    const next = await notes.addMeetingToDailyNote("2026-11-19", null, entry("b"));
    expect(next.createdMeetingNote).toBe(false);
    expect(next.meetingNote?.id).toBe(first.meetingNote?.id);
    await expect(notes.addMeetingToDailyNote("2026-11-12", null, entry("c"))).rejects.toThrow("changed");
  });
  it("resolves existing daily meeting links without changing either note, across rename and Trash", async () => {
    const reference = { version: 1 as const, kind: "event" as const, id: "occurrence", account: "you@example.com", calendar: "work", series: "series" };
    const notes = createDemoNotes();
    const meeting = await notes.createNote("My EO Software Consultancy Forum - November 18th, 2026", "Meeting notes");
    const day = (await notes.saveDailyNote("2026-11-18", `## Meetings\n\n- [10:00 am](${noteReferenceHref(reference)}) met with chris@pagepro.co, hector@codehesion.co.za, Ciapka, Mark, Nils Löwe, Miguel Muñoz, harley@coderpush.com, Hector Beyers, MEng for [[${meeting.title}|Forum]]\n`, null)).note!;
    const rows = await notes.noteBacklinks(reference);
    expect(rows.map((row) => row.noteId).sort()).toEqual([day.id, meeting.id].sort());
    expect(primaryMeetingNote(rows)?.noteId).toBe(meeting.id);
    expect(await notes.getNote(day.id)).toEqual(day);
    expect(await notes.getNote(meeting.id)).toEqual(meeting);
    expect(primaryMeetingNote(await notes.noteBacklinks({ ...reference, id: "series", series: undefined }))?.noteId).toBe(meeting.id);
    for (const other of [{ ...reference, id: "another-occurrence" }, { ...reference, account: "other" }, { ...reference, calendar: "other" }]) expect(await notes.noteBacklinks(other)).toEqual([]);
    const renamed = (await notes.updateNote(meeting.id, "Renamed forum", meeting.body, meeting.updatedAt)).note!;
    expect(primaryMeetingNote(await notes.noteBacklinks(reference))?.noteId).toBe(meeting.id);
    await notes.trashNote(renamed.id);
    expect(primaryMeetingNote(await notes.noteBacklinks(reference))?.noteId).toBe(day.id);
    await notes.restoreNote(renamed.id);
    await notes.trashNote(day.id);
    expect(await notes.noteBacklinks(reference)).toEqual([]);
  });
  it.each([
    "- [Meeting](EVENT) for plain title\n- [[Target]]",
    "- [Meeting](EVENT) for plain title\n  - Read [[Target]]",
    "- [Meeting](EVENT) for `[[Target]]`",
    "- [Meeting](EVENT) for \\[[Target]]",
    "- [Meeting](EVENT) for [[Target]] and [[Other]]",
    "> [Meeting](EVENT) for [[Target]]",
    "```\n- [Meeting](EVENT) for [[Target]]\n```",
  ])("does not promote unrelated or ambiguous wiki links: %s", async (body) => {
    const notes = createDemoNotes();
    const reference = { version: 1 as const, kind: "event" as const, id: "occurrence", account: "you@example.com", calendar: "work" };
    const target = await notes.createNote("Target", "");
    await notes.createNote("Other", "");
    await notes.saveDailyNote("2026-11-18", body.replace("EVENT", noteReferenceHref(reference)), null);
    expect((await notes.noteBacklinks(reference)).some((row) => row.noteId === target.id)).toBe(false);
  });
  it("lists a series' notes from every occurrence, and an occurrence's own only", async () => {
    const notes = createDemoNotes();
    const event = (id: string, series?: string) => ({ version: 1 as const, kind: "event" as const, id, account: "you@example.com", calendar: "work", ...(series ? { series } : {}) });
    const tuesday = await notes.createNote("Tuesday", `[Standup](${noteReferenceHref(event("s_1110", "s"))})`);
    const whole = await notes.createNote("Standup", `[Standup](${noteReferenceHref(event("s"))})`);
    expect((await notes.noteBacklinks(event("s"))).map((row) => row.noteId).sort()).toEqual([tuesday.id, whole.id].sort());
    expect((await notes.noteBacklinks(event("s_1110"))).map((row) => row.noteId)).toEqual([tuesday.id]);
  });
});
