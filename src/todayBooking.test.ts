import { describe, expect, it } from "vitest";
import type { CalendarOption, ScheduleEvent } from "./domain";
import { blockRequest, bookedFor, nextFreeSlot, withinWorkingHours } from "./todayBooking";
import type { WorkingHours } from "./todayInsights";

const at = (hours: number, minutes = 0) => new Date(2026, 9, 28, hours, minutes);
const busy = (id: string, start: Date, end: Date, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent =>
  ({ id, accountId: "cal@example.com", calendarId: "primary", title: id, allDay: false, start: start.toISOString(), end: end.toISOString(), ...overrides });
const calendar: CalendarOption = { id: "primary", accountId: "cal@example.com", name: "Work", primary: true, selected: true, writable: true };

const zone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
/** Wednesday 2026-10-28 is weekday 3. */
const hours = (...windows: [string, string][]): WorkingHours => ({ timeZone: zone, workingWindows: windows.map(([start, end]) => ({ weekday: 3, start, end })) });
const nineToFive = hours(["09:00", "17:00"]);

describe("nextFreeSlot", () => {
  it("rounds now up to the next quarter hour, keeping an exact quarter hour", () => {
    expect(nextFreeSlot([], nineToFive, at(10, 7))).toEqual(at(10, 15));
    expect(nextFreeSlot([], nineToFive, at(10, 15))).toEqual(at(10, 15));
    expect(nextFreeSlot([], nineToFive, new Date(2026, 9, 28, 10, 15, 30))).toEqual(at(10, 30));
  });

  it("skips attending timed events, but not declined, all-day, or working-location ones", () => {
    const events = [
      busy("standup", at(10, 0), at(10, 45)),
      busy("review", at(11, 0), at(11, 30)),
      busy("declined", at(10, 45), at(11, 0), { responseStatus: "declined" }),
      busy("offsite", at(0), at(23, 59), { allDay: true }),
      busy("home", at(9), at(17), { eventType: "workingLocation" }),
    ];
    // 10:45–11:15 would overlap the review, so the next half hour starts at 11:30.
    expect(nextFreeSlot(events, nineToFive, at(10, 7))).toEqual(at(11, 30));
    // A block may end exactly where an event starts.
    expect(nextFreeSlot([busy("review", at(11, 0), at(11, 30))], nineToFive, at(10, 30))).toEqual(at(10, 30));
  });

  it("starts at the beginning of working hours before they open, and fits a block that ends exactly at their close", () => {
    expect(nextFreeSlot([], nineToFive, at(7, 10))).toEqual(at(9));
    expect(nextFreeSlot([], nineToFive, at(16, 30))).toEqual(at(16, 30));
    // 16:31 rounds to 16:45, and 16:45–17:15 runs past five.
    expect(nextFreeSlot([], nineToFive, at(16, 31))).toBeNull();
  });

  it("returns null after working hours and on a day without them", () => {
    expect(nextFreeSlot([], nineToFive, at(17))).toBeNull();
    expect(nextFreeSlot([], nineToFive, at(20))).toBeNull();
    expect(nextFreeSlot([], { timeZone: zone, workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }] }, at(10))).toBeNull();
    expect(nextFreeSlot([busy("all-afternoon", at(13), at(17))], nineToFive, at(12, 45))).toBeNull();
  });

  it("moves to the next window of a split day when the first has no half hour left", () => {
    const split = hours(["09:00", "12:00"], ["13:00", "17:00"]);
    expect(nextFreeSlot([], split, at(11, 40))).toEqual(at(13));
    expect(nextFreeSlot([], split, at(11, 30))).toEqual(at(11, 30));
    expect(nextFreeSlot([busy("lunch-run", at(13), at(14, 10))], split, at(11, 45))).toEqual(at(14, 15));
  });

  it("says free time is unknown without the schedule or the working hours", () => {
    expect(nextFreeSlot(null, nineToFive, at(10))).toBe("unknown");
    expect(nextFreeSlot([], null, at(10))).toBe("unknown");
  });
});

describe("withinWorkingHours", () => {
  it("is true only for a block wholly inside a working window, and null when hours are unknown", () => {
    expect(withinWorkingHours(at(9), nineToFive)).toBe(true);
    expect(withinWorkingHours(at(16, 30), nineToFive)).toBe(true);
    expect(withinWorkingHours(at(16, 45), nineToFive)).toBe(false);
    expect(withinWorkingHours(at(8, 45), nineToFive)).toBe(false);
    expect(withinWorkingHours(at(20), nineToFive)).toBe(false);
    expect(withinWorkingHours(at(9), null)).toBeNull();
  });
});

describe("blockRequest", () => {
  it("tags a task's block with its task, a note task by its note reference, and a check-in with none", () => {
    expect(blockRequest({ title: " Draft ", sourceTaskId: "note:daily:2026-10-28:m1", accountId: null }, calendar, at(14))).toEqual({
      accountId: "cal@example.com", calendarId: "primary", title: "Draft", start: at(14).toISOString(), end: at(14, 30).toISOString(),
      description: "", attendees: [], sourceTaskId: "note:daily:2026-10-28:m1",
    });
    expect(blockRequest({ title: "Check in with Ada", sourceTaskId: null, accountId: null }, calendar, at(14))).not.toHaveProperty("sourceTaskId");
  });
});

describe("bookedFor", () => {
  it("finds the next block for the task still to end, else the latest", () => {
    const events = [busy("a", at(9), at(9, 30), { sourceTaskId: "t" }), busy("b", at(15), at(15, 30), { sourceTaskId: "t" }), busy("c", at(12), at(12, 30))];
    expect(bookedFor("t", events, at(10))?.id).toBe("b");
    expect(bookedFor("t", events, at(16))?.id).toBe("b");
    expect(bookedFor(null, events, at(10))).toBeNull();
    // A note task's block is found by its note reference, as Calendar resolves it.
    expect(bookedFor("note:daily:2026-10-28:m1", [busy("n", at(14), at(14, 30), { sourceTaskId: " note:daily:2026-10-28:m1 " })], at(10))?.id).toBe("n");
  });
});
