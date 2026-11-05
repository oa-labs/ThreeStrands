// The day around an offered time, so the user can judge the breaks and
// places on either side before offering it.

import { useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, ScheduleEvent } from "./domain";
import { isDeclined, mergeCalendarCopies } from "./agendaEvents";
import { occursOnDay } from "./calendarTime";
import { refreshScheduleCache } from "./calendarScheduleCache";
import { scheduleRequestFor } from "./CalendarSidebar";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { errorMessage } from "./errors";
import { TIGHT_BREAK_MINUTES } from "./scheduling";

type Slot = Pick<AvailabilityCandidate, "start" | "end">;

/** The day's schedule events, from the cache. */
export async function eventsOnDay(date: Date): Promise<ScheduleEvent[]> {
  const request = scheduleRequestFor(date);
  const result = await refreshScheduleCache(request, () => mailClient.listScheduleEvents(request.timeMin, request.timeMax, request.timeZone));
  return result.events;
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
  const [state, setState] = useState<LoadState>({ phase: "loading" });
  const [full, setFull] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let current = true;
    setState({ phase: "loading" });
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
              <span className="slot-day-time">{formatSpan(slot.start, slot.end, timeZone)}</span>
              <span className="slot-day-title">This time</span>
            </li>;
          }
          if (row.kind === "break") {
            return <li key={`break-${index}`} className={`slot-day-break${row.minutes < TIGHT_BREAK_MINUTES ? " slot-day-break-tight" : ""}`}>{row.text}</li>;
          }
          const { event } = row;
          return <li key={`${event.accountId}:${event.calendarId ?? ""}:${event.id}`} className="slot-day-row" style={calendarColorStyle(colors, event.accountId, event.calendarId)}>
            <span className="slot-day-time">{event.allDay ? "All day" : formatSpan(event.start, event.end, timeZone)}</span>
            <span className="slot-day-title">{event.title}</span>
            {row.overlaps || event.location ? <span className="slot-day-detail">{[row.overlaps ? "Overlaps, but doesn’t block this time" : null, event.location].filter(Boolean).join(" · ")}</span> : null}
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
