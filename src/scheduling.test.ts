import { describe, expect, it } from "vitest";
import type { MeetingProposal } from "./domain";
import {
  boundedRange, clampMeetingMinutes, MAX_MEETING_MINUTES, MAX_SCHEDULE_RANGE_DAYS, MIN_MEETING_MINUTES,
  AFTERNOONS, ANY_TIME, MIDDAY, MORNINGS, planChatAvailability, planMeeting, planTimeSearch, SCHEDULE_LOOKAHEAD_DAYS, shiftedRange,
  spreadAcrossDays, windowsWithin,
} from "./scheduling";

const now = new Date("2026-09-29T12:00:00Z");
const day = 86_400_000;

function meeting(overrides: Partial<MeetingProposal> = {}): MeetingProposal {
  return {
    type: "meeting", intent: "schedule", title: "Kickoff", participants: [], location: null, rawTimeLanguage: "soon",
    normalizedStart: null, normalizedEnd: null, searchRangeStart: null, searchRangeEnd: null, durationMinutes: null,
    timeZone: "America/New_York", confidence: 0.9, evidence: { sourceMessageId: "m1", excerpt: "soon" }, ...overrides,
  };
}

describe("meeting scheduling plans", () => {
  it("checks an exact future time and takes its duration from the time itself", () => {
    const plan = planMeeting(meeting({ normalizedStart: "2026-10-01T19:00:00Z", normalizedEnd: "2026-10-01T19:45:00Z", durationMinutes: 30 }), now, 30);
    expect(plan).toEqual({ query: { kind: "specific", start: "2026-10-01T19:00:00.000Z", end: "2026-10-01T19:45:00.000Z" }, durationMinutes: 45, timeZoneAssumed: false });
  });

  it("reports an exact time that has already passed", () => {
    expect(planMeeting(meeting({ normalizedStart: "2026-09-28T19:00:00Z", normalizedEnd: "2026-09-28T19:30:00Z" }), now, 30).query).toBeNull();
  });

  it("searches a proposed range from now at the earliest and for at most the maximum span", () => {
    const plan = planMeeting(meeting({ searchRangeStart: "2026-09-20T00:00:00Z", searchRangeEnd: "2026-12-01T00:00:00Z", durationMinutes: 60 }), now, 30);
    expect(plan.query).toEqual({ kind: "range", start: now.toISOString(), end: new Date(now.getTime() + MAX_SCHEDULE_RANGE_DAYS * day).toISOString() });
    expect(plan.durationMinutes).toBe(60);
    expect(planMeeting(meeting({ searchRangeStart: "2026-09-01T00:00:00Z", searchRangeEnd: "2026-09-02T00:00:00Z" }), now, 30).query).toBeNull();
  });

  it("falls back to the next week and the user's timezone for a vague meeting", () => {
    const plan = planMeeting(meeting({ timeZone: null }), now, 25);
    expect(plan.query).toEqual({ kind: "range", start: now.toISOString(), end: new Date(now.getTime() + SCHEDULE_LOOKAHEAD_DAYS * day).toISOString() });
    expect(plan.durationMinutes).toBe(25);
    expect(plan.timeZoneAssumed).toBe(true);
  });

  it("bounds meeting length and search spans at, below, and above the limits", () => {
    expect(clampMeetingMinutes(MIN_MEETING_MINUTES - 1, 30)).toBe(MIN_MEETING_MINUTES);
    expect(clampMeetingMinutes(MIN_MEETING_MINUTES, 30)).toBe(MIN_MEETING_MINUTES);
    expect(clampMeetingMinutes(MAX_MEETING_MINUTES, 30)).toBe(MAX_MEETING_MINUTES);
    expect(clampMeetingMinutes(MAX_MEETING_MINUTES + 1, 30)).toBe(MAX_MEETING_MINUTES);
    expect(clampMeetingMinutes(null, 30)).toBe(30);
    const span = (days: number) => boundedRange(now, new Date(now.getTime() + days * day), now)!;
    const length = (query: { start: string; end: string }) => (Date.parse(query.end) - Date.parse(query.start)) / day;
    expect(length(span(MAX_SCHEDULE_RANGE_DAYS - 1))).toBe(MAX_SCHEDULE_RANGE_DAYS - 1);
    expect(length(span(MAX_SCHEDULE_RANGE_DAYS))).toBe(MAX_SCHEDULE_RANGE_DAYS);
    expect(length(span(MAX_SCHEDULE_RANGE_DAYS + 1))).toBe(MAX_SCHEDULE_RANGE_DAYS);
  });

  it("checks a stated start with no end for its stated or default length", () => {
    const plan = planMeeting(meeting({ normalizedStart: "2026-10-17T22:45:00Z", durationMinutes: 120 }), now, 30);
    expect(plan.query).toEqual({ kind: "specific", start: "2026-10-17T22:45:00.000Z", end: "2026-10-18T00:45:00.000Z" });
    expect(plan.durationMinutes).toBe(120);
    const fallback = planMeeting(meeting({ normalizedStart: "2026-10-17T22:45:00Z" }), now, 30);
    expect(fallback.query).toEqual({ kind: "specific", start: "2026-10-17T22:45:00.000Z", end: "2026-10-17T23:15:00.000Z" });
    expect(planMeeting(meeting({ normalizedStart: "2026-09-28T22:45:00Z" }), now, 30).query).toBeNull();
  });

  it("plans a date-only event as its day, not as a search for open times", () => {
    const plan = planMeeting(meeting({ intent: "attend", eventDate: "2026-11-07" }), now, 30);
    expect(plan).toEqual({ query: null, day: "2026-11-07", durationMinutes: 30, timeZoneAssumed: false, announced: true });
    // Today still counts; yesterday has passed.
    expect(planMeeting(meeting({ eventDate: "2026-09-29" }), now, 30).day).toBe("2026-09-29");
    const past = planMeeting(meeting({ eventDate: "2026-09-28" }), now, 30);
    expect(past.query).toBeNull();
    expect(past.day).toBeUndefined();
    // A start time is more specific than a day.
    expect(planMeeting(meeting({ normalizedStart: "2026-11-07T23:00:00Z", eventDate: "2026-11-07" }), now, 30).query?.kind).toBe("specific");
  });

  it("never searches the next week for an announced event with no date", () => {
    const plan = planMeeting(meeting({ intent: "attend" }), now, 30);
    expect(plan.query).toBeNull();
    expect(plan.undated).toBe(true);
    expect(plan.announced).toBe(true);
    // A meeting still to be arranged keeps the next-week search.
    expect(planMeeting(meeting(), now, 30).query?.kind).toBe("range");
  });

  it("plans a chat availability request from the calendar rather than the model's words", () => {
    const plan = planChatAvailability({ rangeStart: "2026-10-05T13:00:00Z", rangeEnd: "2026-10-09T21:00:00Z", durationMinutes: null }, now, 30);
    expect(plan).toEqual({ query: { kind: "range", start: "2026-10-05T13:00:00.000Z", end: "2026-10-09T21:00:00.000Z" }, durationMinutes: 30, timeZoneAssumed: false });
  });
});

describe("adjusting open-time searches", () => {
  const windows = [
    { weekday: 1, start: "09:00", end: "17:00" },
    { weekday: 2, start: "07:30", end: "11:00" },
    { weekday: 3, start: "13:00", end: "18:00" },
    { weekday: 4, start: "12:00", end: "12:30" },
  ];

  it("keeps working windows as set for any time", () => {
    expect(windowsWithin(windows, ANY_TIME)).toEqual(windows);
  });

  it("clips working windows to the morning, dropping windows that start at or after midday", () => {
    expect(windowsWithin(windows, { ...ANY_TIME, ...MORNINGS })).toEqual([
      { weekday: 1, start: "09:00", end: MIDDAY },
      { weekday: 2, start: "07:30", end: "11:00" },
    ]);
  });

  it("clips working windows to the afternoon, dropping windows that end by midday", () => {
    expect(windowsWithin(windows, { ...ANY_TIME, ...AFTERNOONS })).toEqual([
      { weekday: 1, start: MIDDAY, end: "17:00" },
      { weekday: 3, start: "13:00", end: "18:00" },
      { weekday: 4, start: "12:00", end: "12:30" },
    ]);
  });

  it("keeps only the allowed weekdays, within any hours set", () => {
    expect(windowsWithin(windows, { weekdays: [1, 3], dayStart: "15:00", dayEnd: "17:30" })).toEqual([
      { weekday: 1, start: "15:00", end: "17:00" },
      { weekday: 3, start: "15:00", end: "17:30" },
    ]);
    // Hours that miss every window leave nothing to search.
    expect(windowsWithin(windows, { weekdays: [2], dayStart: "11:00", dayEnd: null })).toEqual([]);
  });

  it("plans the search Find Times read from a conversation", () => {
    const search = {
      rangeStart: "2026-10-19T00:00:00-04:00", rangeEnd: "2026-10-26T00:00:00-04:00", durationMinutes: 45,
      weekdays: [4, 5], dayStart: "13:00", dayEnd: null, reason: "Matt asked for Thursday or Friday afternoon.",
    };
    expect(planTimeSearch(search, now, 30)).toEqual({
      query: { kind: "range", start: "2026-10-19T04:00:00.000Z", end: "2026-10-26T04:00:00.000Z" },
      durationMinutes: 45,
      timeZoneAssumed: false,
      bounds: { weekdays: [4, 5], dayStart: "13:00", dayEnd: null },
    });
    // No stated length uses the default; a range already under way starts now.
    const underway = planTimeSearch({ ...search, durationMinutes: null, rangeStart: "2026-09-28T00:00:00Z", rangeEnd: "2026-10-02T00:00:00Z" }, now, 30);
    expect(underway.durationMinutes).toBe(30);
    expect(underway.query).toEqual({ kind: "range", start: now.toISOString(), end: "2026-10-02T00:00:00.000Z" });
    // A range that has since passed falls back to the coming week.
    const passed = planTimeSearch({ ...search, rangeStart: "2026-09-01T00:00:00Z", rangeEnd: "2026-09-02T00:00:00Z" }, now, 30);
    expect(passed.query).toEqual({ kind: "range", start: now.toISOString(), end: new Date(now.getTime() + SCHEDULE_LOOKAHEAD_DAYS * 86_400_000).toISOString() });
  });

  it("shifts a range by its own length, and caps the step at the longest range", () => {
    const week = { kind: "range" as const, start: "2026-10-19T04:00:00.000Z", end: "2026-10-26T04:00:00.000Z" };
    expect(shiftedRange(week, 1)).toEqual({ kind: "range", start: "2026-10-26T04:00:00.000Z", end: "2026-11-02T04:00:00.000Z" });
    expect(shiftedRange(week, -1)).toEqual({ kind: "range", start: "2026-10-12T04:00:00.000Z", end: "2026-10-19T04:00:00.000Z" });
    const long = { kind: "range" as const, start: "2026-10-01T00:00:00.000Z", end: "2026-12-01T00:00:00.000Z" };
    const next = shiftedRange(long, 1);
    expect(Date.parse(next.end) - Date.parse(next.start)).toBe(MAX_SCHEDULE_RANGE_DAYS * 86_400_000);
  });

  it("puts each day's first open time before any day's second, by day in the given time zone", () => {
    const slot = (start: string) => ({ start });
    // 03:00Z on the 20th is still the 19th in New York.
    const candidates = [slot("2026-10-19T13:00:00Z"), slot("2026-10-19T14:00:00Z"), slot("2026-10-20T03:00:00Z"), slot("2026-10-20T13:00:00Z")];
    expect(spreadAcrossDays(candidates, "America/New_York").map((candidate) => candidate.start)).toEqual([
      "2026-10-19T13:00:00Z", "2026-10-20T13:00:00Z", "2026-10-19T14:00:00Z", "2026-10-20T03:00:00Z",
    ]);
    expect(spreadAcrossDays(candidates, "UTC").map((candidate) => candidate.start)).toEqual([
      "2026-10-19T13:00:00Z", "2026-10-20T03:00:00Z", "2026-10-19T14:00:00Z", "2026-10-20T13:00:00Z",
    ]);
  });
});
