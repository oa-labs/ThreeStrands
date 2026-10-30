import { readLocalStorage, writeLocalStorage } from "./useLocalStorageState";
import { openUrl } from "@tauri-apps/plugin-opener";
import { TextAlignStart, CalendarDays, ChevronLeft, ChevronRight, Clock3, Link as LinkIcon, MapPin, Pencil, RefreshCw, Trash, Users, Video, X } from "lucide-react";
import { type RefObject, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { HoverTooltip } from "./AppChrome";
import {
  HOUR_HEIGHT,
  HOURS,
  formatEventDate,
  formatEventTime,
  hourLabel,
  safeWebUrl,
  startOfLocalDay,
  timeZoneLabel,
} from "./calendarTime";
import { EventLocation } from "./EventLocation";
import { calendarDescriptionText } from "./calendarDescription";
import { attendeeResponseLabel, attendeeResponseSummary, responseLabel } from "./calendarResponse";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { removeFromScheduleCache, revalidateScheduleCache } from "./calendarScheduleCache";
import { isEditableTarget } from "./commands";
import { mailClient } from "./data/client";
import type { CalendarOption, ScheduleEvent } from "./domain";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { errorMessage } from "./errors";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { ICON_SIZE } from "./iconSizes";
import { InlineConfirm } from "./InlineConfirm";
import { EditCalendarEventDialog } from "./CreateCalendarEventDialog";
import { CalendarResponseControls } from "./CalendarResponseControls";

export const CALENDAR_SCROLL_TOP_KEY = "threestrands.calendar.scrollTop";
const DEFAULT_CALENDAR_SCROLL_TOP = 7 * HOUR_HEIGHT;
const MAX_CALENDAR_SCROLL_TOP = 24 * HOUR_HEIGHT;

function readCalendarScrollTop(): number {
  return readLocalStorage(CALENDAR_SCROLL_TOP_KEY, (stored) => {
    const saved = stored === null || stored.trim() === "" ? Number.NaN : Number(stored);
    return Number.isFinite(saved) && saved >= 0 && saved <= MAX_CALENDAR_SCROLL_TOP ? saved : DEFAULT_CALENDAR_SCROLL_TOP;
  }, DEFAULT_CALENDAR_SCROLL_TOP);
}

function saveCalendarScrollTop(scrollTop: number): void {
  if (!Number.isFinite(scrollTop) || scrollTop < 0 || scrollTop > MAX_CALENDAR_SCROLL_TOP) return;
  writeLocalStorage(CALENDAR_SCROLL_TOP_KEY, String(scrollTop));
}

const WHEEL_SWIPE_THRESHOLD = 60;
const WHEEL_SWIPE_IDLE_MS = 250;
const WHEEL_LINE_HEIGHT = 16;
const TOUCH_SWIPE_THRESHOLD = 50;

// Trackpad swipes arrive as horizontal wheel events, touch screens as touch pointers.
// Swiping content left advances a day, swiping right goes back. A trackpad gesture
// (including its momentum tail) moves at most one day until the wheel goes idle.
function useDaySwipe(targetRef: RefObject<HTMLElement | null>, moveDay: (offset: number) => void) {
  useEffect(() => {
    const target = targetRef.current;
    if (!target) return;
    let wheelDistance = 0;
    let wheelLocked = false;
    let idleTimer: number | undefined;
    let touchStart: { id: number; x: number; y: number } | null = null;

    const onWheel = (event: WheelEvent) => {
      if (event.ctrlKey || Math.abs(event.deltaX) <= Math.abs(event.deltaY)) return;
      window.clearTimeout(idleTimer);
      idleTimer = window.setTimeout(() => {
        wheelDistance = 0;
        wheelLocked = false;
      }, WHEEL_SWIPE_IDLE_MS);
      if (wheelLocked) return;
      wheelDistance += event.deltaMode === WheelEvent.DOM_DELTA_LINE ? event.deltaX * WHEEL_LINE_HEIGHT : event.deltaX;
      if (Math.abs(wheelDistance) < WHEEL_SWIPE_THRESHOLD) return;
      moveDay(wheelDistance > 0 ? 1 : -1);
      wheelDistance = 0;
      wheelLocked = true;
    };
    const onPointerDown = (event: PointerEvent) => {
      if (event.pointerType !== "touch" || !event.isPrimary) return;
      touchStart = { id: event.pointerId, x: event.clientX, y: event.clientY };
    };
    const onPointerUp = (event: PointerEvent) => {
      if (!touchStart || event.pointerId !== touchStart.id) return;
      const dx = event.clientX - touchStart.x;
      const dy = event.clientY - touchStart.y;
      touchStart = null;
      if (Math.abs(dx) < TOUCH_SWIPE_THRESHOLD || Math.abs(dx) <= Math.abs(dy) * 2) return;
      moveDay(dx < 0 ? 1 : -1);
    };
    const onPointerCancel = () => {
      touchStart = null;
    };

    target.addEventListener("wheel", onWheel, { passive: true });
    target.addEventListener("pointerdown", onPointerDown);
    target.addEventListener("pointerup", onPointerUp);
    target.addEventListener("pointercancel", onPointerCancel);
    return () => {
      window.clearTimeout(idleTimer);
      target.removeEventListener("wheel", onWheel);
      target.removeEventListener("pointerdown", onPointerDown);
      target.removeEventListener("pointerup", onPointerUp);
      target.removeEventListener("pointercancel", onPointerCancel);
    };
  }, [targetRef, moveDay]);
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

// Expandable rows keep the icon column of the rows around them; this small
// chevron replaces the browser's disclosure marker and turns when opened.
function DisclosureChevron() {
  return <ChevronRight className="calendar-event-disclosure" size={ICON_SIZE.xs} aria-hidden="true" />;
}

function EventAttendees({ event, attendees }: { event: ScheduleEvent; attendees: string[] }) {
  const summary = attendeeResponseSummary(event);
  return (
    <details className="calendar-event-attendees">
      <summary><Users size={ICON_SIZE.lg} /><span>{attendees.length === 1 ? "1 attendee" : `${attendees.length} attendees`}{summary ? <span className="calendar-event-attendee-summary"> · {summary}</span> : null}<DisclosureChevron /></span></summary>
      <ul>{attendees.map((email) => {
        const status = event.attendeeResponses?.[email];
        const label = attendeeResponseLabel(status);
        return <li key={email}>{email}{label ? <span className="calendar-event-attendee-response" data-response-status={status}> · {label}</span> : null}</li>;
      })}</ul>
    </details>
  );
}

/**
 * What the event's calendar row says: the calendar's own name when it has one,
 * plus the account when several accounts share the view. Null when it would
 * only repeat the single connected account.
 */
export function eventCalendarLabel(event: ScheduleEvent, calendars: CalendarOption[]): string | null {
  const calendar = calendars.find((item) => item.accountId === event.accountId && item.id === event.calendarId);
  const named = calendar && !calendar.primary && calendar.name !== event.accountId ? calendar.name : null;
  if (new Set(calendars.map((item) => item.accountId)).size > 1) return named ? `${named} · ${event.accountId}` : event.accountId;
  return named;
}

export function EventViewer({ event, onDismiss, onUpdated, embedded = false, calendarLabel = event.accountId }: {
  embedded?: boolean;
  event: ScheduleEvent;
  onDismiss(): void;
  onUpdated(event: ScheduleEvent): void;
  /** The calendar row's text; null leaves the row out. */
  calendarLabel?: string | null;
}) {
  const calendarColors = useCalendarColors();
  const viewerRef = useRef<HTMLDivElement>(null);
  const conferenceUrl = safeWebUrl(event.conferenceUrl);
  // A location that is only a link reads as its host; the join button already
  // covers a location that repeats the conference link.
  const locationUrl = safeWebUrl(event.location?.trim());
  const showLocation = Boolean(event.location) && !(locationUrl && locationUrl === conferenceUrl);
  const [editing, setEditing] = useState(false);
  const [rescheduling, setRescheduling] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const deleteEvent = async () => {
    setDeletePending(true);
    setDeleteError(null);
    try {
      await mailClient.deleteCalendarEvent(event);
      removeFromScheduleCache(event);
      onDismiss();
    } catch (error) {
      setDeleteError(errorMessage(error));
      setDeletePending(false);
    }
  };
  useEscapeDismiss(onDismiss, !embedded);

  useEffect(() => {
    if (embedded) return;
    viewerRef.current?.focus();
    const dismissOnOutsidePointer = (pointerEvent: PointerEvent) => {
      const target = pointerEvent.target;
      if (!(target instanceof Node)) return;
      if (viewerRef.current?.contains(target)) return;
      // The edit dialog is portaled outside the viewer but belongs to it.
      if (target instanceof Element && target.closest("[data-calendar-event-trigger], .modal-backdrop")) return;
      onDismiss();
    };
    document.addEventListener("pointerdown", dismissOnOutsidePointer);
    return () => document.removeEventListener("pointerdown", dismissOnOutsidePointer);
  }, [embedded, onDismiss]);

  return (
    <div
      className={embedded ? "calendar-event-details" : "calendar-event-viewer"}
      id="calendar-event-viewer"
      ref={viewerRef}
      role={embedded ? "region" : "dialog"}
      aria-label={`${event.title} details`}
      tabIndex={-1}
    >
      <header>
        <span className="calendar-event-color" aria-hidden="true" style={calendarColorStyle(calendarColors, event.accountId, event.calendarId)} />
        <h3>{event.title}</h3>
        <div className="calendar-event-viewer-actions">
          {event.canEdit ? <>
            <HoverTooltip title="Edit event" placement="bottom"><button type="button" className="btn-icon btn-icon-sm" aria-label="Edit Event" disabled={deletePending} onClick={() => { setRescheduling(false); setEditing(true); }}><Pencil size={ICON_SIZE.sm} /></button></HoverTooltip>
            <HoverTooltip title="Delete event" placement="bottom"><button type="button" className="btn-icon btn-icon-sm" aria-label="Delete Event" aria-expanded={confirmingDelete} disabled={deletePending} onClick={() => { setDeleteError(null); setConfirmingDelete(true); }}><Trash size={ICON_SIZE.sm} /></button></HoverTooltip>
          </> : null}
          <button type="button" className="btn-icon btn-icon-sm" aria-label="Close Event Details" onClick={onDismiss}><X size={ICON_SIZE.sm} /></button>
        </div>
      </header>
      {confirmingDelete ? (
        <InlineConfirm ariaLabel="Delete event confirmation" cancelLabel="Keep" onCancel={() => setConfirmingDelete(false)} disabled={deletePending}
          actions={[{ label: deletePending ? "Deleting…" : "Delete", className: "btn-danger", onClick: () => void deleteEvent() }]}>
          <strong>Delete this event?</strong>{event.attendees?.length ? " Guests will be notified." : null}
        </InlineConfirm>
      ) : null}
      {deleteError ? <p className="form-error" role="alert">{deleteError}</p> : null}
      <div className="calendar-event-viewer-details">
        <p><Clock3 size={ICON_SIZE.lg} /><span>{formatEventDate(event)} · {formatEventTime(event)}</span></p>
        {conferenceUrl ? (
          <p>
            <Video size={ICON_SIZE.lg} />
            <a href={conferenceUrl} onClick={(clickEvent) => {
              clickEvent.preventDefault();
              void openUrl(conferenceUrl);
            }}>Join video meeting</a>
          </p>
        ) : null}
        {showLocation && event.location ? <p>{locationUrl ? <LinkIcon size={ICON_SIZE.lg} /> : <MapPin size={ICON_SIZE.lg} />}<span><EventLocation location={event.location} linkLabel={locationUrl ? new URL(locationUrl).host : undefined} /></span></p> : null}
        {calendarLabel ? <p><CalendarDays size={ICON_SIZE.lg} /><span>{calendarLabel}</span></p> : null}
        <CalendarResponseControls event={event} onUpdated={onUpdated} />
        {event.attendees?.length ? <EventAttendees event={event} attendees={event.attendees} /> : null}
        {event.description ? embedded ? <details className="calendar-event-viewer-description"><summary><TextAlignStart size={ICON_SIZE.lg} /><span>Description<DisclosureChevron /></span></summary><p>{calendarDescriptionText(event.description)}</p></details> : <p className="calendar-event-viewer-description"><TextAlignStart size={ICON_SIZE.lg} /><span>{calendarDescriptionText(event.description)}</span></p> : null}
      </div>
      {event.canEdit && !event.allDay ? <button type="button" className="btn btn-sm" disabled={deletePending} onClick={() => { setRescheduling(true); setEditing(true); }}>Reschedule</button> : null}
      {editing ? <EditCalendarEventDialog
        event={event}
        initialFindingTime={rescheduling}
        onClose={() => setEditing(false)}
        onSaved={(updated) => {
          setEditing(false);
          onUpdated(updated);
          revalidateScheduleCache(updated);
        }}
      /> : null}
    </div>
  );
}

export function CalendarSidebar({
  onClose,
  onOpenSettings,
  selectedCalendarAccountIds,
  embedded = false,
  initialDate,
}: {
  onClose(): void;
  onOpenSettings(): void;
  selectedCalendarAccountIds?: string[];
  embedded?: boolean;
  /** Opens on this day, for example an event's day. */
  initialDate?: Date;
}) {
  const [date, setDate] = useState(() => startOfLocalDay(initialDate ?? new Date()));
  const [selectedEvent, setSelectedEvent] = useState<ScheduleEvent | null>(null);
  const calendarColors = useCalendarColors();
  const gridRef = useRef<HTMLDivElement>(null);
  const sidebarRef = useRef<HTMLElement>(null);
  useEscapeDismiss(onClose, !embedded);

  const { events, loading, error, reload } = useCalendarSchedule(scheduleRequestFor(date));
  useEffect(() => { setSelectedEvent(null); }, [date]);

  useEffect(() => {
    if (gridRef.current) gridRef.current.scrollTop = readCalendarScrollTop();
  }, []);

  const timedEvents = useMemo(() => events.filter((event) => !event.allDay), [events]);
  const allDayEvents = useMemo(() => events.filter((event) => event.allDay), [events]);
  const moveDay = useCallback((offset: number) => {
    setDate((current) => {
      const next = new Date(current);
      next.setDate(next.getDate() + offset);
      return next;
    });
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || event.isComposing || event.defaultPrevented) return;
      if (event.target instanceof HTMLElement && event.target.closest("[data-shortcut-scope='modal'], [data-shortcut-scope='palette']")) return;
      if (isEditableTarget(event.target)) return;
      if (event.key === "-") {
        event.preventDefault();
        moveDay(-1);
      } else if (event.key === "=") {
        event.preventDefault();
        moveDay(1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [moveDay]);

  useDaySwipe(sidebarRef, moveDay);

  return (
    <aside className="calendar-sidebar" aria-label="Schedule" ref={sidebarRef}>
      <header className="calendar-sidebar-header">
        <div className="calendar-sidebar-heading">
          {selectedCalendarAccountIds ? <span className="eyebrow">Calendar <span className="eyebrow-account">· {selectedCalendarAccountIds.length ? selectedCalendarAccountIds.join(", ") : "None selected"}</span></span> : null}
          <h2>{new Intl.DateTimeFormat(undefined, {
            weekday: "short",
            month: "short",
            day: "numeric",
          }).format(date)}</h2>
        </div>
        <div className="calendar-sidebar-actions">
          <HoverTooltip title="Previous day (-)" placement="bottom"><button type="button" className="btn-icon" aria-label="Previous day (-)" onClick={() => moveDay(-1)}>
            <ChevronLeft size={ICON_SIZE.lg} />
          </button></HoverTooltip>
          <HoverTooltip title="Next day (=)" placement="bottom"><button type="button" className="btn-icon" aria-label="Next day (=)" onClick={() => moveDay(1)}>
            <ChevronRight size={ICON_SIZE.lg} />
          </button></HoverTooltip>
          {!embedded ? (
            <button type="button" className="btn-icon" aria-label="Close Schedule" onClick={onClose}>
              <X size={ICON_SIZE.lg} />
            </button>
          ) : null}
        </div>
      </header>
      {allDayEvents.length > 0 ? (
        <div className="calendar-all-day" aria-label="All-day events">
          <span>All day</span>
          <div>
            {allDayEvents.map((event) => {
              const eventKey = `${event.accountId}:${event.id}`;
              return (
                <button
                  type="button"
                  className="calendar-all-day-event"
                  key={eventKey}
                  data-calendar-event-trigger
                  data-response-status={event.responseStatus ?? undefined}
                  aria-expanded={selectedEvent === event}
                  aria-controls={selectedEvent === event ? "calendar-event-viewer" : undefined}
                  onClick={() => setSelectedEvent((current) => current === event ? null : event)}
                  style={calendarColorStyle(calendarColors, event.accountId, event.calendarId)}
                >
                  {event.title}
                </button>
              );
            })}
          </div>
        </div>
      ) : null}
      <div className="calendar-timezone">{timeZoneLabel(date)}</div>
      {!loading && error ? (
        <div className="calendar-error-notice" role="alert">
          <p>Calendar couldn’t be loaded. Try again or reconnect in Calendar Accounts.</p>
          <div>
            <button type="button" className="btn btn-sm" onClick={reload}>
              <RefreshCw size={ICON_SIZE.sm} /> Try Again
            </button>
            <button type="button" className="btn btn-sm" onClick={onOpenSettings}>Calendar Accounts</button>
          </div>
        </div>
      ) : null}
      <div
        className="calendar-grid-scroll"
        ref={gridRef}
        onScroll={(event) => saveCalendarScrollTop(event.currentTarget.scrollTop)}
      >
        <div className="calendar-grid">
          <div className="calendar-hour-labels" aria-hidden="true">
            {HOURS.map((hour) => (
              <span key={hour} style={{ top: hour * HOUR_HEIGHT }}>{hourLabel(hour)}</span>
            ))}
          </div>
          <div className="calendar-day-column">
            {HOURS.map((hour) => <div className="calendar-hour-line" key={hour} />)}
            {timedEvents.map((event) => {
              const eventStart = new Date(event.start);
              const eventEnd = new Date(event.end);
              const dayStart = startOfLocalDay(date);
              const dayEnd = new Date(dayStart);
              dayEnd.setDate(dayEnd.getDate() + 1);
              const start = eventStart <= dayStart
                ? 0
                : eventStart >= dayEnd
                  ? 24 * 60
                  : eventStart.getHours() * 60 + eventStart.getMinutes();
              const end = eventEnd >= dayEnd
                ? 24 * 60
                : eventEnd <= dayStart
                  ? 0
                  : eventEnd.getHours() * 60 + eventEnd.getMinutes();
              const duration = Math.max(24, ((Math.max(start, end) - start) / 60) * HOUR_HEIGHT);
              const durationMinutes = (eventEnd.getTime() - eventStart.getTime()) / 60000;
              const compact = !event.allDay && durationMinutes <= 30;
              const tight = !event.allDay && durationMinutes <= 15;
              const className = [
                "calendar-schedule-event",
                compact && "calendar-schedule-event-compact",
                tight && "calendar-schedule-event-tight",
              ].filter(Boolean).join(" ");
              return (
                <button
                  type="button"
                  className={className}
                  key={`${event.accountId}:${event.id}`}
                  data-calendar-event-trigger
                  data-response-status={event.responseStatus ?? undefined}
                  aria-expanded={selectedEvent === event}
                  aria-controls={selectedEvent === event ? "calendar-event-viewer" : undefined}
                  onClick={() => setSelectedEvent((current) => current === event ? null : event)}
                  style={{
                    ...calendarColorStyle(calendarColors, event.accountId, event.calendarId),
                    top: (start / 60) * HOUR_HEIGHT,
                    height: duration,
                  }}
                  title={`${event.title}, ${formatEventTime(event)}${responseLabel(event) ? `, ${responseLabel(event)}` : ""}`}
                >
                  <strong>{event.title}</strong>
                  <span>{formatEventTime(event)}</span>
                </button>
              );
            })}
          </div>
          {loading ? <p className="calendar-grid-status">Loading schedule…</p> : null}
          {!loading && !error && events.length === 0 ? (
            <p className="calendar-grid-status">No events scheduled.</p>
          ) : null}
        </div>
      </div>
      {selectedEvent ? <EventViewer event={selectedEvent} onDismiss={() => setSelectedEvent(null)} onUpdated={setSelectedEvent} /> : null}
    </aside>
  );
}
