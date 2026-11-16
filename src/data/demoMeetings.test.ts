import { describe, expect, it } from "vitest";
import type { ScheduleEvent, Thread, ThreadTask } from "../domain";
import { noteReferenceHref, type NoteReference } from "../noteEntities";
import { createDemoMeetings } from "./demoMeetings";
import { MAX_READINESS_EVENTS } from "../meetingPrep";
import { createDemoNotes } from "./demoNotes";
import type { DemoNoteSeed } from "./demoDataset";

const ME = "me@example.com";
const occurrence = (id: string, series?: string): NoteReference => ({ version: 1, kind: "event", id, account: ME, calendar: "work", ...(series ? { series } : {}) });
const href = (id: string, series?: string) => noteReferenceHref(occurrence(id, series));
const event = (id: string, start: string, extra: Partial<ScheduleEvent> = {}): ScheduleEvent => ({ id, accountId: ME, calendarId: "work", title: "Weekly", start, end: start.replace("T15:", "T16:"), allDay: false, attendees: [], ...extra });
const thread = (id: string, participants: string[], lastMessageAt: string): Thread => ({ id, providerThreadId: id, subject: `About ${id}`, snippet: "", participants, lastMessageAt, lastReceivedAt: lastMessageAt, unread: false, starred: false, archived: false, trashed: false, labels: [], accountId: ME, summary: null, summaryGeneratedAt: null, summaryRevision: null, hasAttachments: false });
const task = (id: string, threadId: string, status: ThreadTask["status"] = "open"): ThreadTask => ({ id, accountId: ME, threadId, subjectSnapshot: "Subject", title: id, kind: "action", dueKind: "none", status, createdAt: "", updatedAt: "" });

function setup(events: ScheduleEvent[], notes: DemoNoteSeed[], threads: Thread[] = [], tasks: ThreadTask[] = [], mailAccounts = [ME]) {
  const demoNotes = createDemoNotes({ threads: () => threads, tasks: () => tasks, events: () => events }, notes);
  return createDemoMeetings({
    events: () => events, threads: () => threads, tasks: () => tasks, mailAccounts: () => mailAccounts, ownAddresses: () => [ME],
    threadForMessage: (id) => id === "origin-msg" ? "origin" : null, now: () => new Date("2026-11-16T12:00:00Z"), notes: demoNotes,
  });
}

describe("demo meeting preparation", () => {
  it("lists this meeting's block, earlier occurrences of the exact series, and series notes", async () => {
    const meetings = setup(
      [event("occ-1", "2026-11-09T15:00:00Z", { seriesId: "s" }), event("occ-2", "2026-11-16T15:00:00Z", { seriesId: "s" }), event("occ-3", "2026-11-23T15:00:00Z", { seriesId: "s" })],
      [
        { dailyDate: "2026-11-09", body: `- [3pm](${href("occ-1", "s")})\n  - Last week\n` },
        { dailyDate: "2026-11-16", body: `- [9am](${href("other")})\n  - Private\n- [3pm](${href("occ-2", "s")})\n  - ( ) Send the deck\n` },
        { dailyDate: "2026-11-23", body: `- [3pm](${href("occ-3", "s")})\n  - Next week\n` },
        { title: "About the series", body: `- [Weekly](${href("s")})\n  - Agenda\n` },
      ],
    );
    const context = await meetings.meetingContext(occurrence("occ-2", "s"));
    // Saving gives the round task its stable marker, which the excerpt keeps.
    expect(context.thisMeeting.items.map((source) => source.text.replace(/ <!-- threestrands-task:[^>]* -->/g, ""))).toEqual([`- [3pm](${href("occ-2", "s")})\n  - ( ) Send the deck`]);
    expect(context.previousMeetings.items.map((item) => item.reference.id)).toEqual(["occ-1"]);
    expect(context.seriesNotes.items.map((source) => source.title)).toEqual(["About the series"]);
    expect(context.commitments.items.map((item) => [item.title, item.origin])).toEqual([["Send the deck", "meetingNote"]]);
  });

  it("finds origin and attendee mail in scope and marks attendee tasks as candidates", async () => {
    const meetings = setup(
      [event("evt", "2026-11-16T15:00:00Z", { attendees: ["ann@example.org", ME], sourceMessageId: "origin-msg" })],
      [],
      [thread("origin", ["Cy <cy@example.org>"], "2026-11-10T09:00:00Z"), thread("with-ann", ["Ann <ann@example.org>"], "2026-11-14T09:00:00Z"), thread("old", ["Ann <ann@example.org>"], "2026-06-01T09:00:00Z")],
      [task("Reply to Cy", "origin"), task("Send Ann numbers", "with-ann"), task("Done", "origin", "completed")],
    );
    const context = await meetings.meetingContext(occurrence("evt"));
    expect(context.event?.attendees).toEqual(["ann@example.org"]);
    expect(context.conversations.items.map((item) => [item.threadId, item.reason])).toEqual([["origin", "origin"], ["with-ann", "attendees"]]);
    expect(context.commitments.items.map((item) => [item.title, item.candidate])).toEqual([["Reply to Cy", false], ["Send Ann numbers", true]]);
    const [readiness] = await meetings.meetingReadiness([occurrence("evt")]);
    expect(readiness).toMatchObject({ eventAvailable: true, openCommitments: 1, candidateCommitments: 1, latestCorrespondenceAt: "2026-11-14T09:00:00Z", scopeRequired: false });
    await expect(meetings.meetingContext(occurrence("evt"), ["unknown@example.com"])).rejects.toThrow();
  });

  it("asks for a mail scope when the calendar isn't a mail account and explains a missing event", async () => {
    const meetings = setup([event("evt", "2026-11-16T15:00:00Z", { attendees: ["ann@example.org"] })], [{ dailyDate: "2026-11-16", body: `- [3pm](${href("gone")})\n` }], [], [], []);
    expect((await meetings.meetingContext(occurrence("evt"))).conversations.status).toBe("scopeRequired");
    const missing = await meetings.meetingContext(occurrence("gone"));
    expect(missing.event).toBeNull();
    expect(missing.thisMeeting.items).toHaveLength(1);
    expect(missing.conversations.status).toBe("unavailable");
  });

  it("reserves one prep task under the meeting's line, adding the line when needed", async () => {
    const meetings = setup([], []);
    const entry = { event: occurrence("evt"), time: "3:00pm", attendees: [], name: "Sync", linkNote: false };
    const first = await meetings.reserveMeetingPrep("2026-11-16", null, entry);
    expect(first).toMatchObject({ added: true, addedMeetingLine: true, previousBody: "" });
    expect(first.note.body).toMatch(/^## Meetings\n\n- \[3:00pm\]\(.+\) for Sync\n {2}- \( \) Prepare for Sync/);
    expect(first.taskId).toMatch(/^note:daily:2026-11-16:/);
    const again = await meetings.reserveMeetingPrep("2026-11-16", first.note.updatedAt, entry);
    expect(again).toMatchObject({ added: false, addedMeetingLine: false, taskId: first.taskId });
    await expect(meetings.reserveMeetingPrep("2026-11-16", "stale", entry)).rejects.toThrow();
  });

  it("bounds readiness requests", async () => {
    const meetings = setup([], []);
    const atLimit = Array.from({ length: MAX_READINESS_EVENTS }, (_, n) => occurrence(`e${n}`));
    expect(await meetings.meetingReadiness(atLimit)).toHaveLength(MAX_READINESS_EVENTS);
    await expect(meetings.meetingReadiness([...atLimit, occurrence("more")])).rejects.toThrow();
  });
});
