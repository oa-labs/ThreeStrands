import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";
import { CalendarCheck, CalendarPlus, ChevronDown, ChevronLeft, ChevronRight, CircleCheck, CircleAlert, RotateCcw } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { availabilityKey, readAvailability, rememberAvailabilityView, sameAvailability, storeAvailability, type AvailabilityView } from "./availabilityCache";
import type { AvailabilityCandidate, AvailabilityPreferences, AvailabilityResult, CoworkerCalendars, ProposedTimeCheck, ScheduleEvent } from "./domain";
import { findCalendarMatch, findDayMatch, SAME_EVENT_START_TOLERANCE_MINUTES, type CalendarMatch, type EventIdentity } from "./eventMatching";
import { mailClient } from "./data/client";
import { breakWarning, cachedEventsInRange, eventsInRange, eventsOnDay, SlotDayPreview } from "./SlotDayPreview";
import { AvailabilityProfileSelect } from "./AvailabilityProfileChoice";
import { errorMessage } from "./errors";
import {
  AFTERNOONS, ANY_TIME, lookaheadRange, MORNINGS, shiftedRange, slotKey, SLOTS_PER_DAY, spreadAcrossDays, SUGGESTED_SLOT_COUNT,
  windowsWithin, type DayBounds, type SchedulePlan, type ScheduleQuery,
} from "./scheduling";
import { ICON_SIZE } from "./iconSizes";
import { MoreActionsMenu, type MoreAction } from "./MoreActionsMenu";

export type ScheduleSlot = Pick<AvailabilityCandidate, "start" | "end">;
/** All-day and time-unspecified additions carry inclusive local `YYYY-MM-DD` days. */
export type CalendarAddition = ScheduleSlot & { allDay?: boolean; timeUnspecified?: boolean };

type CheckState =
  | { phase: "idle" | "loading" }
  | { phase: "error"; message: string }
  | { phase: "specific"; check: ProposedTimeCheck; conflictTitles: string[]; match: CalendarMatch | null }
  | { phase: "range"; candidates: AvailabilityCandidate[]; verified: boolean; unchecked: string[]; errors: string[] };

/** Names the calendars a check could not read, once for the whole card, such as "Couldn't check Team, so these times may conflict with it." */
export function uncheckedNote(unchecked: string[], subject: "this time" | "these times"): string {
  if (unchecked.length === 0) return `Some calendars couldn’t be checked, so ${subject} may conflict with them.`;
  const names = new Intl.ListFormat(undefined, { type: "conjunction" }).format(unchecked);
  return `Couldn’t check ${names}, so ${subject} may conflict with ${unchecked.length === 1 ? "it" : "them"}.`;
}

/** A familiar name for the zone, such as "Eastern Time", which stays right across daylight saving changes. */
export function zoneName(timeZone: string): string {
  try {
    const parts = new Intl.DateTimeFormat(undefined, { timeZone, timeZoneName: "longGeneric" }).formatToParts(new Date());
    return parts.find((part) => part.type === "timeZoneName")?.value ?? timeZone;
  } catch {
    return timeZone;
  }
}

function formatSlot(slot: ScheduleSlot, timeZone: string): string {
  const start = new Date(slot.start);
  const end = new Date(slot.end);
  const day = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", timeZone }).format(start);
  // A shared day period is named once, as in "9:00 – 9:30 AM".
  const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone }).formatRange(start, end);
  return `${day} · ${time}`;
}

/** A meeting length, such as "30 min" or "1 hr". */
export function formatLength(minutes: number): string {
  return minutes >= 60 && minutes % 60 === 0 ? `${minutes / 60} hr` : `${minutes} min`;
}

/** Meeting lengths offered beside a range search; the current one is always among them. */
export const MEETING_LENGTHS = [15, 30, 45, 60, 90, 120];

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

/** The hours a search is limited to, such as "from 3:00 PM"; empty for any time. */
function formatHours(bounds: DayBounds): string {
  return bounds.dayStart && bounds.dayEnd ? `${formatClock(bounds.dayStart)}–${formatClock(bounds.dayEnd)}`
    : bounds.dayStart ? `from ${formatClock(bounds.dayStart)}`
      : bounds.dayEnd ? `until ${formatClock(bounds.dayEnd)}` : "";
}

/** The weekdays a search is limited to, such as "Tue and Thu". */
function formatWeekdays(weekdays: number[]): string {
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" });
  // 2000-01-02 was a Sunday, weekday 0.
  return new Intl.ListFormat(undefined, { type: "conjunction" }).format([...weekdays].sort((left, right) => left - right).map((day) => weekday.format(new Date(2000, 0, 2 + day))));
}

/** The search a range card is showing, after any adjustments. */
export type EffectiveSearch = { query: ScheduleQuery; durationMinutes: number; bounds: DayBounds };

function formatDay(date: string): string {
  const [year, month, day] = date.split("-").map(Number);
  return new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).format(new Date(year, month - 1, day));
}

/** Whether a timed schedule event overlaps the slot. */
function overlaps(event: ScheduleEvent, slot: ScheduleSlot): boolean {
  return !event.allDay && Date.parse(event.start) < Date.parse(slot.end) && Date.parse(event.end) > Date.parse(slot.start);
}

/** "Already on your calendar", with the time when the event sits at another one that day. */
function addedText(match: CalendarMatch, slot: ScheduleSlot, timeZone: string): string {
  const moved = !match.event.allDay
    && Math.abs(Date.parse(match.event.start) - Date.parse(slot.start)) > SAME_EVENT_START_TOLERANCE_MINUTES * 60_000;
  if (!moved) return "Already on your calendar";
  const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone }).format(new Date(match.event.start));
  return `Already on your calendar at ${time}`;
}

/** When a matched event happens on its day, such as "10:00 AM" or "All day". */
function matchedTime(event: ScheduleEvent, timeZone: string): string {
  if (event.allDay) return "All day";
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone }).format(new Date(event.start));
}

/** Whether two schedule entries are the same event. */
const sameEvent = (left: ScheduleEvent, right: ScheduleEvent) => left.accountId === right.accountId && left.id === right.id;

/**
 * Checks a meeting against the user's calendar without involving AI: an
 * exact time is checked for conflicts, a range yields open slots spread
 * across its days. A range can be adjusted in place — mornings, afternoons,
 * or earlier and later days — and shows more slots on request. Each offered
 * slot has a checkbox for whether to offer it, and opens its day below it. Every outcome goes
 * through a review step: the event dialog or a reply in the composer.
 * Given what the event is called, a time or day already on the calendar
 * under that name reads as added rather than as a conflict. Once added,
 * opening it is the main action and adding it again moves to the ⋯ menu.
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
  coworkers: latestCoworkers,
  identity: latestIdentity,
  onOpenCalendarEvent,
  onMatchChange,
  matchedActions,
  moreActions = [],
  extraActions,
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
  coworkers?: CoworkerCalendars;
  /** What the event is called and where, to recognize it already on the calendar. */
  identity?: EventIdentity;
  /** Opens a calendar event the suggestion already appears as. */
  onOpenCalendarEvent?(event: ScheduleEvent): void;
  /** Told the calendar event this one already is, or null, so the caller can drop what the scheduler now covers. */
  onMatchChange?(event: ScheduleEvent | null): void;
  /** The caller's own actions, shown in the scheduler's row once the event is on the calendar. */
  matchedActions?: ReactNode;
  /** The caller's less common actions, added to the ⋯ menu once the event is on the calendar. */
  moreActions?: MoreAction[];
  /** The caller's own actions, after the scheduler's, while the event is not on the calendar. */
  extraActions?: ReactNode;
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
  // A length nothing stated follows the availability profile in use, until the user picks one.
  const [chosenLength, setChosenLength] = useState<number | null>(null);
  const durationMinutes = chosenLength ?? (plan.durationIsDefault ? preferences.defaultDurationMinutes : plan.durationMinutes);
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
  // The one offered time whose day is open below it.
  const [previewing, setPreviewing] = useState<string | null>(null);
  const previewId = useId();
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const refresh = () => setAttempt((value) => value + 1);
    window.addEventListener(CALENDAR_AVAILABILITY_CHANGED, refresh);
    return () => window.removeEventListener(CALENDAR_AVAILABILITY_CHANGED, refresh);
  }, []);
  // Callers build the identity on each render; re-check only when its values change.
  const identityKey = latestIdentity ? `${latestIdentity.title}|${latestIdentity.location ?? ""}|${latestIdentity.sourceMessageId ?? ""}` : "";
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const identity = useMemo(() => latestIdentity, [identityKey]);
  const coworkersKey = JSON.stringify(latestCoworkers);
  // Stable across draft edits that do not change the selected calendars.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const coworkers = useMemo(() => latestCoworkers, [coworkersKey]);
  const timeZone = preferences.timeZone;
  const announced = plan.announced === true;

  // The search whose times are shown.
  const rangeKey = useRef<string | null>(null);
  const run = useCallback(async (target: ScheduleQuery, isCurrent: () => boolean) => {
    // Shows a range's times, keeping the user's choices among them where they still apply.
    const showRange = (result: AvailabilityResult, view: AvailabilityView | null, keepCurrent: boolean) => {
      const candidates = spreadAcrossDays(result.candidates, timeZone);
      const keys = new Set(candidates.map(slotKey));
      const defaults = () => new Set(candidates.slice(0, SUGGESTED_SLOT_COUNT).map(slotKey));
      setState({
        phase: "range",
        candidates,
        verified: result.totalCalendarCount > 0 && result.checkedCalendarCount === result.totalCalendarCount,
        unchecked: result.uncheckedCalendars,
        errors: result.errors,
      });
      if (keepCurrent) {
        setSelected((current) => {
          const kept = [...current].filter((key) => keys.has(key));
          return kept.length ? new Set(kept) : defaults();
        });
        setPreviewing((current) => current && keys.has(current) ? current : null);
        return;
      }
      const restored = view?.selected.filter((key) => keys.has(key)) ?? [];
      setSelected(view && (restored.length || view.selected.length === 0) ? new Set(restored) : defaults());
      setPreviewing(view?.previewing && keys.has(view.previewing) ? view.previewing : null);
      setVisibleCount(view?.visibleCount ?? SUGGESTED_SLOT_COUNT);
    };
    try {
      if (target.kind === "specific") {
        setState({ phase: "loading" });
        const check = await mailClient.checkProposedTime({ start: target.start, end: target.end, timeZone, driveTimeBufferMinutes: preferences.driveTimeBufferMinutes });
        // An event added from this email may sit at another time that day, so
        // its day is read even when the time itself is free.
        const readDay = check.status === "conflicting" || Boolean(identity?.sourceMessageId);
        const dayEvents = readDay ? await eventsOnDay(new Date(target.start)).catch(() => []) : [];
        const overlapping = check.status === "conflicting" ? dayEvents.filter((event) => overlaps(event, target)) : [];
        const match = identity ? findCalendarMatch(identity, target, dayEvents) : null;
        const conflictTitles = [...new Set(overlapping.filter((event) => !match || !sameEvent(event, match.event)).map((event) => event.title))];
        if (isCurrent()) setState({ phase: "specific", check, conflictTitles, match });
      } else {
        const request = {
          rangeStart: target.start,
          rangeEnd: target.end,
          preferences: {
            ...preferences,
            workingWindows: windowsWithin(preferences.workingWindows, bounds),
            defaultDurationMinutes: durationMinutes,
          },
          maxPerDay: SLOTS_PER_DAY,
          ...(coworkers ? { coworkers } : {}),
        };
        const key = availabilityKey(request);
        rangeKey.current = key;
        // Times found moments ago show at once while the calendar is checked again.
        const cached = readAvailability(key);
        if (cached) showRange(cached.result, cached.view, false);
        else setState({ phase: "loading" });
        const result = await mailClient.findAvailability(request);
        storeAvailability(key, result);
        if (!isCurrent()) return;
        if (cached && sameAvailability(cached.result, result)) return;
        showRange(result, null, cached !== null);
      }
    } catch (reason) {
      if (isCurrent()) setState({ phase: "error", message: errorMessage(reason) });
    }
  }, [bounds, coworkers, identity, durationMinutes, preferences, timeZone]);

  // The user's choices among the shown times outlast the card.
  useEffect(() => {
    if (state.phase === "range" && rangeKey.current) {
      rememberAvailabilityView(rangeKey.current, { selected: [...selected], previewing, visibleCount });
    }
  }, [state, selected, previewing, visibleCount]);

  // The range's meetings, read once, flag a time with little break beside it before its day is opened.
  const [rangeEvents, setRangeEvents] = useState<ScheduleEvent[] | null>(null);
  const rangeStart = state.phase === "range" && query?.kind === "range" ? query.start : null;
  const rangeEnd = rangeStart && query ? query.end : null;
  useEffect(() => {
    setRangeEvents(rangeStart && rangeEnd ? cachedEventsInRange(rangeStart, rangeEnd) : null);
    if (!rangeStart || !rangeEnd) return;
    let current = true;
    eventsInRange(rangeStart, rangeEnd)
      .then((events) => { if (current) setRangeEvents(events); })
      // Without them the times still show, just without the flag.
      .catch(() => undefined);
    return () => { current = false; };
  }, [rangeStart, rangeEnd, attempt]);

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
  // A bare day is not checked for conflicts, only for the event already being on it.
  const [dayMatch, setDayMatch] = useState<CalendarMatch | null>(null);
  useEffect(() => {
    const day = plan.day;
    if (!calendarConnected || query || !day || !identity) return;
    let current = true;
    const [year, month, date] = day.split("-").map(Number);
    eventsOnDay(new Date(year, month - 1, date))
      .then((events) => { if (current) setDayMatch(findDayMatch(identity, day, events)); })
      // Without the schedule the day still adds as usual.
      .catch(() => undefined);
    return () => { current = false; };
    // A bare day has no query; `queryKey` stays "none" while it shows.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [calendarConnected, plan.day, queryKey, identity]);
  const matchedEvent = query
    ? (state.phase === "specific" && state.match?.kind === "same" ? state.match.event : null)
    : (plan.day ? dayMatch?.event ?? null : null);
  const matched = matchedEvent !== null;
  const matchedKey = matchedEvent ? `${matchedEvent.accountId}|${matchedEvent.id}|${matchedEvent.conferenceUrl ?? ""}` : "";
  useEffect(() => {
    onMatchChange?.(matchedEvent);
    // Reported when the matched event changes, not when the caller's callback does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [matchedKey]);
  useEffect(() => {
    if (query?.kind === "range") onSearchChange?.({ query, durationMinutes, bounds });
    // Reported once per distinct search, identified by `queryKey` and its length.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey, durationMinutes]);

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
  // Part of the day narrows the times shown; the arrows beside the range move to other days.
  // The length sits with the part of the day: both change which times fit, while the range row only pages.
  const adjust = <div className="meeting-scheduler-adjust">
    <div className="meeting-scheduler-when">
      <div className="segmented meeting-scheduler-hours" role="group" aria-label="Part of the day">
        <button type="button" className="segment" aria-pressed={!bounds.dayStart && !bounds.dayEnd} onClick={() => setBounds((current) => ({ ...current, dayStart: null, dayEnd: null }))}>Any Time</button>
        <button type="button" className="segment" aria-pressed={mornings} onClick={() => toggleHours(MORNINGS, mornings)}>Mornings</button>
        <button type="button" className="segment" aria-pressed={afternoons} onClick={() => toggleHours(AFTERNOONS, afternoons)}>Afternoons</button>
      </div>
      <select
        className="meeting-scheduler-length"
        aria-label="Meeting Length"
        value={durationMinutes}
        onChange={(event) => setChosenLength(Number(event.target.value))}
      >{[...new Set([...MEETING_LENGTHS, durationMinutes])].sort((left, right) => left - right)
          .map((minutes) => <option key={minutes} value={minutes}>{formatLength(minutes)}</option>)}</select>
    </div>
    {/* Days the email named narrow the search; the user can still look at the others. */}
    {bounds.weekdays.length ? <p className="meeting-scheduler-note">
      Only {formatWeekdays(bounds.weekdays)}{" "}
      <button type="button" className="btn-link" onClick={() => setBounds((current) => ({ ...current, weekdays: [] }))}>Show All Days</button>
    </p> : null}
    <AvailabilityProfileSelect />
    {adjustInput}
  </div>;
  const coverageNote = (text: string) => <p className="meeting-scheduler-note" role="note">
    {text} <button type="button" className="btn-link" onClick={onOpenCalendarSettings}>Calendar Settings</button>
  </p>;
  const assumedZone = plan.timeZoneAssumed ? <small className="meeting-scheduler-note">Times in {zoneName(timeZone)} (the email didn&rsquo;t say)</small> : null;

  if (!calendarConnected) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      <p className="context-status">Connect a calendar to check times for this meeting.</p>
      <button type="button" className="btn-link context-link-button" onClick={onOpenCalendarSettings}>Calendar Settings</button>
      {extraActions ? <div className="meeting-scheduler-actions">{extraActions}</div> : null}
    </div>;
  }
  // A bare day or an undated event has nothing to check against the calendar.
  const day = plan.day;
  if (!query && day) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      {/* The calendar copy says when it happens, which the email did not. */}
      <p className="meeting-slot">{formatDay(day)} · {dayMatch ? matchedTime(dayMatch.event, timeZone) : "Time not set"}</p>
      {dayMatch ? <p className="meeting-slot-status meeting-slot-added"><CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> Already on your calendar</p> : null}
      {dayMatch ? <div className="meeting-scheduler-actions">
        {onOpenCalendarEvent ? <button type="button" className="btn btn-sm btn-primary" onClick={() => onOpenCalendarEvent(dayMatch.event)}><CalendarCheck size={ICON_SIZE.sm} /> Open in Calendar</button> : null}
        {matchedActions}
        <MoreActionsMenu label="More actions" actions={[{ label: "Add Again", onSelect: () => onAddToCalendar({ start: day, end: day, allDay: false, timeUnspecified: true }) }, ...moreActions]} />
      </div> : <div className="meeting-scheduler-actions">
        <button type="button" className={`btn btn-sm${announced ? " btn-primary" : ""}`} onClick={() => onAddToCalendar({ start: day, end: day, allDay: false, timeUnspecified: true })}><CalendarPlus size={ICON_SIZE.sm} /> Add to Calendar</button>
      </div>}
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
      {state.match?.kind === "same" ? <>
        <p className="meeting-slot-status meeting-slot-added"><CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> {addedText(state.match, query, timeZone)}</p>
        {state.conflictTitles.length ? <p className="meeting-slot-status meeting-slot-conflicting"><CircleAlert size={ICON_SIZE.xs} aria-hidden="true" /> Also overlaps {state.conflictTitles.join(", ")}</p> : null}
      </> : state.match?.kind === "maybe" ? <>
        <p className="meeting-slot-status"><CircleAlert size={ICON_SIZE.xs} aria-hidden="true" /> May already be on your calendar as {state.match.event.title}</p>
        {state.conflictTitles.length ? <p className="meeting-slot-status meeting-slot-conflicting"><CircleAlert size={ICON_SIZE.xs} aria-hidden="true" /> Also overlaps {state.conflictTitles.join(", ")}</p> : null}
      </> : <p className={`meeting-slot-status meeting-slot-${state.check.status}`}>
        {state.check.status === "free" ? <><CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> You&rsquo;re free</> : null}
        {state.check.status === "conflicting" ? <><CircleAlert size={ICON_SIZE.xs} aria-hidden="true" /> {state.conflictTitles.length ? `Conflicts with ${state.conflictTitles.join(", ")}` : "Conflicts with another event"}</> : null}
        {state.check.status === "partiallyChecked" ? <>Free on the calendars that could be checked</> : null}
        {state.check.status === "unverified" ? <>No calendar could be checked</> : null}
      </p>}
      {state.check.status === "partiallyChecked" ? coverageNote(uncheckedNote(state.check.uncheckedCalendars, "this time")) : null}
      {state.check.errors.length > 0 ? <details className="meeting-scheduler-note"><summary>Calendar check details</summary><ul>{state.check.errors.map((error, index) => <li key={index}>{error}</li>)}</ul></details> : null}
      <div className="meeting-scheduler-actions">
        {state.match && onOpenCalendarEvent ? <button type="button" className={`btn btn-sm${matched ? " btn-primary" : ""}`} onClick={() => onOpenCalendarEvent(state.match!.event)}><CalendarCheck size={ICON_SIZE.sm} /> Open in Calendar</button> : null}
        {/* Adding a second copy of an event already there takes a deliberate choice, so it waits in the ⋯ menu. */}
        {matched ? null : <button type="button" className={`btn btn-sm${announced && !state.match ? " btn-primary" : ""}`} onClick={() => onAddToCalendar(query)}><CalendarPlus size={ICON_SIZE.sm} /> Add to Calendar</button>}
        {/* The matched event is this one, so only other overlaps rule out a reply that the time works. */}
        {announced || (state.check.status === "conflicting" && !(state.match?.kind === "same" && state.conflictTitles.length === 0)) ? null
          : <button type="button" className="btn btn-sm" onClick={() => onConfirmTime(query)}>Reply &ldquo;That Works&rdquo;</button>}
        {announced ? null : <button type="button" className="btn btn-sm" onClick={() => findOtherTimes(query.start)}>Find Other Times</button>}
        {matched ? null : extraActions}
        {matched ? <>
          {matchedActions}
          <MoreActionsMenu label="More actions" actions={[{ label: "Add Again", onSelect: () => onAddToCalendar(query) }, ...moreActions]} />
        </> : null}
      </div>
    </> : null}
    {state.phase === "range" && query.kind === "range" ? <>
      <div className="meeting-scheduler-range">
        <button type="button" className="btn-icon btn-icon-sm" aria-label="Earlier" title="Earlier days" disabled={shift === 0} onClick={() => setShift((current) => current - 1)}><ChevronLeft size={ICON_SIZE.sm} /></button>
        <p>{[formatRange(query, timeZone), formatHours(bounds)].filter(Boolean).join(" · ")}</p>
        <button type="button" className="btn-icon btn-icon-sm" aria-label="Later" title="Later days" onClick={() => setShift((current) => current + 1)}><ChevronRight size={ICON_SIZE.sm} /></button>
      </div>
      {/* What the times mean comes before the times, and so do the ways to change them. */}
      {assumedZone}
      {adjust}
      {state.candidates.length === 0 ? <>
        <p className="context-status">No open times in your working hours for this range.</p>
        {extraActions ? <div className="meeting-scheduler-actions">{extraActions}</div> : null}
      </> : <>
        <div className="meeting-slots" role="group" aria-label="Open times">
          {state.candidates.slice(0, visibleCount)
            .sort((left, right) => Date.parse(left.start) - Date.parse(right.start))
            .map((candidate, index) => {
              const key = slotKey(candidate);
              const checked = selected.has(key);
              const open = previewing === key;
              const label = formatSlot(candidate, timeZone);
              const id = `${previewId}-${index}`;
              const warning = rangeEvents ? breakWarning(rangeEvents, candidate) : null;
              // Including a time and looking at its day are separate choices.
              return <div key={key} className="meeting-slot-item">
                <div className={`meeting-slot-row${checked ? " meeting-slot-row-checked" : ""}`}>
                  <label className="meeting-slot-check">
                    <input type="checkbox" aria-label={`Include ${label}`} checked={checked} onChange={() => setSelected((current) => {
                      const next = new Set(current);
                      if (checked) next.delete(key);
                      else next.add(key);
                      return next;
                    })} />
                  </label>
                  <button
                    type="button"
                    className="meeting-slot-day"
                    aria-expanded={open}
                    aria-controls={open ? id : undefined}
                    title="Show this day on your calendar"
                    onClick={() => setPreviewing(open ? null : key)}
                  ><span>{label}</span>
                    {warning ? <span className="meeting-slot-warning">{warning}</span> : null}
                    <ChevronDown className="meeting-slot-chevron" size={ICON_SIZE.sm} aria-hidden="true" /></button>
                </div>
                {open ? <SlotDayPreview id={id} slot={candidate} timeZone={timeZone} /> : null}
              </div>;
            })}
        </div>
        {(() => {
          const chosen = state.candidates
            .filter((candidate) => selected.has(slotKey(candidate)))
            .sort((left, right) => Date.parse(left.start) - Date.parse(right.start));
          // The count follows each checkbox, so the times above read as the button's choice.
          return <div className="meeting-scheduler-actions">
            <button type="button" className="btn btn-sm btn-primary" disabled={chosen.length === 0} onClick={() => onReplyWithTimes(chosen)}>{intoDraft ? "Insert" : "Draft Reply With"} {chosen.length === 0 ? "Times" : chosen.length === 1 ? "1 Time" : `${chosen.length} Times`}</button>
            {visibleCount < state.candidates.length
              ? <button type="button" className="btn btn-sm" onClick={() => setVisibleCount((current) => current + SUGGESTED_SLOT_COUNT)}>Show {Math.min(SUGGESTED_SLOT_COUNT, state.candidates.length - visibleCount)} More</button>
              : null}
            {extraActions}
          </div>;
        })()}
      </>}
      {state.verified ? null : coverageNote(state.unchecked.length === 0 && !state.candidates.some((candidate) => candidate.status === "partiallyChecked")
          ? "No calendar could be checked, so these times may conflict."
          : uncheckedNote(state.unchecked, "these times"))}
      {!state.verified && state.errors.length > 0 ? <details className="meeting-scheduler-note">
        <summary>Calendar check details</summary>
        <ul>{state.errors.map((error, index) => <li key={index}>{error}</li>)}</ul>
      </details> : null}
    </> : null}
    {state.phase === "range" ? null : assumedZone}
  </div>;
}
