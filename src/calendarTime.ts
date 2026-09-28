import type { ScheduleEvent } from "./domain";

/** Pixel height of one hour row in every time-grid calendar surface. */
export const HOUR_HEIGHT = 64;
export const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

export function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

export function addDays(date: Date, offset: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + offset);
  return next;
}

export function isSameDay(left: Date, right: Date): boolean {
  return left.getFullYear() === right.getFullYear()
    && left.getMonth() === right.getMonth()
    && left.getDate() === right.getDate();
}

/** Sunday-anchored start of the week containing `date`. */
export function startOfWeek(date: Date): Date {
  const start = startOfLocalDay(date);
  return addDays(start, -start.getDay());
}

export function dateInputValue(date: Date): string {
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 10);
}

export function timeZoneLabel(date: Date): string {
  const part = new Intl.DateTimeFormat(undefined, {
    timeZoneName: "short",
    hour: "numeric",
  })
    .formatToParts(date)
    .find((candidate) => candidate.type === "timeZoneName");
  return part?.value ?? "UTC";
}

export function hourLabel(hour: number): string {
  if (hour === 0) return "12 am";
  if (hour === 12) return "12 pm";
  return `${hour > 12 ? hour - 12 : hour} ${hour >= 12 ? "pm" : "am"}`;
}

export function isValidTimeZone(value: string): boolean {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: value }).format();
    return true;
  } catch {
    return false;
  }
}

export function listSupportedTimeZones(): string[] {
  try {
    return Intl.supportedValuesOf("timeZone");
  } catch {
    return ["UTC"];
  }
}

/**
 * Carries the entered date across a date/date-time input switch instead of
 * discarding it: a `date` value truncates to its first 10 characters, and a
 * bare date gains a default time when switching to `datetime`.
 */
export function convertDueInputValue(value: string, toKind: "date" | "datetime"): string {
  if (!value) return "";
  if (toKind === "date") return value.slice(0, 10);
  return value.length > 10 ? value : `${value}T09:00`;
}

export function formatEventTime(event: ScheduleEvent): string {
  if (event.allDay) return "All day";
  const formatter = new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  const start = formatter.formatToParts(new Date(event.start))
    .filter((part) => part.type !== "dayPeriod")
    .map((part) => part.value)
    .join("")
    .trim();
  const end = formatter.formatToParts(new Date(event.end))
    .map((part) => part.type === "dayPeriod" ? part.value.toLocaleLowerCase() : part.value)
    .join("");
  return `${start}–${end}`;
}

export function eventDate(event: ScheduleEvent): Date {
  if (!event.allDay) return new Date(event.start);
  const [year, month, day] = event.start.split("-").map(Number);
  return new Date(year, month - 1, day);
}

export function formatEventDate(event: ScheduleEvent): string {
  return new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  }).format(eventDate(event));
}

export function safeWebUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
  } catch {
    return null;
  }
}

/**
 * Minute offsets of an event clamped to the given local day, so a multi-day
 * or overnight event still renders as a bounded block inside one column.
 */
export function eventDayBounds(event: ScheduleEvent, day: Date): { start: number; end: number } {
  const dayStart = startOfLocalDay(day);
  const dayEnd = addDays(dayStart, 1);
  const start = new Date(event.start);
  const end = new Date(event.end);
  const clamp = (value: Date) => value <= dayStart
    ? 0
    : value >= dayEnd
      ? 24 * 60
      : value.getHours() * 60 + value.getMinutes();
  return { start: clamp(start), end: clamp(end) };
}

export function occursOnDay(event: ScheduleEvent, day: Date): boolean {
  if (event.allDay) return isSameDay(eventDate(event), day);
  const dayStart = startOfLocalDay(day);
  const dayEnd = addDays(dayStart, 1);
  return new Date(event.start) < dayEnd && new Date(event.end) > dayStart;
}

export type LaidOutEvent = { event: ScheduleEvent; lane: number; lanes: number; span: number };

/**
 * Side-by-side placement for events that overlap in time. Events are grouped
 * into clusters of mutual overlap; every event in a cluster shares the same
 * lane count so the columns line up. An event fills adjacent lanes to its right
 * when no event in those lanes overlaps its own time range.
 */
export function layOutDayEvents(events: ScheduleEvent[], day: Date): LaidOutEvent[] {
  const ordered = [...events].sort((left, right) => {
    const leftBounds = eventDayBounds(left, day);
    const rightBounds = eventDayBounds(right, day);
    return leftBounds.start - rightBounds.start || rightBounds.end - leftBounds.end;
  });
  const placed: LaidOutEvent[] = [];
  let cluster: { entry: LaidOutEvent; start: number; end: number }[] = [];
  let clusterEnd = -1;

  const closeCluster = () => {
    const lanes = cluster.reduce((max, item) => Math.max(max, item.entry.lane + 1), 0);
    for (const item of cluster) {
      item.entry.lanes = lanes;
      for (let lane = item.entry.lane + 1; lane < lanes; lane += 1) {
        const occupied = cluster.some((other) => other.entry.lane === lane
          && other.start < item.end && other.end > item.start);
        if (occupied) break;
        item.entry.span += 1;
      }
    }
    cluster = [];
    clusterEnd = -1;
  };

  for (const event of ordered) {
    const bounds = eventDayBounds(event, day);
    const end = Math.max(bounds.end, bounds.start + 15);
    if (cluster.length > 0 && bounds.start >= clusterEnd) closeCluster();
    const taken = new Set(cluster.filter((item) => item.end > bounds.start).map((item) => item.entry.lane));
    let lane = 0;
    while (taken.has(lane)) lane += 1;
    const entry: LaidOutEvent = { event, lane, lanes: lane + 1, span: 1 };
    cluster.push({ entry, start: bounds.start, end });
    clusterEnd = Math.max(clusterEnd, end);
    placed.push(entry);
  }
  closeCluster();
  return placed;
}
