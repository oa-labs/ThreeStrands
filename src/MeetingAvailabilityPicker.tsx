import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, RefreshCw } from "lucide-react";
import { mailClient } from "./data/client";
import type { MeetingAvailabilityRequest, MeetingAvailabilityResult } from "./domain";
import { errorMessage } from "./errors";
import { readAvailabilityPreferencesFor } from "./settings";
import { schedulingConflicts, schedulingCoverage, schedulingDay, schedulingDayStart, schedulingWeek, shiftSchedulingDay, SCHEDULING_STEP_MINUTES } from "./meetingAvailability";
import { ICON_SIZE } from "./iconSizes";

type Props = {
  eventId: string;
  accountId: string;
  calendarId: string;
  attendees: string[];
  originalStart: string;
  originalEnd: string;
  start: number;
  end: number;
  disabled: boolean;
  onSelect(start: number, end: number): void;
};

export function MeetingAvailabilityPicker({ eventId, accountId, calendarId, attendees, originalStart, originalEnd, start, end, disabled, onSelect }: Props) {
  const [preferences] = useState(() => readAvailabilityPreferencesFor(accountId));
  const timeZone = preferences.timeZone;
  const [day, setDay] = useState(() => schedulingDay(Math.max(start, Date.now()), timeZone));
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<{ key: string; result?: MeetingAvailabilityResult; error?: string }>();
  const scroll = useRef<HTMLDivElement>(null);
  const week = schedulingWeek(day);
  const duration = (end - start) / 60_000;
  const attendeesKey = JSON.stringify([...new Set(attendees.map((email) => email.toLowerCase()))].sort());
  const request = useMemo<MeetingAvailabilityRequest>(() => ({
    eventId, accountId, calendarId, attendees: JSON.parse(attendeesKey) as string[],
    rangeStart: new Date(schedulingDayStart(week, timeZone)).toISOString(),
    rangeEnd: new Date(schedulingDayStart(shiftSchedulingDay(week, 7), timeZone)).toISOString(),
    preferences: { ...preferences, defaultDurationMinutes: duration },
  }), [eventId, accountId, calendarId, attendeesKey, week, timeZone, preferences, duration]);
  const key = JSON.stringify(request) + attempt;
  const validDuration = Number.isInteger(duration) && duration >= 5 && duration <= 720;
  useEffect(() => {
    if (!validDuration) return;
    let current = true;
    void mailClient.meetingAvailability(request).then(
      (result) => { if (current) setState({ key, result }); },
      (reason) => { if (current) setState({ key, error: errorMessage(reason) }); },
    );
    return () => { current = false; };
  }, [request, key, validDuration]);
  const result = state?.key === key ? state.result : undefined;
  const error = state?.key === key ? state.error : undefined;
  useEffect(() => {
    if (scroll.current) scroll.current.scrollTop = Number.isFinite(start) && schedulingDay(start, timeZone) === day
      ? Math.max(0, (start - schedulingDayStart(day, timeZone)) / 3_600_000 - 1) * 48 : 8 * 48;
  }, [day, result, start, timeZone]);
  const dayStart = schedulingDayStart(day, timeZone);
  const dayEnd = schedulingDayStart(shiftSchedulingDay(day, 1), timeZone);
  const dayLength = dayEnd - dayStart;
  const timeFormat = new Intl.DateTimeFormat(undefined, { timeZone, hour: "numeric", minute: "2-digit", timeZoneName: "short" });
  const dateFormat = new Intl.DateTimeFormat(undefined, { timeZone, weekday: "short", month: "short", day: "numeric" });
  const hourFormat = new Intl.DateTimeFormat(undefined, { timeZone, hour: "numeric", minute: "2-digit" });
  const time = (at: number) => timeFormat.format(at);
  const date = (at: number) => dateFormat.format(at);
  const slotLabel = (at: number, until: number) => `${date(at)} · ${time(at)} – ${schedulingDay(at, timeZone) === schedulingDay(until, timeZone) ? "" : `${date(until)} · `}${time(until)}`;
  const people = result ? [{ ...result.own, label: "You" }, ...result.guests.map((guest) => ({ ...guest, label: guest.email }))] : [];
  const conflicts = result && Number.isFinite(start) && Number.isFinite(end) ? schedulingConflicts(result, start, end) : [];
  const labels = conflicts.map((person) => person === result?.own ? "your calendar" : person.email);
  const position = (at: number) => `${(at - dayStart) / dayLength * 100}%`;
  const ticks = Array.from({ length: Math.ceil(dayLength / (SCHEDULING_STEP_MINUTES * 60_000)) }, (_, index) => dayStart + index * SCHEDULING_STEP_MINUTES * 60_000);
  return <section className="meeting-availability" aria-label="Find a time" aria-busy={validDuration && !result && !error}>
    <div className="meeting-availability-toolbar">
      <button type="button" className="btn-icon" aria-label="Previous week" disabled={disabled} onClick={() => setDay(shiftSchedulingDay(day, -7))}><ChevronLeft size={ICON_SIZE.sm} /></button>
      <strong>{date(schedulingDayStart(week, timeZone))} – {date(schedulingDayStart(shiftSchedulingDay(week, 6), timeZone))}</strong>
      <button type="button" className="btn-icon" aria-label="Next week" disabled={disabled} onClick={() => setDay(shiftSchedulingDay(day, 7))}><ChevronRight size={ICON_SIZE.sm} /></button>
      <span>{duration} minutes · {timeZone}</span>
      <button type="button" className="btn btn-sm" disabled={disabled} onClick={() => setAttempt((value) => value + 1)}><RefreshCw size={ICON_SIZE.sm} /> Refresh</button>
    </div>
    <div className="meeting-availability-days" role="group" aria-label="Choose day">
      {Array.from({ length: 7 }, (_, index) => shiftSchedulingDay(week, index)).map((value) => <button type="button" className="btn btn-sm" key={value} aria-pressed={day === value} disabled={disabled} onClick={() => setDay(value)}>{date(schedulingDayStart(value, timeZone))}</button>)}
    </div>
    {!validDuration ? <p role="status">Choose a meeting duration between 5 minutes and 12 hours to find a time.</p> : error ? <p role="alert">Could not check availability: {error} <button type="button" className="btn-link" onClick={() => setAttempt((value) => value + 1)}>Try again</button></p> : !result ? <p role="status">Checking your calendars and guests’ availability…</p> : <>
      <p className="modal-form-context" role="status">{schedulingCoverage(result)}</p>
      <div className="meeting-availability-layout">
        <div className="meeting-availability-calendar">
          <div className="meeting-availability-lane-headings" style={{ gridTemplateColumns: `4.5rem repeat(${people.length}, minmax(9rem, 1fr))`, minWidth: `${4.5 + people.length * 9}rem` }}>
            <span>Time</span>{people.map((person) => <span key={person.email} title={person.email}>{person.label}<small>{person.checked ? "Calendar checked" : "Availability unknown"}</small></span>)}
          </div>
          <div className="meeting-availability-scroll" ref={scroll} style={{ minWidth: `${4.5 + people.length * 9}rem` }}>
            <div className="meeting-availability-timeline" style={{ height: dayLength / 3_600_000 * 48 }}>
              <div className="meeting-availability-time-labels" aria-hidden="true">{ticks.filter((_, index) => index % 4 === 0).map((at) => <span key={at} style={{ top: position(at) }}>{hourFormat.format(at)}</span>)}</div>
              <div className="meeting-availability-lanes" style={{ gridTemplateColumns: `repeat(${people.length}, minmax(9rem, 1fr))` }}>
                {people.map((person, index) => <div className={`meeting-availability-lane${person.checked ? "" : " is-unknown"}`} key={person.email} aria-label={`${person.label} availability`}>
                  {person.busy.map((interval, intervalIndex) => {
                    const from = Math.max(dayStart, Date.parse(interval.start));
                    const until = Math.min(dayEnd, Date.parse(interval.end));
                    return until > from ? <span className={`meeting-availability-busy busy-color-${index % 4}`} key={intervalIndex} style={{ top: position(from), height: `${(until - from) / dayLength * 100}%` }} role="img" aria-label={`${person.label}: Busy, ${slotLabel(from, until)}`}>Busy</span> : null;
                  })}
                </div>)}
                <div className="meeting-availability-time-targets" role="group" aria-label={`Choose a time on ${date(dayStart)}`}>
                  {ticks.map((at) => <button type="button" className="meeting-availability-slot" key={at} aria-label={`Select ${slotLabel(at, at + end - start)}`} disabled={disabled || at <= Date.now()} style={{ top: position(at), height: `${SCHEDULING_STEP_MINUTES * 60_000 / dayLength * 100}%` }} onClick={() => onSelect(at, at + end - start)} />)}
                </div>
                {end > dayStart && start < dayEnd ? <span className="meeting-availability-proposal" style={{ top: position(Math.max(start, dayStart)), height: `${(Math.min(end, dayEnd) - Math.max(start, dayStart)) / dayLength * 100}%` }}>Proposed · {time(start)} – {time(end)}</span> : null}
              </div>
            </div>
          </div>
        </div>
        <aside className="meeting-availability-suggestions" aria-label="Suggested times">
          <h3>Suggested times</h3>
          <p className="modal-form-context">Within your working hours. Guests’ working hours may differ.</p>
          {!result.candidates.length ? <p>No open times found in your working hours this week.</p> : result.candidates.map((candidate) => <button type="button" key={candidate.start} className="meeting-slot-option" aria-pressed={Date.parse(candidate.start) === start && Date.parse(candidate.end) === end} disabled={disabled} onClick={() => { setDay(schedulingDay(Date.parse(candidate.start), timeZone)); onSelect(Date.parse(candidate.start), Date.parse(candidate.end)); }}>{slotLabel(Date.parse(candidate.start), Date.parse(candidate.end))}</button>)}
        </aside>
      </div>
      <p className="modal-form-context">The current meeting ({slotLabel(Date.parse(originalStart), Date.parse(originalEnd))}) stays marked busy. Its time is not subtracted because another commitment could overlap it.</p>
      {labels.length ? <p className="form-error" role="status">Proposed time overlaps {labels.join(", ")}. You can still choose it.</p> : null}
    </>}
  </section>;
}
