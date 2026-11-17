import { describe, expect, it } from "vitest";
import type { CalendarOption, ScheduleEvent } from "./domain";
import { workingLocationFor } from "./WorkingLocation";

const day = new Date(2026, 10, 18);
const calendar = (accountId: string, id: string, primary: boolean, selected = true): CalendarOption => ({ id, accountId, name: id, primary, selected, writable: true });
const calendars = [calendar("work@x.com", "work", true), calendar("work@x.com", "team", false), calendar("home@x.com", "home", true)];

function location(title: string, accountId: string, calendarId: string, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent {
  return { id: `${accountId}:${calendarId}:${title}`, accountId, calendarId, title, allDay: true, start: "2026-11-18", end: "2026-11-19", eventType: "workingLocation", workingLocationType: "office", ...overrides };
}

describe("workingLocationFor", () => {
  const office = location("Office", "work@x.com", "work");
  const home = location("Home", "home@x.com", "home", { workingLocationType: "home" });

  it("prefers the account in view when accounts disagree", () => {
    expect(workingLocationFor([home, office], day, { accountId: "work@x.com", calendars })?.title).toBe("Office");
    expect(workingLocationFor([office, home], day, { accountId: "home@x.com", calendars })?.title).toBe("Home");
  });

  it("prefers the in-view account's primary calendar over its other calendars", () => {
    const team = location("Lab", "work@x.com", "team");
    expect(workingLocationFor([team, office], day, { accountId: "work@x.com", calendars })?.title).toBe("Office");
    expect(workingLocationFor([team], day, { accountId: "work@x.com", calendars })?.title).toBe("Lab");
  });

  it("follows account order for All accounts, then calendar order", () => {
    expect(workingLocationFor([office, home], day, { accountId: null, calendars, accountOrder: ["home@x.com", "work@x.com"] })?.title).toBe("Home");
    expect(workingLocationFor([home, office], day, { accountId: null, calendars })?.title).toBe("Office");
  });

  it("falls back when the account in view has none", () => {
    expect(workingLocationFor([home], day, { accountId: "work@x.com", calendars })?.title).toBe("Home");
  });

  it("is null without a working location that day on a selected calendar", () => {
    expect(workingLocationFor([], day, { accountId: null, calendars })).toBeNull();
    expect(workingLocationFor([{ ...office, eventType: undefined }], day, { accountId: null, calendars })).toBeNull();
    expect(workingLocationFor([office], new Date(2026, 10, 19), { accountId: null, calendars })).toBeNull();
    expect(workingLocationFor([office], day, { accountId: null, calendars: [calendar("work@x.com", "work", true, false)] })).toBeNull();
  });

  it("returns a location set for part of the day", () => {
    const partial = location("Cafe", "work@x.com", "work", { allDay: false, start: new Date(2026, 10, 18, 13).toISOString(), end: new Date(2026, 10, 18, 17).toISOString(), workingLocationType: "custom" });
    expect(workingLocationFor([partial], day, { accountId: "work@x.com", calendars })).toBe(partial);
  });
});
