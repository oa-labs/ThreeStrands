import { AttachmentChip } from "./AttachmentChip";
import { FileStarButton } from "./FileStarButton";
import { CalendarDays, Download, ExternalLink, MapPin, Repeat2, Users, Video } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { EventLocation } from "./EventLocation";
import { InvitationResponse, type InvitationActions } from "./InvitationSection";
import { calendarInvitationDetails } from "./calendarDescription";
import { mailClient } from "./data/client";
import type { CalendarEventPreview, CalendarPreview, MessageAttachment } from "./domain";
import { errorMessage } from "./errors";
import { HoverTooltip } from "./AppChrome";
import { ICON_SIZE } from "./iconSizes";
import { parsedInvitationDate } from "./calendarInvitations";
export { calendarEventRange, isCalendarAttachment } from "./calendarInvitations";

const MAX_PREVIEWABLE_SIZE = 2 * 1024 * 1024;

/** Identifies the set of events described by a calendar preview, so two attachments that
 * describe the same event(s) (e.g. Google sends both a text/calendar part and an .ics
 * attachment for one invite) can be recognized as duplicates. */
function calendarPreviewKey(preview: CalendarPreview): string {
  return preview.events
    .map((event) => event.uid?.trim() || `${event.title}|${event.start ?? ""}|${event.end ?? ""}|${event.location ?? ""}`)
    .join(" | ");
}

type GroupProps = {
  messageId: string;
  threadId?: string;
  attachments: MessageAttachment[];
  onError(message: string): void;
  invitationActions?: InvitationActions;
};

/** Renders one calendar-invitation card per distinct event among `attachments`, collapsing
 * attachments that turn out to describe the same event. */
export function CalendarAttachmentGroup({ messageId, threadId, attachments, onError, invitationActions }: GroupProps) {
  const [visibleIds, setVisibleIds] = useState<Set<string> | null>(null);
  const [loadedPreviews, setLoadedPreviews] = useState<Map<string, CalendarPreview | null> | null>(null);
  // Parents rebuild the attachment array on every render (e.g. after a background sync), so
  // reload only when the attachments themselves change; otherwise the card flashes back to
  // its loading state and the conversation jumps.
  const attachmentsKey = attachments.map((attachment) => `${attachment.id}:${attachment.size}`).join("|");
  const attachmentsRef = useRef(attachments);
  attachmentsRef.current = attachments;
  useEffect(() => {
    let active = true;
    const attachments = attachmentsRef.current;
    setVisibleIds(null);
    setLoadedPreviews(null);
    const previewable = attachments.filter((attachment) => attachment.size <= MAX_PREVIEWABLE_SIZE);
    if (previewable.length <= 1) {
      setVisibleIds(new Set(attachments.map((attachment) => attachment.id)));
      return () => { active = false; };
    }
    void Promise.all(
      previewable.map((attachment) =>
        mailClient.previewCalendarAttachment(messageId, attachment.id)
          .then((preview): [string, CalendarPreview | null] => [attachment.id, preview])
          .catch((): [string, CalendarPreview | null] => [attachment.id, null])
      )
    ).then((results) => {
      if (!active) return;
      setLoadedPreviews(new Map(results));
      const seenKeys = new Set<string>();
      const kept = new Set<string>();
      for (const [id, preview] of results) {
        const key = preview === null ? null : calendarPreviewKey(preview);
        if (key === null || !seenKeys.has(key)) {
          if (key !== null) seenKeys.add(key);
          kept.add(id);
        }
      }
      for (const attachment of attachments) {
        if (!previewable.includes(attachment)) kept.add(attachment.id);
      }
      setVisibleIds(kept);
    });
    return () => { active = false; };
  }, [attachmentsKey, messageId]);

  if (!visibleIds) {
    return (
      <div className="calendar-card calendar-card-loading" role="status">
        <CalendarDays size={ICON_SIZE.lg} />
        <span>Loading calendar invitation…</span>
      </div>
    );
  }

  return (
    <>
      {attachments.filter((attachment) => visibleIds.has(attachment.id)).map((attachment) => (
        <CalendarAttachment
          key={`${messageId}:${attachment.id}`}
          messageId={messageId}
          threadId={threadId}
          attachment={attachment}
          onError={onError}
          loadedPreview={loadedPreviews?.get(attachment.id)}
          invitationActions={invitationActions}
        />
      ))}
    </>
  );
}

type Props = {
  messageId: string;
  threadId?: string;
  attachment: MessageAttachment;
  onError(message: string): void;
  invitationActions?: InvitationActions;
  /** A preview already loaded by CalendarAttachmentGroup. Null records a failed load;
   * undefined means this component still needs to load it. */
  loadedPreview?: CalendarPreview | null;
};

export function CalendarAttachment({ messageId, threadId, attachment, onError, loadedPreview, invitationActions }: Props) {
  const [preview, setPreview] = useState<CalendarPreview | null>(null);
  const [failed, setFailed] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    setPreview(null);
    setFailed(false);
    setPreviewError(null);
    if (loadedPreview !== undefined && attempt === 0) {
      setPreview(loadedPreview);
      setFailed(loadedPreview === null);
      return () => { active = false; };
    }
    if (attachment.size > MAX_PREVIEWABLE_SIZE) {
      setFailed(true);
      return () => { active = false; };
    }
    void mailClient.previewCalendarAttachment(messageId, attachment.id).then((value) => {
      if (active) setPreview(value);
    }).catch((reason: unknown) => {
      if (active) { setFailed(true); setPreviewError(errorMessage(reason)); }
    });
    return () => { active = false; };
  }, [attachment.id, attachment.size, loadedPreview, messageId, attempt]);

  const open = () => {
    void mailClient.openAttachment(messageId, attachment.id).catch((reason: unknown) => {
      onError(`Could not open attachment: ${errorMessage(reason)}`);
    });
  };
  const save = () => {
    void mailClient.saveAttachment(messageId, attachment.id).catch((reason: unknown) => {
      onError(`Could not save attachment: ${errorMessage(reason)}`);
    });
  };

  if (failed) return <div className="calendar-card calendar-card-failed">
    <AttachmentChip messageId={messageId} attachment={attachment} icon={<CalendarDays size={ICON_SIZE.sm} aria-hidden="true" />} starrable={Boolean(threadId)} onOpen={open} onSave={save} />
    {invitationActions && attachment.size <= MAX_PREVIEWABLE_SIZE ? <>
      <p role="alert">Could not read invitation{previewError ? `: ${previewError}` : "."}</p>
      <button type="button" className="btn btn-sm" onClick={() => setAttempt((value) => value + 1)}>Retry</button>
    </> : null}
  </div>;
  if (!preview) {
    return (
      <div className="calendar-card calendar-card-loading" role="status">
        <CalendarDays size={ICON_SIZE.lg} />
        <span>Loading calendar invitation…</span>
      </div>
    );
  }

  return (
    <section className="calendar-card" aria-label="Calendar invitation">
      {preview.events.map((event, index) => (
        <CalendarEvent event={event} key={`${messageId}:${event.uid ?? index}`}>
          {invitationActions ? <InvitationResponse {...invitationActions} invitation={event} sourceMessageId={messageId} /> : null}
        </CalendarEvent>
      ))}
      {preview.events.length === 0 ? <p className="calendar-card-more">No events could be read from this invitation.</p> : null}
      {preview.truncated ? <p className="calendar-card-more">Additional events are included in this file.</p> : null}
      <footer className="calendar-card-actions">
        <span><CalendarDays size={ICON_SIZE.sm} /> {attachment.filename}</span>
        <button type="button" className="btn btn-sm btn-ghost" onClick={open}><ExternalLink size={ICON_SIZE.sm} /> Open Invitation</button>
        <HoverTooltip label={`Save ${attachment.filename} as…`} placement="bottom"><button type="button" className="btn-icon btn-icon-sm" aria-label={`Save ${attachment.filename} as…`} onClick={save}>
          <Download size={ICON_SIZE.sm} aria-hidden="true" />
        </button></HoverTooltip>
        {threadId ? <FileStarButton messageId={messageId} attachmentId={attachment.id} filename={attachment.filename} /> : null}
      </footer>
    </section>
  );
}

export function CalendarEvent({ event, children }: { event: CalendarEventPreview; children?: ReactNode }) {
  const status = event.status?.toLocaleLowerCase();
  const { location, description, joinUrl } = calendarInvitationDetails(event.location, event.description);
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
        {location ? <p><MapPin size={ICON_SIZE.sm} /><span><EventLocation location={location} /></span></p> : null}
        {joinUrl ? <p><Video size={ICON_SIZE.sm} /><EventLocation location={joinUrl} linkLabel="Join meeting" /></p> : null}
        {event.organizer || event.attendeeCount > 0 ? (
          <p><Users size={ICON_SIZE.sm} /><span>{peopleLabel(event)}</span></p>
        ) : null}
        {event.recurring ? <p><Repeat2 size={ICON_SIZE.sm} /><span>Recurring event</span></p> : null}
        {description ? <details className="calendar-description-details"><summary>Show details</summary><p className="calendar-description">{description}</p></details> : null}
        {children}
      </div>
    </article>
  );
}

function calendarMonth(value: string | null): string {
  const date = parsedInvitationDate(value);
  return date ? new Intl.DateTimeFormat(undefined, { month: "short" }).format(date).toLocaleUpperCase() : "EVENT";
}

function calendarDay(value: string | null): string {
  const date = parsedInvitationDate(value);
  return date ? String(date.getDate()) : "•";
}

function formatEventTime(event: CalendarEventPreview): string {
  const start = parsedInvitationDate(event.start);
  if (!start) return "Time not specified";
  const date = new Intl.DateTimeFormat(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    year: start.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
  }).format(start);
  if (event.allDay) return `${date} · All day`;
  const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
  const end = parsedInvitationDate(event.end);
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
