import {  isDeclined } from "./agendaEvents";
import { eventDate } from "./calendarTime";
import type { AvailabilityPreferences, ScheduleEvent } from "./domain";
import { schedulingDay, shiftSchedulingDay } from "./meetingAvailability";
import { previewScheduleChoices } from "./scheduleTimes";
import { TASK_BLOCK_MINUTES } from "./taskBooking";

/** Shortest open stretch before the next meeting that Today calls free time. */
export const FREE_STRETCH_MINUTES = 60;

/** A meeting with a set time the user hasn't declined; a working location for part of a day is where they are, not a meeting. */
function isAttending(event: ScheduleEvent): boolean {
  return !event.allDay && !isDeclined(event) && event.eventType !== "workingLocation";
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

/** The meetings still ahead today, or under way, that `address` is invited to. */
export function meetingsWith(address: string, events: readonly ScheduleEvent[], now: Date): ScheduleEvent[] {
  const email = address.toLocaleLowerCase();
  return events.filter((event) => isAttending(event) && new Date(event.end).getTime() > now.getTime()
    && (event.attendees ?? []).includes(email))
    .sort((left, right) => eventDate(left).getTime() - eventDate(right).getTime());
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
