import { formatClockTime, isDeclined } from "./agendaEvents";
import { eventDate } from "./calendarTime";
import type { AvailabilityPreferences, ScheduleEvent } from "./domain";
import { schedulingDay, shiftSchedulingDay } from "./meetingAvailability";
import { previewScheduleChoices } from "./scheduleTimes";
import { TASK_BLOCK_MINUTES } from "./taskBooking";

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
 * The rest of the day in one line, such as "Free until 2:00 pm · Double-booked
 * at 4:00 pm · Last meeting ends 5:00 pm". Two or more clashes are counted
 * instead, since the agenda marks each one. Empty for a day without meetings.
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
  if (clashes.size > 0) {
    const recorded = [...new Set([...clashes.values()].flat())].filter(mentionsRecording);
    const note = recorded.length ? ` (${recorded.map((event) => event.title).join(", ")} ${recorded.length === 1 ? "is" : "are"} recorded)` : "";
    parts.push(clashes.size === 1 ? `Double-booked at ${formatClockTime(new Date([...clashes.keys()][0]))}${note}` : `${clashes.size} double-bookings${note}`);
  }

  const lastEnd = Math.max(...remaining.map((event) => new Date(event.end).getTime()));
  parts.push(`Last meeting ends ${formatClockTime(new Date(lastEnd))}`);
  return parts.join(" · ");
}

/**
 * Today's header line about meetings, true at any time of day: "No meetings
 * today", "2 meetings · next at 2:00 pm", "2 meetings · in Standup until
 * 10:30 am", or "2 meetings, both finished · last ended 3:30 pm", followed
 * by any double-booking still ahead and, with more than one meeting left,
 * when the last ends. Declined and all-day events aren't meetings here.
 */
export function daySummary(events: readonly ScheduleEvent[], now: Date): string {
  const meetings = events.filter(isAttending).sort((left, right) => eventDate(left).getTime() - eventDate(right).getTime());
  if (meetings.length === 0) return "No meetings today";
  const total = `${meetings.length} meeting${meetings.length === 1 ? "" : "s"}`;
  const remaining = meetings.filter((event) => new Date(event.end).getTime() > now.getTime());
  if (remaining.length === 0) {
    const lastEnd = new Date(Math.max(...meetings.map((event) => new Date(event.end).getTime())));
    const finished = meetings.length === 1 ? "finished" : meetings.length === 2 ? "both finished" : "all finished";
    return `${total}, ${finished} · ${meetings.length === 1 ? "ended" : "last ended"} ${formatClockTime(lastEnd)}`;
  }
  const current = remaining.find((event) => eventDate(event).getTime() <= now.getTime());
  const parts = [total, current ? `in ${current.title} until ${formatClockTime(new Date(current.end))}` : `next at ${formatClockTime(eventDate(remaining[0]))}`];
  // The rest of dayShape: double-bookings, and when the last meeting ends once there's more than one left.
  for (const part of dayShape(events, now).split(" · ")) {
    if (!part || part.startsWith("Free until")) continue;
    if (part.startsWith("Last meeting ends") && remaining.length < 2) continue;
    parts.push(part.charAt(0).toLocaleLowerCase() + part.slice(1));
  }
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

/** The default availability profile's hours: its zone and weekly windows. */
export type WorkingHours = Pick<AvailabilityPreferences, "timeZone" | "workingWindows">;
/** A free stretch of working time. `holdsBlock` when a task block fits; `named` when it is long enough to call free time. */
export type FreeStretch = { start: Date; end: Date; minutes: number; holdsBlock: boolean; named: boolean };
export type PlanCapacity = {
  /** The plan's date in the working hours' zone. */
  day: string;
  /** Working minutes on that day. */
  workingMinutes: number;
  /** Free working minutes left from now, or from the start of working hours. */
  freeMinutes: number;
  stretches: FreeStretch[];
  /** Booked task blocks that fall in working hours. */
  bookedBlocks: number;
};

/** The instant of a wall-clock time in `timeZone`; a time skipped by a clock change moves to the next real minute. */
function wallInstant(day: string, clock: string, timeZone: string): number | null {
  if (clock === "24:00") return wallInstant(shiftSchedulingDay(day, 1), "00:00", timeZone);
  const [hours, minutes] = clock.split(":").map(Number);
  for (let minute = hours * 60 + minutes; minute <= hours * 60 + minutes + 180 && minute < 24 * 60; minute += 1) {
    const text = `${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
    try { return previewScheduleChoices(`${day}T${text}`, timeZone)[0].scheduledAt; }
    catch { /* Skipped by a clock change; try the next minute. */ }
  }
  return null;
}

/**
 * Free working time on the plan's day: the working windows of `day` less
 * attending, timed events (booked task blocks included), from `now` or the
 * start of working hours to their end. Facts only: no estimates, no claim
 * that a plan fits. `null` when the schedule or the working hours are
 * unknown, so unknown is never shown as free.
 */
export function planCapacity(events: readonly ScheduleEvent[] | null, workingHours: WorkingHours | null, now: Date, day?: string): PlanCapacity | null {
  if (!events || !workingHours) return null;
  const zone = workingHours.timeZone;
  const date = day ?? schedulingDay(now.getTime(), zone);
  const weekday = new Date(`${date}T12:00:00Z`).getUTCDay();
  // A weekday's windows, merged where they overlap or touch.
  const windows: [number, number][] = [];
  for (const window of workingHours.workingWindows.filter((candidate) => candidate.weekday === weekday)) {
    const start = wallInstant(date, window.start, zone), end = wallInstant(date, window.end, zone);
    if (start !== null && end !== null && end > start) windows.push([start, end]);
  }
  windows.sort((a, b) => a[0] - b[0]);
  const merged: [number, number][] = [];
  for (const [start, end] of windows) {
    const last = merged.at(-1);
    if (last && start <= last[1]) last[1] = Math.max(last[1], end);
    else merged.push([start, end]);
  }
  const busy = events.filter((event) => isAttending(event) && event.eventType !== "workingLocation")
    .map((event): [number, number] => [eventDate(event).getTime(), new Date(event.end).getTime()])
    .filter(([start, end]) => Number.isFinite(start) && Number.isFinite(end) && end > start)
    .sort((a, b) => a[0] - b[0]);
  const stretches: FreeStretch[] = [];
  for (const [windowStart, windowEnd] of merged) {
    let cursor = Math.max(windowStart, now.getTime());
    const close = (until: number) => {
      if (until > cursor) {
        const minutes = Math.floor((until - cursor) / 60_000);
        if (minutes > 0) stretches.push({ start: new Date(cursor), end: new Date(until), minutes, holdsBlock: minutes >= TASK_BLOCK_MINUTES, named: minutes >= FREE_STRETCH_MINUTES });
      }
    };
    for (const [start, end] of busy) {
      if (end <= cursor || start >= windowEnd) continue;
      close(Math.min(start, windowEnd));
      cursor = Math.max(cursor, end);
      if (cursor >= windowEnd) break;
    }
    close(windowEnd);
  }
  const inHours = (event: ScheduleEvent) => merged.some(([start, end]) => eventDate(event).getTime() < end && new Date(event.end).getTime() > start);
  return {
    day: date,
    workingMinutes: merged.reduce((total, [start, end]) => total + Math.floor((end - start) / 60_000), 0),
    freeMinutes: stretches.reduce((total, stretch) => total + stretch.minutes, 0),
    stretches,
    bookedBlocks: events.filter((event) => !!event.sourceTaskId && isAttending(event) && inHours(event)).length,
  };
}
