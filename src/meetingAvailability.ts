import type { AttendeeAvailability, MeetingAvailabilityResult } from "./domain";
import { previewScheduleChoices } from "./scheduleTimes";

export const SCHEDULING_STEP_MINUTES = 15;

/** Calendar days in the displayed zone, independent of the device's zone. */
export function schedulingDay(at: number, timeZone: string): string {
  const parts = new Intl.DateTimeFormat("en-US", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).formatToParts(at);
  const part = (name: string) => parts.find((value) => value.type === name)!.value;
  return `${part("year")}-${part("month")}-${part("day")}`;
}

export function shiftSchedulingDay(day: string, count: number): string {
  const at = new Date(`${day}T12:00:00Z`);
  at.setUTCDate(at.getUTCDate() + count);
  return at.toISOString().slice(0, 10);
}

export function schedulingDayStart(day: string, timeZone: string): number {
  // Some zones advance their clocks at midnight. The day starts at the
  // first real wall time, rather than crashing or selecting the previous day.
  for (let minute = 0; minute <= 180; minute += 1) {
    const clock = `${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
    try { return previewScheduleChoices(`${day}T${clock}`, timeZone)[0].scheduledAt; }
    catch { /* Try the next wall minute across a midnight clock change. */ }
  }
  throw new Error(`Cannot resolve the start of ${day} in ${timeZone}`);
}

export function schedulingWeek(day: string): string {
  const weekday = new Date(`${day}T12:00:00Z`).getUTCDay();
  return shiftSchedulingDay(day, -((weekday + 6) % 7));
}

export function schedulingConflicts(result: MeetingAvailabilityResult, start: number, end: number): AttendeeAvailability[] {
  return [result.own, ...result.guests].filter((person) => person.busy.some((interval) => Date.parse(interval.start) < end && Date.parse(interval.end) > start));
}

export function schedulingCoverage(result: MeetingAvailabilityResult): string {
  const unknown = [result.own, ...result.guests].filter((person) => !person.checked);
  return unknown.length ? `Availability unavailable for ${unknown.map((person) => person === result.own ? "some of your calendars" : person.email).join(", ")}. Suggested times only avoid known conflicts.`
    : "All requested calendars checked. Suggestions are free on those calendars.";
}
