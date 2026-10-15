import { CalendarPlus, CircleCheck, CircleAlert, RotateCcw } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import type { AvailabilityCandidate, AvailabilityPreferences, ProposedTimeCheck } from "./domain";
import { mailClient } from "./data/client";
import { refreshScheduleCache } from "./calendarScheduleCache";
import { scheduleRequestFor } from "./CalendarSidebar";
import { errorMessage } from "./errors";
import {
  AFTERNOONS, ANY_TIME, lookaheadRange, MORNINGS, shiftedRange, slotKey, SLOTS_PER_DAY, spreadAcrossDays, SUGGESTED_SLOT_COUNT,
  windowsWithin, type DayBounds, type SchedulePlan, type ScheduleQuery,
} from "./scheduling";
import { ICON_SIZE } from "./iconSizes";

export type ScheduleSlot = Pick<AvailabilityCandidate, "start" | "end">;
/** A time to add to the calendar; an all-day addition carries inclusive `YYYY-MM-DD` days. */
export type CalendarAddition = ScheduleSlot & { allDay?: boolean };

type CheckState =
  | { phase: "idle" | "loading" }
  | { phase: "error"; message: string }
  | { phase: "specific"; check: ProposedTimeCheck; conflictTitles: string[] }
  | { phase: "range"; candidates: AvailabilityCandidate[] };

function formatSlot(slot: ScheduleSlot, timeZone: string): string {
  const start = new Date(slot.start);
  const end = new Date(slot.end);
  const day = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", timeZone }).format(start);
  const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone });
  return `${day} · ${time.format(start)}–${time.format(end)}`;
}

function formatRange(query: ScheduleQuery, timeZone: string): string {
  const day = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", timeZone });
  // The range end is exclusive; name the last day it covers.
  const first = day.format(new Date(query.start));
  const last = day.format(new Date(Date.parse(query.end) - 1));
  return first === last ? first : `${first} – ${last}`;
}

function formatClock(clock: string): string {
  const [hours, minutes] = clock.split(":").map(Number);
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(new Date(2000, 0, 1, hours, minutes));
}

/** The days and hours a search is limited to, such as "Thu, Fri · from 3:00 PM"; empty for any time. */
function formatBounds(bounds: DayBounds): string {
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" });
  // 2000-01-02 was a Sunday, weekday 0.
  const days = bounds.weekdays.map((day) => weekday.format(new Date(2000, 0, 2 + day))).join(", ");
  const hours = bounds.dayStart && bounds.dayEnd ? `${formatClock(bounds.dayStart)}–${formatClock(bounds.dayEnd)}`
    : bounds.dayStart ? `from ${formatClock(bounds.dayStart)}`
      : bounds.dayEnd ? `until ${formatClock(bounds.dayEnd)}` : "";
  return [days, hours].filter(Boolean).join(" · ");
}

/** The search a range card is showing, after any adjustments. */
export type EffectiveSearch = { query: ScheduleQuery; durationMinutes: number; bounds: DayBounds };

function formatDay(date: string): string {
  const [year, month, day] = date.split("-").map(Number);
  return new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).format(new Date(year, month - 1, day));
}

/** Titles of timed schedule events that overlap the slot, from the day's cached schedule. */
async function overlappingTitles(slot: ScheduleSlot): Promise<string[]> {
  const request = scheduleRequestFor(new Date(slot.start));
  const result = await refreshScheduleCache(request, () => mailClient.listScheduleEvents(request.timeMin, request.timeMax, request.timeZone));
  const start = Date.parse(slot.start);
  const end = Date.parse(slot.end);
  return [...new Set(result.events
    .filter((event) => !event.allDay && Date.parse(event.start) < end && Date.parse(event.end) > start)
    .map((event) => event.title))];
}

/**
 * Checks a meeting against the user's calendar without involving AI: an
 * exact time is checked for conflicts, a range yields open slots spread
 * across its days. A range can be adjusted in place — mornings, afternoons,
 * or a later range — and shows more slots on request. Every outcome goes
 * through a review step: the event dialog or a reply in the composer.
 */
export function MeetingScheduler({
  plan: initialPlan,
  preferences: latestPreferences,
  calendarConnected,
  onAddToCalendar,
  onReplyWithTimes,
  onConfirmTime,
  onOpenCalendarSettings,
  onSearchChange,
  adjustInput,
  intoDraft = false,
}: {
  plan: SchedulePlan;
  preferences: AvailabilityPreferences;
  calendarConnected: boolean;
  onAddToCalendar(slot: CalendarAddition): void;
  onReplyWithTimes(slots: AvailabilityCandidate[]): void;
  onConfirmTime(slot: ScheduleSlot): void;
  onOpenCalendarSettings(): void;
  /** Times go into the open draft rather than a new reply. */
  intoDraft?: boolean;
  /** Told the range being searched each time it changes. */
  onSearchChange?(search: EffectiveSearch): void;
  /** Shown with the adjustment chips, for example a field to adjust with AI. */
  adjustInput?: ReactNode;
}) {
  // Plans computed from "now" differ on every render; keep the one this
  // scheduler mounted with. Callers key the scheduler by what it schedules.
  const [plan] = useState(initialPlan);
  // Re-check only when preference values change, not when an equal object
  // arrives: replacing the result with "Checking…" reads as a flash.
  const [preferences, setPreferences] = useState(latestPreferences);
  if (preferences !== latestPreferences && JSON.stringify(preferences) !== JSON.stringify(latestPreferences)) {
    setPreferences(latestPreferences);
  }
  // "Find Other Times" replaces a conflicting exact time with a search.
  const [override, setOverride] = useState<ScheduleQuery | null>(null);
  const baseQuery = override ?? plan.query;
  // Adjustments to a range search: part of the day, and ranges later or earlier.
  const [bounds, setBounds] = useState<DayBounds>(plan.bounds ?? ANY_TIME);
  const [shift, setShift] = useState(0);
  const query = baseQuery?.kind === "range" && shift !== 0 ? shiftedRange(baseQuery, shift) : baseQuery;
  const [visibleCount, setVisibleCount] = useState(SUGGESTED_SLOT_COUNT);
  const [state, setState] = useState<CheckState>({ phase: "idle" });
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  const [attempt, setAttempt] = useState(0);
  const timeZone = preferences.timeZone;
  const announced = plan.announced === true;

  const run = useCallback(async (target: ScheduleQuery, isCurrent: () => boolean) => {
    setState({ phase: "loading" });
    try {
      if (target.kind === "specific") {
        const check = await mailClient.checkProposedTime({ start: target.start, end: target.end, timeZone });
        const conflictTitles = check.status === "conflicting" ? await overlappingTitles(target).catch(() => []) : [];
        if (isCurrent()) setState({ phase: "specific", check, conflictTitles });
      } else {
        const result = await mailClient.findAvailability({
          rangeStart: target.start,
          rangeEnd: target.end,
          preferences: {
            ...preferences,
            workingWindows: windowsWithin(preferences.workingWindows, bounds),
            defaultDurationMinutes: plan.durationMinutes,
          },
          maxPerDay: SLOTS_PER_DAY,
        });
        const candidates = spreadAcrossDays(result.candidates, timeZone);
        if (!isCurrent()) return;
        setState({ phase: "range", candidates });
        setVisibleCount(SUGGESTED_SLOT_COUNT);
        setSelected(new Set(candidates.slice(0, SUGGESTED_SLOT_COUNT).map(slotKey)));
      }
    } catch (reason) {
      if (isCurrent()) setState({ phase: "error", message: errorMessage(reason) });
    }
  }, [bounds, plan.durationMinutes, preferences, timeZone]);

  const boundsKey = `${bounds.weekdays.join(",")}|${bounds.dayStart ?? ""}|${bounds.dayEnd ?? ""}`;
  const queryKey = query ? `${query.kind}:${query.start}:${query.end}:${query.kind === "range" ? boundsKey : ""}` : "none";
  useEffect(() => {
    if (!calendarConnected || !query) return;
    let current = true;
    void run(query, () => current);
    return () => { current = false; };
    // The query is identified by `queryKey`; `attempt` re-runs after a failure.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [calendarConnected, queryKey, attempt, run]);
  useEffect(() => {
    if (query?.kind === "range") onSearchChange?.({ query, durationMinutes: plan.durationMinutes, bounds });
    // Reported once per distinct search, identified by `queryKey`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey]);

  const findOtherTimes = (start: string) => {
    const day = new Date(start);
    day.setHours(0, 0, 0, 0);
    setOverride(lookaheadRange(new Date(Math.max(day.getTime(), Date.now()))));
  };
  const mornings = bounds.dayStart === MORNINGS.dayStart && bounds.dayEnd === MORNINGS.dayEnd;
  const afternoons = bounds.dayStart === AFTERNOONS.dayStart && bounds.dayEnd === AFTERNOONS.dayEnd;
  // A part of the day replaces any hours already set and keeps the weekdays; pressing it again clears the hours.
  const toggleHours = (hours: typeof MORNINGS | typeof AFTERNOONS, pressed: boolean) =>
    setBounds((current) => ({ ...current, ...(pressed ? { dayStart: null, dayEnd: null } : hours) }));
  const adjust = <div className="meeting-scheduler-adjust">
    <div className="meeting-scheduler-adjust-chips" role="group" aria-label="Adjust times">
      <button type="button" className="btn btn-sm" aria-pressed={mornings} onClick={() => toggleHours(MORNINGS, mornings)}>Mornings</button>
      <button type="button" className="btn btn-sm" aria-pressed={afternoons} onClick={() => toggleHours(AFTERNOONS, afternoons)}>Afternoons</button>
      {shift > 0 ? <button type="button" className="btn btn-sm" onClick={() => setShift((current) => current - 1)}>Earlier</button> : null}
      <button type="button" className="btn btn-sm" onClick={() => setShift((current) => current + 1)}>Later</button>
    </div>
    {adjustInput}
  </div>;
  const assumedZone = plan.timeZoneAssumed ? <small className="meeting-scheduler-note">Using your time zone ({timeZone})</small> : null;

  if (!calendarConnected) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      <p className="context-status">Connect a calendar to check times for this meeting.</p>
      <button type="button" className="btn-link context-link-button" onClick={onOpenCalendarSettings}>Calendar Settings</button>
    </div>;
  }
  // A bare day or an undated event has nothing to check against the calendar.
  const day = plan.day;
  if (!query && day) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      <p className="meeting-slot">{formatDay(day)} · Time not set</p>
      <div className="meeting-scheduler-actions">
        <button type="button" className="btn btn-sm" onClick={() => onAddToCalendar({ start: day, end: day, allDay: true })}><CalendarPlus size={ICON_SIZE.sm} /> Add to Calendar</button>
      </div>
    </div>;
  }
  if (!query && plan.undated) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      <p className="context-status">No date yet. Use Edit to set one.</p>
    </div>;
  }
  if (!query) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      <p className="context-status">{announced ? "This event has already happened." : "That time has already passed."}</p>
      {announced ? null : <div className="meeting-scheduler-actions">
        <button type="button" className="btn btn-sm" onClick={() => setOverride(lookaheadRange(new Date()))}>Find New Times</button>
      </div>}
    </div>;
  }

  return <div className="meeting-scheduler" role="group" aria-label="Schedule">
    {state.phase === "idle" || state.phase === "loading" ? <p className="context-status" role="status">Checking your calendar…</p> : null}
    {state.phase === "error" ? <div className="meeting-scheduler-error" role="alert">
      <p>{state.message}</p>
      <button type="button" className="btn btn-sm" onClick={() => setAttempt((value) => value + 1)}><RotateCcw size={ICON_SIZE.sm} /> Try Again</button>
    </div> : null}
    {state.phase === "specific" && query.kind === "specific" ? <>
      <p className="meeting-slot">{formatSlot(query, timeZone)}</p>
      <p className={`meeting-slot-status meeting-slot-${state.check.status}`}>
        {state.check.status === "free" ? <><CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> You&rsquo;re free</> : null}
        {state.check.status === "conflicting" ? <><CircleAlert size={ICON_SIZE.xs} aria-hidden="true" /> {state.conflictTitles.length ? `Conflicts with ${state.conflictTitles.join(", ")}` : "Conflicts with another event"}</> : null}
        {state.check.status === "partiallyChecked" ? <>Free on the calendars that could be checked</> : null}
        {state.check.status === "unverified" ? <>No calendar could be checked</> : null}
      </p>
      <div className="meeting-scheduler-actions">
        <button type="button" className="btn btn-sm" onClick={() => onAddToCalendar(query)}><CalendarPlus size={ICON_SIZE.sm} /> Add to Calendar</button>
        {announced || state.check.status === "conflicting" ? null
          : <button type="button" className="btn btn-sm" onClick={() => onConfirmTime(query)}>Reply &ldquo;That Works&rdquo;</button>}
        {announced ? null : <button type="button" className="btn btn-sm" onClick={() => findOtherTimes(query.start)}>Find Other Times</button>}
      </div>
    </> : null}
    {state.phase === "range" && query.kind === "range" ? <>
      <p className="meeting-scheduler-range">{[formatRange(query, timeZone), formatBounds(bounds)].filter(Boolean).join(" · ")}</p>
      {state.candidates.length === 0 ? <p className="context-status">No open times in your working hours for this range.</p> : <>
        <div className="meeting-slots" role="group" aria-label="Open times">
          {state.candidates.slice(0, visibleCount)
            .sort((left, right) => Date.parse(left.start) - Date.parse(right.start))
            .map((candidate) => {
              const key = slotKey(candidate);
              const pressed = selected.has(key);
              return <button
                type="button"
                key={key}
                aria-pressed={pressed}
                className="meeting-slot-option"
                onClick={() => setSelected((current) => {
                  const next = new Set(current);
                  if (pressed) next.delete(key);
                  else next.add(key);
                  return next;
                })}
              >{formatSlot(candidate, timeZone)}{candidate.status !== "verified" ? <small> · not every calendar checked</small> : null}</button>;
            })}
        </div>
        {(() => {
          const chosen = state.candidates
            .filter((candidate) => selected.has(slotKey(candidate)))
            .sort((left, right) => Date.parse(left.start) - Date.parse(right.start));
          return <div className="meeting-scheduler-actions">
            <button type="button" className="btn btn-sm" disabled={chosen.length === 0} onClick={() => onReplyWithTimes(chosen)}>{intoDraft ? "Insert" : "Draft Reply With"} {chosen.length === 1 ? "This Time" : "These Times"}</button>
            {visibleCount < state.candidates.length
              ? <button type="button" className="btn btn-sm" onClick={() => setVisibleCount((current) => current + SUGGESTED_SLOT_COUNT)}>Show More</button>
              : null}
          </div>;
        })()}
      </>}
      {adjust}
    </> : null}
    {assumedZone}
  </div>;
}
