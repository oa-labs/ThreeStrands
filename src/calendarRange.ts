import { addDays, eventDate, startOfLocalDay, startOfWeek } from "./calendarTime";
import type { ScheduleEvent } from "./domain";

export type CalendarRangeMode = "day" | "threeDays" | "workweek" | "week";
export const CALENDAR_RANGE_KEY = "threestrands.calendar.range";
export const CALENDAR_RANGES: { value: CalendarRangeMode; label: string }[] = [
  { value: "day", label: "Day" }, { value: "threeDays", label: "3-day" }, { value: "workweek", label: "Workweek" }, { value: "week", label: "Week" },
];
export function readCalendarRange(): CalendarRangeMode {
  try { const value = localStorage.getItem(CALENDAR_RANGE_KEY); return CALENDAR_RANGES.find((range) => range.value===value)?.value ?? "workweek"; }
  catch { return "workweek"; }
}
export function calendarRange(anchor: Date, mode: CalendarRangeMode) {
  const day = startOfLocalDay(anchor);
  const start = mode === "week" ? startOfWeek(day) : mode === "workweek" ? addDays(day, day.getDay()===0 ? 1 : day.getDay()===6 ? 2 : 1-day.getDay()) : day;
  const count = mode === "week" ? 7 : mode === "workweek" ? 5 : mode === "threeDays" ? 3 : 1;
  return { start, end: addDays(start, count), days: Array.from({ length: count }, (_, index) => addDays(start, index)), step: mode==="week" || mode==="workweek" ? 7 : count };
}
export function rangeForEvent(event: ScheduleEvent, preferred: CalendarRangeMode): CalendarRangeMode {
  return preferred === "workweek" && [0,6].includes(eventDate(event).getDay()) ? "day" : preferred;
}
