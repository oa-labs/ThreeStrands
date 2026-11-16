import { CalendarDays, ClipboardList, NotebookPen } from "lucide-react";
import { useCallback, useRef, type ReactNode } from "react";
import { ContextRow, ContextSection } from "./ContextSections";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { addDays, formatEventTime, startOfLocalDay } from "./calendarTime";
import { agendaEventsFor, formatClockTime, isDeclined, isPastEvent } from "./agendaEvents";
import { responseLabel } from "./calendarResponse";
import { agendaConflicts, mentionsRecording } from "./todayInsights";
import { useCalendarSchedule } from "./useCalendarSchedule";
import type { CalendarOption, ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { isPreparable, preparationKey } from "./meetingPrep";

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
  /** Offers Prepare on each event that a note can link. */
  onPrepare?(event: ScheduleEvent): void;
  /** The meeting being prepared (`preparationKey`), shown selected. */
  selectedKey?: string | null;
  /** A short line about the event after its time, such as its readiness. */
  describe?(event: ScheduleEvent): string | null | undefined;
};

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

function Agenda({ pullToRefresh = true, today, calendars, onOpenEvent, onAddNote, onPrepare, selectedKey = null, describe, now, day = "today", id, showDate = true, markConflicts = false, schedule: { events, loading, error, reload, refresh: refreshSchedule } }: AgendaProps & { schedule: Schedule }) {
  const agendaRef = useRef<HTMLElement>(null);
  const refresh = useCallback(async () => { if (refreshSchedule) await refreshSchedule(); else reload(); }, [refreshSchedule, reload]);
  const pull = usePullToRefresh(agendaRef, pullToRefresh ? refresh : undefined);
  const colors = useCalendarColors();
  const selected = calendars.filter((calendar) => calendar.selected);
  const agenda = agendaEventsFor(events, today, calendars);
  const multipleAccounts = new Set(selected.map((calendar) => calendar.accountId)).size > 1;
  const conflicts = markConflicts ? agendaConflicts(agenda) : new Map<ScheduleEvent, ScheduleEvent[]>();
  const pastCount = now ? agenda.filter((event) => isPastEvent(event, now)).length : 0;
  // All-day events list first and are never past during their day, so the
  // divider goes before the first timed event still to finish.
  const upcomingIndex = now ? agenda.findIndex((event) => !event.allDay && !isPastEvent(event, now)) : -1;
  const rows: ReactNode[] = [];
  agenda.forEach((event, index) => {
    if (now && pastCount > 0 && index === upcomingIndex) rows.push(<NowDivider key="now" now={now} />);
    const calendar = selected.find((calendar) => calendar.accountId === event.accountId && calendar.id === event.calendarId);
    // A calendar named after its account already says which account it is.
    const account = multipleAccounts && calendar?.name.toLocaleLowerCase() !== event.accountId.toLocaleLowerCase() ? event.accountId : null;
    const overlapping = conflicts.get(event) ?? [];
    const conflict = overlapping.length
      ? [`Overlaps ${overlapping.map((other) => other.title).join(", ")}`, mentionsRecording(event) ? "Recorded" : null]
      : [];
    const detail = [formatEventTime(event), describe?.(event), ...conflict, event.location, calendar?.name, account, responseLabel(event)].filter(Boolean).join(" · ");
    const key = preparationKey(event);
    const isSelected = selectedKey !== null && key === selectedKey;
    rows.push(<ContextRow
      rowKey={key ? `meeting:${key}` : undefined}
      pressed={selectedKey !== null ? isSelected : undefined}
      key={`${event.accountId}:${event.calendarId}:${event.id}`}
      className={[now && isPastEvent(event, now) ? "agenda-event-past" : "", isDeclined(event) ? "agenda-event-declined" : "", overlapping.length ? "agenda-event-conflict" : "", isSelected ? "agenda-event-selected" : ""].filter(Boolean).join(" ") || undefined}
      icon={<CalendarDays size={ICON_SIZE.sm} style={{ ...calendarColorStyle(colors, event.accountId, event.calendarId), color: "var(--calendar-color, var(--accent))" }} />}
      title={event.title}
      detail={detail}
      onActivate={() => onOpenEvent(event)}
      actions={isPreparable(event) && (onPrepare || onAddNote) ? [
        ...(onPrepare ? [<PrepareButton key="prepare" event={event} onPrepare={onPrepare} pressed={selectedKey !== null ? isSelected : undefined} />] : []),
        ...(onAddNote ? [<AddNoteButton key="note" event={event} onAddNote={onAddNote} />] : []),
      ] : undefined}
    />);
  });
  if (now && pastCount > 0 && pastCount === agenda.length) rows.push(<NowDivider key="now" now={now} />);
  const text = AGENDA_TEXT[day];
  return <ContextSection
    sectionRef={agendaRef}
    busy={pull.refreshing}
    id={id ?? text.id}
    title={text.title}
    note={showDate ? today.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" }) : undefined}
    count={agenda.length}
    rows={rows}
    limit={day === "tomorrow" ? TOMORROW_AGENDA_ROWS : rows.length}
    status={<PullToRefreshStatus {...pull} />}
    footer={<>
      {loading ? <p className="context-status" role="status">{text.loading}</p> : null}
      {error ? <p className="context-status">{text.title} couldn't be fully loaded. <button type="button" className="btn-link" onClick={reload}>Try Again</button></p> : null}
      {!loading && !error && agenda.length === 0 ? <p className="context-status">{selected.length ? text.empty : `Select a calendar to see ${day === "chosen" ? "this day's" : "today's"} agenda.`}</p> : null}
    </>}
  />;
}

/** Prepare for one agenda event: its notes, earlier meetings, mail, and commitments. */
export function PrepareButton({ event, onPrepare, pressed }: { event: ScheduleEvent; onPrepare(event: ScheduleEvent): void; pressed?: boolean }) {
  return <button type="button" className="btn-icon btn-icon-sm" aria-label={`Prepare for ${event.title}`} title="Prepare" aria-pressed={pressed} onClick={() => onPrepare(event)}><ClipboardList size={ICON_SIZE.sm} aria-hidden="true" /></button>;
}

/** Add Note for one agenda event. */
export function AddNoteButton({ event, onAddNote }: { event: ScheduleEvent; onAddNote(event: ScheduleEvent): void }) {
  return <button type="button" className="btn-icon btn-icon-sm" aria-label={`Add Note for ${event.title}`} title="Add Note" onClick={() => onAddNote(event)}><NotebookPen size={ICON_SIZE.sm} aria-hidden="true" /></button>;
}

function NowDivider({ now }: { now: Date }) {
  const time = formatClockTime(now);
  return <div className="agenda-now" role="separator" aria-label={`Now, ${time}`}><span>Now · {time}</span></div>;
}
