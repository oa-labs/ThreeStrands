import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { responseLabel } from "./calendarResponse";
import type { ScheduleEvent } from "./domain";

export function formatAgendaSpan(start: string, end: string, timeZone: string): string {
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone }).formatRange(new Date(start), new Date(end));
}

/** The same calendar-colored event card in a slot preview or a whole day's agenda. */
export function DayAgendaEvent({ event, timeZone, detail, conflict = false, showResponse = false, onOpenEvent }: {
  event: ScheduleEvent;
  timeZone: string;
  detail?: string;
  conflict?: boolean;
  showResponse?: boolean;
  onOpenEvent?(event: ScheduleEvent): void;
}) {
  const colors = useCalendarColors();
  const response = showResponse ? responseLabel(event) : null;
  const content = <>
    <strong className="slot-day-title">{event.title}</strong>
    <span className={`slot-day-meta${response ? " slot-day-meta-with-response" : ""}`} title={event.location ?? undefined}>
      <span className="slot-day-meta-content">{[event.allDay ? "All day" : formatAgendaSpan(event.start, event.end, timeZone), event.location].filter(Boolean).join(" · ")}</span>
      {response ? <span className="slot-day-response"> · {response}</span> : null}
    </span>
    {detail ? <span className={`slot-day-detail${conflict ? " slot-day-detail-conflict" : ""}`}>{detail}</span> : null}
  </>;
  return <li className={`slot-day-row slot-day-event${onOpenEvent ? " slot-day-event-interactive" : ""}`} style={calendarColorStyle(colors, event.accountId, event.calendarId)}>
    {onOpenEvent ? <button type="button" className="slot-day-event-button" onClick={() => onOpenEvent(event)}>{content}</button> : content}
  </li>;
}
