import { openUrl } from "@tauri-apps/plugin-opener";
import { AlignLeft, CalendarDays, ChevronLeft, ChevronRight, Clock3, MapPin, Pencil, RefreshCw, Trash2, Video, X } from "lucide-react";
import { type RefObject, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { HoverTooltip, Modal } from "./AppChrome";
import {
  HOUR_HEIGHT,
  HOURS,
  dateInputValue,
  formatEventDate,
  formatEventTime,
  hourLabel,
  safeWebUrl,
  startOfLocalDay,
  timeZoneLabel,
} from "./calendarTime";
import { calendarDescriptionText } from "./calendarDescription";
import { responseLabel } from "./calendarResponse";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { removeFromScheduleCache, revalidateScheduleCache } from "./calendarScheduleCache";
import { isEditableTarget } from "./commands";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, AvailabilityPreferences, AvailabilityResult, ScheduleEvent } from "./domain";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { errorMessage } from "./errors";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { ICON_SIZE } from "./iconSizes";
import { InlineConfirm } from "./InlineConfirm";
import { EditCalendarEventDialog } from "./CreateCalendarEventDialog";

export const CALENDAR_SCROLL_TOP_KEY = "threestrands.calendar.scrollTop";
const DEFAULT_CALENDAR_SCROLL_TOP = 7 * HOUR_HEIGHT;
const MAX_CALENDAR_SCROLL_TOP = 24 * HOUR_HEIGHT;

function readCalendarScrollTop(): number {
  try {
    const stored = localStorage.getItem(CALENDAR_SCROLL_TOP_KEY);
    const saved = stored === null || stored.trim() === "" ? Number.NaN : Number(stored);
    if (Number.isFinite(saved) && saved >= 0 && saved <= MAX_CALENDAR_SCROLL_TOP) return saved;
  } catch {
    // A blocked storage backend should not prevent the calendar from opening.
  }
  return DEFAULT_CALENDAR_SCROLL_TOP;
}

function saveCalendarScrollTop(scrollTop: number): void {
  if (!Number.isFinite(scrollTop) || scrollTop < 0 || scrollTop > MAX_CALENDAR_SCROLL_TOP) return;
  try {
    localStorage.setItem(CALENDAR_SCROLL_TOP_KEY, String(scrollTop));
  } catch {
    // The scroll position still applies for this session when storage is unavailable.
  }
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

function AvailabilityRequestDialog({
  date,
  durationMinutes,
  error,
  loading,
  onClose,
  onSubmit,
}: {
  date: Date;
  durationMinutes: number;
  error: string | null;
  loading: boolean;
  onClose(): void;
  onSubmit(date: Date, durationMinutes: number): Promise<void> | void;
}) {
  const [dateValue, setDateValue] = useState(dateInputValue(date));
  const [duration, setDuration] = useState(durationMinutes);
  const dateRef = useRef<HTMLInputElement>(null);
  return (
    <Modal title="Check Availability" className="availability-request-modal" onClose={onClose} initialFocusRef={dateRef}>
      <form className="modal-form" onSubmit={(event) => {
        event.preventDefault();
        const [year, month, day] = dateValue.split("-").map(Number);
        void onSubmit(new Date(year, month - 1, day), duration);
      }}>
        <label><span>Date</span><input ref={dateRef} type="date" value={dateValue} onChange={(event) => setDateValue(event.target.value)} required /></label>
        <label><span>Duration</span><select value={duration} onChange={(event) => setDuration(Number(event.target.value))}>{[...new Set([15, 30, 45, 60, 90, 120, durationMinutes])].sort((left, right) => left - right).map((value) => <option key={value} value={value}>{value} minutes</option>)}</select></label>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions"><button type="button" className="btn" onClick={onClose}>Cancel</button><button type="submit" className="btn btn-primary" disabled={loading}>{loading ? "Checking…" : "Check Schedule"}</button></div>
      </form>
    </Modal>
  );
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

export function hasWorkingHoursOnDate(
  date: Date,
  preferences: Pick<AvailabilityPreferences, "workingWindows">,
): boolean {
  return preferences.workingWindows.some((window) => window.weekday === date.getDay());
}

const locationUrlPattern = /(https?:\/\/[^\s<>"]+)/;

/** Renders a location with its http(s) URLs opened in the system browser. */
function EventLocation({ location }: { location: string }) {
  return location.split(locationUrlPattern).map((part, index) => {
    // Odd indexes are captured URLs; trailing punctuation stays plain text.
    const text = index % 2 === 1 ? part.replace(/[.,;:!?)\]]+$/, "") : part;
    const url = index % 2 === 1 ? safeWebUrl(text) : null;
    if (!url) return part;
    return (
      <span key={index}>
        <a className="calendar-event-location-link" href={url} onClick={(clickEvent) => {
          clickEvent.preventDefault();
          void openUrl(url);
        }}>{text}</a>{part.slice(text.length)}
      </span>
    );
  });
}

export function EventViewer({ event, onDismiss, onUpdated }: { event: ScheduleEvent; onDismiss(): void; onUpdated(event: ScheduleEvent): void }) {
  const calendarColors = useCalendarColors();
  const viewerRef = useRef<HTMLDivElement>(null);
  const conferenceUrl = safeWebUrl(event.conferenceUrl);
  const [responsePending, setResponsePending] = useState(false);
  const [responseError, setResponseError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const label = responseLabel(event);
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
  const respond = async (status: "accepted" | "declined" | "tentative") => {
    setResponsePending(true);
    setResponseError(null);
    try {
      const updated = await mailClient.updateCalendarResponse(event, status);
      onUpdated(updated);
      revalidateScheduleCache(updated);
    } catch (error) {
      setResponseError(errorMessage(error));
    } finally {
      setResponsePending(false);
    }
  };
  useEscapeDismiss(onDismiss);

  useEffect(() => {
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
  }, [onDismiss]);

  return (
    <div
      className="calendar-event-viewer"
      id="calendar-event-viewer"
      ref={viewerRef}
      role="dialog"
      aria-label={`${event.title} details`}
      tabIndex={-1}
    >
      <header>
        <span className="calendar-event-color" aria-hidden="true" style={calendarColorStyle(calendarColors, event.accountId, event.calendarId)} />
        <h3>{event.title}</h3>
        <div className="calendar-event-viewer-actions">
          {event.canEdit ? <>
            <HoverTooltip title="Edit event"><button type="button" className="btn-icon btn-icon-sm" aria-label="Edit Event" disabled={deletePending} onClick={() => setEditing(true)}><Pencil size={ICON_SIZE.sm} /></button></HoverTooltip>
            <HoverTooltip title="Delete event"><button type="button" className="btn-icon btn-icon-sm" aria-label="Delete Event" aria-expanded={confirmingDelete} disabled={deletePending} onClick={() => { setDeleteError(null); setConfirmingDelete(true); }}><Trash2 size={ICON_SIZE.sm} /></button></HoverTooltip>
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
        {event.location ? <p><MapPin size={ICON_SIZE.lg} /><span><EventLocation location={event.location} /></span></p> : null}
        <p><CalendarDays size={ICON_SIZE.lg} /><span>{event.accountId}</span></p>
        {label ? <div className="calendar-event-response">
          <span>Your response: <strong>{label}</strong></span>
          {event.canRespond ? <div className="segmented calendar-response-actions" role="group" aria-label="Going?">
            {([ ["accepted", "Yes"], ["declined", "No"], ["tentative", "Maybe"] ] as const).map(([status, title]) => (
              <button key={status} type="button" className="segment" aria-pressed={event.responseStatus === status} disabled={responsePending} onClick={() => void respond(status)}>{title}</button>
            ))}
          </div> : null}
          {responseError ? <p role="alert">{responseError}</p> : null}
        </div> : null}
        {event.description ? <p className="calendar-event-viewer-description"><AlignLeft size={ICON_SIZE.lg} /><span>{calendarDescriptionText(event.description)}</span></p> : null}
      </div>
      {editing ? <EditCalendarEventDialog
        event={event}
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
  availabilityPreferences,
  onDraftAvailability,
  selectedCalendarAccountIds,
  embedded = false,
  initialDate,
  initialDurationMinutes,
  draftLabel = "Draft Reply With Selected Times",
}: {
  onClose(): void;
  onOpenSettings(): void;
  availabilityPreferences: AvailabilityPreferences;
  onDraftAvailability?(candidates: AvailabilityCandidate[]): void;
  selectedCalendarAccountIds?: string[];
  embedded?: boolean;
  /** Opens on this day, for example a meeting suggestion's first day. */
  initialDate?: Date;
  /** Starts availability checks at a meeting's duration. */
  initialDurationMinutes?: number;
  /** The label of the button that hands selected times to `onDraftAvailability`. */
  draftLabel?: string;
}) {
  const [date, setDate] = useState(() => startOfLocalDay(initialDate ?? new Date()));
  const [selectedEvent, setSelectedEvent] = useState<ScheduleEvent | null>(null);
  const calendarColors = useCalendarColors();
  const [availability, setAvailability] = useState<AvailabilityResult | null>(null);
  const [availabilityLoading, setAvailabilityLoading] = useState(false);
  const [availabilityError, setAvailabilityError] = useState<string | null>(null);
  const [selectedCandidates, setSelectedCandidates] = useState<Set<string>>(() => new Set());
  const [durationMinutes, setDurationMinutes] = useState(initialDurationMinutes ?? availabilityPreferences.defaultDurationMinutes);
  const [availabilityDialogOpen, setAvailabilityDialogOpen] = useState(false);
  const gridRef = useRef<HTMLDivElement>(null);
  const sidebarRef = useRef<HTMLElement>(null);
  useEscapeDismiss(onClose, !embedded);
  const hasWorkingHours = hasWorkingHoursOnDate(date, availabilityPreferences);
  const canCheckAvailability = hasWorkingHours && date >= startOfLocalDay(new Date());

  useEffect(() => {
    if (canCheckAvailability) return;
    setAvailability(null);
    setAvailabilityError(null);
    setSelectedCandidates(new Set());
    setAvailabilityDialogOpen(false);
  }, [canCheckAvailability]);

  const { events, loading, error, reload } = useCalendarSchedule(scheduleRequestFor(date));
  useEffect(() => { setSelectedEvent(null); }, [date]);

  useEffect(() => {
    if (gridRef.current) gridRef.current.scrollTop = readCalendarScrollTop();
  }, []);

  const timedEvents = useMemo(() => events.filter((event) => !event.allDay), [events]);
  const allDayEvents = useMemo(() => events.filter((event) => event.allDay), [events]);
  const moveDay = useCallback((offset: number) => {
    setSelectedCandidates(new Set());
    setAvailability(null);
    setAvailabilityError(null);
    setAvailabilityDialogOpen(false);
    setDate((current) => {
      const next = new Date(current);
      next.setDate(next.getDate() + offset);
      return next;
    });
  }, []);

  const checkAvailability = useCallback(async (targetDate: Date, targetDurationMinutes: number) => {
    setDate(targetDate);
    setDurationMinutes(targetDurationMinutes);
    setAvailability(null);
    setAvailabilityLoading(true);
    setAvailabilityError(null);
    setSelectedCandidates(new Set());
    const dayStart = startOfLocalDay(targetDate);
    const dayEnd = new Date(dayStart);
    dayEnd.setDate(dayEnd.getDate() + 1);
    try {
      setAvailability(await mailClient.findAvailability({
        rangeStart: dayStart.toISOString(),
        rangeEnd: dayEnd.toISOString(),
        preferences: { ...availabilityPreferences, defaultDurationMinutes: targetDurationMinutes },
      }));
      setAvailabilityDialogOpen(false);
    } catch (reason) {
      setAvailability(null);
      setAvailabilityError(errorMessage(reason));
    } finally {
      setAvailabilityLoading(false);
    }
  }, [availabilityPreferences]);

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
    <aside className="calendar-sidebar" aria-label="Calendar schedule" ref={sidebarRef}>
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
          <HoverTooltip title="Previous day (-)"><button type="button" className="btn-icon" aria-label="Previous day (-)" onClick={() => moveDay(-1)}>
            <ChevronLeft size={ICON_SIZE.lg} />
          </button></HoverTooltip>
          <HoverTooltip title="Next day (=)"><button type="button" className="btn-icon" aria-label="Next day (=)" onClick={() => moveDay(1)}>
            <ChevronRight size={ICON_SIZE.lg} />
          </button></HoverTooltip>
          {!embedded ? (
            <button type="button" className="btn-icon" aria-label="Close Calendar" onClick={onClose}>
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
      {canCheckAvailability ? (
        <section className="availability-panel" aria-label="Check availability">
          <div className="availability-panel-header"><strong>Find a time</strong><button type="button" className="btn btn-sm" onClick={() => setAvailabilityDialogOpen(true)} disabled={availabilityLoading}>{availabilityLoading ? "Checking…" : "Check Schedule"}</button></div>
          {availability ? <p className="availability-coverage">{durationMinutes} minute slots</p> : null}
          {availabilityError ? <p className="calendar-error-notice" role="alert">{availabilityError}</p> : null}
          {availability ? <>
            <p className="availability-coverage">{availability.totalCalendarCount === 0 ? "Not checked against a calendar" : availability.checkedCalendarCount === availability.totalCalendarCount ? "Verified against all selected calendars" : "Partially checked — review before sharing"}</p>
            {availability.errors.length > 0 ? <p className="calendar-error-notice" role="alert">Some calendars could not be checked. Suggested times are not fully verified.</p> : null}
            <div className="availability-candidates" aria-label="Suggested times">
              {availability.candidates.map((candidate) => {
                const key = `${candidate.start}:${candidate.end}`;
                const selected = selectedCandidates.has(key);
                const formatCandidateTime = (value: string) => new Intl.DateTimeFormat(undefined, {
                  hour: "numeric",
                  minute: "2-digit",
                  timeZone: availabilityPreferences.timeZone,
                }).format(new Date(value));
                return <button type="button" key={key} className={`availability-candidate availability-${candidate.status}`} aria-pressed={selected} onClick={() => setSelectedCandidates((current) => {
                  const next = new Set(current);
                  if (next.has(key)) next.delete(key);
                  else next.add(key);
                  return next;
                })}>{formatCandidateTime(candidate.start)}–{formatCandidateTime(candidate.end)}<small>{candidate.status === "verified" ? "Verified" : candidate.status === "partiallyChecked" ? "Partial" : "Not Checked"}</small></button>;
              })}
            </div>
            {selectedCandidates.size > 0 ? <p className="availability-coverage">{selectedCandidates.size} time{selectedCandidates.size === 1 ? "" : "s"} selected</p> : null}
            {selectedCandidates.size > 0 && onDraftAvailability ? (
              <button
                type="button"
                className="btn btn-sm btn-primary availability-draft-reply"
                onClick={() => onDraftAvailability(availability.candidates.filter((candidate) => selectedCandidates.has(`${candidate.start}:${candidate.end}`)))}
              >{draftLabel}</button>
            ) : null}
            {availability.candidates.length === 0 ? <p className="calendar-grid-status">No open working-hours slots found.</p> : null}
          </> : null}
        </section>
      ) : null}
      {availabilityDialogOpen && canCheckAvailability ? <AvailabilityRequestDialog date={date} durationMinutes={durationMinutes} error={availabilityError} loading={availabilityLoading} onClose={() => setAvailabilityDialogOpen(false)} onSubmit={checkAvailability} /> : null}
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
