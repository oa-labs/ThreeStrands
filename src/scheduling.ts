import type { AvailabilityCandidate, AvailabilityWindow, ChatAvailability, MeetingProposal, TimeSearch } from "./domain";
import { dateInputValue } from "./calendarTime";

/** Days searched when a meeting names no usable time or range. */
export const SCHEDULE_LOOKAHEAD_DAYS = 7;
/** Longest range searched for open times in one card. */
export const MAX_SCHEDULE_RANGE_DAYS = 14;
/** Open times offered in a card at first, and added by each "Show More". */
export const SUGGESTED_SLOT_COUNT = 3;
/** Open times fetched per day, so "Show More" has later slots to offer. */
export const SLOTS_PER_DAY = 3;
/** Longest adjustment the user can type for Find Times; the native layer enforces the same limit. */
export const FIND_TIMES_INSTRUCTION_MAX_CHARS = 300;
/** Where Mornings end and Afternoons begin, in the user's time zone. */
export const MIDDAY = "12:00";
export const MIN_MEETING_MINUTES = 5;
export const MAX_MEETING_MINUTES = 720;

const DAY_MS = 86_400_000;

/**
 * What to check on the calendar: one exact time, or a range to search for
 * open times. `null` means the proposed time or range has already passed,
 * unless the plan names a `day` or is `undated`.
 */
export type ScheduleQuery =
  | { kind: "specific"; start: string; end: string }
  | { kind: "range"; start: string; end: string };

export type SchedulePlan = {
  query: ScheduleQuery | null;
  durationMinutes: number;
  /** The meeting gave no timezone, so the user's own is assumed. */
  timeZoneAssumed: boolean;
  /**
   * An event the email announces at a set time, rather than one still to be
   * agreed: it is added as given, never answered with other times.
   */
  announced?: boolean;
  /** A `YYYY-MM-DD` day with no time, added as an all-day event. */
  day?: string;
  /** An announced event that names no date yet. */
  undated?: boolean;
  /** Days and hours a range search starts limited to. */
  bounds?: DayBounds;
};

const DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;

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
 * has a start (ending at its stated end, or after its duration), otherwise its
 * day, otherwise its search range, otherwise the next week. An announced event
 * without a date is never matched against the next week's open times.
 */
export function planMeeting(proposal: MeetingProposal, now: Date, defaultDurationMinutes: number): SchedulePlan {
  const timeZoneAssumed = !proposal.timeZone;
  const announced = proposal.intent === "attend";
  const kind = announced ? { announced } : {};
  const start = proposal.normalizedStart ? new Date(proposal.normalizedStart) : null;
  if (start && Number.isFinite(start.getTime())) {
    const statedEnd = proposal.normalizedEnd ? new Date(proposal.normalizedEnd) : null;
    const hasEnd = statedEnd !== null && Number.isFinite(statedEnd.getTime()) && statedEnd > start;
    const durationMinutes = hasEnd
      ? clampMeetingMinutes(Math.round((statedEnd.getTime() - start.getTime()) / 60_000), defaultDurationMinutes)
      : clampMeetingMinutes(proposal.durationMinutes, defaultDurationMinutes);
    const end = hasEnd ? statedEnd : new Date(start.getTime() + durationMinutes * 60_000);
    return {
      query: start > now ? { kind: "specific", start: start.toISOString(), end: end.toISOString() } : null,
      durationMinutes,
      timeZoneAssumed,
      ...kind,
    };
  }
  const durationMinutes = clampMeetingMinutes(proposal.durationMinutes, defaultDurationMinutes);
  if (proposal.eventDate && DATE_PATTERN.test(proposal.eventDate)) {
    return {
      query: null,
      ...(proposal.eventDate >= dateInputValue(now) ? { day: proposal.eventDate } : {}),
      durationMinutes,
      timeZoneAssumed: false,
      ...kind,
    };
  }
  if (proposal.searchRangeStart && proposal.searchRangeEnd) {
    return { query: boundedRange(new Date(proposal.searchRangeStart), new Date(proposal.searchRangeEnd), now), durationMinutes, timeZoneAssumed, ...kind };
  }
  if (announced) return { query: null, undated: true, durationMinutes, timeZoneAssumed, announced };
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

/** Which days and hours of the working week a range search keeps. */
export type DayBounds = {
  /** Allowed weekdays, 0 for Sunday; empty allows every day. */
  weekdays: number[];
  /** Earliest local start, `HH:MM`. */
  dayStart: string | null;
  /** Latest local end, `HH:MM`. */
  dayEnd: string | null;
};

export const ANY_TIME: DayBounds = { weekdays: [], dayStart: null, dayEnd: null };
export const MORNINGS = { dayStart: null, dayEnd: MIDDAY } as const;
export const AFTERNOONS = { dayStart: MIDDAY, dayEnd: null } as const;

/**
 * Working windows limited to the allowed weekdays and clipped to the day's
 * bounds; a window left empty is dropped.
 */
export function windowsWithin(windows: AvailabilityWindow[], bounds: DayBounds): AvailabilityWindow[] {
  return windows.flatMap((window) => {
    if (bounds.weekdays.length > 0 && !bounds.weekdays.includes(window.weekday)) return [];
    // Clock values are zero-padded `HH:MM`, so they compare as strings.
    const start = bounds.dayStart && bounds.dayStart > window.start ? bounds.dayStart : window.start;
    const end = bounds.dayEnd && bounds.dayEnd < window.end ? bounds.dayEnd : window.end;
    return start < end ? [{ ...window, start, end }] : [];
  });
}

/** Plans the calendar search Find Times read from a conversation. */
export function planTimeSearch(search: TimeSearch, now: Date, defaultDurationMinutes: number): SchedulePlan {
  return {
    // The provider only returns ranges that end in the future; one that has
    // since passed falls back to the coming week.
    query: boundedRange(new Date(search.rangeStart), new Date(search.rangeEnd), now) ?? lookaheadRange(now),
    durationMinutes: clampMeetingMinutes(search.durationMinutes, defaultDurationMinutes),
    timeZoneAssumed: false,
    bounds: { weekdays: search.weekdays, dayStart: search.dayStart, dayEnd: search.dayEnd },
  };
}

/** The same-length range after (positive steps) or before (negative steps) a search range. */
export function shiftedRange(query: ScheduleQuery, steps: number): ScheduleQuery {
  const start = Date.parse(query.start);
  const span = Math.min(Date.parse(query.end) - start, MAX_SCHEDULE_RANGE_DAYS * DAY_MS);
  return { kind: "range", start: new Date(start + steps * span).toISOString(), end: new Date(start + (steps + 1) * span).toISOString() };
}

/**
 * Orders open times so each day's first slot comes before any day's second:
 * the first few cover different days, and "Show More" fills in later slots.
 */
export function spreadAcrossDays<T extends Pick<AvailabilityCandidate, "start">>(candidates: T[], timeZone: string): T[] {
  const dayOf = new Intl.DateTimeFormat("en-CA", { year: "numeric", month: "2-digit", day: "2-digit", timeZone });
  const seen = new Map<string, number>();
  return candidates
    .map((candidate) => {
      const day = dayOf.format(new Date(candidate.start));
      const rank = seen.get(day) ?? 0;
      seen.set(day, rank + 1);
      return { candidate, rank };
    })
    .sort((left, right) => left.rank - right.rank || Date.parse(left.candidate.start) - Date.parse(right.candidate.start))
    .map(({ candidate }) => candidate);
}
