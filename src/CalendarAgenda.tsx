import { CalendarDays } from "lucide-react";
import { ContextRow, ContextSection } from "./ContextSections";
import { calendarColorStyle, useCalendarColors } from "./calendarColors";
import { addDays, eventDate, formatEventTime, occursOnDay, startOfLocalDay } from "./calendarTime";
import { responseLabel } from "./calendarResponse";
import { useCalendarSchedule } from "./useCalendarSchedule";
import type { CalendarOption, ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";

type Schedule = ReturnType<typeof useCalendarSchedule>;
type AgendaProps = {
  today: Date;
  calendars: CalendarOption[];
  onOpenEvent(event: ScheduleEvent): void;
};

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

function Agenda({ today, calendars, onOpenEvent, schedule: { events, loading, error, reload } }: AgendaProps & { schedule: Schedule }) {
  const colors = useCalendarColors();
  const selected = calendars.filter((calendar) => calendar.selected);
  const agenda = events.filter((event) => occursOnDay(event, today) && selected.some((calendar) =>
    calendar.accountId === event.accountId && (!event.calendarId || calendar.id === event.calendarId)))
    .sort((left, right) => Number(right.allDay) - Number(left.allDay)
      || eventDate(left).getTime() - eventDate(right).getTime() || left.title.localeCompare(right.title));
  const multipleAccounts = new Set(selected.map((calendar) => calendar.accountId)).size > 1;
  const rows = agenda.map((event) => {
    const calendar = selected.find((calendar) => calendar.accountId === event.accountId && calendar.id === event.calendarId);
    const detail = [formatEventTime(event), event.location, calendar?.name, multipleAccounts ? event.accountId : null, responseLabel(event)].filter(Boolean).join(" · ");
    return <ContextRow
      key={`${event.accountId}:${event.calendarId}:${event.id}`}
      icon={<CalendarDays size={ICON_SIZE.sm} style={{ ...calendarColorStyle(colors, event.accountId, event.calendarId), color: "var(--calendar-color, var(--accent))" }} />}
      title={event.title}
      detail={detail}
      onActivate={() => onOpenEvent(event)}
    />;
  });
  return <ContextSection
    id="calendar-agenda"
    title="Today's agenda"
    note={today.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" })}
    count={agenda.length}
    rows={rows}
    limit={rows.length}
    footer={<>
      {loading ? <p className="context-status" role="status">Loading today's agenda…</p> : null}
      {error ? <p className="context-status">Today's agenda couldn't be fully loaded. <button type="button" className="btn-link" onClick={reload}>Retry agenda</button></p> : null}
      {!loading && !error && agenda.length === 0 ? <p className="context-status">{selected.length ? "No events today." : "Select a calendar to see today's agenda."}</p> : null}
    </>}
  />;
}
