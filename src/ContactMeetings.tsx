import { CalendarDays } from "lucide-react";
import { useState } from "react";
import type { ScheduleEvent } from "./domain";
import { addDays, eventDate, formatEventDate, startOfLocalDay } from "./calendarTime";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { responseLabel } from "./calendarResponse";

/** How far ahead the context panel looks for meetings with conversation participants. */
export const UPCOMING_MEETING_DAYS = 30;
export const MAX_UPCOMING_MEETINGS = 3;

function eventEnd(event: ScheduleEvent): number {
  if (!event.allDay) return new Date(event.end).getTime();
  const [year, month, day] = event.end.split("-").map(Number);
  return new Date(year, month - 1, day).getTime();
}

/**
 * The next few calendar events that include someone on the conversation.
 * Renders nothing when there are none or the schedule can't be loaded, since
 * this is supplemental context; events from calendars that did load still show
 * when another calendar fails.
 */
export function ContactMeetings({ people, timeZone, onOpenEvent }: {
  people: { email: string; name: string }[];
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
  const namesByEmail = new Map(people.map(({ email, name }) => [email.toLocaleLowerCase(), name]));
  const now = Date.now();
  const meetings = events
    .flatMap((event) => {
      if (eventEnd(event) <= now) return [];
      const attendee = event.attendees?.find((address) => namesByEmail.has(address.toLocaleLowerCase()));
      return attendee ? [{ event, name: namesByEmail.get(attendee.toLocaleLowerCase())! }] : [];
    })
    .sort((left, right) => eventDate(left.event).getTime() - eventDate(right.event).getTime())
    .slice(0, MAX_UPCOMING_MEETINGS);
  if (meetings.length === 0) return null;

  return <section className="context-section context-meetings" aria-label="Upcoming meetings">
    <header className="context-section-header"><h3>Upcoming meetings</h3></header>
    {meetings.map(({ event, name }) => (
      <button type="button" key={`${event.accountId}:${event.id}`} className="context-meeting" data-response-status={event.responseStatus ?? undefined} onClick={() => onOpenEvent(event)}>
        <CalendarDays size={14} aria-hidden="true" />
        <span>
          <strong>{event.title}</strong>
          <small className="context-meeting-meta">
            <span className="context-meeting-details">{formatEventDate(event)} · with {name}</span>
            {responseLabel(event) ? <span className="context-meeting-response"> · {responseLabel(event)}</span> : null}
          </small>
        </span>
      </button>
    ))}
  </section>;
}
