import { CalendarDays, CalendarSearch } from "lucide-react";
import { useMemo } from "react";
import { HoverTooltip } from "./AppChrome";
import { calendarEventRange } from "./calendarInvitations";
import { responseLabel } from "./calendarResponse";
import { addDays, eventDate, formatEventTime, occursOnDay } from "./calendarTime";
import { ContextRow, ContextSection } from "./ContextSections";
import type { CalendarEventPreview, ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { useCalendarSchedule } from "./useCalendarSchedule";

const sameUid = (left: string | null | undefined, right: string | null | undefined) =>
  Boolean(left?.trim()) && left?.trim().toLocaleLowerCase() === right?.trim().toLocaleLowerCase();

const overlaps = (event: ScheduleEvent, range: { start: Date; end: Date }) =>
  !event.allDay && new Date(event.start) < range.end && new Date(event.end) > range.start;

const isWorkingLocation = (event: ScheduleEvent) => event.eventType === "workingLocation";

const timeFormat = () => new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });

/**
 * What else is on the user's calendar the day an invitation is for, so they
 * can answer it without paging the calendar to that date. A timed invitation
 * says whether its time is free and marks the events it overlaps.
 *
 * The invitation's own event is left out, since calendars often add it as
 * tentative before the user answers, and so are events the user declined.
 * Working locations, such as Office or Home, are named on the date line.
 * Renders nothing until the day's schedule loads, or if it cannot load, as
 * the rest of the panel's calendar context does; when only some calendars
 * load, it lists what did without calling the time free.
 */
export function InvitationDay({ invitation, day, timeZone, onShowDay, onOpenEvent }: {
  invitation: CalendarEventPreview;
  /** The local day the invitation starts on. */
  day: Date;
  timeZone: string;
  /** Shows the whole day in the schedule. */
  onShowDay(day: Date): void;
  onOpenEvent(event: ScheduleEvent): void;
}) {
  const range = useMemo(() => ({ timeMin: day.toISOString(), timeMax: addDays(day, 1).toISOString() }), [day]);
  const { events, loading, error } = useCalendarSchedule({ ...range, timeZone });
  const invited = invitation.allDay ? null : calendarEventRange(invitation);
  const onDay = events.filter((event) => occursOnDay(event, day));
  // A working location says where the user is, not something they are doing,
  // so it joins the date line, once per place, rather than listing as an event.
  const places = onDay.filter(isWorkingLocation).map((event) => event.title.trim())
    .filter((place, index, all) => all.findIndex((other) => other.toLocaleLowerCase() === place.toLocaleLowerCase()) === index);
  const others = onDay
    .filter((event) => !isWorkingLocation(event) && event.responseStatus !== "declined" && !sameUid(event.uid, invitation.uid))
    .sort((left, right) => Number(right.allDay) - Number(left.allDay) || eventDate(left).getTime() - eventDate(right).getTime());
  if (others.length === 0 && (loading || error)) return null;

  const conflicts = invited ? others.filter((event) => overlaps(event, invited)).length : 0;
  // With a calendar missing, the day may hold more than it shows, so it is never called free.
  const summary = conflicts > 0 ? `Overlaps ${conflicts === 1 ? "1 event" : `${conflicts} events`}`
    : error ? null
    : invited ? `Free ${timeFormat().formatRange(invited.start, invited.end)}`
    : others.length === 0 ? "Nothing else on your calendar" : null;
  const date = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).format(day);

  return <ContextSection
    id="invitation-day"
    className="context-meetings context-invitation-day"
    title="That day"
    label="Your calendar that day"
    count={others.length}
    note={<>{date}{places.length > 0 ? <> · <span className="context-invitation-day-place">{places.join(", ")}</span></> : null}{summary ? <> · <span className={conflicts > 0 ? "context-invitation-day-conflict" : undefined}>{summary}</span></> : null}</>}
    actions={<HoverTooltip title="Show day in schedule" shortcut="t" placement="bottom">
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Show Day in Schedule" onClick={() => onShowDay(day)}><CalendarSearch size={ICON_SIZE.sm} /></button>
    </HoverTooltip>}
    rows={others.map((event) => {
      const conflict = invited ? overlaps(event, invited) : false;
      const response = responseLabel(event);
      return <ContextRow
        key={`${event.accountId}:${event.id}`}
        className={`context-meeting${conflict ? " context-meeting-conflict" : ""}`}
        icon={<CalendarDays size={ICON_SIZE.sm} />}
        title={event.title}
        wrapTitle
        date={event.allDay ? "All day" : timeFormat().format(new Date(event.start))}
        detail={<>
          <span className="context-meeting-details">{formatEventTime(event)}{conflict ? " · Overlaps" : ""}</span>
          {response ? <span className="context-meeting-response" data-response-status={event.responseStatus ?? undefined}> · {response}</span> : null}
        </>}
        onActivate={() => onOpenEvent(event)}
      />;
    })}
  />;
}
