// The day around an offered time, so the user can judge the breaks and
// places on either side before offering it.

import { useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, ScheduleEvent } from "./domain";
import { isDeclined, mergeCalendarCopies } from "./agendaEvents";
import { occursOnDay } from "./calendarTime";
import { readScheduleCache, refreshScheduleCache } from "./calendarScheduleCache";
import { scheduleRequestFor } from "./CalendarSidebar";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { errorMessage } from "./errors";
import { TIGHT_BREAK_MINUTES } from "./scheduling";

type Slot = Pick<AvailabilityCandidate, "start" | "end">;

/** The day's schedule events last read, if any, to show while they are read again. */
function cachedEventsOnDay(date: Date): ScheduleEvent[] | null {
  return readScheduleCache(scheduleRequestFor(date))?.events ?? null;
}

/** The day's schedule events, from the cache. */
export async function eventsOnDay(date: Date): Promise<ScheduleEvent[]> {
  const request = scheduleRequestFor(date);
  const result = await refreshScheduleCache(request, () => mailClient.listScheduleEvents(request.timeMin, request.timeMax, request.timeZone));
  return result.events;
}

const rangeRequest = (start: string, end: string) => ({ timeMin: start, timeMax: end, timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC" });

/** The schedule events across a search's range last read, if any, to show while they are read again. */
export function cachedEventsInRange(start: string, end: string): ScheduleEvent[] | null {
  return readScheduleCache(rangeRequest(start, end))?.events ?? null;
}

/** The schedule events across a search's range, read once for all its offered times. */
export async function eventsInRange(start: string, end: string): Promise<ScheduleEvent[]> {
  const request = rangeRequest(start, end);
  const result = await refreshScheduleCache(request, () => mailClient.listScheduleEvents(request.timeMin, request.timeMax, request.timeZone));
  return result.events;
}

/**
 * Whether an offered time leaves too little break beside another meeting,
 * such as "Back-to-back" with no break at all, so it shows before the day
 * is opened. Null when every break is at least the tight limit.
 */
export function breakWarning(events: readonly ScheduleEvent[], slot: Slot): string | null {
  const breaks = slotNeighborhood(events, slot, false).rows.flatMap((row) => row.kind === "break" ? [row.minutes] : []);
  if (breaks.some((minutes) => minutes <= 0)) return "Back-to-back";
  if (breaks.some((minutes) => minutes < TIGHT_BREAK_MINUTES)) return "Short break";
  return null;
}

/** A length of free time, such as "30 min" or "1 hr 15 min". */
function formatBreak(minutes: number): string {
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return [hours ? `${hours} hr` : "", rest ? `${rest} min` : ""].filter(Boolean).join(" ");
}

function breakText(minutes: number, other: string, side: "before" | "after"): string {
  if (minutes === 0) return side === "before" ? `Right after ${other}` : `Right before ${other}`;
  return `${formatBreak(minutes)} ${side === "before" ? "after" : "before"} ${other}`;
}

type Row =
  | { kind: "event"; event: ScheduleEvent; overlaps: boolean }
  | { kind: "proposed" }
  | { kind: "break"; minutes: number; text: string };

/**
 * What the day holds around a slot: the event that ends before it, any that
 * overlap it without making it busy, and the event that starts after it,
 * with the free time between. The full day adds the rest and all-day events.
 */
export function slotNeighborhood(events: readonly ScheduleEvent[], slot: Slot, full: boolean): { rows: Row[]; hidden: number } {
  const start = Date.parse(slot.start);
  const end = Date.parse(slot.end);
  const day = mergeCalendarCopies(events.filter((event) => occursOnDay(event, new Date(slot.start)) && !isDeclined(event)));
  const allDay = day.filter((event) => event.allDay);
  const timed = day.filter((event) => !event.allDay).sort((left, right) => Date.parse(left.start) - Date.parse(right.start));
  const overlapping = timed.filter((event) => Date.parse(event.start) < end && Date.parse(event.end) > start);
  const before = timed.filter((event) => Date.parse(event.end) <= start)
    .reduce<ScheduleEvent | null>((latest, event) => !latest || Date.parse(event.end) >= Date.parse(latest.end) ? event : latest, null);
  const after = timed.find((event) => Date.parse(event.start) >= end) ?? null;
  const near = new Set([before, ...overlapping, after].filter(Boolean));
  const shown = full ? timed : timed.filter((event) => near.has(event));

  const rows: Row[] = full ? allDay.map((event) => ({ kind: "event" as const, event, overlaps: false })) : [];
  const earlier = shown.filter((event) => Date.parse(event.start) < start || (Date.parse(event.start) === start && overlapping.includes(event)));
  const later = shown.filter((event) => !earlier.includes(event));
  rows.push(...earlier.map((event) => ({ kind: "event" as const, event, overlaps: overlapping.includes(event) })));
  if (before && overlapping.length === 0) {
    const minutes = Math.round((start - Date.parse(before.end)) / 60_000);
    rows.push({ kind: "break", minutes, text: breakText(minutes, before.title, "before") });
  }
  rows.push({ kind: "proposed" });
  if (after) {
    const minutes = Math.round((Date.parse(after.start) - end) / 60_000);
    rows.push({ kind: "break", minutes, text: breakText(minutes, after.title, "after") });
  }
  rows.push(...later.map((event) => ({ kind: "event" as const, event, overlaps: overlapping.includes(event) })));
  return { rows, hidden: full ? 0 : allDay.length + timed.length - shown.length };
}

function formatSpan(start: string, end: string, timeZone: string): string {
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone }).formatRange(new Date(start), new Date(end));
}

type LoadState = { phase: "loading" } | { phase: "ready"; events: ScheduleEvent[] } | { phase: "error"; message: string };

/**
 * An offered time in its day: the meetings on either side, where they are,
 * and how much free time separates them. Read from the calendar only; the
 * AI provider never sees it.
 */
export function SlotDayPreview({ id, slot, timeZone }: { id: string; slot: Slot; timeZone: string }) {
  const colors = useCalendarColors();
  const [state, setState] = useState<LoadState>(() => {
    const cached = cachedEventsOnDay(new Date(slot.start));
    return cached ? { phase: "ready", events: cached } : { phase: "loading" };
  });
  const [full, setFull] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let current = true;
    // A day read before stays on screen while it is read again.
    const cached = cachedEventsOnDay(new Date(slot.start));
    setState(cached ? { phase: "ready", events: cached } : { phase: "loading" });
    eventsOnDay(new Date(slot.start))
      .then((events) => { if (current) setState({ phase: "ready", events }); })
      .catch((reason) => { if (current) setState({ phase: "error", message: errorMessage(reason) }); });
    return () => { current = false; };
  }, [slot.start, attempt]);

  const dayName = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric", timeZone }).format(new Date(slot.start));
  const view = state.phase === "ready" ? slotNeighborhood(state.events, slot, full) : null;
  const alone = view !== null && view.rows.length === 1 && view.hidden === 0;
  return <section id={id} className="slot-day-preview" aria-label={dayName}>
    {state.phase === "loading" ? <p className="context-status" role="status">Loading {dayName}…</p> : null}
    {state.phase === "error" ? <p className="context-status" role="alert">
      {state.message} <button type="button" className="btn-link" onClick={() => setAttempt((value) => value + 1)}>Try Again</button>
    </p> : null}
    {view ? <>
      <ol className="slot-day-rows">
        {view.rows.map((row, index) => {
          if (row.kind === "proposed") {
            return <li key="proposed" className="slot-day-row slot-day-proposed">
              <span className="slot-day-title">This time</span>
              <span className="slot-day-meta">{formatSpan(slot.start, slot.end, timeZone)}</span>
            </li>;
          }
          if (row.kind === "break") {
            return <li key={`break-${index}`} className={`slot-day-break${row.minutes < TIGHT_BREAK_MINUTES ? " slot-day-break-tight" : ""}`}>{row.text}</li>;
          }
          const { event } = row;
          // Each meeting is a card like the week's; a long place fits one line, in full on hover.
          return <li key={`${event.accountId}:${event.calendarId ?? ""}:${event.id}`} className="slot-day-row slot-day-event" style={calendarColorStyle(colors, event.accountId, event.calendarId)}>
            <span className="slot-day-title">{event.title}</span>
            <span className="slot-day-meta" title={event.location ?? undefined}>
              {[event.allDay ? "All day" : formatSpan(event.start, event.end, timeZone), event.location].filter(Boolean).join(" · ")}
            </span>
            {row.overlaps ? <span className="slot-day-detail">Overlaps, but doesn’t block this time</span> : null}
          </li>;
        })}
      </ol>
      {alone ? <p className="context-status">Nothing else on your calendar that day.</p> : null}
      {view.hidden > 0 || full ? <button type="button" className="btn-link" onClick={() => setFull((value) => !value)}>
        {full ? "Show Less" : "Show Full Day"}
      </button> : null}
    </> : null}
  </section>;
}
