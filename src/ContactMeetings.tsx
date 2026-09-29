import { CalendarDays } from "lucide-react";
import { useState } from "react";
import type { ScheduleEvent } from "./domain";
import { addDays, eventDate, formatEventDate, formatEventTime, startOfLocalDay } from "./calendarTime";
import { useCalendarSchedule } from "./useCalendarSchedule";

/** How far ahead the context panel looks for meetings with a person. */
export const UPCOMING_MEETING_DAYS = 30;
export const MAX_UPCOMING_MEETINGS = 3;

function eventEnd(event: ScheduleEvent): number {
  if (!event.allDay) return new Date(event.end).getTime();
  const [year, month, day] = event.end.split("-").map(Number);
  return new Date(year, month - 1, day).getTime();
}

/**
 * The next few calendar events that include one of the person's addresses.
 * Renders nothing when there are none or the schedule can't be loaded, since
 * this is supplemental context; events from calendars that did load still show
 * when another calendar fails.
 */
export function ContactMeetings({ addresses, timeZone, onOpenEvent }: {
  addresses: string[];
  timeZone: string;
  onOpenEvent(event: ScheduleEvent): void;
}) {
  // A range fixed for the panel's lifetime keeps the schedule cache warm
  // while the reader moves between conversations.
  const [range] = useState(() => {
    const start = startOfLocalDay(new Date());
    return { timeMin: start.toISOString(), timeMax: addDays(start, UPCOMING_MEETING_DAYS).toISOString() };
  });
  const { events } = useCalendarSchedule({ ...range, timeZone });
  const people = new Set(addresses.map((address) => address.toLocaleLowerCase()));
  const now = Date.now();
  const meetings = events
    .filter((event) => eventEnd(event) > now && event.attendees?.some((attendee) => people.has(attendee)))
    .sort((left, right) => eventDate(left).getTime() - eventDate(right).getTime())
    .slice(0, MAX_UPCOMING_MEETINGS);
  if (meetings.length === 0) return null;

  return <section className="context-section context-meetings" aria-label="Upcoming meetings">
    <header className="context-section-header"><h3>Upcoming meetings</h3></header>
    {meetings.map((event) => (
      <button type="button" key={event.id} className="context-meeting" onClick={() => onOpenEvent(event)}>
        <CalendarDays size={14} aria-hidden="true" />
        <span>
          <strong>{event.title}</strong>
          <small>{formatEventDate(event)} · {formatEventTime(event)}</small>
        </span>
      </button>
    ))}
  </section>;
}
