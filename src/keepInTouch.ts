import type { ContactProfile } from "./domain";

/** Mirrors `MAX_KEEP_IN_TOUCH_DAYS` in `src-tauri/src/db/contacts.rs`. */
export const MAX_KEEP_IN_TOUCH_DAYS = 730;

/** Preset intervals offered by the frequency picker, shortest first. */
export const KEEP_IN_TOUCH_FREQUENCIES: { days: number; label: string }[] = [
  { days: 7, label: "Weekly" },
  { days: 14, label: "Every 2 Weeks" },
  { days: 30, label: "Monthly" },
  { days: 60, label: "Every 2 Months" },
  { days: 91, label: "Quarterly" },
  { days: 182, label: "Every 6 Months" },
  { days: 365, label: "Yearly" },
];

/** Snooze presets in days from today. */
export const KEEP_IN_TOUCH_SNOOZES: { days: number; label: string }[] = [
  { days: 7, label: "1 Week" },
  { days: 14, label: "2 Weeks" },
  { days: 30, label: "1 Month" },
];

export function frequencyLabel(days: number): string {
  return KEEP_IN_TOUCH_FREQUENCIES.find((item) => item.days === days)?.label ?? (days === 1 ? "Every Day" : `Every ${days} Days`);
}

/** A whole number of days the backend accepts, or null. */
export function parseIntervalDays(value: string): number | null {
  if (!/^\d+$/.test(value.trim())) return null;
  const days = Number(value.trim());
  return days >= 1 && days <= MAX_KEEP_IN_TOUCH_DAYS ? days : null;
}

export type KeepInTouchStatus = "overdue" | "today" | "week" | "later";

export const KEEP_IN_TOUCH_GROUPS: { status: KeepInTouchStatus; label: string }[] = [
  { status: "overdue", label: "Overdue" },
  { status: "today", label: "Due Today" },
  { status: "week", label: "This Week" },
  { status: "later", label: "Later" },
];

const startOfDay = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate());
const DAY_MS = 86_400_000;
/** Whole local calendar days from `now` to `date`; DST-safe because both sides round. */
const calendarDaysUntil = (date: Date, now: Date) => Math.round((startOfDay(date).getTime() - startOfDay(now).getTime()) / DAY_MS);

/** Groups a reminder by the local calendar day it falls due on. */
export function keepInTouchStatus(dueAt: string | null, now = new Date()): KeepInTouchStatus | null {
  if (!dueAt) return null;
  const due = new Date(dueAt);
  if (Number.isNaN(due.getTime())) return null;
  const days = calendarDaysUntil(due, now);
  if (days < 0) return "overdue";
  if (days === 0) return "today";
  return days <= 7 ? "week" : "later";
}

/** Overdue or due today: the reminders that need attention now. */
export function isKeepInTouchDue(profile: Pick<ContactProfile, "keepInTouchDueAt">, now = new Date()): boolean {
  const status = keepInTouchStatus(profile.keepInTouchDueAt, now);
  return status === "overdue" || status === "today";
}

const monthDay = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const fullDate = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
/** "Oct 1", with the year only outside the current year. */
const shortDate = (date: Date, now: Date) => (date.getFullYear() === now.getFullYear() ? monthDay : fullDate).format(date);

/** "Overdue since Oct 1", "Due today", or "Due Oct 12". */
export function describeDue(dueAt: string, now = new Date()): string {
  const status = keepInTouchStatus(dueAt, now);
  const date = shortDate(new Date(dueAt), now);
  if (status === "overdue") return `Overdue since ${date}`;
  if (status === "today") return "Due today";
  return `Due ${date}`;
}

/** Local midnight `days` days after today, as an ISO instant. */
export function snoozeUntilDays(days: number, now = new Date()): string {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() + days).toISOString();
}

/** Local midnight on a `YYYY-MM-DD` date input value, or null when it is not after today. */
export function snoozeUntilDate(value: string, now = new Date()): string | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return null;
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  if (date.getMonth() !== Number(match[2]) - 1 || calendarDaysUntil(date, now) < 1) return null;
  return date.toISOString();
}

/** `YYYY-MM-DD` for a local date, as a date input expects. */
export function dateInputValue(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

const birthdayParts = (birthday: string) => {
  const match = /^(?:(\d{4})-)?(\d{2})-(\d{2})$/.exec(birthday);
  return match ? { year: match[1] ? Number(match[1]) : null, month: Number(match[2]), day: Number(match[3]) } : null;
};

/** "Dec 9", or "Dec 9, 1984" when the year is known. */
export function formatBirthday(birthday: string): string {
  const parts = birthdayParts(birthday);
  if (!parts) return birthday;
  const date = new Date(parts.year ?? 2000, parts.month - 1, parts.day);
  return (parts.year ? fullDate : monthDay).format(date);
}

/**
 * The next local date the birthday falls on (today counts), how many days
 * away it is, and the age turned when the year is known. February 29 falls
 * on February 28 outside leap years.
 */
export function nextBirthday(birthday: string, now = new Date()): { date: Date; daysAway: number; turning: number | null } | null {
  const parts = birthdayParts(birthday);
  if (!parts) return null;
  const on = (year: number) => {
    const leapDayMissing = parts.month === 2 && parts.day === 29 && new Date(year, 1, 29).getMonth() !== 1;
    return new Date(year, parts.month - 1, leapDayMissing ? 28 : parts.day);
  };
  let date = on(now.getFullYear());
  if (calendarDaysUntil(date, now) < 0) date = on(now.getFullYear() + 1);
  return { date, daysAway: calendarDaysUntil(date, now), turning: parts.year ? date.getFullYear() - parts.year : null };
}

/** How far ahead the keep-in-touch view lists birthdays. */
export const UPCOMING_BIRTHDAY_DAYS = 14;

/** "Today", "Tomorrow", or "Dec 9", with the age turned when known. */
export function describeBirthday(birthday: string, now = new Date()): string {
  const next = nextBirthday(birthday, now);
  if (!next) return birthday;
  const when = next.daysAway === 0 ? "Today" : next.daysAway === 1 ? "Tomorrow" : shortDate(next.date, now);
  return next.turning ? `${when} · turns ${next.turning}` : when;
}

/** The newest touch by mail either way or logged by hand, as an ISO instant. */
export function lastTouchAt(profile: Pick<ContactProfile, "lastInteractedAt" | "keepInTouch">): string | null {
  const candidates = [profile.lastInteractedAt, profile.keepInTouch.lastTouchAt].filter((value): value is string => !!value && Number.isFinite(Date.parse(value)));
  return candidates.sort((a, b) => Date.parse(b) - Date.parse(a))[0] ?? null;
}

/** Whether the due date currently comes from a snooze rather than the interval. */
export function isSnoozeActive(profile: Pick<ContactProfile, "keepInTouch" | "keepInTouchDueAt">): boolean {
  const { snoozedUntil } = profile.keepInTouch;
  return !!snoozedUntil && !!profile.keepInTouchDueAt && Date.parse(snoozedUntil) === Date.parse(profile.keepInTouchDueAt);
}

/** "Oct 1", with the year only outside the current year. */
export function formatKeepInTouchDate(iso: string, now = new Date()): string {
  return shortDate(new Date(iso), now);
}
