import { useLayoutEffect, useMemo, type RefObject } from "react";
import { HOUR_HEIGHT, startOfLocalDay } from "./calendarTime";
import { DayTimeGrid, type DayGridEventDecoration, type DayGridPreview } from "./DayTimeGrid";
import type { ScheduleEvent } from "./domain";

/**
 * Today's day view: Calendar's time grid for today only, scrolled to now on
 * open. Choosing an event is the caller's business (prepare a meeting, or
 * open anything else in Calendar).
 */
export function TodayDayView({ events, now, loading, error, onRetry, selectedEvent, onChooseEvent, eventDecoration, gridRef, preview, dragging }: {
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
}) {
  const day = startOfLocalDay(now);
  const dayKey = day.getTime();
  // One stable array per day, so the grid's layouts aren't recomputed every minute.
  const days = useMemo(() => [new Date(dayKey)], [dayKey]);
  useLayoutEffect(() => {
    const grid = gridRef.current;
    if (!grid) return;
    const minutes = new Date().getHours() * 60 + new Date().getMinutes();
    grid.scrollTop = Math.max(0, minutes / 60 * HOUR_HEIGHT - grid.clientHeight / 3);
  }, [gridRef, dayKey]);
  return <div className={`today-day-view calendar-week-main${dragging ? " calendar-week-task-drag" : ""}`} role="group" aria-label="Day view">
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
    />
  </div>;
}
