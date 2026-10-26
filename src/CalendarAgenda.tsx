import { CalendarDays } from "lucide-react";
import type { ReactNode } from "react";
import { ContextRow, ContextSection } from "./ContextSections";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { addDays, formatEventTime, startOfLocalDay } from "./calendarTime";
import { agendaEventsFor, formatClockTime, isDeclined, isPastEvent } from "./agendaEvents";
import { responseLabel } from "./calendarResponse";
import { agendaConflicts, mentionsRecording } from "./todayInsights";
import { useCalendarSchedule } from "./useCalendarSchedule";
import type { CalendarOption, ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";

type Schedule = ReturnType<typeof useCalendarSchedule>;
type AgendaProps = {
  /** The day the agenda lists. */
  today: Date;
  calendars: CalendarOption[];
  onOpenEvent(event: ScheduleEvent): void;
  /**
   * The current time, for an agenda of the current day: finished events are
   * dimmed and a Now divider sits before the rest.
   */
  now?: Date;
  /** Lists the day as Tomorrow, showing its first few events before "Show more". */
  tomorrow?: boolean;
  /** Names the day under the heading; left out where the page already says which day it is. */
  showDate?: boolean;
  /** Says which meetings overlap another, and which of those are recorded. */
  markConflicts?: boolean;
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

function Agenda({ today, calendars, onOpenEvent, now, tomorrow = false, showDate = true, markConflicts = false, schedule: { events, loading, error, reload } }: AgendaProps & { schedule: Schedule }) {
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
    const detail = [formatEventTime(event), ...conflict, event.location, calendar?.name, account, responseLabel(event)].filter(Boolean).join(" · ");
    rows.push(<ContextRow
      key={`${event.accountId}:${event.calendarId}:${event.id}`}
      className={[now && isPastEvent(event, now) ? "agenda-event-past" : "", isDeclined(event) ? "agenda-event-declined" : "", overlapping.length ? "agenda-event-conflict" : ""].filter(Boolean).join(" ") || undefined}
      icon={<CalendarDays size={ICON_SIZE.sm} style={{ ...calendarColorStyle(colors, event.accountId, event.calendarId), color: "var(--calendar-color, var(--accent))" }} />}
      title={event.title}
      detail={detail}
      onActivate={() => onOpenEvent(event)}
    />);
  });
  if (now && pastCount > 0 && pastCount === agenda.length) rows.push(<NowDivider key="now" now={now} />);
  const name = tomorrow ? "Tomorrow" : "Today's agenda";
  return <ContextSection
    id={tomorrow ? "calendar-agenda-tomorrow" : "calendar-agenda"}
    title={name}
    note={showDate ? today.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" }) : undefined}
    count={agenda.length}
    rows={rows}
    limit={tomorrow ? TOMORROW_AGENDA_ROWS : rows.length}
    footer={<>
      {loading ? <p className="context-status" role="status">Loading {tomorrow ? "tomorrow's" : "today's"} agenda…</p> : null}
      {error ? <p className="context-status">{name} couldn't be fully loaded. <button type="button" className="btn-link" onClick={reload}>Retry agenda</button></p> : null}
      {!loading && !error && agenda.length === 0 ? <p className="context-status">{selected.length ? `No events ${tomorrow ? "tomorrow" : "today"}.` : "Select a calendar to see today's agenda."}</p> : null}
    </>}
  />;
}

function NowDivider({ now }: { now: Date }) {
  const time = formatClockTime(now);
  return <div className="agenda-now" role="separator" aria-label={`Now, ${time}`}><span>Now · {time}</span></div>;
}
