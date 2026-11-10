import { describe, expect, it } from "vitest";
import type { ScheduleEvent } from "./domain";
import { meetingAttendees, meetingDate, meetingEntry, meetingEventReference, meetingTime } from "./meetingNoteEntry";
import { dailyNoteName } from "./noteDates";

const event = (overrides: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
  id: "evt-1", accountId: "you@example.com", calendarId: "work", title: "Sync", allDay: false,
  start: new Date(2026, 10, 10, 23, 30).toISOString(), end: new Date(2026, 10, 11, 0, 30).toISOString(), ...overrides,
});

describe("a meeting's entry from its event", () => {
  it("links the occurrence on its calendar, with its series, and nothing without a calendar", () => {
    expect(meetingEventReference(event({ seriesId: "series" }))).toEqual({ version: 1, kind: "event", id: "evt-1", account: "you@example.com", calendar: "work", series: "series" });
    expect(meetingEventReference(event({ seriesId: "evt-1" }))).toEqual({ version: 1, kind: "event", id: "evt-1", account: "you@example.com", calendar: "work" });
    expect(meetingEventReference(event({ calendarId: undefined }))).toBeNull();
  });

  it("goes in the note for its local start day, with its start time or All day", () => {
    expect(meetingDate(event())).toBe("2026-11-10");
    expect(meetingTime(event())).toMatch(/^11:30\s?pm$/i);
    const allDay = event({ allDay: true, start: "2026-11-12", end: "2026-11-13" });
    expect(meetingDate(allDay)).toBe("2026-11-12");
    expect(meetingTime(allDay)).toBe("All day");
  });

  it("lists the other people once each, without the user's own addresses", () => {
    const attendees = ["Pete@Example.com", "you@example.com", "pete@example.com", "alias@example.com", "not an address", "ann@example.com"];
    expect(meetingAttendees(event({ attendees }), ["You@example.com", "alias@example.com"])).toEqual(["pete@example.com", "ann@example.com"]);
    expect(meetingAttendees(event({ attendees: undefined }), [])).toEqual([]);
  });

  it("labels attendees by name, and keeps only saved contacts' ids", () => {
    const entry = meetingEntry(event(), {
      name: "Weekly sync", attendees: ["Pete@example.com", "ann@example.com", "mac@example.com"], linkNote: true,
      names: new Map([["pete@example.com", "Pete B"]]),
      contactIds: { "pete@example.com": "c-pete", "mac@example.com": "derived:mac:1" },
    });
    expect(entry).toMatchObject({ time: meetingTime(event()), name: "Weekly sync", linkNote: true });
    expect(entry.attendees).toEqual([
      { label: "Pete B", reference: { version: 1, kind: "contact", id: "pete@example.com", contact: "c-pete" } },
      { label: "ann@example.com", reference: { version: 1, kind: "contact", id: "ann@example.com" } },
      { label: "mac@example.com", reference: { version: 1, kind: "contact", id: "mac@example.com" } },
    ]);
    expect(() => meetingEntry(event({ calendarId: undefined }), { name: "", attendees: [], names: new Map(), contactIds: {}, linkNote: false })).toThrow();
  });
});

describe("naming a day's note in a notice", () => {
  const now = new Date(2026, 10, 10, 9);
  it.each([["2026-11-10", "today's note"], ["2026-11-11", "tomorrow's note"], ["2026-11-09", "yesterday's note"]])("calls %s %s", (date, name) => expect(dailyNoteName(date, now)).toBe(name));
  it("names other days by date, with the year only when it differs", () => {
    expect(dailyNoteName("2026-11-17", now)).toBe("the note for Tuesday, November 17");
    expect(dailyNoteName("2027-01-04", now)).toBe("the note for Monday, January 4, 2027");
  });
});
