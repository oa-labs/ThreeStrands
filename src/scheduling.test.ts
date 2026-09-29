import { describe, expect, it } from "vitest";
import type { MeetingProposal } from "./domain";
import {
  boundedRange, clampMeetingMinutes, MAX_MEETING_MINUTES, MAX_SCHEDULE_RANGE_DAYS, MIN_MEETING_MINUTES,
  planChatAvailability, planMeeting, SCHEDULE_LOOKAHEAD_DAYS,
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

  it("plans a chat availability request from the calendar rather than the model's words", () => {
    const plan = planChatAvailability({ rangeStart: "2026-10-05T13:00:00Z", rangeEnd: "2026-10-09T21:00:00Z", durationMinutes: null }, now, 30);
    expect(plan).toEqual({ query: { kind: "range", start: "2026-10-05T13:00:00.000Z", end: "2026-10-09T21:00:00.000Z" }, durationMinutes: 30, timeZoneAssumed: false });
  });
});
