import { useLayoutEffect, useMemo, type CSSProperties, type RefObject } from "react";
import { HOUR_HEIGHT, startOfLocalDay } from "./calendarTime";
import { DayTimeGrid, type DayGridEventDecoration, type DayGridPreview, type DayGridShade } from "./DayTimeGrid";
import type { ScheduleEvent } from "./domain";
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

/**
 * Today's day view: Calendar's time grid for today only, opened on the
 * working day (see `dayViewLayout`). Choosing an event is the caller's
 * business (prepare a meeting, or open anything else in Calendar).
 */
export function TodayDayView({ events, now, loading, error, onRetry, selectedEvent, onChooseEvent, eventDecoration, gridRef, preview, dragging, workingHours = null }: {
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
}) {
  const day = startOfLocalDay(now);
  const dayKey = day.getTime();
  // One stable array per day, so the grid's layouts aren't recomputed every minute.
  const days = useMemo(() => [new Date(dayKey)], [dayKey]);
  // Laid out from the time Today opened (or the day turned), not every minute.
  const layout = useMemo(() => dayViewLayout(workingHours, new Date(Math.max(Date.now(), dayKey))), [workingHours, dayKey]);
  useLayoutEffect(() => {
    const grid = gridRef.current;
    if (!grid) return;
    if (layout.scrollMinutes !== null) { grid.scrollTop = layout.scrollMinutes / 60 * HOUR_HEIGHT; return; }
    const minutes = new Date().getHours() * 60 + new Date().getMinutes();
    grid.scrollTop = Math.max(0, minutes / 60 * HOUR_HEIGHT - grid.clientHeight / 3);
  }, [gridRef, dayKey, layout.scrollMinutes]);
  const style = layout.heightPx === null ? undefined : { "--today-day-view-grid-h": `${layout.heightPx}px` } as CSSProperties;
  return <div className={`today-day-view calendar-week-main${layout.heightPx === null ? "" : " today-day-view-working"}${dragging ? " calendar-week-task-drag" : ""}`} role="group" aria-label="Day view" style={style}>
    {error ? <div className="calendar-error-notice" role="alert">
      <p>Today's calendar couldn't be loaded, so free time is unknown.</p>
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
    />
  </div>;
}
