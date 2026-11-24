import { EventNoteAction, EventNoteBacklinks } from "./NoteEntityContext";
import { readLocalStorage, writeLocalStorage, useLocalStorageState } from "./useLocalStorageState";
import { calendarAccountVisibility, showHiddenCalendarAccounts } from "./calendarVisibility";
import { useWorkspaceCommands } from "./workspaceCommands";
import { WorkspaceContextRail } from "./WorkspaceContextRail";
import { WorkspaceRelatedContext } from "./WorkspaceRelatedContext";
import { ContextSectionHeader } from "./ContextSections";
import { CalendarAgenda } from "./CalendarAgenda";
import { BookedTask, CalendarTasks, useAccountTasks } from "./CalendarTasks";
import { calendarRange, CALENDAR_RANGES, CALENDAR_RANGE_KEY, readCalendarRange, rangeForEvent, type CalendarRangeMode } from "./calendarRange";
import { CalendarDays, ChevronLeft, ChevronRight, ClipboardList, Plus, X } from "lucide-react";
import { meetingEventReference } from "./meetingNoteEntry";
import { isPreparable, preparationKey, type MeetingPreparationHost } from "./meetingPrep";
import { useCallback, useEffect, useEffectEvent, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type RefObject } from "react";
import { EventViewer, eventCalendarLabel } from "./CalendarSidebar";
import { HoverTooltip } from "./AppChrome";
import {
  HOUR_HEIGHT,
  addDays,
  formatEventTime,
  dateInputValue,
  isSameDay,
  occursOnDay,
  startOfLocalDay,
} from "./calendarTime";
import { isEditableTarget } from "./commands";
import { useCalendarSchedule } from "./useCalendarSchedule";
import { revalidateScheduleCache } from "./calendarScheduleCache";
import { CreateCalendarEventDialog } from "./CreateCalendarEventDialog";
import { CalendarColorRow } from "./CalendarColorMenu";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, ScheduleEvent, ThreadTask } from "./domain";
import { errorMessage } from "./errors";
import { MonthGrid, shiftMonth } from "./MonthGrid";
import { parseCalendarDate } from "./DatePickerField";
import { useDrawerDismiss } from "./useDrawerDismiss";
import { ICON_SIZE } from "./iconSizes";
import { bookedTaskFor, defaultEventCalendar, openTasksFor, showCalendar, TASK_BLOCK_MINUTES, taskBlockRequest, writableCalendars } from "./taskBooking";
import type { Notice } from "./useNotice";
import { formatShortcut } from "./shortcutLabels";
import { DayTimeGrid, eventRangeFromSlots, sameEvent, slotAt, type DayGridPreview } from "./DayTimeGrid";
import { useTaskBlockDrag } from "./useTaskBlockDrag";

export const WEEK_SCROLL_TOP_KEY = "threestrands.calendarWeek.scrollTop";
/** Calendar accounts collapsed in the week view's calendar list, remembered on this device. */
export const COLLAPSED_CALENDAR_ACCOUNTS_KEY = "threestrands.calendarWeek.collapsedAccounts";
const DEFAULT_WEEK_SCROLL_TOP = 7 * HOUR_HEIGHT;
const MAX_WEEK_SCROLL_TOP = 24 * HOUR_HEIGHT;
/** How often the "now" indicator re-renders, in milliseconds. */
const NOW_TICK_MS = 60_000;

function readWeekScrollTop(): number {
  return readLocalStorage(WEEK_SCROLL_TOP_KEY, (stored) => {
    const saved = stored === null || stored.trim() === "" ? Number.NaN : Number(stored);
    return Number.isFinite(saved) && saved >= 0 && saved <= MAX_WEEK_SCROLL_TOP ? saved : DEFAULT_WEEK_SCROLL_TOP;
  }, DEFAULT_WEEK_SCROLL_TOP);
}

function saveWeekScrollTop(scrollTop: number): void {
  if (!Number.isFinite(scrollTop) || scrollTop < 0 || scrollTop > MAX_WEEK_SCROLL_TOP) return;
  writeLocalStorage(WEEK_SCROLL_TOP_KEY, String(scrollTop));
}

const serializeAccounts = (accounts: ReadonlySet<string>) => JSON.stringify([...accounts]);

function readCollapsedCalendarAccounts(): Set<string> {
  return readLocalStorage(COLLAPSED_CALENDAR_ACCOUNTS_KEY, (raw) => {
    const stored: unknown = JSON.parse(raw ?? "[]");
    return new Set(Array.isArray(stored) ? stored.filter((email): email is string => typeof email === "string") : []);
  }, new Set<string>());
}

function monthTitle(date: Date): string {
  return new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(date);
}

function MiniMonth({
  month,
  selected,
  weekStart,
  onSelect,
  onMonthChange,
}: {
  month: Date;
  selected: Date;
  weekStart: number;
  onSelect(date: Date): void;
  onMonthChange(month: Date): void;
}) {
  const titleId = useId();
  return (
    <section className="calendar-mini-month" aria-label="Month picker">
      <header>
        <h3 id={titleId}>{monthTitle(month)}</h3>
        <div>
          <button type="button" className="btn-icon btn-icon-sm" aria-label="Previous Month" onClick={() => onMonthChange(shiftMonth(month, -1))}><ChevronLeft size={ICON_SIZE.sm} /></button>
          <button type="button" className="btn-icon btn-icon-sm" aria-label="Next Month" onClick={() => onMonthChange(shiftMonth(month, 1))}><ChevronRight size={ICON_SIZE.sm} /></button>
        </div>
      </header>
      <MonthGrid
        labelledBy={titleId}
        cursor={month}
        onCursorChange={onMonthChange}
        selected={dateInputValue(selected)}
        onSelect={(value) => { const day = parseCalendarDate(value); if (day) onSelect(day); }}
        weekStart={weekStart}
        weekdays="narrow"
      />
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
  const [collapsed, setCollapsed] = useLocalStorageState<Set<string>>(COLLAPSED_CALENDAR_ACCOUNTS_KEY, readCollapsedCalendarAccounts, serializeAccounts);
  const { hidden } = calendarAccountVisibility(accounts, calendars);
  return (
    <section className="calendar-list" aria-label="Calendars">
      <header>
        <CalendarDays size={ICON_SIZE.lg} />
        <h3>Calendars</h3>
        <HoverTooltip title="Add calendar account"><button type="button" className="btn-icon btn-icon-sm" aria-label="Add Calendar Account" onClick={onAdd}><Plus size={ICON_SIZE.sm} /></button></HoverTooltip>
      </header>
      {accounts.length === 0 ? <p className="calendar-list-empty">No calendar accounts connected.</p> : null}
      {hidden.length > 0 ? (
        <p className="calendar-list-hidden">
          {hidden.length === 1 ? "1 account hidden" : `${hidden.length} accounts hidden`}
          <HoverTooltip title="Turn on each hidden account's primary calendar"><button type="button" className="btn-link" onClick={() => showHiddenCalendarAccounts(accounts, calendars, onToggle)}>Show All</button></HoverTooltip>
        </p>
      ) : null}
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
              <ChevronLeft size={ICON_SIZE.md} className={open ? "calendar-list-chevron-open" : "calendar-list-chevron"} />
            </button>
            {open ? accountCalendars.map((calendar) => (
              <CalendarColorRow key={calendar.id} accountId={account.email} calendarId={calendar.id} calendarName={calendar.name}>
                <label>
                  <input
                    type="checkbox"
                    checked={calendar.selected}
                    onChange={(event) => onToggle(account.email, calendar.id, event.target.checked)}
                  />
                  <span>{calendar.name}</span>
                </label>
              </CalendarColorRow>
            )) : null}
          </div>
        );
      })}
    </section>
  );
}

export type CalendarWorkspaceSnapshot = { selected: ScheduleEvent | null; initialEvent: ScheduleEvent | null | undefined; mode: CalendarRangeMode; navigationOpen: boolean };

export function CalendarWeekView({
  anchor,
  initialEvent,
  createEventRequested = false,
  onCreateEventRequestHandled,
  onAnchorChange,
  accounts,
  calendars,
  onToggleCalendar,
  onAddCalendarAccount,
  onOpenSettings,
  onCreated,
  mailAccountIds = [],
  onOpenThread = () => {},
  onOpenNote,
  onAddNote,
  onSelectedEventChange,
  taskRefreshKey = 0,
  onTasksChanged,
  onEditTask,
  onNotice,
  active = true,
  session,
  preparation,
}: {
  session?: RefObject<CalendarWorkspaceSnapshot | null>;
  /** Meeting preparation in the event details: which meeting is being
   * prepared, and the view to show for it. */
  preparation?: MeetingPreparationHost;
  createEventRequested?: boolean;
  onCreateEventRequestHandled?(): void;
  /** Changes whenever tasks change elsewhere, to reload Open tasks. */
  taskRefreshKey?: number;
  onTasksChanged?(): void;
  /** Opens the task editor, from the task an event books time for. */
  onEditTask?(task: ThreadTask): void;
  /** Says a task block was booked, with Undo, or why it was not. */
  onNotice?(notice: Notice): void;
  active?: boolean;
  mailAccountIds?: string[];
  onOpenThread?(id: string): void;
  onOpenNote?(id: string): void;
  /** Add Note for an event: its line in that day's note. */
  onAddNote?(event: ScheduleEvent): void;
  /** Reports the event whose details are open, for commands that act on it. */
  onSelectedEventChange?(event: ScheduleEvent | null): void;
  anchor: Date;
  /** Opens these event details when arriving from a conversation. */
  initialEvent?: ScheduleEvent | null;
  onAnchorChange(date: Date): void;
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  onToggleCalendar(accountId: string, calendarId: string, selected: boolean): void;
  onAddCalendarAccount(): void;
  onOpenSettings(): void;
  onCreated?(): void;
}) {
  const [month, setMonth] = useState(() => startOfLocalDay(anchor));
  const [selectedEvent, setSelectedEvent] = useState<ScheduleEvent | null>(() => session?.current?.initialEvent===initialEvent ? session?.current?.selected ?? initialEvent ?? null : initialEvent ?? null);
  const [mode, setMode] = useState<CalendarRangeMode>(() => session?.current?.initialEvent===initialEvent && session?.current ? session.current.mode : initialEvent ? rangeForEvent(initialEvent, readCalendarRange()) : readCalendarRange());
  const [navigationOpen, setNavigationOpen] = useState(session?.current?.navigationOpen ?? false);
  const navigationToggle = useRef<HTMLButtonElement>(null);
  const navigationDismiss = useDrawerDismiss(navigationOpen, setNavigationOpen, navigationToggle);
  const eventTrigger = useRef<HTMLButtonElement | null>(null);
  const focusAgendaDetails = useRef(false);
  const [newEventRange, setNewEventRange] = useState<{ start: Date; end: Date; task?: ThreadTask } | null>(null);
  const [selectionPreview, setSelectionPreview] = useState<DayGridPreview | null>(null);
  const dragRef = useRef<{ dayIndex: number; anchor: number; pointerId: number } | null>(null);
  const [now, setNow] = useState(() => new Date());
  const gridRef = useRef<HTMLDivElement>(null);

  const range = useMemo(() => calendarRange(anchor, mode), [anchor, mode]);
  const weekStart = range.start;
  const days = range.days;
  const today = startOfLocalDay(now);
  const todayVisible = days.some((day) => isSameDay(day, today));
  // The week merges every connected calendar account, so name the scope like the mail "All accounts" view.
  const visibility = calendarAccountVisibility(accounts, calendars);
  const { selectedAccountEmails, label: accountScope } = visibility;
  const showsHiddenCount = visibility.total > 1 && visibility.hidden.length > 0;

  const { events, loading, error, reload, refresh } = useCalendarSchedule({
    timeMin: weekStart.toISOString(),
    timeMax: range.end.toISOString(),
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  }, range.step);

  useEffect(() => { setSelectedEvent((current) => current && days.some((day) => occursOnDay(current, day)) ? current : null); }, [days]);
  const reportSelection = useEffectEvent((event: ScheduleEvent | null) => onSelectedEventChange?.(event));
  useEffect(() => { reportSelection(selectedEvent); }, [selectedEvent]);
  useEffect(() => () => reportSelection(null), []);
  const restoredInitial=useRef(session?.current?.initialEvent===initialEvent && !!session?.current);
  useEffect(() => { if (restoredInitial.current) { restoredInitial.current=false; return; } if (initialEvent) { setMode(rangeForEvent(initialEvent, readCalendarRange())); setSelectedEvent(initialEvent); } }, [initialEvent]);
  useEffect(() => { if (!loading) setSelectedEvent((current) => current ? events.find((event) => sameEvent(current, event)) ?? (initialEvent && sameEvent(current, initialEvent) ? current : null) : null); }, [events, loading, initialEvent]);

  useEffect(() => {
    if (selectedEvent && focusAgendaDetails.current) {
      focusAgendaDetails.current = false;
      document.getElementById("calendar-event-viewer")?.focus();
    }
  }, [selectedEvent]);

  const prepareButton = useRef<HTMLButtonElement | null>(null);
  const preparingSelected = Boolean(preparation && selectedEvent && preparation.key !== null && preparation.key === preparationKey(selectedEvent));
  const closePreparation = () => {
    preparation?.onChange(null);
    requestAnimationFrame(() => prepareButton.current?.focus());
  };
  useWorkspaceCommands({ "meetings.closePrepare": () => { if (preparingSelected) closePreparation(); } }, active);
  const prepareAgendaEvent = (event: ScheduleEvent) => {
    openAgendaEvent(event);
    preparation?.onChange(preparationKey(event));
  };
  const openAgendaEvent = (event: ScheduleEvent) => {
    focusAgendaDetails.current = true;
    eventTrigger.current = null;
    if (!todayVisible) {
      // A weekend agenda remains available even when the preferred grid is Workweek.
      if (mode === "workweek" && [0, 6].includes(today.getDay())) setMode("day");
      onAnchorChange(today);
      setMonth(today);
    }
    setSelectedEvent(event);
    if (!event.allDay && gridRef.current) {
      const start = new Date(event.start);
      const minutes = isSameDay(start, today) ? start.getHours() * 60 + start.getMinutes() : 0;
      gridRef.current.scrollTop = Math.max(0, minutes / 60 * HOUR_HEIGHT - gridRef.current.clientHeight / 2);
    }
  };

  useEffect(() => {
    if (gridRef.current) gridRef.current.scrollTop = readWeekScrollTop();
  }, []);

  useEffect(() => {
    if (!initialEvent || initialEvent.allDay || !gridRef.current) return;
    const start = new Date(initialEvent.start);
    const minutes = start.getHours() * 60 + start.getMinutes();
    gridRef.current.scrollTop = Math.max(0, minutes / 60 * HOUR_HEIGHT - gridRef.current.clientHeight / 2);
  }, [initialEvent]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), NOW_TICK_MS);
    return () => window.clearInterval(timer);
  }, []);

  const moveWeek = useCallback((offset: number) => {
    const next = addDays(anchor, offset * range.step);
    setMonth(next);
    onAnchorChange(next);
  }, [anchor, onAnchorChange, range.step]);

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

  useEffect(() => {
    if (!active || !createEventRequested) return;
    newEventAtAnchor();
    onCreateEventRequestHandled?.();
  }, [active, createEventRequested, newEventAtAnchor, onCreateEventRequestHandled]);

  useWorkspaceCommands({ "calendar.new": newEventAtAnchor, "calendar.goToday": goToToday }, active);

  const finishSelection = (day: Date, dayIndex: number, column: HTMLElement, clientY: number, pointerId: number) => {
    const drag = dragRef.current;
    if (!drag || drag.dayIndex !== dayIndex || drag.pointerId !== pointerId) return;
    const current = slotAt(clientY, column);
    const range = eventRangeFromSlots(day, drag.anchor, current, current !== drag.anchor);
    dragRef.current = null;
    setSelectionPreview(null);
    setSelectedEvent(null);
    setNewEventRange({ start: range.start, end: range.end });
  };

  const { tasks: accountTasks, loading: tasksLoading, error: tasksError, reload: reloadTasks } = useAccountTasks(mailAccountIds, taskRefreshKey);
  const openTasks = openTasksFor(accountTasks, mailAccountIds, { includeNoteTasks: true });
  const bookedTask = selectedEvent ? bookedTaskFor(selectedEvent, accountTasks) : undefined;
  const tasksChanged = () => { reloadTasks(); onTasksChanged?.(); };

  const bookTask = async (task: ThreadTask, day: Date, startMinutes: number) => {
    const calendar = defaultEventCalendar(writableCalendars(accounts, calendars), task.accountId);
    if (!calendar) {
      onNotice?.({ message: "Connect a Google Calendar account with event access to book time for tasks" });
      return;
    }
    try {
      await showCalendar(calendar, calendars);
      const created = await mailClient.createCalendarEvent(taskBlockRequest(task, calendar, new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, startMinutes)));
      revalidateScheduleCache();
      onCreated?.();
      const when = `${new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(new Date(created.start))} ${formatEventTime(created)}`;
      onNotice?.({ message: `Booked ${when} for “${task.title}”`, undo: () => void (async () => {
        try {
          await mailClient.deleteCalendarEvent(created);
          revalidateScheduleCache();
          onNotice?.({ message: "Booked time removed" });
        } catch (reason) {
          onNotice?.({ message: errorMessage(reason) });
        }
      })() });
    } catch (reason) {
      onNotice?.({ message: errorMessage(reason) });
    }
  };

  const taskDrag = useTaskBlockDrag<ThreadTask>({
    gridRef,
    days,
    blockMinutes: TASK_BLOCK_MINUTES,
    onDrop: (task, day, startMinutes) => void bookTask(task, day, startMinutes),
  });

  /** Opens New Event for the task, at the next half hour on the shown day. */
  const scheduleTask = (task: ThreadTask) => {
    if (taskDrag.clickSuppressed()) return;
    const day = startOfLocalDay(anchor);
    const minutes = isSameDay(day, now)
      ? Math.min(Math.ceil((now.getHours() * 60 + now.getMinutes()) / TASK_BLOCK_MINUTES) * TASK_BLOCK_MINUTES, 24 * 60 - TASK_BLOCK_MINUTES)
      : 9 * 60;
    const start = new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, minutes);
    setSelectedEvent(null);
    setNewEventRange({ start, end: new Date(start.getTime() + TASK_BLOCK_MINUTES * 60_000), task });
  };

  const selectDate = useCallback((date: Date) => {
    const target = startOfLocalDay(date);
    onAnchorChange(target);
    setMonth(target);
  }, [onAnchorChange]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!active) return;
      if (event.metaKey || event.ctrlKey || event.altKey || event.isComposing || event.defaultPrevented) return;
      if (event.target instanceof HTMLElement && event.target.closest("[data-shortcut-scope='modal'], [data-shortcut-scope='palette']")) return;
      if (isEditableTarget(event.target)) return;
      if (event.key === "Tab") {
        // A focused control keeps native Tab traversal, matching the mail and task Tab shortcuts.
        if (event.target instanceof HTMLElement && event.target !== document.body && event.target.matches("button, a[href], [tabindex]")) return;
        event.preventDefault();
        moveWeek(event.shiftKey ? -1 : 1);
      } else if (event.key === "-") {
        event.preventDefault();
        moveWeek(-1);
      } else if (event.key === "=") {
        event.preventDefault();
        moveWeek(1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [moveWeek, active]);

  const snapshot=useRef<CalendarWorkspaceSnapshot | null>(null);
  snapshot.current={selected:selectedEvent,initialEvent,mode,navigationOpen};
  useLayoutEffect(() => () => { if (session) session.current=snapshot.current; }, [session]);

  return (
    <section className={`calendar-week${taskDrag.draggingId ? " calendar-week-task-drag" : ""}`} aria-label="Calendar week" style={{ "--calendar-day-count": days.length } as CSSProperties}>
      <div className="calendar-week-main">
        <header className="calendar-week-header">
          <div className="calendar-week-heading"><span className="eyebrow">Calendar <span className="eyebrow-account" title={selectedAccountEmails.join(", ") || undefined}>· {accountScope}</span></span><h1>{monthTitle(weekStart)}</h1><p className="calendar-visible-range">{weekStart.toLocaleDateString(undefined, { month: "short", day: "numeric" })}{days.length > 1 ? ` – ${days.at(-1)!.toLocaleDateString(undefined, { month: "short", day: "numeric" })}` : ""}</p></div>
          <div className="calendar-week-controls">
            <div className="segmented" role="group" aria-label="Calendar Range">{CALENDAR_RANGES.map((item) => <button key={item.value} type="button" className="segment" aria-pressed={mode === item.value} onClick={() => { setMode(item.value); writeLocalStorage(CALENDAR_RANGE_KEY,item.value); }}>{item.label}</button>)}</div>
            <button ref={navigationToggle} type="button" className="btn" aria-expanded={navigationOpen} aria-label={showsHiddenCount ? `Calendars, ${visibility.shown} of ${visibility.total} accounts shown` : undefined} onClick={() => setNavigationOpen((value) => !value)}><CalendarDays size={ICON_SIZE.sm} />Calendars{showsHiddenCount ? <span className="calendar-shown-count" aria-hidden="true">{visibility.shown}/{visibility.total}</span> : null}</button>
            <button type="button" className="btn" title="Today (t)" aria-keyshortcuts="t" onClick={goToToday}>Today</button>
            <div className="calendar-week-nav">
              <HoverTooltip title={`Previous ${mode === "week" || mode === "workweek" ? "week" : "range"} (-)`}><button type="button" className="btn-icon" aria-label={`Previous ${mode === "week" || mode === "workweek" ? "Week" : "Range"} (-)`} onClick={() => moveWeek(-1)}><ChevronLeft size={ICON_SIZE.lg} /></button></HoverTooltip>
              <HoverTooltip title={`Next ${mode === "week" || mode === "workweek" ? "week" : "range"} (=)`}><button type="button" className="btn-icon" aria-label={`Next ${mode === "week" || mode === "workweek" ? "Week" : "Range"} (=)`} onClick={() => moveWeek(1)}><ChevronRight size={ICON_SIZE.lg} /></button></HoverTooltip>
            </div>
            <button type="button" className="btn" title={`New event (b, ${formatShortcut("Shift+n")})`} aria-keyshortcuts="b Shift+N" onClick={newEventAtAnchor}><Plus size={ICON_SIZE.md} />New Event…</button>
          </div>
        </header>
        {error ? (
          <div className="calendar-error-notice" role="alert">
            <p>Calendar couldn’t be loaded. Try again or reconnect in Calendar Accounts.</p>
            <div>
              <button type="button" className="btn btn-sm" onClick={reload}>Try Again</button>
              <button type="button" className="btn btn-sm" onClick={onOpenSettings}>Calendar Accounts</button>
            </div>
          </div>
        ) : null}
        <DayTimeGrid
          days={days}
          events={events}
          now={now}
          loading={loading}
          selectedEvent={selectedEvent}
          onToggleEvent={(event, trigger) => { eventTrigger.current=trigger; setSelectedEvent((current) => sameEvent(current,event) ? null : event); }}
          onOpenEventContext={(event) => { setSelectedEvent(event); window.dispatchEvent(new CustomEvent("threestrands:focus-context", { detail: "Meeting context" })); }}
          preview={taskDrag.preview ?? selectionPreview}
          scrollRef={gridRef}
          onScroll={(event) => saveWeekScrollTop(event.currentTarget.scrollTop)}
          timeZoneAt={anchor}
          columnProps={(day, dayIndex) => ({
            "aria-label": `Create event on ${new Intl.DateTimeFormat(undefined, { dateStyle: "full" }).format(day)}`,
            onPointerDown: (event) => {
              if (event.button !== 0 || (event.target as HTMLElement).closest("[data-calendar-event-trigger]")) return;
              const anchor = slotAt(event.clientY, event.currentTarget);
              dragRef.current = { dayIndex, anchor, pointerId: event.pointerId };
              setSelectionPreview({ dayIndex, startMinutes: anchor, endMinutes: anchor + 60 });
              event.currentTarget.setPointerCapture?.(event.pointerId);
            },
            onPointerMove: (event) => {
              const drag = dragRef.current;
              if (!drag || drag.dayIndex !== dayIndex || drag.pointerId !== event.pointerId) return;
              const current = slotAt(event.clientY, event.currentTarget);
              const range = eventRangeFromSlots(day, drag.anchor, current, current !== drag.anchor);
              setSelectionPreview({ dayIndex, startMinutes: range.startMinutes, endMinutes: range.endMinutes });
            },
            onPointerUp: (event) => finishSelection(day, dayIndex, event.currentTarget, event.clientY, event.pointerId),
            onPointerCancel: () => { dragRef.current = null; setSelectionPreview(null); },
          })}
        />
        {newEventRange ? <CreateCalendarEventDialog
          start={newEventRange.start}
          end={newEventRange.end}
          accounts={accounts}
          calendars={calendars}
          initialTitle={newEventRange.task?.title}
          sourceTask={newEventRange.task}
          onClose={() => setNewEventRange(null)}
          onCreated={(event) => { setNewEventRange(null); setSelectedEvent(event); revalidateScheduleCache(); onCreated?.(); }}
        /> : null}
      </div>
      {navigationOpen ? <aside ref={navigationDismiss.drawer} onKeyDown={navigationDismiss.onKeyDown} className="calendar-week-side workspace-calendar-navigation" aria-label="Calendar navigation">
        <button type="button" className="btn-icon btn-icon-sm drawer-close" aria-label="Close Calendars" title="Close Calendars" onClick={navigationDismiss.close}><X size={ICON_SIZE.sm} aria-hidden="true" /></button>
        <MiniMonth
          month={month}
          selected={anchor}
          weekStart={0}
          onSelect={selectDate}
          onMonthChange={setMonth}
        />
        <CalendarList
          accounts={accounts}
          calendars={calendars}
          onToggle={onToggleCalendar}
          onAdd={onAddCalendarAccount}
        />
      </aside> : null}
      <WorkspaceContextRail label="Meeting context" onReturn={() => (eventTrigger.current?.isConnected ? eventTrigger.current : gridRef.current)?.focus()}>
        {selectedEvent ? <>
          <ContextSectionHeader title="Event details" icon={<CalendarDays size={ICON_SIZE.xs} />}
            actions={<>
              {preparation && isPreparable(selectedEvent) ? <button ref={prepareButton} type="button" className="btn-icon btn-icon-sm" aria-label="Prepare" title="Prepare" aria-pressed={preparingSelected}
                onClick={() => preparation.onChange(preparingSelected ? null : preparationKey(selectedEvent))}><ClipboardList size={ICON_SIZE.sm} aria-hidden="true" /></button> : null}
              {onAddNote && isPreparable(selectedEvent) ? <EventNoteAction key={`${selectedEvent.accountId}:${selectedEvent.calendarId}:${selectedEvent.id}`} occurrence={meetingEventReference(selectedEvent)!} onActivate={() => onAddNote(selectedEvent)} /> : null}
            </>} />
          <EventViewer key={`${selectedEvent.accountId}:${selectedEvent.calendarId}:${selectedEvent.id}`} embedded event={selectedEvent} calendarLabel={eventCalendarLabel(selectedEvent, calendars)} onDismiss={() => {
            setSelectedEvent(null);
            if (!eventTrigger.current) document.getElementById("workspace-Meeting-context")?.focus();
          }} onUpdated={setSelectedEvent} />
          {bookedTask ? <BookedTask task={bookedTask} onChanged={tasksChanged} onEditTask={onEditTask} /> : null}
          {preparingSelected ? preparation!.render(selectedEvent, closePreparation) : <>
            {meetingEventReference(selectedEvent) ? <EventNoteBacklinks occurrence={meetingEventReference(selectedEvent)!} onOpenNote={onOpenNote} /> : null}
            <WorkspaceRelatedContext target={{ kind: "meeting", event: selectedEvent }} accountIds={mailAccountIds} onOpenThread={onOpenThread} />
          </>}
        </> : <>
          <CalendarAgenda today={today} calendars={calendars} onOpenEvent={openAgendaEvent} onAddNote={onAddNote} onPrepare={preparation ? prepareAgendaEvent : undefined} schedule={todayVisible ? { events, loading, error, reload, refresh } : undefined} />
          <CalendarTasks
            tasks={openTasks}
            loading={tasksLoading}
            error={tasksError}
            reload={reloadTasks}
            events={events}
            now={now}
            multipleAccounts={new Set(mailAccountIds).size > 1}
            draggingTaskId={taskDrag.draggingId}
            onSchedule={scheduleTask}
            onDragStart={taskDrag.startDrag}
            onChanged={tasksChanged}
            onNotice={onNotice}
          />
        </>}
      </WorkspaceContextRail>
    </section>
  );
}
