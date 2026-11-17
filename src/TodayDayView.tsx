import { useCallback, useLayoutEffect, useMemo, useState, type CSSProperties, type RefObject } from "react";
import { HOUR_HEIGHT, eventDayBounds, occursOnDay, startOfLocalDay } from "./calendarTime";
import { formatClockTime } from "./agendaEvents";
import { DayTimeGrid, type DayGridEventDecoration, type DayGridPreview, type DayGridShade } from "./DayTimeGrid";
import type { ScheduleEvent } from "./domain";
import { isWorkingLocation } from "./WorkingLocation";
import { workingSpans } from "./todayBooking";
import type { WorkingHours } from "./todayInsights";

/** Room kept above now when the grid opens inside working hours, so the current period shows. */
export const DAY_VIEW_LEAD_MINUTES = 30;
/** Room below the end of working hours when sizing the grid to the working day. */
export const DAY_VIEW_TAIL_MINUTES = 30;

/**
 * Where Today's day view opens and how tall it is. With working hours, the
 * top is their start, or a little before now once now is inside them; the
 * grid is as tall as the working day, so what follows (Tomorrow) stays in
 * view, and evening hours scroll inside it. Outside time is shaded. Without
 * working hours, it opens on now at its old height and shades nothing.
 */
export function dayViewLayout(workingHours: WorkingHours | null, now: Date): { scrollMinutes: number | null; heightPx: number | null; shade: DayGridShade[] } {
  const spans = workingSpans(workingHours, now);
  if (!spans?.length) return { scrollMinutes: null, heightPx: null, shade: [] };
  const start = spans[0].startMinutes;
  const end = spans.at(-1)!.endMinutes;
  const current = now.getHours() * 60 + now.getMinutes();
  const scrollMinutes = current > start && current < end ? Math.max(start, current - DAY_VIEW_LEAD_MINUTES) : start;
  const shade: DayGridShade[] = [];
  let cursor = 0;
  for (const span of spans) {
    if (span.startMinutes > cursor) shade.push({ dayIndex: 0, startMinutes: cursor, endMinutes: span.startMinutes });
    cursor = Math.max(cursor, span.endMinutes);
  }
  if (cursor < 24 * 60) shade.push({ dayIndex: 0, startMinutes: cursor, endMinutes: 24 * 60 });
  return { scrollMinutes, heightPx: Math.round((end - start + DAY_VIEW_TAIL_MINUTES) / 60 * HOUR_HEIGHT), shade };
}

/** A timed event's place on the grid, in minutes after midnight. */
export type GridSpan = { startMinutes: number; endMinutes: number };
export type HiddenEvents = {
  above: { count: number; firstMinutes: number } | null;
  below: { count: number; firstMinutes: number } | null;
};
/** Room kept above an event when scrolling to it from an indicator. */
const REVEAL_LEAD_PX = HOUR_HEIGHT / 2;
/** The least height the grid draws for an event, in pixels. */
const MIN_EVENT_PX = 20;

/** The timed events wholly scrolled out of view above or below the viewport; one partly in view is visible. */
export function hiddenEvents(spans: GridSpan[], scrollTop: number, viewportHeight: number): HiddenEvents {
  const result: HiddenEvents = { above: null, below: null };
  if (viewportHeight <= 0) return result;
  for (const { startMinutes, endMinutes } of spans) {
    const top = startMinutes / 60 * HOUR_HEIGHT;
    const bottom = top + Math.max(MIN_EVENT_PX, (endMinutes - startMinutes) / 60 * HOUR_HEIGHT);
    const side = bottom <= scrollTop ? "above" : top >= scrollTop + viewportHeight ? "below" : null;
    if (!side) continue;
    const seen = result[side];
    result[side] = { count: (seen?.count ?? 0) + 1, firstMinutes: Math.min(seen?.firstMinutes ?? startMinutes, startMinutes) };
  }
  return result;
}

/** A time on the grid, written as Today writes times ("7:00 pm"). */
function clockLabel(minutes: number): string {
  return formatClockTime(new Date(2000, 0, 1, Math.floor(minutes / 60), minutes % 60));
}

/**
 * Today's day view: Calendar's time grid for today only, opened on the
 * working day (see `dayViewLayout`). Choosing an event is the caller's
 * business (prepare a meeting, or open anything else in Calendar).
 */
export function TodayDayView({ events, now, loading, error, onRetry, selectedEvent, onChooseEvent, eventDecoration, gridRef, preview, dragging, workingHours = null, day: shown, label = "Day view" }: {
  events: ScheduleEvent[];
  now: Date;
  loading: boolean;
  error: string | null;
  onRetry(): void;
  selectedEvent: ScheduleEvent | null;
  onChooseEvent(event: ScheduleEvent, trigger: HTMLButtonElement): void;
  eventDecoration(event: ScheduleEvent): DayGridEventDecoration | null;
  gridRef: RefObject<HTMLDivElement | null>;
  preview: DayGridPreview | null;
  dragging: boolean;
  workingHours?: WorkingHours | null;
  /** The day shown, such as tomorrow while planning it; today by default. */
  day?: Date;
  label?: string;
}) {
  const day = startOfLocalDay(shown ?? now);
  const dayKey = day.getTime();
  // One stable array per day, so the grid's layouts aren't recomputed every minute.
  const days = useMemo(() => [new Date(dayKey)], [dayKey]);
  // Laid out from the time Today opened (or the day turned), not every minute.
  const layout = useMemo(() => dayViewLayout(workingHours, new Date(Math.max(Date.now(), dayKey))), [workingHours, dayKey]);
  const [metrics, setMetrics] = useState({ scrollTop: 0, height: 0 });
  const measure = useCallback(() => {
    const grid = gridRef.current;
    if (grid) setMetrics((seen) => seen.scrollTop === grid.scrollTop && seen.height === grid.clientHeight ? seen : { scrollTop: grid.scrollTop, height: grid.clientHeight });
  }, [gridRef]);
  useLayoutEffect(() => {
    const grid = gridRef.current;
    if (!grid) return;
    if (layout.scrollMinutes !== null) grid.scrollTop = layout.scrollMinutes / 60 * HOUR_HEIGHT;
    else {
      const minutes = new Date().getHours() * 60 + new Date().getMinutes();
      grid.scrollTop = Math.max(0, minutes / 60 * HOUR_HEIGHT - grid.clientHeight / 3);
    }
    measure();
  }, [gridRef, dayKey, layout.scrollMinutes, layout.heightPx, measure]);
  useLayoutEffect(() => {
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [measure]);
  const spans = useMemo(() => events
    .filter((event) => !event.allDay && !isWorkingLocation(event) && occursOnDay(event, days[0]))
    .map((event) => { const bounds = eventDayBounds(event, days[0]); return { startMinutes: bounds.start, endMinutes: bounds.end }; }), [events, days]);
  const hidden = useMemo(() => hiddenEvents(spans, metrics.scrollTop, metrics.height), [spans, metrics]);
  const reveal = (minutes: number) => {
    const grid = gridRef.current;
    if (!grid) return;
    grid.scrollTop = Math.max(0, minutes / 60 * HOUR_HEIGHT - REVEAL_LEAD_PX);
    measure();
  };
  const indicator = (side: "above" | "below") => {
    const found = hidden[side];
    if (!found) return undefined;
    const text = side === "below"
      ? `${found.count} more later, from ${clockLabel(found.firstMinutes)}`
      : `${found.count} earlier, from ${clockLabel(found.firstMinutes)}`;
    return <button type="button" className="btn btn-sm calendar-hidden-events" data-side={side} onClick={() => reveal(found.firstMinutes)}>
      <span aria-hidden="true">{side === "below" ? "↓ " : "↑ "}</span>{text}
    </button>;
  };
  const style = layout.heightPx === null ? undefined : { "--today-day-view-grid-h": `${layout.heightPx}px` } as CSSProperties;
  return <div className={`today-day-view calendar-week-main${layout.heightPx === null ? "" : " today-day-view-working"}${dragging ? " calendar-week-task-drag" : ""}`} role="group" aria-label={label} style={style}>
    {error ? <div className="calendar-error-notice" role="alert">
      <p>{shown ? "The calendar" : "Today's calendar"} couldn't be loaded, so free time is unknown.</p>
      <div className="today-day-view-actions"><button type="button" className="btn btn-sm" onClick={onRetry}>Try Again</button></div>
    </div> : null}
    <DayTimeGrid
      days={days}
      events={events}
      now={now}
      loading={loading}
      selectedEvent={selectedEvent}
      onToggleEvent={onChooseEvent}
      preview={preview}
      scrollRef={gridRef}
      eventDecoration={eventDecoration}
      shade={layout.shade}
      shortTimes
      showDayLabels={false}
      onScroll={measure}
      scrollOverlay={{ top: indicator("above"), bottom: indicator("below") }}
    />
  </div>;
}
