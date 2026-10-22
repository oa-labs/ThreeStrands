import { describe, expect, it } from "vitest";
import type { ScheduleEvent } from "./domain";
import { findCalendarMatch, findDayMatch, locationsMatch, SAME_EVENT_START_TOLERANCE_MINUTES, titlesMatch } from "./eventMatching";

const START = "2026-11-17T21:00:00.000Z";
const minutes = (iso: string, count: number) => new Date(Date.parse(iso) + count * 60_000).toISOString();
const slot = { start: START, end: minutes(START, 120) };
const event = (overrides: Partial<ScheduleEvent>): ScheduleEvent => ({
  id: "event", accountId: "you@example.com", title: "Untitled", start: START, end: minutes(START, 120), allDay: false, ...overrides,
});

describe("titlesMatch", () => {
  it.each([
    ["Entrepreneur Day 2026", "Entrepreneur Day 2026"],
    ["You're invited to Entrepreneur Day 2026", "Entrepreneur Day 2026"],
    ["Entrepreneur Day", "Entrepreneur Day 2026"],
    ["Invitation: Quarterly Board Review", "quarterly board review"],
    ["Café Night — Fall Edition", "Cafe Night: Fall Edition"],
  ])("treats %j and %j as the same event", (left, right) => {
    expect(titlesMatch(left, right)).toBe(true);
    expect(titlesMatch(right, left)).toBe(true);
  });

  it.each([
    ["Meeting", "Team Meeting"],
    ["Lunch", "Lunch with Sam"],
    ["Entrepreneur Day 2026", "Premier Labs Bi-Weekly Member Meeting"],
    ["Design Review", "Design Systems Office Hours Review Session"],
    ["You're invited", "Invitation"],
  ])("keeps %j and %j apart", (left, right) => {
    expect(titlesMatch(left, right)).toBe(false);
  });
});

describe("locationsMatch", () => {
  it("matches the same address written differently", () => {
    expect(locationsMatch("Energy Innovation Center - 1435 Bedford Ave, Pittsburgh, PA 15219, USA", "1435 Bedford Ave, Pittsburgh, PA 15219")).toBe(true);
    expect(locationsMatch("Room 4B, 200 Main Street", "200 Main Street, Room 4B")).toBe(true);
  });

  it("never matches on a city, street, or postal code alone, or on a missing place", () => {
    expect(locationsMatch("Pittsburgh, PA, USA", "Carnegie Library, Pittsburgh, PA, USA 15213 Forbes")).toBe(false);
    expect(locationsMatch("100 Liberty Ave, Pittsburgh, PA", "1435 Liberty Ave, Pittsburgh, PA")).toBe(false);
    expect(locationsMatch("100 Liberty Ave, Pittsburgh, PA 15222", "200 Smithfield St, Pittsburgh, PA 15222")).toBe(false);
    // Without a number to agree on, a venue name alone stays uncertain.
    expect(locationsMatch("Carnegie Music Hall", "Carnegie Music Hall Oakland")).toBe(false);
    expect(locationsMatch(null, "1435 Bedford Ave")).toBe(false);
    expect(locationsMatch("  ", "1435 Bedford Ave")).toBe(false);
  });
});

describe("findCalendarMatch", () => {
  const identity = { title: "Entrepreneur Day 2026", location: "Energy Innovation Center - 1435 Bedford Ave, Pittsburgh, PA 15219, USA" };

  it("finds the same event among other overlapping ones", () => {
    const standing = event({ id: "standing", title: "Premier Labs Bi-Weekly Member Meeting", start: minutes(START, -30), end: minutes(START, 30) });
    const same = event({ id: "same", title: "Entrepreneur Day 2026", end: minutes(START, 90) });
    expect(findCalendarMatch(identity, slot, [standing, same])).toEqual({ kind: "same", event: same });
  });

  it("allows a start entered a little differently, up to the tolerance", () => {
    const at = (offset: number) => event({ start: minutes(START, offset), title: "Entrepreneur Day" });
    expect(findCalendarMatch(identity, slot, [at(SAME_EVENT_START_TOLERANCE_MINUTES - 1)])?.kind).toBe("same");
    expect(findCalendarMatch(identity, slot, [at(SAME_EVENT_START_TOLERANCE_MINUTES)])?.kind).toBe("same");
    expect(findCalendarMatch(identity, slot, [at(-SAME_EVENT_START_TOLERANCE_MINUTES)])?.kind).toBe("same");
    expect(findCalendarMatch(identity, slot, [at(SAME_EVENT_START_TOLERANCE_MINUTES + 1)])).toBeNull();
  });

  it("calls a shared place at the same time a possibility, not the same event", () => {
    const venue = event({ title: "Pitch Practice", location: "1435 Bedford Ave, Pittsburgh, PA 15219" });
    expect(findCalendarMatch(identity, slot, [venue])).toEqual({ kind: "maybe", event: venue });
    expect(findCalendarMatch({ title: identity.title }, slot, [venue])).toBeNull();
  });

  it("prefers a title match over a place match, then the closer start", () => {
    const venue = event({ id: "venue", title: "Pitch Practice", location: identity.location });
    const later = event({ id: "later", title: "Entrepreneur Day", start: minutes(START, 10) });
    const exact = event({ id: "exact", title: "Entrepreneur Day 2026" });
    expect(findCalendarMatch(identity, slot, [venue, later, exact])?.event.id).toBe("exact");
    expect(findCalendarMatch(identity, slot, [venue, later])?.event.id).toBe("later");
  });

  it("ignores all-day events and unrelated meetings at the same time", () => {
    const allDay = event({ title: "Entrepreneur Day 2026", allDay: true, start: "2026-11-17", end: "2026-11-18" });
    const other = event({ title: "Weekly sync", location: "Zoom" });
    expect(findCalendarMatch(identity, slot, [allDay, other])).toBeNull();
  });
});

describe("events added from the same email", () => {
  const identity = { title: "Entrepreneur Day 2026", sourceMessageId: "message-1" };
  const tagged = (overrides: Partial<ScheduleEvent>) => event({ sourceMessageId: "message-1", ...overrides });

  it("recognizes the event after its time or its title was changed when adding it", () => {
    const moved = tagged({ id: "moved", title: "Entrepreneur Day", start: minutes(START, 60), end: minutes(START, 180) });
    expect(findCalendarMatch(identity, slot, [moved])).toEqual({ kind: "same", event: moved });
    const renamed = tagged({ id: "renamed", title: "EIC open house" });
    expect(findCalendarMatch(identity, slot, [renamed])).toEqual({ kind: "same", event: renamed });
  });

  it("does not mark a second event from the same email added when only the first was", () => {
    const workshop = tagged({ title: "Founder workshop", start: minutes(START, -240), end: minutes(START, -120) });
    expect(findCalendarMatch(identity, slot, [workshop])).toBeNull();
  });

  it("prefers the tagged event over a look-alike, and ignores events added from other emails", () => {
    const lookAlike = event({ id: "look-alike", title: "Entrepreneur Day 2026" });
    const renamed = tagged({ id: "renamed", title: "EIC open house" });
    expect(findCalendarMatch(identity, slot, [lookAlike, renamed])?.event.id).toBe("renamed");
    const otherEmail = event({ id: "other", title: "EIC open house", sourceMessageId: "message-2" });
    expect(findCalendarMatch(identity, slot, [otherEmail])).toBeNull();
    expect(findCalendarMatch({ title: identity.title }, slot, [renamed])).toBeNull();
  });

  it("finds a tagged event on a date-only suggestion's day under any title", () => {
    const allDay = tagged({ title: "EIC", allDay: true, start: "2026-11-17", end: "2026-11-18" });
    expect(findDayMatch(identity, "2026-11-17", [allDay])).toEqual({ kind: "same", event: allDay });
    expect(findDayMatch(identity, "2026-11-18", [allDay])).toBeNull();
  });
});

describe("findDayMatch", () => {
  const identity = { title: "Pittsburgh Marathon" };

  it("finds an all-day or timed event with the title on that day", () => {
    const allDay = event({ title: "Pittsburgh Marathon 2026", allDay: true, start: "2026-11-07", end: "2026-11-08" });
    expect(findDayMatch(identity, "2026-11-07", [allDay])).toEqual({ kind: "same", event: allDay });
    // Tests run in America/New_York; 11:00Z is the morning of Nov 7 there.
    const timed = event({ title: "Pittsburgh Marathon", start: "2026-11-07T11:00:00.000Z", end: "2026-11-07T17:00:00.000Z" });
    expect(findDayMatch(identity, "2026-11-07", [timed])).toEqual({ kind: "same", event: timed });
  });

  it("does not match a different day or a different title", () => {
    const nextDay = event({ title: "Pittsburgh Marathon", allDay: true, start: "2026-11-08", end: "2026-11-09" });
    const other = event({ title: "Long run", allDay: true, start: "2026-11-07", end: "2026-11-08" });
    expect(findDayMatch(identity, "2026-11-07", [nextDay, other])).toBeNull();
  });
});
