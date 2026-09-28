import { describe, expect, it } from "vitest";
import { addDays, eventDayBounds, layOutDayEvents, occursOnDay, startOfWeek } from "./calendarTime";
import type { ScheduleEvent } from "./domain";

function timedEvent(id: string, start: string, end: string): ScheduleEvent {
  return { id, accountId: "a@example.com", title: id, start, end, allDay: false };
}

describe("calendar time helpers", () => {
  it("anchors weeks on Sunday regardless of the day passed in", () => {
    // 2026-09-22 is a Tuesday; its week starts Sunday 2026-09-20.
    for (let offset = 0; offset < 7; offset += 1) {
      const start = startOfWeek(new Date(2026, 8, 20 + offset));
      expect(start.getDay()).toBe(0);
      expect(start.getDate()).toBe(20);
    }
  });

  it("clamps events that spill outside the day to the day's own minutes", () => {
    const day = new Date(2026, 8, 22);
    const overnight = timedEvent("overnight", "2026-09-21T22:00:00", "2026-09-22T02:00:00");
    expect(eventDayBounds(overnight, day)).toEqual({ start: 0, end: 120 });

    const trailing = timedEvent("trailing", "2026-09-22T23:00:00", "2026-09-23T01:00:00");
    expect(eventDayBounds(trailing, day)).toEqual({ start: 23 * 60, end: 24 * 60 });
  });

  it("includes multi-day events on every day they span", () => {
    const event = timedEvent("span", "2026-09-21T22:00:00", "2026-09-23T02:00:00");
    expect(occursOnDay(event, new Date(2026, 8, 21))).toBe(true);
    expect(occursOnDay(event, new Date(2026, 8, 22))).toBe(true);
    expect(occursOnDay(event, new Date(2026, 8, 23))).toBe(true);
    expect(occursOnDay(event, new Date(2026, 8, 24))).toBe(false);
  });

  it("matches all-day events to their own local date only", () => {
    const allDay: ScheduleEvent = {
      id: "holiday", accountId: "a@example.com", title: "Holiday",
      start: "2026-09-22", end: "2026-09-23", allDay: true,
    };
    expect(occursOnDay(allDay, new Date(2026, 8, 22))).toBe(true);
    expect(occursOnDay(allDay, new Date(2026, 8, 23))).toBe(false);
  });

  it("gives overlapping events side-by-side lanes and sequential events the full width", () => {
    const day = new Date(2026, 8, 22);
    const overlapping = layOutDayEvents([
      timedEvent("a", "2026-09-22T09:00:00", "2026-09-22T10:00:00"),
      timedEvent("b", "2026-09-22T09:30:00", "2026-09-22T10:30:00"),
      timedEvent("c", "2026-09-22T09:45:00", "2026-09-22T10:15:00"),
    ], day);
    expect(overlapping.map((entry) => entry.lane)).toEqual([0, 1, 2]);
    expect(overlapping.every((entry) => entry.lanes === 3)).toBe(true);
    expect(overlapping.map((entry) => entry.span)).toEqual([1, 1, 1]);

    const sequential = layOutDayEvents([
      timedEvent("a", "2026-09-22T09:00:00", "2026-09-22T10:00:00"),
      timedEvent("b", "2026-09-22T11:00:00", "2026-09-22T12:00:00"),
    ], day);
    expect(sequential.map((entry) => ({ lane: entry.lane, lanes: entry.lanes, span: entry.span }))).toEqual([
      { lane: 0, lanes: 1, span: 1 },
      { lane: 0, lanes: 1, span: 1 },
    ]);
  });

  it("lets an event fill lanes that are free throughout its time range", () => {
    const day = new Date(2026, 8, 22);
    const placed = layOutDayEvents([
      timedEvent("long", "2026-09-22T09:00:00", "2026-09-22T11:00:00"),
      timedEvent("early", "2026-09-22T09:00:00", "2026-09-22T09:30:00"),
      timedEvent("middle", "2026-09-22T09:30:00", "2026-09-22T10:30:00"),
      timedEvent("third", "2026-09-22T10:00:00", "2026-09-22T10:15:00"),
      timedEvent("late", "2026-09-22T10:30:00", "2026-09-22T11:00:00"),
    ], day);
    expect(placed.map(({ event, lane, lanes, span }) => ({ id: event.id, lane, lanes, span }))).toEqual([
      { id: "long", lane: 0, lanes: 3, span: 1 },
      { id: "early", lane: 1, lanes: 3, span: 2 },
      { id: "middle", lane: 1, lanes: 3, span: 1 },
      { id: "third", lane: 2, lanes: 3, span: 1 },
      { id: "late", lane: 1, lanes: 3, span: 2 },
    ]);
  });

  it("adds days across month boundaries", () => {
    const next = addDays(new Date(2026, 8, 30), 1);
    expect(next.getMonth()).toBe(9);
    expect(next.getDate()).toBe(1);
  });
});
