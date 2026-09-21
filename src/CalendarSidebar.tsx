import { openUrl } from "@tauri-apps/plugin-opener";
import { AlignLeft, CalendarDays, ChevronLeft, ChevronRight, Clock3, MapPin, RefreshCw, Video, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { isEditableTarget } from "./commands";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, AvailabilityPreferences, AvailabilityResult, ScheduleEvent } from "./domain";
import { useEscapeDismiss } from "./useEscapeDismiss";

const HOUR_HEIGHT = 64;
const HOURS = Array.from({ length: 24 }, (_, hour) => hour);
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

function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function dateInputValue(date: Date): string {
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 10);
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
    <Modal title="Check availability" className="availability-request-modal" onClose={onClose} initialFocusRef={dateRef}>
      <form className="modal-form" onSubmit={(event) => {
        event.preventDefault();
        const [year, month, day] = dateValue.split("-").map(Number);
        void onSubmit(new Date(year, month - 1, day), duration);
      }}>
        <label><span>Date</span><input ref={dateRef} type="date" value={dateValue} onChange={(event) => setDateValue(event.target.value)} required /></label>
        <label><span>Duration</span><select value={duration} onChange={(event) => setDuration(Number(event.target.value))}>{[15, 30, 45, 60, 90, 120].map((value) => <option key={value} value={value}>{value} minutes</option>)}</select></label>
        {error ? <p className="modal-form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions"><button type="button" onClick={onClose}>Cancel</button><button type="submit" disabled={loading}>{loading ? "Checking…" : "Check schedule"}</button></div>
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

function formatEventTime(event: ScheduleEvent): string {
  if (event.allDay) return "All day";
  const formatter = new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  const start = formatter.formatToParts(new Date(event.start))
    .filter((part) => part.type !== "dayPeriod")
    .map((part) => part.value)
    .join("")
    .trim();
  const end = formatter.formatToParts(new Date(event.end))
    .map((part) => part.type === "dayPeriod" ? part.value.toLocaleLowerCase() : part.value)
    .join("");
  return `${start}–${end}`;
}

function eventDate(event: ScheduleEvent): Date {
  if (!event.allDay) return new Date(event.start);
  const [year, month, day] = event.start.split("-").map(Number);
  return new Date(year, month - 1, day);
}

function formatEventDate(event: ScheduleEvent): string {
  return new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  }).format(eventDate(event));
}

function safeWebUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
  } catch {
    return null;
  }
}

function EventViewer({ event, onDismiss }: { event: ScheduleEvent; onDismiss(): void }) {
  const viewerRef = useRef<HTMLDivElement>(null);
  const conferenceUrl = safeWebUrl(event.conferenceUrl);
  useEscapeDismiss(onDismiss);

  useEffect(() => {
    viewerRef.current?.focus();
    const dismissOnOutsidePointer = (pointerEvent: PointerEvent) => {
      const target = pointerEvent.target;
      if (!(target instanceof Node)) return;
      if (viewerRef.current?.contains(target)) return;
      if (target instanceof Element && target.closest("[data-calendar-event-trigger]")) return;
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
        <span className="calendar-event-color" aria-hidden="true" />
        <h3>{event.title}</h3>
        <button type="button" aria-label="Close event details" onClick={onDismiss}><X size={16} /></button>
      </header>
      <div className="calendar-event-viewer-details">
        <p><Clock3 size={17} /><span>{formatEventDate(event)} · {formatEventTime(event)}</span></p>
        {conferenceUrl ? (
          <p>
            <Video size={17} />
            <a href={conferenceUrl} onClick={(clickEvent) => {
              clickEvent.preventDefault();
              void openUrl(conferenceUrl);
            }}>Join video meeting</a>
          </p>
        ) : null}
        {event.location ? <p><MapPin size={17} /><span>{event.location}</span></p> : null}
        <p><CalendarDays size={17} /><span>{event.accountId}</span></p>
        {event.description ? <p className="calendar-event-viewer-description"><AlignLeft size={17} /><span>{event.description}</span></p> : null}
      </div>
    </div>
  );
}

export function CalendarSidebar({
  onClose,
  onOpenSettings,
  availabilityPreferences,
  onDraftAvailability,
  embedded = false,
}: {
  onClose(): void;
  onOpenSettings(): void;
  availabilityPreferences: AvailabilityPreferences;
  onDraftAvailability?(candidates: AvailabilityCandidate[]): void;
  embedded?: boolean;
}) {
  const [date, setDate] = useState(() => startOfLocalDay(new Date()));
  const [events, setEvents] = useState<ScheduleEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedEvent, setSelectedEvent] = useState<ScheduleEvent | null>(null);
  const [availability, setAvailability] = useState<AvailabilityResult | null>(null);
  const [availabilityLoading, setAvailabilityLoading] = useState(false);
  const [availabilityError, setAvailabilityError] = useState<string | null>(null);
  const [selectedCandidates, setSelectedCandidates] = useState<Set<string>>(() => new Set());
  const [durationMinutes, setDurationMinutes] = useState(availabilityPreferences.defaultDurationMinutes);
  const [availabilityDialogOpen, setAvailabilityDialogOpen] = useState(false);
  const gridRef = useRef<HTMLDivElement>(null);
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

  const load = useCallback(async (target: Date) => {
    setSelectedEvent(null);
    setLoading(true);
    setError(null);
    const request = scheduleRequestFor(target);
    try {
      const result = await mailClient.listScheduleEvents(
        request.timeMin,
        request.timeMax,
        request.timeZone,
      );
      setEvents(result.events);
      if (result.errors.length > 0) {
        console.error("Calendar schedule load failed:", result.errors);
        setError("Calendar schedule load failed");
      }
    } catch (reason) {
      setEvents([]);
      console.error("Calendar schedule load failed:", reason);
      setError("Calendar schedule load failed");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(date);
  }, [date, load]);

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
      setAvailabilityError(reason instanceof Error ? reason.message : String(reason));
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

  return (
    <aside className="calendar-sidebar" aria-label="Calendar schedule">
      <header className="calendar-sidebar-header">
        <h2>{new Intl.DateTimeFormat(undefined, {
          weekday: "short",
          month: "short",
          day: "numeric",
        }).format(date)}</h2>
        <div>
          <button type="button" aria-label="Previous day (-)" title="Previous day (-)" onClick={() => moveDay(-1)}>
            <ChevronLeft size={18} />
          </button>
          <button type="button" aria-label="Next day (=)" title="Next day (=)" onClick={() => moveDay(1)}>
            <ChevronRight size={18} />
          </button>
          {!embedded ? (
            <button type="button" aria-label="Close calendar" onClick={onClose}>
              <X size={18} />
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
                  key={eventKey}
                  data-calendar-event-trigger
                  aria-expanded={selectedEvent === event}
                  aria-controls={selectedEvent === event ? "calendar-event-viewer" : undefined}
                  onClick={() => setSelectedEvent((current) => current === event ? null : event)}
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
          <div className="availability-panel-header"><strong>Find a time</strong><button type="button" onClick={() => setAvailabilityDialogOpen(true)} disabled={availabilityLoading}>{availabilityLoading ? "Checking…" : "Check schedule"}</button></div>
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
                })}>{formatCandidateTime(candidate.start)}–{formatCandidateTime(candidate.end)}<small>{candidate.status === "verified" ? "Verified" : candidate.status === "partiallyChecked" ? "Partial" : "Not checked"}</small></button>;
              })}
            </div>
            {selectedCandidates.size > 0 ? <p className="availability-coverage">{selectedCandidates.size} time{selectedCandidates.size === 1 ? "" : "s"} selected</p> : null}
            {selectedCandidates.size > 0 && onDraftAvailability ? (
              <button
                type="button"
                className="availability-draft-reply"
                onClick={() => onDraftAvailability(availability.candidates.filter((candidate) => selectedCandidates.has(`${candidate.start}:${candidate.end}`)))}
              >Draft reply with selected times</button>
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
            <button type="button" onClick={() => void load(date)}>
              <RefreshCw size={14} /> Try again
            </button>
            <button type="button" onClick={onOpenSettings}>Calendar Accounts</button>
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
                  aria-expanded={selectedEvent === event}
                  aria-controls={selectedEvent === event ? "calendar-event-viewer" : undefined}
                  onClick={() => setSelectedEvent((current) => current === event ? null : event)}
                  style={{
                    top: (start / 60) * HOUR_HEIGHT,
                    height: duration,
                  }}
                  title={`${event.title}, ${formatEventTime(event)}`}
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
      {selectedEvent ? <EventViewer event={selectedEvent} onDismiss={() => setSelectedEvent(null)} /> : null}
    </aside>
  );
}
