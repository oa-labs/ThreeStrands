/** Shared display policy; see docs/date-formatting.md. All formats use the device locale. */
const compactDate = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const fullDate = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
const monthYear = new Intl.DateTimeFormat(undefined, { month: "short", year: "numeric" });
const diagnosticDateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" });
const timeOfDay = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });
const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** Exact compact date; retain the day and add the year outside the current year. */
export function formatCompactDate(date: Date, now = new Date()): string {
  return date.getFullYear() === now.getFullYear() ? compactDate.format(date) : fullDate.format(date);
}

/** Explicit date for filters and facts that must remain clear independent of today. */
export function formatFullDate(date: Date): string {
  return fullDate.format(date);
}

/** Month precision is reserved for aggregate relationship-age facts. */
export function formatMonthYear(date: Date): string {
  return monthYear.format(date);
}

/** Full local timestamp for metadata, history details, and audit information. */
export function formatDateTime(date: Date): string {
  return dateTime.format(date);
}

/** Operational diagnostics retain seconds to distinguish events within a minute. */
export function formatDiagnosticDateTime(date: Date): string {
  return diagnosticDateTime.format(date);
}

/** Metadata timestamp that names today and yesterday ("Today at 11:05 AM"); other days use the full local format. */
export function formatRelativeDateTime(date: Date, now = new Date()): string {
  const day = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate());
  const offset = Math.round((day(now).getTime() - day(date).getTime()) / 86_400_000);
  if (offset === 0) return `Today at ${timeOfDay.format(date)}`;
  if (offset === 1) return `Yesterday at ${timeOfDay.format(date)}`;
  return dateTime.format(date);
}
