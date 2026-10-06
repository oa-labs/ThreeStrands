import { CalendarPlus, CheckCircle2, CircleAlert, RotateCcw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import type { AvailabilityCandidate, AvailabilityPreferences, ProposedTimeCheck } from "./domain";
import { mailClient } from "./data/client";
import { refreshScheduleCache } from "./calendarScheduleCache";
import { scheduleRequestFor } from "./CalendarSidebar";
import { errorMessage } from "./errors";
import { lookaheadRange, slotKey, SUGGESTED_SLOT_COUNT, type SchedulePlan, type ScheduleQuery } from "./scheduling";

export type ScheduleSlot = Pick<AvailabilityCandidate, "start" | "end">;

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
 * exact time is checked for conflicts, a range yields the first open slot on
 * each of the next few days. Every outcome goes through a review step —
 * the event dialog, a reply in the composer, or the Calendar sidebar.
 */
export function MeetingScheduler({
  plan: initialPlan,
  preferences,
  calendarConnected,
  onAddToCalendar,
  onReplyWithTimes,
  onConfirmTime,
  onMoreTimes,
  onOpenCalendarSettings,
  intoDraft = false,
}: {
  plan: SchedulePlan;
  preferences: AvailabilityPreferences;
  calendarConnected: boolean;
  onAddToCalendar(slot: ScheduleSlot): void;
  onReplyWithTimes(slots: AvailabilityCandidate[]): void;
  onConfirmTime(slot: ScheduleSlot): void;
  onMoreTimes(day: Date, durationMinutes: number): void;
  onOpenCalendarSettings(): void;
  /** Times go into the open draft rather than a new reply. */
  intoDraft?: boolean;
}) {
  // Plans computed from "now" differ on every render; keep the one this
  // scheduler mounted with. Callers key the scheduler by what it schedules.
  const [plan] = useState(initialPlan);
  // "Find Other Times" replaces a conflicting exact time with a search.
  const [override, setOverride] = useState<ScheduleQuery | null>(null);
  const query = override ?? plan.query;
  const [state, setState] = useState<CheckState>({ phase: "idle" });
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  const [attempt, setAttempt] = useState(0);
  const timeZone = preferences.timeZone;

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
          preferences: { ...preferences, defaultDurationMinutes: plan.durationMinutes },
          maxPerDay: 1,
        });
        const candidates = result.candidates.slice(0, SUGGESTED_SLOT_COUNT);
        if (!isCurrent()) return;
        setState({ phase: "range", candidates });
        setSelected(new Set(candidates.map(slotKey)));
      }
    } catch (reason) {
      if (isCurrent()) setState({ phase: "error", message: errorMessage(reason) });
    }
  }, [plan.durationMinutes, preferences, timeZone]);

  const queryKey = query ? `${query.kind}:${query.start}:${query.end}` : "none";
  useEffect(() => {
    if (!calendarConnected || !query) return;
    let current = true;
    void run(query, () => current);
    return () => { current = false; };
    // The query is identified by `queryKey`; `attempt` re-runs after a failure.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [calendarConnected, queryKey, attempt, run]);

  const firstDay = query ? new Date(query.start) : new Date();
  const moreTimes = <button type="button" onClick={() => onMoreTimes(firstDay, plan.durationMinutes)}>More Times</button>;
  const assumedZone = plan.timeZoneAssumed ? <small className="meeting-scheduler-note">Using your time zone ({timeZone})</small> : null;

  if (!calendarConnected) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      <p className="context-status">Connect a calendar to check times for this meeting.</p>
      <button type="button" className="context-link-button" onClick={onOpenCalendarSettings}>Calendar Settings</button>
    </div>;
  }
  if (!query) {
    return <div className="meeting-scheduler" role="group" aria-label="Schedule">
      <p className="context-status">That time has already passed.</p>
      <div className="meeting-scheduler-actions">
        <button type="button" onClick={() => setOverride(lookaheadRange(new Date()))}>Find New Times</button>
      </div>
    </div>;
  }

  return <div className="meeting-scheduler" role="group" aria-label="Schedule">
    {state.phase === "idle" || state.phase === "loading" ? <p className="context-status" role="status">Checking your calendar…</p> : null}
    {state.phase === "error" ? <div className="meeting-scheduler-error" role="alert">
      <p>{state.message}</p>
      <button type="button" onClick={() => setAttempt((value) => value + 1)}><RotateCcw size={13} /> Try Again</button>
    </div> : null}
    {state.phase === "specific" && query.kind === "specific" ? <>
      <p className="meeting-slot">{formatSlot(query, timeZone)}</p>
      <p className={`meeting-slot-status meeting-slot-${state.check.status}`}>
        {state.check.status === "free" ? <><CheckCircle2 size={13} aria-hidden="true" /> You&rsquo;re free</> : null}
        {state.check.status === "conflicting" ? <><CircleAlert size={13} aria-hidden="true" /> {state.conflictTitles.length ? `Conflicts with ${state.conflictTitles.join(", ")}` : "Conflicts with another event"}</> : null}
        {state.check.status === "partiallyChecked" ? <>Free on the calendars that could be checked</> : null}
        {state.check.status === "unverified" ? <>No calendar could be checked</> : null}
      </p>
      <div className="meeting-scheduler-actions">
        <button type="button" onClick={() => onAddToCalendar(query)}><CalendarPlus size={13} /> Add to Calendar</button>
        {state.check.status === "conflicting"
          ? <button type="button" onClick={() => {
            const day = new Date(query.start);
            day.setHours(0, 0, 0, 0);
            setOverride(lookaheadRange(new Date(Math.max(day.getTime(), Date.now()))));
          }}>Find Other Times</button>
          : <button type="button" onClick={() => onConfirmTime(query)}>Reply &ldquo;That Works&rdquo;</button>}
        {moreTimes}
      </div>
    </> : null}
    {state.phase === "range" ? <>
      {state.candidates.length === 0 ? <p className="context-status">No open times in your working hours for this range.</p> : <>
        <div className="meeting-slots" role="group" aria-label="Open times">
          {state.candidates.map((candidate) => {
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
          const chosen = state.candidates.filter((candidate) => selected.has(slotKey(candidate)));
          return <div className="meeting-scheduler-actions">
            <button type="button" disabled={chosen.length === 0} onClick={() => onReplyWithTimes(chosen)}>{intoDraft ? "Insert" : "Draft Reply With"} {chosen.length === 1 ? "This Time" : "These Times"}</button>
            <button type="button" disabled={chosen.length !== 1} title={chosen.length !== 1 ? "Select one time to add it" : undefined} onClick={() => onAddToCalendar(chosen[0])}><CalendarPlus size={13} /> Add to Calendar</button>
            {moreTimes}
          </div>;
        })()}
      </>}
      {state.candidates.length === 0 ? <div className="meeting-scheduler-actions">{moreTimes}</div> : null}
    </> : null}
    {assumedZone}
  </div>;
}
