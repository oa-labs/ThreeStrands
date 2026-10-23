import { eventDate, occursOnDay } from "./calendarTime";
import type { CalendarOption, ScheduleEvent } from "./domain";

/** The user said they won't go. */
export function isDeclined(event: ScheduleEvent): boolean {
  return event.responseStatus === "declined";
}

/** A timed event that has ended by `now`; all-day events are never past during their day. */
export function isPastEvent(event: ScheduleEvent, now: Date): boolean {
  return !event.allDay && new Date(event.end).getTime() <= now.getTime();
}

/**
 * One event per meeting when the same invitation sits on more than one
 * calendar: same title, start, and end. The copy the user hasn't declined is
 * kept, so a meeting declined on a personal calendar but still on the work
 * one shows once, as going. Order is otherwise unchanged.
 */
export function mergeCalendarCopies(events: readonly ScheduleEvent[]): ScheduleEvent[] {
  const kept = new Map<string, ScheduleEvent>();
  const order: string[] = [];
  for (const event of events) {
    const key = [
      event.title.trim().toLocaleLowerCase(),
      event.allDay ? event.start : new Date(event.start).getTime(),
      event.allDay ? event.end : new Date(event.end).getTime(),
    ].join("|");
    const current = kept.get(key);
    if (!current) {
      kept.set(key, event);
      order.push(key);
    } else if (isDeclined(current) && !isDeclined(event)) {
      kept.set(key, event);
    }
  }
  return order.map((key) => kept.get(key)!);
}

/** A clock time written the way event times are, such as "4:51 pm". */
export function formatClockTime(date: Date): string {
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).formatToParts(date)
    .map((part) => part.type === "dayPeriod" ? part.value.toLocaleLowerCase() : part.value)
    .join("");
}

/**
 * A day's events on the selected calendars, as an agenda lists them: all-day
 * first, then by start time, with copies on more than one calendar merged.
 */
export function agendaEventsFor(events: readonly ScheduleEvent[], day: Date, calendars: readonly CalendarOption[]): ScheduleEvent[] {
  const selected = calendars.filter((calendar) => calendar.selected);
  return mergeCalendarCopies(events.filter((event) => occursOnDay(event, day) && selected.some((calendar) =>
    calendar.accountId === event.accountId && (!event.calendarId || calendar.id === event.calendarId)))
    .sort((left, right) => Number(right.allDay) - Number(left.allDay)
      || eventDate(left).getTime() - eventDate(right).getTime() || left.title.localeCompare(right.title)));
}
