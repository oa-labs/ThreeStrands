import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import type { FollowThroughItem, MeetingContext } from "./domain";
import {
  adhocFollowThroughSectionText, discardFollowThroughRecovery, FOLLOW_THROUGH_MARKER, FOLLOW_THROUGH_REVIEW_RETENTION_MS, followThroughSections, followUpBody, followUpRecipients, insertAdhocFollowThrough, insertFollowThrough,
  itemLines, itemsFromSection, MAX_FOLLOW_THROUGH_ITEM_CHARS, MAX_FOLLOW_THROUGH_ITEMS, MAX_FOLLOW_THROUGH_REVIEWS,
  readFollowThroughRecovery, readFollowUpHandoff, rememberFollowUpHandoff, saveFollowThroughRecovery} from "./meetingFollowThrough";
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

  it("remembers one handoff per occurrence", () => {
    expect(readFollowUpHandoff(reference)).toBeNull();
    rememberFollowUpHandoff(reference, "h1");
    expect(readFollowUpHandoff(reference)).toBe("h1");
    rememberFollowUpHandoff(reference, null);
    expect(readFollowUpHandoff(reference)).toBeNull();
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

  it("writes Summary, Open question, Transcript link, and ad-hoc sections with exact undo range", () => {
    const items: FollowThroughItem[] = [
      { kind: "decision", text: "Ship Friday" },
      { kind: "question", text: "Does legal need final copy?" },
    ];
    const withExtras = insertFollowThrough("", reference, "Sync", "2026-11-16", items, {
      summary: "Aligned on shipping Friday.",
      transcript: { label: "Transcript – Sync – 2026-11-16.md", href: "attachment:att1" },
    });
    expect(withExtras.body).toContain("  - Summary: Aligned on shipping Friday.");
    expect(withExtras.body).toContain("  - Open question: Does legal need final copy?");
    expect(withExtras.body).toContain("  - Transcript: [Transcript – Sync – 2026-11-16.md](attachment:att1)");
    expect(itemsFromSection(withExtras.body)).toEqual(items);

    const existing = "## Journal\n\nMorning.\n";
    const adhoc = insertAdhocFollowThrough(existing, "2:15 PM", "Quick chat", "2026-11-16", [{ kind: "task", text: "Send notes" }], { summary: "Wrapped up." });
    expect(adhoc.body).toContain(`## Meetings\n\n- 2:15 PM for Quick chat ${FOLLOW_THROUGH_MARKER}\n  - Summary: Wrapped up.\n  - ( ) Send notes`);
    expect(adhocFollowThroughSectionText("2:15 PM", "Quick chat", [{ kind: "task", text: "Send notes" }])).toContain("2:15 PM for Quick chat");
    expect(adhoc.body.slice(0, adhoc.start) + adhoc.body.slice(adhoc.end)).toBe(existing);
  });
});
