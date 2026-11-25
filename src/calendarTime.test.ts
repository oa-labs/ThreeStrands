import { describe, expect, it } from "vitest";
import { SIDE_BY_SIDE_START_MINUTES, STACK_INSET, addDays, eventColumnBox, eventDayBounds, formatUpcomingDate, friendlyZoneName, layOutDayEvents, localDayStart, occursOnDay, startOfWeek, timeZoneChoices } from "./calendarTime";
import type { ScheduleEvent } from "./domain";

function timedEvent(id: string, start: string, end: string): ScheduleEvent {
  return { id, accountId: "a@example.com", title: id, start, end, allDay: false };
}

describe("formatUpcomingDate", () => {
  // Wednesday, 2026-10-14, mid-morning.
  const now = new Date(2026, 9, 14, 10, 0);
  const on = (year: number, month: number, day: number, hour = 15) => timedEvent("e", new Date(year, month, day, hour).toISOString(), new Date(year, month, day, hour + 1).toISOString());
  const weekday = (date: Date) => new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(date);

  it("says Today and Tomorrow, including for a meeting already under way", () => {
    expect(formatUpcomingDate(on(2026, 9, 14), now)).toBe("Today");
    expect(formatUpcomingDate(on(2026, 9, 14, 8), now)).toBe("Today");
    expect(formatUpcomingDate(on(2026, 9, 15), now)).toBe("Tomorrow");
    expect(formatUpcomingDate({ id: "a", accountId: "a@example.com", title: "a", allDay: true, start: "2026-10-15", end: "2026-10-16" }, now)).toBe("Tomorrow");
  });

  it("names the weekday within the coming week, then the month and day", () => {
    expect(formatUpcomingDate(on(2026, 9, 16), now)).toBe(weekday(new Date(2026, 9, 16)));
    expect(formatUpcomingDate(on(2026, 9, 20), now)).toBe(weekday(new Date(2026, 9, 20)));
    expect(formatUpcomingDate(on(2026, 9, 21), now)).toBe(new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(new Date(2026, 9, 21)));
  });

  it("adds the year only for another year", () => {
    expect(formatUpcomingDate(on(2027, 0, 5), now)).toBe(new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" }).format(new Date(2027, 0, 5)));
  });
});

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

  it("includes every local date of a multi-day all-day event, excluding its end date", () => {
    const allDay: ScheduleEvent = {
      id: "offsite", accountId: "a@example.com", title: "Offsite",
      start: "2026-09-21", end: "2026-09-24", allDay: true,
    };
    expect(occursOnDay(allDay, new Date(2026, 8, 20))).toBe(false);
    for (const date of [21, 22, 23]) expect(occursOnDay(allDay, new Date(2026, 8, date))).toBe(true);
    expect(occursOnDay(allDay, new Date(2026, 8, 24))).toBe(false);
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

  describe("stacking events that start well inside another", () => {
    const day = new Date(2026, 8, 22);
    const shape = (events: ScheduleEvent[]) => layOutDayEvents(events, day).map(({ event, lane, lanes, span, layer }) => ({ id: event.id, lane, lanes, span, layer }));

    it("keeps a long event full width and stacks two later starters side by side on top", () => {
      expect(shape([
        timedEvent("cs", "2026-09-22T09:30:00", "2026-09-22T11:35:00"),
        timedEvent("jblc", "2026-09-22T11:30:00", "2026-09-22T12:00:00"),
        timedEvent("josh", "2026-09-22T11:30:00", "2026-09-22T12:30:00"),
      ])).toEqual([
        { id: "cs", lane: 0, lanes: 1, span: 1, layer: 0 },
        { id: "josh", lane: 0, lanes: 2, span: 1, layer: 1 },
        { id: "jblc", lane: 1, lanes: 2, span: 1, layer: 1 },
      ]);
      expect(eventColumnBox({ lane: 0, lanes: 1, span: 1, layer: 0 })).toEqual({ left: 0, width: 1 });
      const box = eventColumnBox({ lane: 1, lanes: 2, span: 1, layer: 1 });
      expect(box.left).toBeCloseTo(STACK_INSET + (1 - STACK_INSET) / 2);
      expect(box.left + box.width).toBeCloseTo(1);
    });

    it("still puts a long and a short event that start together side by side", () => {
      expect(shape([
        timedEvent("long", "2026-09-22T16:00:00", "2026-09-22T20:00:00"),
        timedEvent("short", "2026-09-22T16:00:00", "2026-09-22T16:30:00"),
      ])).toEqual([
        { id: "long", lane: 0, lanes: 2, span: 1, layer: 0 },
        { id: "short", lane: 1, lanes: 2, span: 1, layer: 0 },
      ]);
    });

    it("treats a start exactly at the threshold as side by side and one minute later as stacked", () => {
      const base = timedEvent("base", "2026-09-22T09:00:00", "2026-09-22T12:00:00");
      const at = (minutes: number) => `2026-09-22T09:${String(minutes).padStart(2, "0")}:00`;
      expect(SIDE_BY_SIDE_START_MINUTES).toBe(30);
      expect(shape([base, timedEvent("e", at(30), "2026-09-22T10:00:00")]).map((entry) => entry.layer)).toEqual([0, 0]);
      expect(shape([base, timedEvent("e", at(31), "2026-09-22T10:00:00")]).map((entry) => entry.layer)).toEqual([0, 1]);
    });

    it("places three events starting within the threshold in three lanes", () => {
      expect(shape([
        timedEvent("a", "2026-09-22T09:00:00", "2026-09-22T10:00:00"),
        timedEvent("b", "2026-09-22T09:10:00", "2026-09-22T10:00:00"),
        timedEvent("c", "2026-09-22T09:20:00", "2026-09-22T10:00:00"),
      ]).map(({ lane, lanes, layer }) => [lane, lanes, layer])).toEqual([[0, 3, 0], [1, 3, 0], [2, 3, 0]]);
    });

    it("cascades a chain of later starters one layer at a time", () => {
      expect(shape([
        timedEvent("a", "2026-09-22T09:00:00", "2026-09-22T13:00:00"),
        timedEvent("b", "2026-09-22T10:00:00", "2026-09-22T13:00:00"),
        timedEvent("c", "2026-09-22T11:00:00", "2026-09-22T13:00:00"),
      ]).map(({ id, lanes, layer }) => [id, lanes, layer])).toEqual([["a", 1, 0], ["b", 1, 1], ["c", 1, 2]]);
    });

    it("lays out what follows a stack from the base again", () => {
      expect(shape([
        timedEvent("a", "2026-09-22T09:00:00", "2026-09-22T11:00:00"),
        timedEvent("b", "2026-09-22T10:00:00", "2026-09-22T10:30:00"),
        timedEvent("later", "2026-09-22T11:00:00", "2026-09-22T12:00:00"),
      ]).map(({ id, lanes, layer }) => [id, lanes, layer])).toEqual([["a", 1, 0], ["b", 1, 1], ["later", 1, 0]]);
    });
  });

  it("gives the instant local midnight begins a date, in summer and winter", () => {
    // Tests run in America/New_York.
    expect(localDayStart("2026-11-16")).toBe("2026-11-16T05:00:00.000Z");
    expect(localDayStart("2026-07-01")).toBe("2026-07-01T04:00:00.000Z");
    // Clocks change at 2am, after that day's midnight.
    expect(localDayStart("2026-03-08")).toBe("2026-03-08T05:00:00.000Z");
  });

  it("adds days across month boundaries", () => {
    const next = addDays(new Date(2026, 8, 30), 1);
    expect(next.getMonth()).toBe(9);
    expect(next.getDate()).toBe(1);
  });
});

describe("timezone names", () => {
  it("uses names people recognize, the same in summer and winter, without bare offsets", () => {
    for (const at of [new Date("2026-07-01T12:00:00Z"), new Date("2026-12-01T12:00:00Z")]) {
      expect(friendlyZoneName("America/New_York", at)).toBe("Eastern Time (ET)");
      expect(friendlyZoneName("America/Los_Angeles", at)).toBe("Pacific Time (PT)");
    }
    expect(friendlyZoneName("Europe/Paris")).not.toMatch(/GMT[+-]/);
    expect(friendlyZoneName("Not/AZone")).toBe("Not/AZone");
  });

  it("lists common zones first and every other zone once by city, keeping an unlisted current zone", () => {
    const { common, all } = timeZoneChoices("Etc/GMT+5");
    expect(common[0]).toEqual({ value: "America/New_York", label: "Eastern Time (ET)" });
    expect(all.some((choice) => choice.value === "America/New_York")).toBe(false);
    expect(all.find((choice) => choice.value === "America/Detroit")?.label).toBe("Detroit — Eastern Time (ET)");
    expect(all.some((choice) => choice.value === "Etc/GMT+5")).toBe(true);
    const values = [...common, ...all].map((choice) => choice.value);
    expect(new Set(values).size).toBe(values.length);
  });
});
