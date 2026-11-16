import { describe, expect, it } from "vitest";
import type { CalendarOption, ScheduleEvent } from "./domain";
import { blockRequest, bookedFor, nextFreeSlot } from "./todayBooking";

const at = (hours: number, minutes = 0) => new Date(2026, 9, 28, hours, minutes);
const busy = (id: string, start: Date, end: Date, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent =>
  ({ id, accountId: "cal@example.com", calendarId: "primary", title: id, allDay: false, start: start.toISOString(), end: end.toISOString(), ...overrides });
const calendar: CalendarOption = { id: "primary", accountId: "cal@example.com", name: "Work", primary: true, selected: true, writable: true };

describe("nextFreeSlot", () => {
  it("rounds now up to the next quarter hour, keeping an exact quarter hour", () => {
    expect(nextFreeSlot([], at(10, 7))).toEqual(at(10, 15));
    expect(nextFreeSlot([], at(10, 15))).toEqual(at(10, 15));
    expect(nextFreeSlot([], new Date(2026, 9, 28, 10, 15, 30))).toEqual(at(10, 30));
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
    expect(nextFreeSlot(events, at(10, 7))).toEqual(at(11, 30));
    // A block may end exactly where an event starts.
    expect(nextFreeSlot([busy("review", at(11, 0), at(11, 30))], at(10, 30))).toEqual(at(10, 30));
  });

  it("returns null when no half hour is left before midnight", () => {
    expect(nextFreeSlot([], at(23, 31))).toBeNull();
    expect(nextFreeSlot([], at(23, 30))).toEqual(at(23, 30));
    expect(nextFreeSlot([busy("late", at(20), at(23, 50))], at(19, 50))).toBeNull();
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
  });
});
