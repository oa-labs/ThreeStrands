import { useMemo, type CSSProperties, type HTMLAttributes, type KeyboardEvent as ReactKeyboardEvent, type Ref, type UIEvent } from "react";
import {
  HOUR_HEIGHT,
  HOURS,
  addDays,
  formatEventTime,
  hourLabel,
  isSameDay,
  layOutDayEvents,
  occursOnDay,
  startOfLocalDay,
  timeZoneLabel,
} from "./calendarTime";
import { responseLabel } from "./calendarResponse";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import type { ScheduleEvent } from "./domain";
import { distinctWorkingLocations, isWorkingLocation, WorkingLocationChip } from "./WorkingLocation";

/** The grid's time resolution: selections and drops snap to it. */
export const SLOT_MINUTES = 15;
/** Each day column carries this class and a `data-day-index`, which drop targeting reads. */
export const DAY_COLUMN_CLASS = "calendar-week-column";

/** A highlighted stretch of one day column: a selection or a drop preview. */
export type DayGridPreview = { dayIndex: number; startMinutes: number; endMinutes: number; label?: string };

export function sameEvent(left: ScheduleEvent | null, right: ScheduleEvent): boolean {
  return left?.id === right.id && left.accountId === right.accountId && left.calendarId === right.calendarId;
}

/** The slot start, in minutes after midnight, under `clientY` in a day column. */
export function slotAt(clientY: number, column: HTMLElement): number {
  const offset = Math.max(0, Math.min(24 * HOUR_HEIGHT - 1, clientY - column.getBoundingClientRect().top));
  return Math.floor(offset / HOUR_HEIGHT * 60 / SLOT_MINUTES) * SLOT_MINUTES;
}

function weekdayLabel(date: Date): string {
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(date);
  return `${weekday} ${date.getDate()}`;
}

/**
 * The time grid for one to seven days: day labels, all-day events and working
 * locations, and timed events laid out per day with the Now line and a preview.
 * It renders sibling rows for a flex column container to lay out.
 */
export function DayTimeGrid({
  days,
  events,
  now,
  loading = false,
  selectedEvent,
  onToggleEvent,
  onOpenEventContext,
  preview,
  scrollRef,
  onScroll,
  timeZoneAt = days[0],
  columnProps,
}: {
  days: Date[];
  events: ScheduleEvent[];
  /** Places the Now line and marks today's label. */
  now: Date;
  loading?: boolean;
  selectedEvent: ScheduleEvent | null;
  /** An event was clicked; `trigger` is the element to return focus to. */
  onToggleEvent(event: ScheduleEvent, trigger: HTMLButtonElement): void;
  /** Enter on an event: select it and move to its context. */
  onOpenEventContext?(event: ScheduleEvent): void;
  preview?: DayGridPreview | null;
  scrollRef?: Ref<HTMLDivElement>;
  onScroll?(event: UIEvent<HTMLDivElement>): void;
  /** The date whose time zone names the gutter; the first day by default. */
  timeZoneAt?: Date;
  /** Extra attributes for a day column, such as a label and pointer handlers. */
  columnProps?(day: Date, dayIndex: number): HTMLAttributes<HTMLDivElement>;
}) {
  const calendarColors = useCalendarColors();
  const today = startOfLocalDay(now);
  const dayCount = { "--calendar-day-count": days.length } as CSSProperties;
  // Working locations, even one set for part of a day, head the all-day row
  // rather than taking a block of time in the grid.
  const locationsByDay = useMemo(
    () => days.map((day) => distinctWorkingLocations(events.filter((event) => occursOnDay(event, day)))),
    [days, events],
  );
  const allDayByDay = useMemo(
    () => days.map((day) => events.filter((event) => event.allDay && !isWorkingLocation(event) && occursOnDay(event, day))),
    [days, events],
  );
  const hasAllDay = allDayByDay.some((dayEvents) => dayEvents.length > 0) || locationsByDay.some((locations) => locations.length > 0);
  const timedByDay = useMemo(
    () => days.map((day) => layOutDayEvents(events.filter((event) => !event.allDay && !isWorkingLocation(event) && occursOnDay(event, day)), day)),
    [days, events],
  );
  const openOnEnter = (event: ScheduleEvent) => (key: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (key.key === "Enter" && onOpenEventContext) { key.preventDefault(); onOpenEventContext(event); }
  };

  return (
    <>
      <div className="calendar-week-days" role="row" style={dayCount}>
        <span className="calendar-week-gutter" aria-hidden="true" />
        {days.map((day) => (
          <span
            key={day.toDateString()}
            className={`calendar-week-day-label${isSameDay(day, today) ? " calendar-week-day-today" : ""}`}
            aria-current={isSameDay(day, today) ? "date" : undefined}
          >
            {weekdayLabel(day)}
          </span>
        ))}
      </div>
      {hasAllDay ? (
        <div className="calendar-week-all-day" aria-label="All-day events" style={dayCount}>
          <span className="calendar-week-gutter">All day</span>
          {days.map((day, index) => (
            <div key={day.toDateString()}>
              {locationsByDay[index].map((event) => (
                <WorkingLocationChip
                  key={`${event.accountId}:${event.calendarId}:${event.id}`}
                  event={event}
                  data-calendar-event-trigger
                  aria-expanded={sameEvent(selectedEvent, event)}
                  onClick={(click) => onToggleEvent(event, click.currentTarget)}
                  style={calendarColorStyle(calendarColors, event.accountId, event.calendarId)}
                />
              ))}
              {allDayByDay[index].map((event) => (
                <button
                  type="button"
                  className="calendar-all-day-event"
                  key={`${event.accountId}:${event.calendarId}:${event.id}`}
                  data-calendar-event-trigger
                  data-response-status={event.responseStatus ?? undefined}
                  aria-expanded={sameEvent(selectedEvent, event)}
                  onClick={(click) => onToggleEvent(event, click.currentTarget)}
                  onKeyDown={openOnEnter(event)}
                  style={calendarColorStyle(calendarColors, event.accountId, event.calendarId)}
                  title={event.location?.trim() ? `${event.title}, ${event.location.trim()}` : undefined}
                >
                  {event.title}{event.location?.trim() ? `, ${event.location.trim()}` : ""}
                </button>
              ))}
            </div>
          ))}
        </div>
      ) : null}
      <div className="calendar-week-timezone"><span className="calendar-week-gutter">{timeZoneLabel(timeZoneAt)}</span></div>
      <div
        className="calendar-week-scroll"
        tabIndex={-1}
        ref={scrollRef}
        onScroll={onScroll}
      >
        <div className="calendar-week-grid" style={dayCount}>
          <div className="calendar-hour-labels" aria-hidden="true">
            {HOURS.map((hour) => (
              <span key={hour} style={{ top: hour * HOUR_HEIGHT }}>{hourLabel(hour)}</span>
            ))}
          </div>
          {days.map((day, dayIndex) => (
            <div
              {...columnProps?.(day, dayIndex)}
              className={DAY_COLUMN_CLASS}
              key={day.toDateString()}
              data-day-index={dayIndex}
            >
              {HOURS.map((hour) => <div className="calendar-hour-line" key={hour} />)}
              {preview?.dayIndex === dayIndex ? (
                <div className="calendar-create-selection" aria-hidden="true" style={{
                  top: preview.startMinutes / 60 * HOUR_HEIGHT,
                  height: (preview.endMinutes - preview.startMinutes) / 60 * HOUR_HEIGHT,
                }}>{preview.label ? <span>{preview.label}</span> : null}</div>
              ) : null}
              {timedByDay[dayIndex].map(({ event, lane, lanes, span }) => {
                const start = new Date(event.start);
                const end = new Date(event.end);
                const dayStart = startOfLocalDay(day);
                const dayEnd = addDays(dayStart, 1);
                const startMinutes = start <= dayStart ? 0 : start.getHours() * 60 + start.getMinutes();
                const endMinutes = end >= dayEnd ? 24 * 60 : end.getHours() * 60 + end.getMinutes();
                const height = Math.max(20, ((Math.max(startMinutes, endMinutes) - startMinutes) / 60) * HOUR_HEIGHT);
                const durationMinutes = (end.getTime() - start.getTime()) / 60000;
                const location = event.location?.trim();
                const compactLocation = durationMinutes <= 30 && location;
                const separateLocation = height > HOUR_HEIGHT;
                const startTime = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(start);
                const className = [
                  "calendar-schedule-event",
                  durationMinutes <= 30 && "calendar-schedule-event-compact",
                  durationMinutes <= 15 && "calendar-schedule-event-tight",
                ].filter(Boolean).join(" ");
                return (
                  <button
                    type="button"
                    className={className}
                    key={`${event.accountId}:${event.calendarId}:${event.id}`}
                    data-calendar-event-trigger
                    data-response-status={event.responseStatus ?? undefined}
                    aria-expanded={sameEvent(selectedEvent, event)}
                    onClick={(click) => onToggleEvent(event, click.currentTarget)}
                    onKeyDown={openOnEnter(event)}
                    style={{
                      ...calendarColorStyle(calendarColors, event.accountId, event.calendarId),
                      top: (startMinutes / 60) * HOUR_HEIGHT,
                      height,
                      left: `${(lane / lanes) * 100}%`,
                      width: `${(span / lanes) * 100}%`,
                    }}
                    title={`${event.title}, ${formatEventTime(event)}${location ? `, ${location}` : ""}${responseLabel(event) ? `, ${responseLabel(event)}` : ""}`}
                  >
                    <strong>{event.title}{compactLocation ? <span>, {startTime}, {location}</span> : null}</strong>
                    {durationMinutes > 30 ? <span>{formatEventTime(event)}{location && !separateLocation ? ` · ${location}` : ""}</span> : null}
                    {durationMinutes > 30 && location && separateLocation ? <span>{location}</span> : null}
                  </button>
                );
              })}
              {isSameDay(day, today) ? (
                <div
                  className="calendar-now-indicator"
                  data-testid="calendar-now-indicator"
                  aria-hidden="true"
                  style={{ top: ((now.getHours() * 60 + now.getMinutes()) / 60) * HOUR_HEIGHT }}
                />
              ) : null}
            </div>
          ))}
        </div>
        {loading ? <p className="calendar-grid-status">Loading schedule…</p> : null}
      </div>
    </>
  );
}
