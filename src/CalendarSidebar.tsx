import { ChevronLeft, ChevronRight, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { ScheduleEvent } from "./domain";
import { useEscapeDismiss } from "./useEscapeDismiss";

const HOUR_HEIGHT = 64;
const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

export function scheduleRequestFor(date: Date) {
  const start = startOfLocalDay(date);
  const end = new Date(start);
  end.setDate(end.getDate() + 1);
  return {
    timeMin: start.toISOString(),
    timeMax: end.toISOString(),
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  };
}

function timeZoneLabel(date: Date): string {
  const part = new Intl.DateTimeFormat(undefined, {
    timeZoneName: "short",
    hour: "numeric",
  })
    .formatToParts(date)
    .find((candidate) => candidate.type === "timeZoneName");
  return part?.value ?? "UTC";
}

function hourLabel(hour: number): string {
  if (hour === 0) return "12 am";
  if (hour === 12) return "12 pm";
  return `${hour > 12 ? hour - 12 : hour} ${hour >= 12 ? "pm" : "am"}`;
}

function eventMinutes(value: string): number {
  const date = new Date(value);
  return date.getHours() * 60 + date.getMinutes();
}

function formatEventTime(event: ScheduleEvent): string {
  if (event.allDay) return "All day";
  const formatter = new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  return `${formatter.format(new Date(event.start))}–${formatter.format(new Date(event.end))}`;
}

export function CalendarSidebar({ onClose }: { onClose(): void }) {
  const [date, setDate] = useState(() => startOfLocalDay(new Date()));
  const [events, setEvents] = useState<ScheduleEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  useEscapeDismiss(onClose);

  const load = useCallback(async (target: Date) => {
    setLoading(true);
    setError(null);
    const request = scheduleRequestFor(target);
    try {
      setEvents(await mailClient.listScheduleEvents(
        request.timeMin,
        request.timeMax,
        request.timeZone,
      ));
    } catch (reason) {
      setEvents([]);
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(date);
  }, [date, load]);

  useEffect(() => {
    if (gridRef.current) gridRef.current.scrollTop = 7 * HOUR_HEIGHT;
  }, []);

  const timedEvents = useMemo(() => events.filter((event) => !event.allDay), [events]);
  const allDayEvents = useMemo(() => events.filter((event) => event.allDay), [events]);
  const moveDay = (offset: number) => {
    setDate((current) => {
      const next = new Date(current);
      next.setDate(next.getDate() + offset);
      return next;
    });
  };

  return (
    <aside className="calendar-sidebar" aria-label="Calendar schedule">
      <header className="calendar-sidebar-header">
        <h2>{new Intl.DateTimeFormat(undefined, {
          weekday: "short",
          month: "short",
          day: "numeric",
        }).format(date)}</h2>
        <div>
          <button type="button" aria-label="Previous day" onClick={() => moveDay(-1)}>
            <ChevronLeft size={18} />
          </button>
          <button type="button" aria-label="Next day" onClick={() => moveDay(1)}>
            <ChevronRight size={18} />
          </button>
          <button type="button" aria-label="Close calendar" onClick={onClose}>
            <X size={18} />
          </button>
        </div>
      </header>
      {allDayEvents.length > 0 ? (
        <div className="calendar-all-day" aria-label="All-day events">
          <span>All day</span>
          <div>
            {allDayEvents.map((event) => <strong key={`${event.accountId}:${event.id}`}>{event.title}</strong>)}
          </div>
        </div>
      ) : null}
      <div className="calendar-timezone">{timeZoneLabel(date)}</div>
      <div className="calendar-grid-scroll" ref={gridRef}>
        <div className="calendar-grid">
          <div className="calendar-hour-labels" aria-hidden="true">
            {HOURS.map((hour) => (
              <span key={hour} style={{ top: hour * HOUR_HEIGHT }}>{hourLabel(hour)}</span>
            ))}
          </div>
          <div className="calendar-day-column">
            {HOURS.map((hour) => <div className="calendar-hour-line" key={hour} />)}
            {timedEvents.map((event) => {
              const start = eventMinutes(event.start);
              const end = eventMinutes(event.end);
              const duration = Math.max(24, ((end - start) / 60) * HOUR_HEIGHT);
              return (
                <article
                  className="calendar-schedule-event"
                  key={`${event.accountId}:${event.id}`}
                  style={{
                    top: (start / 60) * HOUR_HEIGHT,
                    height: duration,
                  }}
                  title={`${event.title}, ${formatEventTime(event)}`}
                >
                  <strong>{event.title}</strong>
                  <span>{formatEventTime(event)}</span>
                </article>
              );
            })}
          </div>
          {loading ? <p className="calendar-grid-status">Loading schedule…</p> : null}
          {!loading && error ? (
            <div className="calendar-grid-status calendar-grid-error" role="alert">
              <p>Calendar couldn’t be loaded.</p>
              <button type="button" onClick={() => void load(date)}>
                <RefreshCw size={14} /> Try again
              </button>
            </div>
          ) : null}
          {!loading && !error && events.length === 0 ? (
            <p className="calendar-grid-status">No events scheduled.</p>
          ) : null}
        </div>
      </div>
    </aside>
  );
}
