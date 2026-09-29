import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EventViewer } from "./CalendarSidebar";
import { HoverTooltip } from "./AppChrome";
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
  startOfWeek,
  timeZoneLabel,
} from "./calendarTime";
import { isEditableTarget } from "./commands";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { clearScheduleCache } from "./calendarScheduleCache";
import { CreateCalendarEventDialog } from "./CreateCalendarEventDialog";
import type { CalendarAccount, CalendarOption, ScheduleEvent } from "./domain";

export const WEEK_SCROLL_TOP_KEY = "threestrands.calendarWeek.scrollTop";
const DEFAULT_WEEK_SCROLL_TOP = 7 * HOUR_HEIGHT;
const MAX_WEEK_SCROLL_TOP = 24 * HOUR_HEIGHT;
/** How often the "now" indicator re-renders, in milliseconds. */
const NOW_TICK_MS = 60_000;
const SLOT_MINUTES = 15;

function slotAt(clientY: number, column: HTMLElement): number {
  const offset = Math.max(0, Math.min(24 * HOUR_HEIGHT - 1, clientY - column.getBoundingClientRect().top));
  return Math.floor(offset / HOUR_HEIGHT * 60 / SLOT_MINUTES) * SLOT_MINUTES;
}

function eventRangeFromSlots(day: Date, anchor: number, current: number, dragged: boolean) {
  const startMinutes = Math.min(anchor, current);
  const endMinutes = dragged ? Math.max(anchor, current) + SLOT_MINUTES : anchor + 60;
  const at = (minutes: number) => new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, minutes);
  return { start: at(startMinutes), end: at(endMinutes), startMinutes, endMinutes };
}

function readWeekScrollTop(): number {
  try {
    const stored = localStorage.getItem(WEEK_SCROLL_TOP_KEY);
    const saved = stored === null || stored.trim() === "" ? Number.NaN : Number(stored);
    if (Number.isFinite(saved) && saved >= 0 && saved <= MAX_WEEK_SCROLL_TOP) return saved;
  } catch {
    // A blocked storage backend should not prevent the week view from opening.
  }
  return DEFAULT_WEEK_SCROLL_TOP;
}

function saveWeekScrollTop(scrollTop: number): void {
  if (!Number.isFinite(scrollTop) || scrollTop < 0 || scrollTop > MAX_WEEK_SCROLL_TOP) return;
  try {
    localStorage.setItem(WEEK_SCROLL_TOP_KEY, String(scrollTop));
  } catch {
    // The scroll position still applies for this session when storage is unavailable.
  }
}

function weekdayLabel(date: Date): string {
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(date);
  return `${weekday} ${date.getDate()}`;
}

function monthTitle(date: Date): string {
  return new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(date);
}

/** Six Sunday-anchored rows covering the month containing `date`. */
export function monthGridDays(date: Date): Date[] {
  const first = new Date(date.getFullYear(), date.getMonth(), 1);
  const start = startOfWeek(first);
  return Array.from({ length: 42 }, (_, index) => addDays(start, index));
}

function MiniMonth({
  month,
  selected,
  today,
  onSelect,
  onMoveMonth,
}: {
  month: Date;
  selected: Date;
  today: Date;
  onSelect(date: Date): void;
  onMoveMonth(offset: number): void;
}) {
  const days = useMemo(() => monthGridDays(month), [month]);
  const weekdayInitials = useMemo(() => {
    const formatter = new Intl.DateTimeFormat(undefined, { weekday: "narrow" });
    const base = startOfWeek(new Date());
    return Array.from({ length: 7 }, (_, index) => formatter.format(addDays(base, index)));
  }, []);
  return (
    <section className="calendar-mini-month" aria-label="Month picker">
      <header>
        <h3>{monthTitle(month)}</h3>
        <div>
          <button type="button" aria-label="Previous Month" onClick={() => onMoveMonth(-1)}><ChevronLeft size={17} /></button>
          <button type="button" aria-label="Next Month" onClick={() => onMoveMonth(1)}><ChevronRight size={17} /></button>
        </div>
      </header>
      <div className="calendar-mini-month-grid" role="grid">
        <div className="calendar-mini-month-weekdays" role="row" aria-hidden="true">
          {weekdayInitials.map((initial, index) => <span key={index}>{initial}</span>)}
        </div>
        <div className="calendar-mini-month-days" role="row">
          {days.map((day) => {
            const outside = day.getMonth() !== month.getMonth();
            const className = [
              "calendar-mini-day",
              outside && "calendar-mini-day-outside",
              isSameDay(day, selected) && "calendar-mini-day-selected",
              isSameDay(day, today) && "calendar-mini-day-today",
            ].filter(Boolean).join(" ");
            return (
              <button
                type="button"
                role="gridcell"
                key={day.toDateString()}
                className={className}
                aria-current={isSameDay(day, selected) ? "date" : undefined}
                aria-label={new Intl.DateTimeFormat(undefined, { dateStyle: "full" }).format(day)}
                onClick={() => onSelect(day)}
              >
                {day.getDate()}
              </button>
            );
          })}
        </div>
      </div>
    </section>
  );
}

function CalendarList({
  accounts,
  calendars,
  onToggle,
  onAdd,
}: {
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  onToggle(accountId: string, calendarId: string, selected: boolean): void;
  onAdd(): void;
}) {
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  return (
    <section className="calendar-list" aria-label="Calendars">
      <header>
        <CalendarDays size={17} />
        <h3>Calendars</h3>
        <HoverTooltip title="Add calendar account"><button type="button" aria-label="Add calendar account" onClick={onAdd}>+</button></HoverTooltip>
      </header>
      {accounts.length === 0 ? <p className="calendar-list-empty">No calendar accounts connected.</p> : null}
      {accounts.map((account) => {
        const accountCalendars = calendars.filter((calendar) => calendar.accountId === account.email);
        const open = !collapsed.has(account.email);
        return (
          <div className="calendar-list-account" key={account.email}>
            <button
              type="button"
              className="calendar-list-account-toggle"
              aria-expanded={open}
              onClick={() => setCollapsed((current) => {
                const next = new Set(current);
                if (next.has(account.email)) next.delete(account.email);
                else next.add(account.email);
                return next;
              })}
            >
              <span>{account.email}</span>
              <ChevronLeft size={16} className={open ? "calendar-list-chevron-open" : "calendar-list-chevron"} />
            </button>
            {open ? accountCalendars.map((calendar) => (
              <label key={calendar.id}>
                <input
                  type="checkbox"
                  checked={calendar.selected}
                  onChange={(event) => onToggle(account.email, calendar.id, event.target.checked)}
                />
                <span>{calendar.name}</span>
              </label>
            )) : null}
          </div>
        );
      })}
    </section>
  );
}

export function CalendarWeekView({
  anchor,
  onAnchorChange,
  accounts,
  calendars,
  onToggleCalendar,
  onAddCalendarAccount,
  onOpenSettings,
}: {
  anchor: Date;
  onAnchorChange(date: Date): void;
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  onToggleCalendar(accountId: string, calendarId: string, selected: boolean): void;
  onAddCalendarAccount(): void;
  onOpenSettings(): void;
}) {
  const [month, setMonth] = useState(() => startOfLocalDay(anchor));
  const [selectedEvent, setSelectedEvent] = useState<ScheduleEvent | null>(null);
  const [newEventRange, setNewEventRange] = useState<{ start: Date; end: Date } | null>(null);
  const [dragPreview, setDragPreview] = useState<{ dayIndex: number; startMinutes: number; endMinutes: number } | null>(null);
  const dragRef = useRef<{ dayIndex: number; anchor: number; pointerId: number } | null>(null);
  const [now, setNow] = useState(() => new Date());
  const gridRef = useRef<HTMLDivElement>(null);

  const weekStart = useMemo(() => startOfWeek(anchor), [anchor]);
  const days = useMemo(() => Array.from({ length: 7 }, (_, index) => addDays(weekStart, index)), [weekStart]);
  const today = startOfLocalDay(now);
  const selectedAccountEmails = [...new Set(calendars.filter((calendar) => calendar.selected).map((calendar) => calendar.accountId))];

  const { events, loading, error, reload } = useCalendarSchedule({
    timeMin: weekStart.toISOString(),
    timeMax: addDays(weekStart, 7).toISOString(),
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  }, true);

  useEffect(() => { setSelectedEvent(null); }, [weekStart]);

  useEffect(() => {
    if (gridRef.current) gridRef.current.scrollTop = readWeekScrollTop();
  }, []);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), NOW_TICK_MS);
    return () => window.clearInterval(timer);
  }, []);

  const moveWeek = useCallback((offset: number) => {
    const next = addDays(anchor, offset * 7);
    setMonth(next);
    onAnchorChange(next);
  }, [anchor, onAnchorChange]);

  const goToToday = useCallback(() => {
    const target = startOfLocalDay(new Date());
    onAnchorChange(target);
    setMonth(target);
  }, [onAnchorChange]);

  const newEventAtAnchor = useCallback(() => {
    const day = startOfLocalDay(anchor);
    const minutes = isSameDay(day, now) ? Math.min(Math.ceil((now.getHours() * 60 + now.getMinutes()) / 60) * 60, 23 * 60) : 9 * 60;
    const range = eventRangeFromSlots(day, minutes, minutes, false);
    setNewEventRange({ start: range.start, end: range.end });
  }, [anchor, now]);

  const finishSelection = (day: Date, dayIndex: number, column: HTMLElement, clientY: number, pointerId: number) => {
    const drag = dragRef.current;
    if (!drag || drag.dayIndex !== dayIndex || drag.pointerId !== pointerId) return;
    const current = slotAt(clientY, column);
    const range = eventRangeFromSlots(day, drag.anchor, current, current !== drag.anchor);
    dragRef.current = null;
    setDragPreview(null);
    setSelectedEvent(null);
    setNewEventRange({ start: range.start, end: range.end });
  };

  const selectDate = useCallback((date: Date) => {
    const target = startOfLocalDay(date);
    onAnchorChange(target);
    setMonth(target);
  }, [onAnchorChange]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || event.isComposing || event.defaultPrevented) return;
      if (event.target instanceof HTMLElement && event.target.closest("[data-shortcut-scope='modal'], [data-shortcut-scope='palette']")) return;
      if (isEditableTarget(event.target)) return;
      if (event.key === "-") {
        event.preventDefault();
        moveWeek(-1);
      } else if (event.key === "=") {
        event.preventDefault();
        moveWeek(1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [moveWeek]);

  const allDayByDay = useMemo(
    () => days.map((day) => events.filter((event) => event.allDay && occursOnDay(event, day))),
    [days, events],
  );
  const hasAllDay = allDayByDay.some((dayEvents) => dayEvents.length > 0);
  const timedByDay = useMemo(
    () => days.map((day) => layOutDayEvents(events.filter((event) => !event.allDay && occursOnDay(event, day)), day)),
    [days, events],
  );

  return (
    <section className="calendar-week" aria-label="Calendar week">
      <div className="calendar-week-main">
        <header className="calendar-week-header">
          <div className="calendar-week-heading"><span className="eyebrow">Calendar <span className="eyebrow-account">· {selectedAccountEmails.length ? selectedAccountEmails.join(", ") : "None selected"}</span></span><h1>{monthTitle(weekStart)}</h1></div>
          <div className="calendar-week-controls">
            <button type="button" className="calendar-today-button" onClick={newEventAtAnchor}>New event</button>
            <button type="button" className="calendar-today-button" onClick={goToToday}>Today</button>
            <HoverTooltip title="Previous week (-)"><button type="button" aria-label="Previous week (-)" onClick={() => moveWeek(-1)}><ChevronLeft size={20} /></button></HoverTooltip>
            <HoverTooltip title="Next week (=)"><button type="button" aria-label="Next week (=)" onClick={() => moveWeek(1)}><ChevronRight size={20} /></button></HoverTooltip>
          </div>
        </header>
        {error ? (
          <div className="calendar-error-notice" role="alert">
            <p>Calendar couldn’t be loaded. Try again or reconnect in Calendar Accounts.</p>
            <div>
              <button type="button" onClick={reload}>Try Again</button>
              <button type="button" onClick={onOpenSettings}>Calendar Accounts</button>
            </div>
          </div>
        ) : null}
        <div className="calendar-week-days" role="row">
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
          <div className="calendar-week-all-day" aria-label="All-day events">
            <span className="calendar-week-gutter">All day</span>
            {days.map((day, index) => (
              <div key={day.toDateString()}>
                {allDayByDay[index].map((event) => (
                  <button
                    type="button"
                    key={`${event.accountId}:${event.id}`}
                    data-calendar-event-trigger
                    aria-expanded={selectedEvent === event}
                    onClick={() => setSelectedEvent((current) => current === event ? null : event)}
                  >
                    {event.title}
                  </button>
                ))}
              </div>
            ))}
          </div>
        ) : null}
        <div className="calendar-week-timezone"><span className="calendar-week-gutter">{timeZoneLabel(anchor)}</span></div>
        <div
          className="calendar-week-scroll"
          ref={gridRef}
          onScroll={(event) => saveWeekScrollTop(event.currentTarget.scrollTop)}
        >
          <div className="calendar-week-grid">
            <div className="calendar-hour-labels" aria-hidden="true">
              {HOURS.map((hour) => (
                <span key={hour} style={{ top: hour * HOUR_HEIGHT }}>{hourLabel(hour)}</span>
              ))}
            </div>
            {days.map((day, dayIndex) => (
              <div
                className="calendar-week-column"
                key={day.toDateString()}
                aria-label={`Create event on ${new Intl.DateTimeFormat(undefined, { dateStyle: "full" }).format(day)}`}
                onPointerDown={(event) => {
                  if (event.button !== 0 || (event.target as HTMLElement).closest("[data-calendar-event-trigger]")) return;
                  const anchor = slotAt(event.clientY, event.currentTarget);
                  dragRef.current = { dayIndex, anchor, pointerId: event.pointerId };
                  setDragPreview({ dayIndex, startMinutes: anchor, endMinutes: anchor + 60 });
                  event.currentTarget.setPointerCapture?.(event.pointerId);
                }}
                onPointerMove={(event) => {
                  const drag = dragRef.current;
                  if (!drag || drag.dayIndex !== dayIndex || drag.pointerId !== event.pointerId) return;
                  const current = slotAt(event.clientY, event.currentTarget);
                  const range = eventRangeFromSlots(day, drag.anchor, current, current !== drag.anchor);
                  setDragPreview({ dayIndex, startMinutes: range.startMinutes, endMinutes: range.endMinutes });
                }}
                onPointerUp={(event) => finishSelection(day, dayIndex, event.currentTarget, event.clientY, event.pointerId)}
                onPointerCancel={() => { dragRef.current = null; setDragPreview(null); }}
              >
                {HOURS.map((hour) => <div className="calendar-hour-line" key={hour} />)}
                {dragPreview?.dayIndex === dayIndex ? (
                  <div className="calendar-create-selection" aria-hidden="true" style={{
                    top: dragPreview.startMinutes / 60 * HOUR_HEIGHT,
                    height: (dragPreview.endMinutes - dragPreview.startMinutes) / 60 * HOUR_HEIGHT,
                  }} />
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
                  const className = [
                    "calendar-schedule-event",
                    durationMinutes <= 30 && "calendar-schedule-event-compact",
                    durationMinutes <= 15 && "calendar-schedule-event-tight",
                  ].filter(Boolean).join(" ");
                  return (
                    <button
                      type="button"
                      className={className}
                      key={`${event.accountId}:${event.id}`}
                      data-calendar-event-trigger
                      aria-expanded={selectedEvent === event}
                      onClick={() => setSelectedEvent((current) => current === event ? null : event)}
                      style={{
                        top: (startMinutes / 60) * HOUR_HEIGHT,
                        height,
                        left: `${(lane / lanes) * 100}%`,
                        width: `${(span / lanes) * 100}%`,
                      }}
                      title={`${event.title}, ${formatEventTime(event)}`}
                    >
                      <strong>{event.title}</strong>
                      {durationMinutes > 30 ? <span>{formatEventTime(event)}</span> : null}
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
        {selectedEvent ? <EventViewer event={selectedEvent} onDismiss={() => setSelectedEvent(null)} /> : null}
        {newEventRange ? <CreateCalendarEventDialog
          start={newEventRange.start}
          end={newEventRange.end}
          accounts={accounts}
          calendars={calendars}
          onClose={() => setNewEventRange(null)}
          onCreated={() => { setNewEventRange(null); clearScheduleCache(); }}
        /> : null}
      </div>
      <aside className="calendar-week-side" aria-label="Calendar navigation">
        <MiniMonth
          month={month}
          selected={anchor}
          today={today}
          onSelect={selectDate}
          onMoveMonth={(offset) => setMonth((current) => new Date(current.getFullYear(), current.getMonth() + offset, 1))}
        />
        <CalendarList
          accounts={accounts}
          calendars={calendars}
          onToggle={onToggleCalendar}
          onAdd={onAddCalendarAccount}
        />
      </aside>
    </section>
  );
}
