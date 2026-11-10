/** Shared display policy; see docs/date-formatting.md. All formats use the device locale. */
const compactDate = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const fullDate = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
const monthYear = new Intl.DateTimeFormat(undefined, { month: "short", year: "numeric" });
const diagnosticDateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" });
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
