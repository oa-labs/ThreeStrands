import { describe, expect, it } from "vitest";
import { formatClockTime, isDeclined, mergeCalendarCopies } from "./agendaEvents";
import type { ScheduleEvent } from "./domain";

const at = (id: string, start: string, end: string, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent =>
  ({ id, accountId: "a@example.com", title: "Sync", start, end, allDay: false, ...overrides });

describe("mergeCalendarCopies", () => {
  it("keeps one copy of the same meeting, preferring one not declined, in its first place", () => {
    const merged = mergeCalendarCopies([
      at("early", "2026-10-23T08:00:00Z", "2026-10-23T08:30:00Z", { title: "Breakfast" }),
      at("declined", "2026-10-23T16:00:00Z", "2026-10-23T17:00:00Z", { responseStatus: "declined", accountId: "b@example.com" }),
      at("going", "2026-10-23T12:00:00-04:00", "2026-10-23T13:00:00-04:00", { title: " sync " }),
    ]);
    expect(merged.map((event) => event.id)).toEqual(["early", "going"]);
  });

  it("keeps events that differ in title, start, or end", () => {
    const merged = mergeCalendarCopies([
      at("one", "2026-10-23T16:00:00Z", "2026-10-23T17:00:00Z"),
      at("later", "2026-10-23T16:30:00Z", "2026-10-23T17:00:00Z"),
      at("longer", "2026-10-23T16:00:00Z", "2026-10-23T18:00:00Z"),
      at("other", "2026-10-23T16:00:00Z", "2026-10-23T17:00:00Z", { title: "Review" }),
    ]);
    expect(merged).toHaveLength(4);
  });

  it("keeps a declined meeting when it is the only copy", () => {
    const merged = mergeCalendarCopies([at("only", "2026-10-23T16:00:00Z", "2026-10-23T17:00:00Z", { responseStatus: "declined" })]);
    expect(merged.map(isDeclined)).toEqual([true]);
  });
});

describe("formatClockTime", () => {
  it("lowercases the day period, as event times do", () => {
    expect(formatClockTime(new Date(2026, 9, 23, 16, 51))).not.toMatch(/[AP]M/);
  });
});
