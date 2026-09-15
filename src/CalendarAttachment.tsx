import { CalendarDays, Download, ExternalLink, MapPin, Repeat2, Users } from "lucide-react";
import { useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { CalendarEventPreview, CalendarPreview, MessageAttachment } from "./domain";

export function isCalendarAttachment(attachment: MessageAttachment): boolean {
  return attachment.mimeType.split(";", 1)[0]?.trim().toLocaleLowerCase() === "text/calendar"
    || attachment.filename.toLocaleLowerCase().endsWith(".ics");
}

type Props = {
  messageId: string;
  attachment: MessageAttachment;
  onError(message: string): void;
};

export function CalendarAttachment({ messageId, attachment, onError }: Props) {
  const [preview, setPreview] = useState<CalendarPreview | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    setPreview(null);
    setFailed(false);
    if (attachment.size > 2 * 1024 * 1024) {
      setFailed(true);
      return () => { active = false; };
    }
    void mailClient.previewCalendarAttachment(messageId, attachment.id).then((value) => {
      if (active) setPreview(value);
    }).catch(() => {
      if (active) setFailed(true);
    });
    return () => { active = false; };
  }, [attachment.id, attachment.size, messageId]);

  const open = () => {
    void mailClient.openAttachment(messageId, attachment.id).catch((reason: unknown) => {
      onError(`Could not open attachment: ${reason instanceof Error ? reason.message : String(reason)}`);
    });
  };
  const download = () => {
    void mailClient.saveAttachment(messageId, attachment.id).catch((reason: unknown) => {
      onError(`Could not download attachment: ${reason instanceof Error ? reason.message : String(reason)}`);
    });
  };

  if (failed) return <StandardCalendarAttachment attachment={attachment} onOpen={open} onDownload={download} />;
  if (!preview) {
    return (
      <div className="calendar-card calendar-card-loading" role="status">
        <CalendarDays size={18} />
        <span>Loading calendar invitation…</span>
      </div>
    );
  }

  return (
    <section className="calendar-card" aria-label="Calendar invitation">
      {preview.events.map((event, index) => (
        <CalendarEvent event={event} key={`${event.start ?? "event"}-${index}`} />
      ))}
      {preview.truncated ? <p className="calendar-card-more">Additional events are included in this file.</p> : null}
      <footer className="calendar-card-actions">
        <span><CalendarDays size={14} /> {attachment.filename}</span>
        <button type="button" onClick={open}><ExternalLink size={14} /> Open invitation</button>
        <button type="button" aria-label={`Download ${attachment.filename}`} title={`Download ${attachment.filename}`} onClick={download}>
          <Download size={14} />
        </button>
      </footer>
    </section>
  );
}

function CalendarEvent({ event }: { event: CalendarEventPreview }) {
  const status = event.status?.toLocaleLowerCase();
  return (
    <article className={`calendar-event${status === "cancelled" ? " calendar-event-cancelled" : ""}`}>
      <div className="calendar-date-tile" aria-hidden="true">
        <strong>{calendarMonth(event.start)}</strong>
        <span>{calendarDay(event.start)}</span>
      </div>
      <div className="calendar-event-details">
        <div className="calendar-event-heading">
          <h4>{event.title}</h4>
          {status === "cancelled" ? <span className="calendar-status">Cancelled</span> : null}
        </div>
        <p className="calendar-when">{formatEventTime(event)}</p>
        {event.location ? <p><MapPin size={14} /><span>{event.location}</span></p> : null}
        {event.organizer || event.attendeeCount > 0 ? (
          <p><Users size={14} /><span>{peopleLabel(event)}</span></p>
        ) : null}
        {event.recurring ? <p><Repeat2 size={14} /><span>Recurring event</span></p> : null}
        {event.description ? <p className="calendar-description">{event.description}</p> : null}
      </div>
    </article>
  );
}

function StandardCalendarAttachment({
  attachment,
  onOpen,
  onDownload,
}: {
  attachment: MessageAttachment;
  onOpen(): void;
  onDownload(): void;
}) {
  return (
    <div className="message-attachment">
      <button type="button" className="attachment-badge" aria-label={`View ${attachment.filename}`} onClick={onOpen}>
        <CalendarDays size={14} />
        <span>{attachment.filename}</span>
        <ExternalLink size={13} />
      </button>
      <button type="button" className="attachment-download" aria-label={`Download ${attachment.filename}`} onClick={onDownload}>
        <Download size={14} />
      </button>
    </div>
  );
}

function parsedDate(value: string | null): Date | null {
  if (!value) return null;
  const allDay = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  const date = allDay
    ? new Date(Number(allDay[1]), Number(allDay[2]) - 1, Number(allDay[3]))
    : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function calendarMonth(value: string | null): string {
  const date = parsedDate(value);
  return date ? new Intl.DateTimeFormat(undefined, { month: "short" }).format(date).toLocaleUpperCase() : "EVENT";
}

function calendarDay(value: string | null): string {
  const date = parsedDate(value);
  return date ? String(date.getDate()) : "•";
}

function formatEventTime(event: CalendarEventPreview): string {
  const start = parsedDate(event.start);
  if (!start) return "Time not specified";
  const date = new Intl.DateTimeFormat(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    year: start.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
  }).format(start);
  if (event.allDay) return `${date} · All day`;
  const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
  const end = parsedDate(event.end);
  const range = end ? `${time.format(start)}–${time.format(end)}` : time.format(start);
  const zone = event.timeZone ? ` · ${event.timeZone.replaceAll("_", " ")}` : "";
  return `${date} · ${range}${zone}`;
}

function peopleLabel(event: CalendarEventPreview): string {
  const attendees = event.attendeeCount === 1 ? "1 attendee" : `${event.attendeeCount} attendees`;
  if (event.organizer && event.attendeeCount > 0) return `Organized by ${event.organizer} · ${attendees}`;
  if (event.organizer) return `Organized by ${event.organizer}`;
  return attendees;
}
