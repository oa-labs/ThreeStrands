import { CalendarDays, Check, ClipboardList, ListChecks, ListFilter, NotebookPen } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ContextRow, ContextSection } from "./ContextSections";
import { calendarColorStyle, useCalendarColors, type CalendarColorMap } from "./calendarColors";
import { addDays, formatEventTime, startOfLocalDay } from "./calendarTime";
import { agendaEventsFor, formatClockTime, isDeclined, isPastEvent } from "./agendaEvents";
import { responseLabel } from "./calendarResponse";
import { agendaConflicts, mentionsRecording } from "./todayInsights";
import { useCalendarSchedule } from "./useCalendarSchedule";
import type { CalendarOption, ScheduleEvent, ThreadTask } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { isWorkingLocation, WorkingLocationIcon } from "./WorkingLocation";
import { isPreparable, preparationKey } from "./meetingPrep";
import { meetingEventReference } from "./meetingNoteEntry";
import { noteReferenceHref } from "./noteEntities";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { readLocalStorage, writeLocalStorage } from "./useLocalStorageState";

import { usePullToRefresh } from "./usePullToRefresh";
import { PullToRefreshStatus } from "./PullToRefreshStatus";

type Schedule = Omit<ReturnType<typeof useCalendarSchedule>, "refresh"> & { refresh?: () => Promise<void> };
type AgendaProps = {
  pullToRefresh?: boolean;
  /** The day the agenda lists. */
  today: Date;
  calendars: CalendarOption[];
  onOpenEvent(event: ScheduleEvent): void;
  /**
   * The current time, for an agenda of the current day: finished events are
   * dimmed and a Now divider sits before the rest.
   */
  now?: Date;
  /**
   * Which day the agenda names: today, tomorrow (showing its first few events
   * before "Show more"), or a day the page has chosen, such as a daily note's.
   */
  day?: AgendaDay;
  /** The section's id, for its remembered collapse; defaults by `day`. */
  id?: string;
  /** Names the day under the heading; left out where the page already says which day it is. */
  showDate?: boolean;
  /** Says which meetings overlap another, and which of those are recorded. */
  markConflicts?: boolean;
  /** Offers Add Note on each event that a note can link. */
  onAddNote?(event: ScheduleEvent): void;
  /** The task an event books time for, if any; with `onOpenTask`, that event's row offers a button to open it. */
  taskFor?(event: ScheduleEvent): ThreadTask | undefined;
  onOpenTask?(task: ThreadTask): void;
  /** Offers Prepare on each event that a note can link. */
  onPrepare?(event: ScheduleEvent): void;
  /** The meeting being prepared (`preparationKey`), shown selected. */
  selectedKey?: string | null;
  /** A short line about the event after its time, such as its readiness. */
  describe?(event: ScheduleEvent): string | null | undefined;
  /**
   * For a page that states the day's working location once: working locations
   * are left out of the rows and the count, and this one is named in the
   * section's note instead.
   */
  workingLocation?: ScheduleEvent | null;
  /**
   * When set, offers a calendar filter in the section heading (when more than
   * one calendar is selected) and remembers hidden calendars under this key.
   */
  calendarFilterKey?: string;
};

const AGENDA_FILTER_PREFIX = "threestrands.agenda.hiddenCalendars.";
const EMPTY_HIDDEN = new Set<string>();

/** Stable per-calendar key safe for arbitrary account emails and calendar ids. */
export function agendaCalendarOptionKey(calendar: Pick<CalendarOption, "accountId" | "id">): string {
  return JSON.stringify([calendar.accountId, calendar.id]);
}

function readHiddenCalendars(storageKey: string): Set<string> {
  return readLocalStorage(storageKey, (raw) => {
    const saved: unknown = JSON.parse(raw ?? "[]");
    return new Set(Array.isArray(saved) ? saved.filter((item): item is string => typeof item === "string") : []);
  }, new Set<string>());
}

const serializeHiddenCalendars = (keys: ReadonlySet<string>) => JSON.stringify([...keys]);

function useHiddenAgendaCalendars(filterKey?: string) {
  const storageKey = filterKey ? `${AGENDA_FILTER_PREFIX}${filterKey}` : "";
  const [state, setState] = useState(() => ({
    key: storageKey,
    hidden: storageKey ? readHiddenCalendars(storageKey) : EMPTY_HIDDEN,
  }));
  if (state.key !== storageKey) {
    setState({ key: storageKey, hidden: storageKey ? readHiddenCalendars(storageKey) : EMPTY_HIDDEN });
  }
  const hidden = state.key === storageKey ? state.hidden : EMPTY_HIDDEN;
  const toggle = useCallback((calendar: CalendarOption) => {
    if (!storageKey) return;
    const key = agendaCalendarOptionKey(calendar);
    setState((current) => {
      const next = new Set(current.key === storageKey ? current.hidden : readHiddenCalendars(storageKey));
      if (next.has(key)) next.delete(key);
      else next.add(key);
      writeLocalStorage(storageKey, serializeHiddenCalendars(next));
      return { key: storageKey, hidden: next };
    });
  }, [storageKey]);
  const showAll = useCallback(() => {
    if (!storageKey) return;
    setState((current) => {
      if (current.key === storageKey && current.hidden.size === 0) return current;
      const next = new Set<string>();
      writeLocalStorage(storageKey, serializeHiddenCalendars(next));
      return { key: storageKey, hidden: next };
    });
  }, [storageKey]);
  return { hidden, toggle, showAll };
}

type AgendaDay = "today" | "tomorrow" | "chosen";

/** How the section and its states name each kind of day. */
const AGENDA_TEXT: Record<AgendaDay, { id: string; title: string; loading: string; empty: string }> = {
  today: { id: "calendar-agenda", title: "Today's agenda", loading: "Loading today's agenda…", empty: "No events today." },
  tomorrow: { id: "calendar-agenda-tomorrow", title: "Tomorrow", loading: "Loading tomorrow's agenda…", empty: "No events tomorrow." },
  chosen: { id: "calendar-agenda-day", title: "Agenda", loading: "Loading this day's agenda…", empty: "No events on this day." },
};

/** Events Tomorrow shows before "Show more". */
export const TOMORROW_AGENDA_ROWS = 3;

/** Reuse the visible range when it includes today; otherwise load today's local day. */
export function CalendarAgenda({ schedule, ...props }: AgendaProps & { schedule?: Schedule }) {
  return schedule ? <Agenda {...props} schedule={schedule} /> : <TodayAgenda {...props} />;
}

function TodayAgenda(props: AgendaProps) {
  const start = startOfLocalDay(props.today);
  const schedule = useCalendarSchedule({
    timeMin: start.toISOString(),
    timeMax: addDays(start, 1).toISOString(),
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  });
  return <Agenda {...props} schedule={schedule} />;
}

function Agenda({ pullToRefresh = true, today, calendars, onOpenEvent, onAddNote, onPrepare, taskFor, onOpenTask, selectedKey = null, describe, now, day = "today", id, showDate = true, markConflicts = false, workingLocation, calendarFilterKey, schedule: { events, loading, error, reload, refresh: refreshSchedule } }: AgendaProps & { schedule: Schedule }) {
  const agendaRef = useRef<HTMLElement>(null);
  const refresh = useCallback(async () => { if (refreshSchedule) await refreshSchedule(); else reload(); }, [refreshSchedule, reload]);
  const pull = usePullToRefresh(agendaRef, pullToRefresh ? refresh : undefined);
  const colors = useCalendarColors();
  const filter = useHiddenAgendaCalendars(calendarFilterKey);
  const selected = calendars.filter((calendar) => calendar.selected);
  const canFilterCalendars = Boolean(calendarFilterKey) && selected.length > 1;
  const visibleCalendars = canFilterCalendars
    ? selected.filter((calendar) => !filter.hidden.has(agendaCalendarOptionKey(calendar)))
    : selected;
  const hiddenSelectedCount = selected.length - visibleCalendars.length;
  const agenda = agendaEventsFor(events, today, visibleCalendars).filter((event) => workingLocation === undefined || !isWorkingLocation(event));
  const multipleAccounts = new Set(visibleCalendars.map((calendar) => calendar.accountId)).size > 1;
  // One calendar needs no name on each row; the name (often the account address) only tells calendars apart.
  const multipleCalendars = new Set(agenda.map((event) => `${event.accountId}\u0000${event.calendarId}`)).size > 1;
  const conflicts = markConflicts ? agendaConflicts(agenda) : new Map<ScheduleEvent, ScheduleEvent[]>();
  const pastCount = now ? agenda.filter((event) => isPastEvent(event, now)).length : 0;
  // All-day events list first and are never past during their day, so the
  // divider goes before the first timed event still to finish.
  const upcomingIndex = now ? agenda.findIndex((event) => !event.allDay && !isPastEvent(event, now)) : -1;
  const rows: ReactNode[] = [];
  agenda.forEach((event, index) => {
    if (now && pastCount > 0 && index === upcomingIndex) rows.push(<NowDivider key="now" now={now} />);
    const calendar = visibleCalendars.find((calendar) => calendar.accountId === event.accountId && calendar.id === event.calendarId);
    // A calendar named after its account already says which account it is.
    const account = multipleAccounts && calendar?.name.toLocaleLowerCase() !== event.accountId.toLocaleLowerCase() ? event.accountId : null;
    const overlapping = conflicts.get(event) ?? [];
    const conflict = overlapping.length
      ? [`Overlaps ${overlapping.map((other) => other.title).join(", ")}`, mentionsRecording(event) ? "Recorded" : null]
      : [];
    const detail = [formatEventTime(event), describe?.(event), ...conflict, event.location, multipleCalendars ? calendar?.name : null, account, responseLabel(event)].filter(Boolean).join(" · ");
    const key = preparationKey(event);
    const isSelected = selectedKey !== null && key === selectedKey;
    const bookedTask = onOpenTask ? taskFor?.(event) : undefined;
    const rowActions = [
      ...(bookedTask && onOpenTask ? [<OpenTaskButton key="task" task={bookedTask} onOpenTask={onOpenTask} />] : []),
      ...(isPreparable(event) && onPrepare ? [<PrepareButton key="prepare" event={event} onPrepare={onPrepare} pressed={selectedKey !== null ? isSelected : undefined} />] : []),
      ...(isPreparable(event) && onAddNote ? [<AddNoteButton key="note" event={event} onAddNote={onAddNote} />] : []),
    ];
    rows.push(<ContextRow
      rowKey={key ? `meeting:${key}` : undefined}
      pressed={selectedKey !== null ? isSelected : undefined}
      key={`${event.accountId}:${event.calendarId}:${event.id}`}
      className={[now && isPastEvent(event, now) ? "agenda-event-past" : "", isDeclined(event) ? "agenda-event-declined" : "", overlapping.length ? "agenda-event-conflict" : "", isSelected ? "agenda-event-selected" : ""].filter(Boolean).join(" ") || undefined}
      icon={<CalendarDays size={ICON_SIZE.sm} style={{ ...calendarColorStyle(colors, event.accountId, event.calendarId), color: "var(--calendar-color, var(--accent))" }} />}
      title={event.title}
      detail={detail}
      onActivate={() => onOpenEvent(event)}
      actions={rowActions.length ? rowActions : undefined}
    />);
  });
  if (now && pastCount > 0 && pastCount === agenda.length) rows.push(<NowDivider key="now" now={now} />);
  const text = AGENDA_TEXT[day];
  const dateText = showDate ? today.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" }) : null;
  const dateNote = dateText || workingLocation
    // One element, so the note's flex layout cannot strand the separator from the location.
    ? <span>{dateText}{dateText && workingLocation ? " · " : null}{workingLocation ? <span className="agenda-working-location"><WorkingLocationIcon type={workingLocation.workingLocationType} />{workingLocation.title}</span> : null}</span>
    : undefined;
  return <ContextSection
    sectionRef={agendaRef}
    busy={pull.refreshing}
    id={id ?? text.id}
    title={text.title}
    note={dateNote}
    count={agenda.length}
    actions={canFilterCalendars ? <AgendaCalendarFilter calendars={selected} hidden={filter.hidden} colors={colors} onToggle={filter.toggle} onShowAll={filter.showAll} /> : undefined}
    rows={rows}
    limit={day === "tomorrow" ? TOMORROW_AGENDA_ROWS : rows.length}
    status={<PullToRefreshStatus {...pull} />}
    footer={<>
      {loading ? <p className="context-status" role="status">{text.loading}</p> : null}
      {error ? <p className="context-status">{text.title} couldn't be fully loaded. <button type="button" className="btn-link" onClick={reload}>Try Again</button></p> : null}
      {!loading && !error && agenda.length === 0 ? <p className="context-status">
        {!selected.length
          ? `Select a calendar to see ${day === "chosen" ? "this day's" : "today's"} agenda.`
          : visibleCalendars.length === 0
            ? <>All calendars are hidden. <button type="button" className="btn-link" onClick={filter.showAll}>Show All</button></>
            : hiddenSelectedCount > 0
              ? <>{text.empty} <button type="button" className="btn-link" onClick={filter.showAll}>Show All</button></>
              : text.empty}
      </p> : null}
    </>}
  />;
}

function AgendaCalendarFilter({ calendars, hidden, colors, onToggle, onShowAll }: {
  calendars: readonly CalendarOption[];
  hidden: ReadonlySet<string>;
  colors: CalendarColorMap;
  onToggle(calendar: CalendarOption): void;
  onShowAll(): void;
}) {
  const [open, setOpen] = useState(false);
  const anchorRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const shownCount = calendars.filter((calendar) => !hidden.has(agendaCalendarOptionKey(calendar))).length;
  const filtered = shownCount < calendars.length;

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (anchorRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [open]);

  const close = useCallback(() => {
    setOpen(false);
    triggerRef.current?.focus();
  }, []);

  const label = filtered ? `Filter calendars, showing ${shownCount} of ${calendars.length}` : "Filter calendars";
  return (
    <div className="agenda-filter-anchor" ref={anchorRef}>
      <button
        ref={triggerRef}
        type="button"
        className={`btn-icon btn-icon-sm agenda-filter-trigger${filtered ? " active" : ""}`}
        aria-label={label}
        title={filtered ? `Filter calendars (showing ${shownCount} of ${calendars.length})` : "Filter calendars"}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <ListFilter size={ICON_SIZE.sm} aria-hidden="true" />
      </button>
      {open ? <AgendaCalendarFilterMenu calendars={calendars} hidden={hidden} colors={colors} filtered={filtered} onToggle={onToggle} onShowAll={onShowAll} onClose={close} /> : null}
    </div>
  );
}

function AgendaCalendarFilterMenu({ calendars, hidden, colors, filtered, onToggle, onShowAll, onClose }: {
  calendars: readonly CalendarOption[];
  hidden: ReadonlySet<string>;
  colors: CalendarColorMap;
  filtered: boolean;
  onToggle(calendar: CalendarOption): void;
  onShowAll(): void;
  onClose(): void;
}) {
  useEscapeDismiss(onClose);
  const multipleAccounts = new Set(calendars.map((calendar) => calendar.accountId)).size > 1;
  return (
    <div className="filters-menu agenda-filter-menu" role="menu" aria-label="Filter calendars">
      <div className="agenda-filter-menu-header">
        <span className="filters-menu-title">Calendars</span>
        {filtered ? <button type="button" className="btn-link" onClick={onShowAll}>Show All</button> : null}
      </div>
      {calendars.map((calendar) => {
        const key = agendaCalendarOptionKey(calendar);
        const checked = !hidden.has(key);
        const showAccount = multipleAccounts && calendar.name.toLocaleLowerCase() !== calendar.accountId.toLocaleLowerCase();
        return (
          <button
            key={key}
            type="button"
            role="menuitemcheckbox"
            aria-checked={checked}
            className={`filters-menu-item agenda-filter-item${checked ? " active" : ""}`}
            onClick={() => onToggle(calendar)}
          >
            <span className="filters-menu-item-label agenda-filter-item-label">
              <span className="filters-menu-item-check" aria-hidden="true">{checked ? <Check size={ICON_SIZE.xs} /> : null}</span>
              <span className="agenda-filter-swatch" style={calendarColorStyle(colors, calendar.accountId, calendar.id)} aria-hidden="true" />
              <span className="agenda-filter-text">
                <span className="agenda-filter-name">{calendar.name}</span>
                {showAccount ? <small className="agenda-filter-account">{calendar.accountId}</small> : null}
              </span>
            </span>
          </button>
        );
      })}
    </div>
  );
}

/** Prepare for one agenda event: its notes, earlier meetings, mail, and commitments. */
export function PrepareButton({ event, onPrepare, pressed }: { event: ScheduleEvent; onPrepare(event: ScheduleEvent): void; pressed?: boolean }) {
  const reference = meetingEventReference(event);
  return <button type="button" className="btn-icon btn-icon-sm" data-prepare-trigger={reference ? `agenda:${noteReferenceHref(reference)}` : undefined} aria-label={`Prepare for ${event.title}`} title="Prepare" aria-pressed={pressed} onClick={() => onPrepare(event)}><ClipboardList size={ICON_SIZE.sm} aria-hidden="true" /></button>;
}

/** Add Note for one agenda event. */
/** Opens the task a booked block is for, beside the row that opens its event. */
export function OpenTaskButton({ task, onOpenTask }: { task: ThreadTask; onOpenTask(task: ThreadTask): void }) {
  return <button type="button" className="btn-icon btn-icon-sm" aria-label={`Open task ${task.title}`} title="Open task" onClick={() => onOpenTask(task)}><ListChecks size={ICON_SIZE.sm} aria-hidden="true" /></button>;
}

export function AddNoteButton({ event, onAddNote }: { event: ScheduleEvent; onAddNote(event: ScheduleEvent): void }) {
  return <button type="button" className="btn-icon btn-icon-sm" aria-label={`Add Note for ${event.title}`} title="Add Note" onClick={() => onAddNote(event)}><NotebookPen size={ICON_SIZE.sm} aria-hidden="true" /></button>;
}

function NowDivider({ now }: { now: Date }) {
  const time = formatClockTime(now);
  return <div className="agenda-now" role="separator" aria-label={`Now, ${time}`}><span>Now · {time}</span></div>;
}
