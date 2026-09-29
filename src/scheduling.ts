import type { AvailabilityCandidate, ChatAvailability, MeetingProposal } from "./domain";

/** Days searched when a meeting names no usable time or range. */
export const SCHEDULE_LOOKAHEAD_DAYS = 7;
/** Longest range searched for open times in one card. */
export const MAX_SCHEDULE_RANGE_DAYS = 14;
/** Open times offered in a card before "More Times". */
export const SUGGESTED_SLOT_COUNT = 3;
export const MIN_MEETING_MINUTES = 5;
export const MAX_MEETING_MINUTES = 720;

const DAY_MS = 86_400_000;

/**
 * What to check on the calendar: one exact time, or a range to search for
 * open times. `null` means the proposed time or range has already passed.
 */
export type ScheduleQuery =
  | { kind: "specific"; start: string; end: string }
  | { kind: "range"; start: string; end: string };

export type SchedulePlan = {
  query: ScheduleQuery | null;
  durationMinutes: number;
  /** The meeting gave no timezone, so the user's own is assumed. */
  timeZoneAssumed: boolean;
};

export function clampMeetingMinutes(value: number | null | undefined, fallback: number): number {
  const minutes = value ?? fallback;
  if (!Number.isFinite(minutes)) return fallback;
  return Math.min(MAX_MEETING_MINUTES, Math.max(MIN_MEETING_MINUTES, Math.round(minutes)));
}

/** A search range that never starts in the past and never exceeds the maximum. */
export function boundedRange(start: Date, end: Date, now: Date): ScheduleQuery | null {
  const from = Math.max(start.getTime(), now.getTime());
  const to = Math.min(end.getTime(), from + MAX_SCHEDULE_RANGE_DAYS * DAY_MS);
  if (!Number.isFinite(from) || !Number.isFinite(to) || to <= from) return null;
  return { kind: "range", start: new Date(from).toISOString(), end: new Date(to).toISOString() };
}

export function lookaheadRange(now: Date): ScheduleQuery {
  return { kind: "range", start: now.toISOString(), end: new Date(now.getTime() + SCHEDULE_LOOKAHEAD_DAYS * DAY_MS).toISOString() };
}

/**
 * Plans the calendar check for a meeting suggestion: its exact time when it
 * has one, otherwise its search range, otherwise the next week.
 */
export function planMeeting(proposal: MeetingProposal, now: Date, defaultDurationMinutes: number): SchedulePlan {
  const timeZoneAssumed = !proposal.timeZone;
  const start = proposal.normalizedStart ? new Date(proposal.normalizedStart) : null;
  const end = proposal.normalizedEnd ? new Date(proposal.normalizedEnd) : null;
  if (start && end && Number.isFinite(start.getTime()) && end > start) {
    const durationMinutes = clampMeetingMinutes(Math.round((end.getTime() - start.getTime()) / 60_000), defaultDurationMinutes);
    return {
      query: start > now ? { kind: "specific", start: start.toISOString(), end: end.toISOString() } : null,
      durationMinutes,
      timeZoneAssumed,
    };
  }
  const durationMinutes = clampMeetingMinutes(proposal.durationMinutes, defaultDurationMinutes);
  if (proposal.searchRangeStart && proposal.searchRangeEnd) {
    return { query: boundedRange(new Date(proposal.searchRangeStart), new Date(proposal.searchRangeEnd), now), durationMinutes, timeZoneAssumed };
  }
  return { query: lookaheadRange(now), durationMinutes, timeZoneAssumed };
}

/** Plans the calendar search a chat answer asked for. */
export function planChatAvailability(request: ChatAvailability, now: Date, defaultDurationMinutes: number): SchedulePlan {
  return {
    query: boundedRange(new Date(request.rangeStart), new Date(request.rangeEnd), now),
    durationMinutes: clampMeetingMinutes(request.durationMinutes, defaultDurationMinutes),
    timeZoneAssumed: false,
  };
}

export const slotKey = (slot: Pick<AvailabilityCandidate, "start" | "end">) => `${slot.start}/${slot.end}`;
