import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import type { FollowThroughItem, MeetingContext, ScheduleEvent } from "./domain";
import {
  discardFollowThroughRecovery, FOLLOW_THROUGH_REVIEW_RETENTION_MS, followThroughSections, followUpBody, followUpRecipients, insertFollowThrough,
  itemLines, itemsFromSection, MAX_FOLLOW_THROUGH_ITEM_CHARS, MAX_FOLLOW_THROUGH_ITEMS, MAX_FOLLOW_THROUGH_REVIEWS, meetingsToFollowThrough, noFollowThroughKey,
  readFollowThroughRecovery, readFollowUpHandoff, readNoFollowThrough, rememberFollowUpHandoff, saveFollowThroughRecovery, setNoFollowThrough,
} from "./meetingFollowThrough";
import { noteReferenceHref, type NoteReference } from "./noteEntities";

const fixtures = JSON.parse(readFileSync("fixtures/meeting-follow-through.json", "utf8")) as {
  references: Record<string, NoteReference>;
  target: string;
  sections: { name: string; body: string; sections: { date: string | null; text: string }[] }[];
  insert: { name: string; body: string; meeting: string; date: string; items: FollowThroughItem[]; expected: string }[];
};
const fill = (text: string) => Object.keys(fixtures.references).sort((a, b) => b.length - a.length)
  .reduce((out, name) => out.replaceAll(`{${name}}`, noteReferenceHref(fixtures.references[name])), text);
const target = fixtures.references[fixtures.target];

describe("meeting follow-through format", () => {
  it.each(fixtures.sections)("sections: $name", ({ body, sections }) => {
    const filled = fill(body);
    expect(followThroughSections(filled, target).map((section) => ({ date: section.date, text: filled.slice(section.start, section.end) })))
      .toEqual(sections.map((section) => ({ date: section.date, text: fill(section.text) })));
  });

  it.each(fixtures.insert)("insert: $name", ({ body, meeting, date, items, expected }) => {
    const filled = fill(body);
    let actual: string;
    try {
      const inserted = insertFollowThrough(filled, target, meeting, date, items);
      expect(inserted.body.slice(0, inserted.start) + inserted.body.slice(inserted.end)).toBe(filled);
      actual = inserted.body;
    } catch (error) {
      actual = `ERROR: ${(error as Error).message}`;
    }
    expect(actual).toBe(fill(expected));
  });

  it("limits item count and length below, at, and above the limit", () => {
    const items = (n: number): FollowThroughItem[] => Array.from({ length: n }, (_, i) => ({ kind: "checklist", text: `item ${i}` }));
    expect(() => itemLines(items(MAX_FOLLOW_THROUGH_ITEMS - 1))).not.toThrow();
    expect(() => itemLines(items(MAX_FOLLOW_THROUGH_ITEMS))).not.toThrow();
    expect(() => itemLines(items(MAX_FOLLOW_THROUGH_ITEMS + 1))).toThrow();
    expect(() => itemLines([{ kind: "task", text: "a".repeat(MAX_FOLLOW_THROUGH_ITEM_CHARS) }])).not.toThrow();
    expect(() => itemLines([{ kind: "task", text: "a".repeat(MAX_FOLLOW_THROUGH_ITEM_CHARS + 1) }])).toThrow();
  });
});

const reference: NoteReference = { version: 1, kind: "event", id: "evt-1", account: "you@example.com", calendar: "work" };
const event = (id: string, end: string, extra: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
  id, accountId: "you@example.com", calendarId: "work", title: id, allDay: false, start: "2026-11-16T09:00:00", end, attendees: [], ...extra,
});

describe("meeting follow-through local state", () => {
  afterEach(() => localStorage.clear());

  it("keeps a review in progress per occurrence, bounded in count and age, until discarded", () => {
    const now = 1_000_000_000_000;
    saveFollowThroughRecovery({ reference, reviewId: "r1", operationId: "op", destinationNoteId: "n1", items: [{ id: "i", kind: "task", text: "Send" }], updatedAt: 0 }, now);
    expect(readFollowThroughRecovery(reference, now)).toMatchObject({ reviewId: "r1", operationId: "op", items: [{ text: "Send" }] });
    expect(readFollowThroughRecovery({ ...reference, id: "evt-2" }, now)).toBeNull();
    expect(readFollowThroughRecovery(reference, now + FOLLOW_THROUGH_REVIEW_RETENTION_MS)).not.toBeNull();
    expect(readFollowThroughRecovery(reference, now + FOLLOW_THROUGH_REVIEW_RETENTION_MS + 1)).toBeNull();
    discardFollowThroughRecovery("r1", now);
    expect(readFollowThroughRecovery(reference, now)).toBeNull();

    for (let index = 0; index <= MAX_FOLLOW_THROUGH_REVIEWS; index++) {
      saveFollowThroughRecovery({ reference: { ...reference, id: `evt-${index}` }, reviewId: `r${index}`, operationId: "op", destinationNoteId: null, items: [], updatedAt: 0 }, now + index);
    }
    expect(JSON.parse(localStorage.getItem("threestrands.meetings.followThroughReviews")!)).toHaveLength(MAX_FOLLOW_THROUGH_REVIEWS);
    expect(readFollowThroughRecovery({ ...reference, id: "evt-0" }, now + 100)).toBeNull();
  });

  it("sets a meeting's follow-through aside for its day, with undo", () => {
    const meeting = event("evt-1", "2026-11-16T10:00:00");
    const key = noFollowThroughKey(meeting)!;
    expect(key.startsWith("2026-11-16|")).toBe(true);
    expect(setNoFollowThrough(key, true)).toEqual([key]);
    expect(readNoFollowThrough()).toEqual([key]);
    expect(setNoFollowThrough(key, false)).toEqual([]);
  });

  it("remembers one handoff per occurrence", () => {
    expect(readFollowUpHandoff(reference)).toBeNull();
    rememberFollowUpHandoff(reference, "h1");
    expect(readFollowUpHandoff(reference)).toBe("h1");
    rememberFollowUpHandoff(reference, null);
    expect(readFollowUpHandoff(reference)).toBeNull();
  });

  it("lists only ended meetings that aren't declined, all-day, or working locations", () => {
    const now = new Date("2026-11-16T12:00:00");
    const events = [
      event("ended", "2026-11-16T10:00:00"),
      event("later", "2026-11-16T15:00:00"),
      event("declined", "2026-11-16T10:00:00", { responseStatus: "declined" }),
      event("allday", "2026-11-17", { allDay: true, start: "2026-11-16" }),
    ];
    expect(meetingsToFollowThrough(events, now).map((item) => item.id)).toEqual(["ended"]);
  });
});

describe("follow-up drafting helpers", () => {
  it("addresses the event's other people, never the user's own addresses", () => {
    const context = { event: { attendees: ["Ann@Example.org", "you@example.com", "cal@example.com", "ann@example.org", "room"] } } as unknown as MeetingContext;
    expect(followUpRecipients(context, ["you@example.com", "cal@example.com"])).toEqual(["ann@example.org"]);
    expect(followUpRecipients(null, [])).toEqual([]);
  });

  it("assembles the body from the reviewed decisions and actions", () => {
    expect(followUpBody("Sync", [{ kind: "decision", text: "Ship  Friday" }, { kind: "task", text: "Send deck" }, { kind: "link", task: { version: 1, kind: "task", id: "t" }, title: "Contract" }]))
      .toBe("Thanks for joining Sync. A quick summary:\n\nDecisions:\n- Ship Friday\n\nNext steps:\n- Send deck\n- Contract\n");
  });
});

describe("reading a saved section back", () => {
  it("returns the items it was written from", () => {
    const items: FollowThroughItem[] = [
      { kind: "decision", text: "Ship *now*" }, { kind: "task", text: "Send [deck]" }, { kind: "checklist", text: "Room" },
      { kind: "action", text: "Dana prices" }, { kind: "link", task: { version: 1, kind: "task", id: "t1" }, title: "Contract" },
    ];
    const text = insertFollowThrough("", reference, "Sync", "2026-11-16", items).body.replace("( ) Send", "(x) Send").replace("\\[deck\\]", "\\[deck\\] <!-- threestrands-task:abc -->");
    expect(itemsFromSection(text)).toEqual([...items.slice(0, 1), { kind: "task", text: "Send [deck]" }, ...items.slice(2)]);
  });
});
