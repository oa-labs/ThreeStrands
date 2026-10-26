import { formatClockTime, isDeclined } from "./agendaEvents";
import { eventDate } from "./calendarTime";
import type { ScheduleEvent } from "./domain";

/** Shortest open stretch before the next meeting that Today calls free time. */
export const FREE_STRETCH_MINUTES = 60;

/** A meeting with a set time the user hasn't declined. */
function isAttending(event: ScheduleEvent): boolean {
  return !event.allDay && !isDeclined(event);
}

function overlaps(left: ScheduleEvent, right: ScheduleEvent): boolean {
  return eventDate(left).getTime() < new Date(right.end).getTime()
    && eventDate(right).getTime() < new Date(left.end).getTime();
}

/**
 * Each meeting that overlaps another, with the meetings it overlaps. All-day
 * and declined events never conflict. Events are expected one per meeting, as
 * `agendaEventsFor` lists them, so a copy on a second calendar is not a clash.
 */
export function agendaConflicts(events: readonly ScheduleEvent[]): Map<ScheduleEvent, ScheduleEvent[]> {
  const attending = events.filter(isAttending);
  const conflicts = new Map<ScheduleEvent, ScheduleEvent[]>();
  attending.forEach((event, index) => {
    for (const other of attending.slice(index + 1)) {
      if (!overlaps(event, other)) continue;
      conflicts.set(event, [...conflicts.get(event) ?? [], other]);
      conflicts.set(other, [...conflicts.get(other) ?? [], event]);
    }
  });
  return conflicts;
}

/**
 * The event's own text says it is recorded, so missing it live costs less:
 * "this session will be recorded", "the recording will be shared". Negations
 * such as "will not be recorded" don't match.
 */
export function mentionsRecording(event: ScheduleEvent): boolean {
  const text = `${event.title}\n${event.description ?? ""}`;
  return /\b(?:will be|is being|is|are|gets?|being) recorded\b|\brecording (?:will be|is) (?:shared|available|posted|sent)\b/i.test(text);
}

/**
 * The rest of the day in one line, such as "Free until 2:00 pm · 4:00 pm
 * double-booked · Last meeting ends 5:00 pm". Empty when no meeting is left.
 */
export function dayShape(events: readonly ScheduleEvent[], now: Date): string {
  const remaining = events.filter((event) => isAttending(event) && new Date(event.end).getTime() > now.getTime())
    .sort((left, right) => eventDate(left).getTime() - eventDate(right).getTime());
  if (remaining.length === 0) return events.some(isAttending) ? "No more meetings today" : "";
  const parts: string[] = [];
  const firstStart = eventDate(remaining[0]);
  if (firstStart.getTime() - now.getTime() >= FREE_STRETCH_MINUTES * 60_000) parts.push(`Free until ${formatClockTime(firstStart)}`);

  // A clash is named by when it begins: the later of the two starts.
  const clashes = new Map<number, ScheduleEvent[]>();
  for (const [event, others] of agendaConflicts(remaining)) {
    for (const other of others) {
      const start = Math.max(eventDate(event).getTime(), eventDate(other).getTime());
      const recorded = clashes.get(start) ?? [];
      if (!recorded.includes(event)) recorded.push(event);
      clashes.set(start, recorded);
    }
  }
  for (const [start, clashing] of [...clashes].sort(([left], [right]) => left - right)) {
    const recorded = clashing.filter(mentionsRecording);
    parts.push(`${formatClockTime(new Date(start))} double-booked${recorded.length ? ` (${recorded.map((event) => event.title).join(", ")} ${recorded.length === 1 ? "is" : "are"} recorded)` : ""}`);
  }

  const lastEnd = Math.max(...remaining.map((event) => new Date(event.end).getTime()));
  parts.push(`Last meeting ends ${formatClockTime(new Date(lastEnd))}`);
  return parts.join(" · ");
}

/** The meetings still ahead today, or under way, that `address` is invited to. */
export function meetingsWith(address: string, events: readonly ScheduleEvent[], now: Date): ScheduleEvent[] {
  const email = address.toLocaleLowerCase();
  return events.filter((event) => isAttending(event) && new Date(event.end).getTime() > now.getTime()
    && (event.attendees ?? []).includes(email))
    .sort((left, right) => eventDate(left).getTime() - eventDate(right).getTime());
}

/** "Also in your 2:00 pm meeting", "Also in your 2:00 pm and 3:00 pm meetings". */
export function sharedMeetingsNote(meetings: readonly ScheduleEvent[]): string {
  if (meetings.length === 0) return "";
  const times = new Intl.ListFormat(undefined, { type: "conjunction" }).format(meetings.map((event) => formatClockTime(eventDate(event))));
  return `Also in your ${times} meeting${meetings.length === 1 ? "" : "s"}`;
}
